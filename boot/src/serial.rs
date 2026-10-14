//! Programming a 16550-compatible UART, the serial port QEMU and most
//! desktop boards provide as COM1. Only the register values live here; the
//! port writes are in `raw`.

pub const COM1: u16 = 0x3f8;

/// The UART's input clock divided by 16, the baud rate at divisor 1.
pub const BASE_BAUD: u32 = 115_200;

/// Line status register and its "transmit holding register empty" bit.
pub const LINE_STATUS: u16 = 5;
pub const TRANSMIT_EMPTY: u8 = 0x20;

/// How often to poll the line status before giving up on a byte. A board
/// without COM1 reads back 0xff, which already says "empty"; this bound
/// only matters for a UART that never drains.
pub const MAX_POLLS: u32 = 100_000;

/// Register writes, as (offset from the base port, value), that set the
/// UART to `baud`, 8 data bits, no parity, one stop bit, with FIFOs on.
/// `None` when `baud` cannot be reached with an integer divisor.
#[must_use]
pub fn init_sequence(baud: u32) -> Option<[(u16, u8); 7]> {
    if baud == 0 || !BASE_BAUD.is_multiple_of(baud) {
        return None;
    }
    let divisor = u16::try_from(BASE_BAUD / baud).ok()?;
    let [low, high] = divisor.to_le_bytes();
    Some([
        (1, 0x00), // interrupt enable: none, the loader polls
        (3, 0x80), // line control: divisor latch access on
        (0, low),  // divisor latch, low byte
        (1, high), // divisor latch, high byte
        (3, 0x03), // line control: 8 data bits, no parity, 1 stop bit, latch off
        (2, 0xc7), // FIFO control: enable, clear both, trigger at 14 bytes
        (4, 0x0b), // modem control: DTR, RTS and OUT2
    ])
}

/// The bytes to send for `s`, with every `\n` turned into `\r\n` so
/// terminals start the next line at the left edge.
pub fn wire_bytes(s: &str) -> impl Iterator<Item = u8> + '_ {
    s.bytes().flat_map(|b| {
        (b == b'\n')
            .then_some(b'\r')
            .into_iter()
            .chain(core::iter::once(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_speed() {
        assert_eq!(
            init_sequence(115_200),
            Some([
                (1, 0x00),
                (3, 0x80),
                (0, 1),
                (1, 0),
                (3, 0x03),
                (2, 0xc7),
                (4, 0x0b)
            ])
        );
    }

    #[test]
    fn slower_rates_split_the_divisor_into_bytes() {
        let divisor = |baud| {
            let s = init_sequence(baud).unwrap();
            u16::from_le_bytes([s[2].1, s[3].1])
        };
        assert_eq!(divisor(38_400), 3);
        assert_eq!(divisor(9_600), 12);
        assert_eq!(divisor(300), 384);
        assert_eq!(init_sequence(300).unwrap()[3], (1, 1));
        assert_eq!(divisor(50), 2304);
    }

    #[test]
    fn impossible_rates_are_refused() {
        assert_eq!(init_sequence(0), None);
        assert_eq!(init_sequence(100_000), None);
        assert_eq!(init_sequence(230_400), None);
        assert_eq!(init_sequence(1), None);
    }

    #[test]
    fn latch_is_opened_before_and_closed_after_the_divisor() {
        let s = init_sequence(9_600).unwrap();
        assert_eq!(s[1], (3, 0x80));
        assert_eq!((s[2].0, s[3].0), (0, 1));
        assert_eq!(s[4], (3, 0x03));
    }

    #[test]
    fn newlines_go_out_as_crlf() {
        let out: Vec<u8> = wire_bytes("a\nb\n\n").collect();
        assert_eq!(out, b"a\r\nb\r\n\r\n");
        let all: String = (0u8..=127).map(char::from).filter(|&c| c != '\n').collect();
        assert_eq!(wire_bytes(&all).collect::<Vec<u8>>(), all.into_bytes());
        assert_eq!(wire_bytes("\u{e9}").collect::<Vec<u8>>(), [0xc3, 0xa9]);
        assert_eq!(wire_bytes("").count(), 0);
    }

    #[test]
    fn register_constants() {
        assert_eq!((COM1, LINE_STATUS, TRANSMIT_EMPTY), (0x3f8, 5, 0x20));
    }
}
