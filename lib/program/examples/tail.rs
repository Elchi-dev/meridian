//! tail: the last lines of each file it is given, or of its standard
//! input without one, ten unless `-n` says how many, each file's after a
//! line that names it when there are more than one, as POSIX's tail
//! writes them.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Lines};

program::main!(tail);

/// What tail was asked for: how many lines, and the files to take them
/// from.
fn asked(arguments: &[String]) -> Result<(usize, &[String]), String> {
    match arguments {
        [flag, count, files @ ..] if flag == "-n" => count
            .parse()
            .map(|count| (count, files))
            .map_err(|_| format!("tail: {count} is no count of lines\n")),
        [flag] if flag == "-n" => Err("tail: -n needs a count of lines\n".into()),
        files => Ok((10, files)),
    }
}

/// The last lines of a text that comes in pieces, no more of them kept
/// than are wanted.
struct Last {
    count: usize,
    kept: VecDeque<Vec<u8>>,
    lines: Lines,
}

impl Last {
    fn new(count: usize) -> Last {
        Last {
            count,
            kept: VecDeque::new(),
            lines: Lines::default(),
        }
    }
}

impl text::Reader for Last {
    fn feed(&mut self, piece: &[u8]) -> bool {
        let (count, kept) = (self.count, &mut self.kept);
        self.lines.feed(piece, |line| {
            keep(kept, count, line, true);
            true
        })
    }

    /// The lines kept, each with the end it had.
    fn finish(self) -> Vec<u8> {
        let Last {
            count,
            mut kept,
            lines,
        } = self;
        lines.finish(|line| keep(&mut kept, count, line, false));
        kept.into_iter().flatten().collect()
    }
}

/// Keeps `line`, with its end when `ended`, among the last `count` lines
/// in `kept`.
fn keep(kept: &mut VecDeque<Vec<u8>>, count: usize, line: &[u8], ended: bool) {
    if count == 0 {
        return;
    }
    if kept.len() == count {
        kept.pop_front();
    }
    let mut line = line.to_vec();
    if ended {
        line.push(b'\n');
    }
    kept.push_back(line);
}

#[cfg_attr(test, allow(dead_code))]
fn tail(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok((count, files)) => u64::from(!text::each_file("tail", files, || Last::new(count))),
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

    /// The last `count` lines of `pieces`.
    fn last(count: usize, pieces: &[&str]) -> String {
        let mut last = Last::new(count);
        for piece in pieces {
            last.feed(piece.as_bytes());
        }
        String::from_utf8(last.finish()).unwrap()
    }

    #[test]
    fn it_takes_ten_lines_unless_told_how_many() {
        let none: [String; 0] = [];
        assert_eq!(asked(&[]), Ok((10, &none[..])));
        let three = words("-n 3 /a /b");
        assert_eq!(asked(&three), Ok((3, &three[2..])));
        assert_eq!(
            asked(&words("-n many")),
            Err("tail: many is no count of lines\n".into())
        );
        assert_eq!(
            asked(&words("-n")),
            Err("tail: -n needs a count of lines\n".into())
        );
    }

    #[test]
    fn the_last_lines_come_with_the_ends_they_had() {
        assert_eq!(last(2, &["a\nb\nc\n"]), "b\nc\n");
        assert_eq!(last(2, &["a\nb\nc"]), "b\nc", "the last without its end");
        assert_eq!(last(5, &["a\nb\n"]), "a\nb\n", "fewer than wanted");
        assert_eq!(last(1, &["a\n", "b", "c\n"]), "bc\n");
        assert_eq!(last(0, &["a\nb\n"]), "");
        assert_eq!(last(3, &[]), "");
        assert_eq!(last(1, &["\n\n"]), "\n", "an empty line counts");
        let many = (0..1000).fold(String::new(), |mut many, n| {
            let _ = core::fmt::Write::write_fmt(&mut many, format_args!("{n}\n"));
            many
        });
        assert_eq!(last(3, &[&many]), "997\n998\n999\n");
    }

    #[test]
    fn a_line_keeps_what_it_holds_wherever_the_pieces_cut_it() {
        assert_eq!(
            last(1, &["a\r\nb\r\n"]),
            "b\r\n",
            "a carriage return is the line's"
        );
        assert_eq!(
            last(2, &["a\n", "b\n"]),
            "a\nb\n",
            "a cut right after a line's end"
        );
        assert_eq!(last(1, &["", "x", "", "y\n"]), "xy\n");
        let mut last_bytes = Last::new(1);
        last_bytes.feed(&[0xff, b'\n', 0xfe, 0x00]);
        assert_eq!(last_bytes.finish(), [0xfe, 0x00], "bytes that are no text");
    }

    #[test]
    fn no_more_lines_are_kept_than_are_wanted() {
        let mut kept = VecDeque::new();
        for n in 0..50u8 {
            keep(&mut kept, 4, &[n], true);
            assert!(kept.len() <= 4);
        }
        assert_eq!(kept, [[46, b'\n'], [47, b'\n'], [48, b'\n'], [49, b'\n']]);
        let mut none = VecDeque::new();
        keep(&mut none, 0, b"x", true);
        assert!(none.is_empty());
    }

    #[test]
    fn random_texts_keep_their_last_lines_whatever_the_pieces() {
        let mut seed = 0x3c6e_f372_fe94_f82b_u64;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let lines: Vec<String> = (0..seed % 20)
                .map(|n| "x".repeat(usize::try_from(n % 4).unwrap()))
                .collect();
            let text = lines.iter().fold(String::new(), |all, l| all + l + "\n");
            let count = usize::try_from(seed % 7).unwrap();
            let want = lines[lines.len().saturating_sub(count)..]
                .iter()
                .fold(String::new(), |all, l| all + l + "\n");
            let size = 1 + usize::try_from(seed % 5).unwrap();
            let pieces: Vec<&str> = text
                .as_bytes()
                .chunks(size)
                .map(|c| core::str::from_utf8(c).unwrap())
                .collect();
            assert_eq!(last(count, &pieces), want, "{count} of {text:?}");
        }
    }
}
