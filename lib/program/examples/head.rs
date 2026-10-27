//! head: the first lines of each file it is given, or of its standard
//! input without one, ten unless `-n` says how many, each file's after a
//! line that names it when there are more than one, as POSIX's head
//! writes them.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Lines};

program::main!(head);

/// What head was asked for: how many lines, and the files to take them
/// from.
fn asked(arguments: &[String]) -> Result<(usize, &[String]), String> {
    match arguments {
        [flag, count, files @ ..] if flag == "-n" => count
            .parse()
            .map(|count| (count, files))
            .map_err(|_| format!("head: {count} is no count of lines\n")),
        [flag] if flag == "-n" => Err("head: -n needs a count of lines\n".into()),
        files => Ok((10, files)),
    }
}

/// The first lines of a text that comes in pieces.
struct First {
    left: usize,
    out: Vec<u8>,
    lines: Lines,
}

impl First {
    fn new(count: usize) -> First {
        First {
            left: count,
            out: Vec::new(),
            lines: Lines::default(),
        }
    }
}

impl text::Reader for First {
    fn feed(&mut self, piece: &[u8]) -> bool {
        if self.left == 0 {
            return false;
        }
        let (left, out) = (&mut self.left, &mut self.out);
        self.lines.feed(piece, |line| {
            out.extend_from_slice(line);
            out.push(b'\n');
            *left -= 1;
            *left > 0
        })
    }

    /// What was taken, the last line without its end with it when the
    /// count reaches it. Once the count is reached the lines hold nothing
    /// back, as they stop where they were told.
    fn finish(self) -> Vec<u8> {
        let First { mut out, lines, .. } = self;
        lines.finish(|line| out.extend_from_slice(line));
        out
    }
}

#[cfg_attr(test, allow(dead_code))]
fn head(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok((count, files)) => u64::from(!text::each_file("head", files, || First::new(count))),
        Err(why) => {
            let _ = program::write_error(&why);
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use program::text::Reader;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    /// The first `count` lines of `pieces`, fed until head wants no more,
    /// and how many pieces it took.
    fn first(count: usize, pieces: &[&str]) -> (String, usize) {
        let mut first = First::new(count);
        let mut taken = 0;
        for piece in pieces {
            taken += 1;
            if !first.feed(piece.as_bytes()) {
                break;
            }
        }
        (String::from_utf8(first.finish()).unwrap(), taken)
    }

    #[test]
    fn it_takes_ten_lines_unless_told_how_many() {
        let none: [String; 0] = [];
        assert_eq!(asked(&[]), Ok((10, &none[..])));
        let files = words("/a /b");
        assert_eq!(asked(&files), Ok((10, &files[..])));
        let three = words("-n 3 /a");
        assert_eq!(asked(&three), Ok((3, &three[2..])));
        assert_eq!(asked(&words("-n 0")), Ok((0, &none[..])));
        assert_eq!(
            asked(&words("-n x /a")),
            Err("head: x is no count of lines\n".into())
        );
        assert_eq!(
            asked(&words("-n")),
            Err("head: -n needs a count of lines\n".into())
        );
        assert_eq!(
            asked(&words("-n -1")),
            Err("head: -1 is no count of lines\n".into())
        );
    }

    #[test]
    fn a_long_line_across_many_pieces_is_one() {
        let long = "x".repeat(300);
        let pieces: Vec<&str> = long
            .as_bytes()
            .chunks(128)
            .map(|c| core::str::from_utf8(c).unwrap())
            .collect();
        let mut all = pieces.clone();
        all.push("\nnext\n");
        assert_eq!(first(1, &all), (format!("{long}\n"), pieces.len() + 1));
        assert_eq!(first(2, &all).0, format!("{long}\nnext\n"));
    }

    #[test]
    fn the_first_lines_come_and_no_more_is_read() {
        assert_eq!(first(2, &["a\nb\nc\n"]), ("a\nb\n".into(), 1));
        assert_eq!(
            first(2, &["a\n", "b\n", "c\n", "d\n"]),
            ("a\nb\n".into(), 2)
        );
        assert_eq!(first(5, &["a\nb"]), ("a\nb".into(), 1), "the last as it is");
        assert_eq!(
            first(2, &["a\nb"]),
            ("a\nb".into(), 1),
            "the last as the count's last"
        );
        assert_eq!(first(1, &["a", "b\nc"]), ("ab\n".into(), 2));
        assert_eq!(first(0, &["a\n", "b\n"]), (String::new(), 1));
        assert_eq!(first(3, &[]), (String::new(), 0));
        assert_eq!(first(2, &["a\n"]), ("a\n".into(), 1));
        assert_eq!(
            first(1, &["\n\n"]),
            ("\n".into(), 1),
            "an empty line counts"
        );
    }
}
