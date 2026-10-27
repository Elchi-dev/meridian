//! wc: counts the lines, words and bytes of each file it is given, or of
//! its standard input without one, and writes them on a line with the
//! file's path, as POSIX's wc does, and a total after more than one file.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::abi::{MESSAGE_BYTES, file};
use program::{Error, Handle};

program::main!(wc);

/// What was counted in a text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    lines: u64,
    words: u64,
    bytes: u64,
}

impl core::ops::Add for Counts {
    type Output = Counts;

    fn add(self, other: Counts) -> Counts {
        Counts {
            lines: self.lines + other.lines,
            words: self.words + other.words,
            bytes: self.bytes + other.bytes,
        }
    }
}

/// Counts that go on over a text that comes in pieces, so a word cut
/// between two of them counts once. A word is a run of bytes that are not
/// white space as POSIX's C locale has it, and a line ends at each line
/// feed.
#[derive(Default)]
struct Counter {
    counts: Counts,
    in_word: bool,
}

impl Counter {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.counts.bytes += 1;
            if byte == b'\n' {
                self.counts.lines += 1;
            }
            let space = matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r');
            if !space && !self.in_word {
                self.counts.words += 1;
            }
            self.in_word = !space;
        }
    }
}

/// The line wc writes for `counts`, with the name they belong to.
fn line(counts: Counts, name: Option<&str>) -> String {
    let Counts {
        lines,
        words,
        bytes,
    } = counts;
    match name {
        Some(name) => format!("{lines} {words} {bytes} {name}\n"),
        None => format!("{lines} {words} {bytes}\n"),
    }
}

/// What wc says when the file at `path` could not be counted.
fn trouble(path: &str, e: Error) -> String {
    let why = match e {
        Error::NotFound => "no such file",
        Error::NotAFile => "a directory, not a file",
        Error::BadArgument => "not a path from the root",
        Error::Denied => "not allowed",
        _ => "could not be read",
    };
    format!("wc: {path}: {why}\n")
}

#[cfg_attr(test, allow(dead_code))]
fn wc(_: Handle) -> u64 {
    let paths = program::arguments();
    if paths.is_empty() {
        let mut counter = Counter::default();
        let mut room = [0u8; MESSAGE_BYTES];
        loop {
            match program::input(&mut room) {
                Ok(0) => break,
                Ok(len) => counter.feed(&room[..len]),
                Err(_) => return 1,
            }
        }
        return u64::from(program::write(&line(counter.counts, None)).is_err());
    }
    let mut total = Counts::default();
    let mut failed = false;
    for path in paths {
        let said = match counted(path) {
            Ok(counts) => {
                total = total + counts;
                line(counts, Some(path))
            }
            Err(e) => {
                failed = true;
                trouble(path, e)
            }
        };
        failed |= program::write(&said).is_err();
    }
    if paths.len() > 1 {
        failed |= program::write(&line(total, Some("total"))).is_err();
    }
    u64::from(failed)
}

/// What the file at `path` holds, counted.
#[cfg_attr(test, allow(dead_code))]
fn counted(path: &str) -> Result<Counts, Error> {
    let handle = program::open(path, file::READ)?;
    let mut counter = Counter::default();
    let mut room = [0u8; 4096];
    let read = loop {
        match program::read(handle, counter.counts.bytes, &mut room) {
            Ok(0) => break Ok(counter.counts),
            Ok(len) => counter.feed(&room[..len]),
            Err(e) => break Err(e),
        }
    };
    let _ = program::close(handle);
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(lines: u64, words: u64, bytes: u64) -> Counts {
        Counts {
            lines,
            words,
            bytes,
        }
    }

    /// What `pieces`, fed one after another, count up to.
    fn fed(pieces: &[&str]) -> Counts {
        let mut counter = Counter::default();
        for piece in pieces {
            counter.feed(piece.as_bytes());
        }
        counter.counts
    }

    #[test]
    fn lines_words_and_bytes_are_counted() {
        assert_eq!(fed(&[]), counts(0, 0, 0));
        assert_eq!(fed(&[""]), counts(0, 0, 0));
        assert_eq!(fed(&["a b\n"]), counts(1, 2, 4));
        assert_eq!(fed(&["  a\n\nb\tc  "]), counts(2, 3, 10));
        assert_eq!(fed(&["no line's end"]), counts(0, 3, 13));
        assert_eq!(fed(&["\n\n\n"]), counts(3, 0, 3));
        assert_eq!(
            fed(&["\u{e9}t\u{e9} \u{e9}\n"]),
            counts(1, 2, 9),
            "bytes, not characters"
        );
        assert_eq!(
            fed(&["a\r\nb\x0bc\x0cd"]),
            counts(1, 4, 8),
            "vertical tabs and form feeds part words too"
        );
    }

    #[test]
    fn a_word_cut_between_pieces_counts_once() {
        assert_eq!(fed(&["hel", "lo world\n"]), counts(1, 2, 12));
        assert_eq!(fed(&["one ", "two"]), counts(0, 2, 7));
        assert_eq!(fed(&["one", " two"]), counts(0, 2, 7));
        assert_eq!(fed(&["a", "b", "c", "\n", "d"]), counts(1, 2, 5));
    }

    #[test]
    fn each_line_names_what_it_counted() {
        assert_eq!(line(counts(1, 2, 4), None), "1 2 4\n");
        assert_eq!(
            line(counts(10, 0, 300), Some("/System/a.txt")),
            "10 0 300 /System/a.txt\n"
        );
        assert_eq!(
            counts(1, 2, 3) + counts(10, 20, 30),
            counts(11, 22, 33),
            "a total adds each"
        );
    }

    #[test]
    fn trouble_says_what_kept_a_file_from_being_counted() {
        assert_eq!(trouble("/x", Error::NotFound), "wc: /x: no such file\n");
        assert_eq!(
            trouble("/System", Error::NotAFile),
            "wc: /System: a directory, not a file\n"
        );
        assert_eq!(
            trouble("x", Error::BadArgument),
            "wc: x: not a path from the root\n"
        );
        assert_eq!(trouble("/x", Error::Denied), "wc: /x: not allowed\n");
        assert_eq!(
            trouble("/x", Error::Unreadable),
            "wc: /x: could not be read\n"
        );
    }
}
