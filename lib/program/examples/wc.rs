//! wc: counts the lines, words and bytes of each file it is given, or of
//! its standard input without one, and writes them on a line with the
//! file's path, as POSIX's wc does, and a total after more than one file.
//! `-l`, `-w` and `-c` ask for only some of them, and `-m` for the
//! characters in place of the bytes; they come in that order however they
//! were asked, and of `-c` and `-m` the last counts. What it could not
//! count it says on its standard error.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use program::abi::{MESSAGE_BYTES, file};
use program::{Error, Handle};

program::main!(wc);

/// What was counted in a text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    lines: u64,
    words: u64,
    bytes: u64,
    characters: u64,
}

impl core::ops::Add for Counts {
    type Output = Counts;

    fn add(self, other: Counts) -> Counts {
        Counts {
            lines: self.lines + other.lines,
            words: self.words + other.words,
            bytes: self.bytes + other.bytes,
            characters: self.characters + other.characters,
        }
    }
}

/// Counts that go on over a text that comes in pieces, so a word or a
/// character cut between two of them counts once. A word is a run of
/// bytes that are not white space as POSIX's C locale has it, a line ends
/// at each line feed, and a character is each byte that does not go on
/// with one before it in UTF-8.
#[derive(Default)]
struct Counter {
    counts: Counts,
    in_word: bool,
}

impl Counter {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.counts.bytes += 1;
            if byte & 0xc0 != 0x80 {
                self.counts.characters += 1;
            }
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

/// What the size of a text is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Size {
    Bytes,
    Characters,
}

/// Which counts wc writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Asked {
    lines: bool,
    words: bool,
    size: Option<Size>,
}

/// What wc's options ask for, and the files after them, or why they
/// cannot be taken. Without options it counts all three.
fn asked(arguments: &[String]) -> Result<(Asked, &[String]), String> {
    let mut asked = Asked {
        lines: false,
        words: false,
        size: None,
    };
    let mut any = false;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        for letter in letters.chars() {
            match letter {
                'l' => asked.lines = true,
                'w' => asked.words = true,
                'c' => asked.size = Some(Size::Bytes),
                'm' => asked.size = Some(Size::Characters),
                _ => return Err(format!("wc: -{letter} is no option\n")),
            }
        }
        any = true;
        words = rest;
    }
    if !any {
        asked = Asked {
            lines: true,
            words: true,
            size: Some(Size::Bytes),
        };
    }
    Ok((asked, words))
}

/// The line wc writes for `counts`, those `asked` for, with the name
/// they belong to.
fn line(counts: Counts, asked: Asked, name: Option<&str>) -> String {
    let size = asked.size.map(|size| match size {
        Size::Bytes => counts.bytes,
        Size::Characters => counts.characters,
    });
    let numbers = [
        asked.lines.then_some(counts.lines),
        asked.words.then_some(counts.words),
        size,
    ];
    let mut fields: Vec<String> = numbers.iter().flatten().map(u64::to_string).collect();
    fields.extend(name.map(String::from));
    fields.join(" ") + "\n"
}

#[cfg_attr(test, allow(dead_code))]
fn wc(_: Handle) -> u64 {
    let (asked, paths) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
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
        return u64::from(program::write(&line(counter.counts, asked, None)).is_err());
    }
    let mut total = Counts::default();
    let mut failed = false;
    for path in paths {
        let written = match counted(path) {
            Ok(counts) => {
                total = total + counts;
                program::write(&line(counts, asked, Some(path)))
            }
            Err(e) => {
                failed = true;
                program::write_error(&program::text::trouble("wc", path, e))
            }
        };
        failed |= written.is_err();
    }
    if paths.len() > 1 {
        failed |= program::write(&line(total, asked, Some("total"))).is_err();
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

    fn counts(lines: u64, words: u64, bytes: u64, characters: u64) -> Counts {
        Counts {
            lines,
            words,
            bytes,
            characters,
        }
    }

    /// What `pieces`, fed one after another, count up to.
    fn fed_bytes(pieces: &[&[u8]]) -> Counts {
        let mut counter = Counter::default();
        for piece in pieces {
            counter.feed(piece);
        }
        counter.counts
    }

    fn fed(pieces: &[&str]) -> Counts {
        let pieces: Vec<&[u8]> = pieces.iter().map(|p| p.as_bytes()).collect();
        fed_bytes(&pieces)
    }

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// What wc asked with `options` writes for `counts`.
    fn written(options: &str, counts: Counts, name: Option<&str>) -> String {
        let given = words(options);
        let (asked, _) = asked(&given).unwrap();
        line(counts, asked, name)
    }

    #[test]
    fn lines_words_and_bytes_are_counted() {
        assert_eq!(fed(&[]), counts(0, 0, 0, 0));
        assert_eq!(fed(&[""]), counts(0, 0, 0, 0));
        assert_eq!(fed(&["a b\n"]), counts(1, 2, 4, 4));
        assert_eq!(fed(&["  a\n\nb\tc  "]), counts(2, 3, 10, 10));
        assert_eq!(fed(&["no line's end"]), counts(0, 3, 13, 13));
        assert_eq!(fed(&["\n\n\n"]), counts(3, 0, 3, 3));
        assert_eq!(
            fed(&["\u{e9}t\u{e9} \u{e9}\n"]),
            counts(1, 2, 9, 6),
            "bytes, and characters apart"
        );
        assert_eq!(
            fed(&["a\r\nb\x0bc\x0cd"]),
            counts(1, 4, 8, 8),
            "vertical tabs and form feeds part words too"
        );
    }

    #[test]
    fn a_word_cut_between_pieces_counts_once() {
        assert_eq!(fed(&["hel", "lo world\n"]), counts(1, 2, 12, 12));
        assert_eq!(fed(&["one ", "two"]), counts(0, 2, 7, 7));
        assert_eq!(fed(&["one", " two"]), counts(0, 2, 7, 7));
        assert_eq!(fed(&["a", "b", "c", "\n", "d"]), counts(1, 2, 5, 5));
    }

    #[test]
    fn a_character_cut_between_pieces_counts_once() {
        let euro = "\u{20ac}".as_bytes();
        assert_eq!(fed_bytes(&[&euro[..1], &euro[1..]]), counts(0, 1, 3, 1));
        assert_eq!(
            fed_bytes(&[&euro[..2], &euro[2..], b" ", euro]),
            counts(0, 2, 7, 3)
        );
        let face = "\u{1f600}".as_bytes();
        let one_by_one: Vec<&[u8]> = face.chunks(1).collect();
        assert_eq!(fed_bytes(&one_by_one), counts(0, 1, 4, 1));
    }

    #[test]
    fn a_byte_that_goes_on_with_nothing_counts_as_no_character() {
        assert_eq!(fed_bytes(&[b"\x80"]), counts(0, 1, 1, 0));
        assert_eq!(
            fed_bytes(&[b"\xffa\xbf"]),
            counts(0, 1, 3, 2),
            "a byte that starts nothing is one"
        );
    }

    #[test]
    fn each_line_names_what_it_counted() {
        assert_eq!(written("", counts(1, 2, 4, 3), None), "1 2 4\n");
        assert_eq!(
            written("", counts(10, 0, 300, 3), Some("/System/a.txt")),
            "10 0 300 /System/a.txt\n"
        );
        assert_eq!(
            counts(1, 2, 3, 4) + counts(10, 20, 30, 40),
            counts(11, 22, 33, 44),
            "a total adds each"
        );
    }

    #[test]
    fn options_ask_for_some_counts_in_their_own_order() {
        let c = counts(1, 2, 9, 6);
        assert_eq!(written("-l", c, None), "1\n");
        assert_eq!(written("-w", c, Some("a")), "2 a\n");
        assert_eq!(written("-c", c, None), "9\n");
        assert_eq!(written("-m", c, None), "6\n");
        assert_eq!(written("-cwl", c, None), "1 2 9\n");
        assert_eq!(written("-m -l", c, Some("total")), "1 6 total\n");
        assert_eq!(written("-wm", c, None), "2 6\n");
        assert_eq!(
            written("-cm", c, None),
            "6\n",
            "the last of -c and -m counts"
        );
        assert_eq!(written("-mc", c, None), "9\n");
        assert_eq!(written("-l -l", c, None), "1\n");
    }

    #[test]
    fn the_words_after_the_options_are_files() {
        let given = words("-l -- -w b");
        let (asked, files) = asked(&given).unwrap();
        assert_eq!(
            asked,
            Asked {
                lines: true,
                words: false,
                size: None
            }
        );
        assert_eq!(files, ["-w", "b"]);
        let given = words("a -l");
        let (asked, files) = super::asked(&given).unwrap();
        assert_eq!(asked.size, Some(Size::Bytes), "no options, all three");
        assert!(asked.lines && asked.words);
        assert_eq!(files, ["a", "-l"]);
        let given = words("- --");
        assert_eq!(super::asked(&given).unwrap().1, ["-", "--"]);
        let given = words("--");
        let (asked, files) = super::asked(&given).unwrap();
        assert_eq!(asked.size, Some(Size::Bytes));
        assert_eq!(files, [] as [String; 0]);
    }

    #[test]
    fn options_wc_does_not_know() {
        assert_eq!(asked(&words("-x")).unwrap_err(), "wc: -x is no option\n");
        assert_eq!(asked(&words("-lq a")).unwrap_err(), "wc: -q is no option\n");
    }

    #[test]
    fn only_the_c_locale_s_white_space_parts_words() {
        assert_eq!(
            fed(&["a\u{a0}b c\n"]),
            counts(1, 2, 7, 6),
            "a no-break space is part of a word"
        );
        assert_eq!(fed(&["a\u{2003}b"]), counts(0, 1, 5, 3));
        assert_eq!(
            fed(&["a\0b \0"]),
            counts(0, 2, 5, 5),
            "a zero byte is no space"
        );
        assert_eq!(
            fed(&["\u{85}"]),
            counts(0, 1, 2, 1),
            "nor is the next line control past ASCII"
        );
    }
}
