//! The lines programs write to the kernel's log, the parts that need no
//! processor.

use abi::LOG_BYTES;

/// How many bytes a line of `len` bytes is, when the log takes it.
#[must_use]
pub fn line_len(len: u64) -> Option<usize> {
    usize::try_from(len).ok().filter(|&n| n <= LOG_BYTES)
}

/// How a program's byte shows in the log: printable ASCII and the space
/// as they are, anything else as a question mark, so no program moves the
/// cursor of a terminal reading the log or makes a line look like the
/// kernel's own.
#[must_use]
pub fn shown(byte: u8) -> char {
    if byte.is_ascii_graphic() || byte == b' ' {
        char::from(byte)
    } else {
        '?'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_may_be_as_long_as_the_log_takes_and_no_longer() {
        assert_eq!(line_len(0), Some(0));
        assert_eq!(line_len(12), Some(12));
        assert_eq!(line_len(256), Some(256), "the longest");
        assert_eq!(line_len(257), None);
        assert_eq!(line_len(u64::MAX), None);
    }

    #[test]
    fn only_printable_text_shows_as_it_is() {
        let text: String = b"hello, Meridian ~!".iter().map(|&b| shown(b)).collect();
        assert_eq!(text, "hello, Meridian ~!");
        for byte in [
            b'\n', b'\r', b'\t', 0x1b, 0x07, 0x08, 0, 0x7f, 0x80, 0xc3, 0xff,
        ] {
            assert_eq!(shown(byte), '?', "byte {byte:#x}");
        }
        let printable = (0..=255u8).filter(|&b| shown(b) != '?').count();
        assert_eq!(
            printable,
            95 - 1,
            "0x20 to 0x7e, without the question mark itself"
        );
    }
}
