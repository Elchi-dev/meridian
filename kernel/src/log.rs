//! The kernel's log, the parts that need no processor: the lines programs
//! write to it, and lines gathered whole before they go out.

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

/// Gathers what is written into whole lines and hands each to `send`, so
/// a line goes out in one piece. A line longer than `N` bytes goes out in
/// pieces of at most `N`, never splitting a character, and what is left
/// when it is dropped goes out then.
pub struct Lines<const N: usize, F: FnMut(&str)> {
    line: [u8; N],
    len: usize,
    send: F,
}

impl<const N: usize, F: FnMut(&str)> Lines<N, F> {
    pub fn new(send: F) -> Lines<N, F> {
        const { assert!(N >= 4, "room for the longest character") };
        Lines {
            line: [0; N],
            len: 0,
            send,
        }
    }

    fn flush(&mut self) {
        if let Ok(text) = core::str::from_utf8(&self.line[..self.len]) {
            (self.send)(text);
        }
        self.len = 0;
    }
}

impl<const N: usize, F: FnMut(&str)> core::fmt::Write for Lines<N, F> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            let mut room = [0u8; 4];
            let bytes = c.encode_utf8(&mut room).as_bytes();
            if self.len + bytes.len() > N {
                self.flush();
            }
            self.line[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
            if c == '\n' {
                self.flush();
            }
        }
        Ok(())
    }
}

impl<const N: usize, F: FnMut(&str)> Drop for Lines<N, F> {
    fn drop(&mut self) {
        if self.len > 0 {
            self.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use super::*;

    /// What a `Lines` of `N` bytes sends for `pieces`, written one after
    /// the other, until it is dropped.
    fn sent<const N: usize>(pieces: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        {
            let mut lines = Lines::<N, _>::new(|line: &str| out.push(line.to_owned()));
            for piece in pieces {
                lines.write_str(piece).unwrap();
            }
        }
        out
    }

    #[test]
    fn a_line_goes_out_whole_once_it_ends() {
        assert_eq!(
            sent::<64>(&["TEST kernel", ".x", " ok\n"]),
            ["TEST kernel.x ok\n"]
        );
        assert_eq!(sent::<64>(&["a\nb\n", "c\n"]), ["a\n", "b\n", "c\n"]);
        let mut out = Vec::new();
        let mut lines = Lines::<64, _>::new(|line: &str| out.push(line.to_owned()));
        let name = "hello";
        write!(lines, "user: {name} ended with {}", 0).unwrap();
        lines.write_str("\n").unwrap();
        drop(lines);
        assert_eq!(out, ["user: hello ended with 0\n"]);
    }

    #[test]
    fn a_line_longer_than_the_room_goes_out_in_pieces() {
        assert_eq!(sent::<8>(&["abcdefghij\n"]), ["abcdefgh", "ij\n"]);
        assert_eq!(
            sent::<4>(&["abcd\n"]),
            ["abcd", "\n"],
            "the room filled exactly"
        );
        assert_eq!(
            sent::<8>(&["abcdefg\u{e4}\n"]),
            ["abcdefg", "\u{e4}\n"],
            "no character cut in two"
        );
    }

    #[test]
    fn what_is_left_goes_out_when_dropped() {
        assert_eq!(sent::<64>(&["a\n", "the rest"]), ["a\n", "the rest"]);
        assert!(
            sent::<64>(&["a\n"]).len() == 1,
            "nothing more for an ended line"
        );
        assert_eq!(sent::<64>(&[]), Vec::<String>::new());
    }

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
