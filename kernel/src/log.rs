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

/// Writes the line the kernel logs when a program ended: under its name,
/// shown as [`shown`] shows bytes, or as "the program" when it has none,
/// and then `how`.
///
/// # Errors
///
/// When `out` fails.
pub fn ended(
    out: &mut impl core::fmt::Write,
    name: &[u8],
    how: core::fmt::Arguments<'_>,
) -> core::fmt::Result {
    out.write_str("user: ")?;
    if name.is_empty() {
        out.write_str("the program")?;
    }
    for &byte in name {
        out.write_char(shown(byte))?;
    }
    writeln!(out, " {how}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that takes nothing.
    struct Full;

    impl core::fmt::Write for Full {
        fn write_str(&mut self, _: &str) -> core::fmt::Result {
            Err(core::fmt::Error)
        }
    }

    #[test]
    fn an_ended_program_is_logged_under_its_name() {
        let line = |name: &[u8]| {
            let mut out = String::new();
            ended(&mut out, name, format_args!("ended with {}", 42)).unwrap();
            out
        };
        assert_eq!(line(b"hello"), "user: hello ended with 42\n");
        assert_eq!(
            line(b""),
            "user: the program ended with 42\n",
            "a program without a name"
        );
        assert_eq!(
            line(b"bad\x1bname"),
            "user: bad?name ended with 42\n",
            "its name shown as lines are"
        );
        assert!(ended(&mut Full, b"x", format_args!("")).is_err());
    }

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
