//! cksum: writes the checksum POSIX's cksum defines and the number of
//! bytes of each file it is given, or of its standard input, which `-`
//! names too, after them the file's name. The checksum is the CRC with
//! the polynomial 0x04C11DB7 over the bytes and then over their number,
//! low byte first, turned over at the end.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::{Handle, text};

program::main!(cksum);

/// The CRC of each byte value, the polynomial's bits shifted through it
/// from the top.
const TABLE: [u32; 256] = table();

const fn table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut at: u32 = 0;
    while at < 256 {
        let mut crc = at << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x04C1_1DB7
            };
            bit += 1;
        }
        table[at as usize] = crc;
        at += 1;
    }
    table
}

/// A checksum as the bytes come.
#[derive(Default)]
struct Sum {
    crc: u32,
    len: u64,
}

impl Sum {
    fn byte(&mut self, byte: u8) {
        let at = ((self.crc >> 24) ^ u32::from(byte)) & 0xff;
        self.crc = (self.crc << 8) ^ TABLE[at as usize];
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
        self.len += bytes.len() as u64;
    }

    /// The checksum, and how many bytes there were.
    fn finish(mut self) -> (u32, u64) {
        let mut left = self.len;
        while left != 0 {
            self.byte(left.to_le_bytes()[0]);
            left >>= 8;
        }
        (!self.crc, self.len)
    }
}

/// The line cksum writes for `sum`, with the name it belongs to.
fn line((crc, len): (u32, u64), name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{crc} {len} {name}\n"),
        None => format!("{crc} {len}\n"),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn cksum(_: Handle) -> u64 {
    let files = program::arguments();
    let mut failed = false;
    for path in text::sources(files) {
        let mut sum = Sum::default();
        let read = text::pieces(path.filter(|&p| p != "-"), |piece| {
            sum.feed(piece);
            true
        });
        match read {
            Ok(()) => failed |= program::write(&line(sum.finish(), path)).is_err(),
            Err(e) => {
                let _ = program::write_error(&text::trouble("cksum", path.unwrap_or("-"), e));
                failed = true;
            }
        }
    }
    u64::from(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summed(pieces: &[&[u8]]) -> (u32, u64) {
        let mut sum = Sum::default();
        for piece in pieces {
            sum.feed(piece);
        }
        sum.finish()
    }

    #[test]
    fn the_checksums_posix_s_cksum_writes() {
        assert_eq!(summed(&[]), (4_294_967_295, 0));
        assert_eq!(summed(&[b"hello\n"]), (3_015_617_425, 6));
        assert_eq!(summed(&[b"a"]), (1_220_704_766, 1));
        assert_eq!(summed(&[b"x"]), (12_738_659, 1));
        assert_eq!(
            summed(&[&[0; 1000]]),
            (2_610_763_910, 1000),
            "a length of two bytes"
        );
    }

    #[test]
    fn the_table_is_the_polynomial_shifted_through_each_byte() {
        assert_eq!(TABLE[0], 0);
        assert_eq!(TABLE[1], 0x04C1_1DB7);
        assert_eq!(TABLE[0x80], 0x690C_E0EE, "the top bit, reduced each shift");
        assert_eq!(TABLE[2], 0x0982_3B6E);
        assert_eq!(
            TABLE[3],
            TABLE[1] ^ TABLE[2],
            "a CRC of bits is the sum of theirs"
        );
        assert_eq!(TABLE[0xff], TABLE[0x80] ^ TABLE[0x7f]);
    }

    #[test]
    fn a_text_sums_the_same_whatever_the_pieces() {
        let text: alloc::vec::Vec<u8> = (0..700u32).map(|n| (n * 7 % 251) as u8).collect();
        let whole = summed(&[&text]);
        for size in [1, 2, 3, 64, 699] {
            let pieces: alloc::vec::Vec<&[u8]> = text.chunks(size).collect();
            assert_eq!(summed(&pieces), whole, "pieces of {size}");
        }
        assert_ne!(summed(&[&text[1..]]), whole);
    }

    #[test]
    fn each_line_names_its_file() {
        assert_eq!(line((1, 2), Some("/a b")), "1 2 /a b\n");
        assert_eq!(line((4_294_967_295, 0), None), "4294967295 0\n");
    }

    /// POSIX's checksum the long way: each bit of the message, the bytes
    /// and then their number, divided through the polynomial one at a
    /// time, with no table.
    fn bit_by_bit(bytes: &[u8]) -> u32 {
        let mut message = bytes.to_vec();
        let mut len = bytes.len();
        while len != 0 {
            message.push(len.to_le_bytes()[0]);
            len >>= 8;
        }
        let mut crc = 0u32;
        for byte in message {
            for bit in (0..8).rev() {
                let top = (crc >> 31) & 1;
                let incoming = u32::from(byte >> bit) & 1;
                crc <<= 1;
                if top ^ incoming == 1 {
                    crc ^= 0x04C1_1DB7;
                }
            }
        }
        !crc
    }

    #[test]
    fn random_texts_sum_as_the_long_way_does() {
        let mut seed = 0x1405_7b7e_f767_814f_u64;
        for _ in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = usize::try_from(seed % 700).unwrap();
            let text: alloc::vec::Vec<u8> = (0..len)
                .map(|n| (seed >> (n % 56)).to_le_bytes()[0])
                .collect();
            assert_eq!(
                summed(&[&text]),
                (bit_by_bit(&text), len as u64),
                "{len} bytes"
            );
        }
        assert_eq!(
            bit_by_bit(b"hello\n"),
            3_015_617_425,
            "the long way agrees with POSIX's"
        );
    }

    #[test]
    fn the_table_adds_up_as_a_crc_does_and_holds_no_value_twice() {
        for i in 0..256 {
            for j in 0..256 {
                assert_eq!(TABLE[i ^ j], TABLE[i] ^ TABLE[j], "{i} and {j}");
            }
        }
        let mut values = TABLE.to_vec();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), 256);
    }
}
