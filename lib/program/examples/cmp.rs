//! cmp: compares two files byte by byte, as POSIX's cmp does, and says
//! where they first differ, by byte and line; `-l` lists each byte that
//! differs, its number and both values in octal, and `-s` says nothing.
//! A `-` for one of them is the standard input. It ends with 0 when they
//! are the same, 1 when they differ, and 2 when one could not be read.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::abi::{MESSAGE_BYTES, file::READ};
use program::{Error, Handle, text};

program::main!(cmp);

/// What cmp says about the bytes that differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Say {
    /// Where the first difference is.
    First,
    /// Each byte that differs.
    Every,
    /// Nothing.
    Nothing,
}

/// What cmp's words ask: how to say it, and the two files.
fn asked(arguments: &[String]) -> Result<(Say, &str, &str), String> {
    let mut say = Say::First;
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
            say = match letter {
                'l' => Say::Every,
                's' => Say::Nothing,
                _ => return Err(format!("cmp: -{letter} is no option\n")),
            };
        }
        words = rest;
    }
    match words {
        [first, second] => Ok((say, first, second)),
        _ => Err(String::from("cmp: compares two files\n")),
    }
}

/// How two texts compare so far, fed matching pieces of each.
#[derive(Debug, Default)]
struct Comparison {
    /// The bytes compared, and the lines that ended among them.
    bytes: u64,
    lines: u64,
    /// The first byte that differed, counted from 1, and its line.
    first: Option<(u64, u64)>,
    /// What `-l` writes.
    listed: Vec<u8>,
}

impl Comparison {
    /// Compares `a` and `b`, as long as each other, which come next in
    /// the two texts; says whether to go on, which only the first
    /// difference stops unless every one is wanted.
    fn feed(&mut self, a: &[u8], b: &[u8], every: bool) -> bool {
        for (&x, &y) in a.iter().zip(b) {
            self.bytes += 1;
            if x != y {
                self.first.get_or_insert((self.bytes, self.lines + 1));
                if !every {
                    return false;
                }
                self.listed
                    .extend_from_slice(format!("{} {x:o} {y:o}\n", self.bytes).as_bytes());
            }
            if x == b'\n' {
                self.lines += 1;
            }
        }
        true
    }
}

/// What cmp writes when `first` and `second` differ first at `byte` of
/// `line`, both counted from 1.
fn differ(first: &str, second: &str, (byte, line): (u64, u64)) -> String {
    format!("{first} {second} differ: char {byte}, line {line}\n")
}

/// What cmp says on its standard error when `shorter` ended where the
/// other went on.
fn ended(shorter: &str) -> String {
    format!("cmp: EOF on {shorter}\n")
}

/// What gives a text a piece at a time.
trait Pieces {
    /// Reads the next piece onto the end of `held`; false at the end.
    fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error>;
}

/// A file or the standard input, read a piece at a time.
enum Source {
    Input,
    File(Handle, u64),
}

impl Source {
    fn open(path: &str) -> Result<Source, Error> {
        if path == "-" {
            return Ok(Source::Input);
        }
        Ok(Source::File(program::open(path, READ)?, 0))
    }

    fn close(self) {
        if let Source::File(file, _) = self {
            let _ = program::close(file);
        }
    }
}

impl Pieces for Source {
    #[cfg_attr(test, allow(dead_code))]
    fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
        let mut room = [0u8; MESSAGE_BYTES];
        let len = match self {
            Source::Input => program::input(&mut room)?,
            Source::File(file, at) => {
                let len = program::read(*file, *at, &mut room)?;
                *at += len as u64;
                len
            }
        };
        held.extend_from_slice(&room[..len]);
        Ok(len > 0)
    }
}

#[cfg_attr(test, allow(dead_code))]
fn cmp(_: Handle) -> u64 {
    let (say, first, second) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let names = [first, second];
    let mut sources = Vec::new();
    for path in names {
        match Source::open(path) {
            Ok(source) => sources.push(source),
            Err(e) => {
                if say != Say::Nothing {
                    let _ = program::write_error(&text::trouble("cmp", path, e));
                }
                sources.into_iter().for_each(Source::close);
                return 2;
            }
        }
    }
    let compared = compare(&mut sources, say == Say::Every);
    sources.into_iter().for_each(Source::close);
    let (comparison, shorter) = match compared {
        Ok(compared) => compared,
        Err((side, e)) => {
            if say != Say::Nothing {
                let _ = program::write_error(&text::trouble("cmp", names[side], e));
            }
            return 2;
        }
    };
    if say != Say::Nothing {
        let out = match (say, comparison.first) {
            (Say::Every, _) => comparison.listed.clone(),
            (_, Some(at)) => differ(first, second, at).into_bytes(),
            _ => Vec::new(),
        };
        let _ = program::write_bytes(&out);
        if let Some(side) = shorter {
            let _ = program::write_error(&ended(names[side]));
        }
    }
    u64::from(comparison.first.is_some() || shorter.is_some())
}

/// Reads both `sources` and compares them, every difference when
/// `every`: how they compared, and which ended while the other went on.
///
/// # Errors
///
/// Which one could not be read, and why.
#[cfg_attr(test, allow(dead_code))]
fn compare(
    sources: &mut [impl Pieces],
    every: bool,
) -> Result<(Comparison, Option<usize>), (usize, Error)> {
    let mut comparison = Comparison::default();
    let mut held = [Vec::new(), Vec::new()];
    let mut open = [true, true];
    loop {
        for side in 0..2 {
            if open[side] && held[side].is_empty() {
                open[side] = sources[side].more(&mut held[side]).map_err(|e| (side, e))?;
            }
        }
        // A side that holds nothing now has ended.
        let both = held[0].len().min(held[1].len());
        if both == 0 {
            let shorter = match (held[0].is_empty(), held[1].is_empty()) {
                (true, true) => None,
                (true, false) => Some(0),
                _ => Some(1),
            };
            return Ok((comparison, shorter));
        }
        if !comparison.feed(&held[0][..both], &held[1][..both], every) {
            return Ok((comparison, None));
        }
        held[0].drain(..both);
        held[1].drain(..both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// How `a` and `b` compare, fed in pieces of `cut` bytes, every
    /// difference when `every`.
    fn compared(a: &[u8], b: &[u8], cut: usize, every: bool) -> Comparison {
        let mut comparison = Comparison::default();
        let both = a.len().min(b.len());
        for start in (0..both).step_by(cut) {
            let end = (start + cut).min(both);
            if !comparison.feed(&a[start..end], &b[start..end], every) {
                break;
            }
        }
        comparison
    }

    #[test]
    fn its_words_are_an_option_and_two_files() {
        assert_eq!(asked(&words("/a /b")), Ok((Say::First, "/a", "/b")));
        assert_eq!(asked(&words("-l /a -")), Ok((Say::Every, "/a", "-")));
        assert_eq!(asked(&words("-s /a /b")), Ok((Say::Nothing, "/a", "/b")));
        assert_eq!(
            asked(&words("-ls /a /b")),
            Ok((Say::Nothing, "/a", "/b")),
            "the last wins"
        );
        assert_eq!(asked(&words("-- -l /b")), Ok((Say::First, "-l", "/b")));
        assert_eq!(
            asked(&words("/a")),
            Err(String::from("cmp: compares two files\n"))
        );
        assert_eq!(
            asked(&words("/a /b /c")),
            Err(String::from("cmp: compares two files\n"))
        );
        assert_eq!(
            asked(&words("-x /a /b")),
            Err(String::from("cmp: -x is no option\n"))
        );
    }

    #[test]
    fn the_first_difference_is_found_by_byte_and_line() {
        let same = compared(b"one\ntwo\n", b"one\ntwo\n", 3, false);
        assert_eq!((same.first, same.bytes, same.lines), (None, 8, 2));
        let first = compared(b"one\ntwo\n", b"one\ntwx\n", 3, false);
        assert_eq!(first.first, Some((7, 2)));
        assert_eq!(first.bytes, 7, "it stops there");
        assert_eq!(compared(b"x", b"y", 1, false).first, Some((1, 1)));
        assert_eq!(
            compared(b"a\nb", b"a\nc", 1, false).first,
            Some((3, 2)),
            "a line feed that matches ends the line before the difference"
        );
        assert_eq!(
            compared(b"a\n", b"ab", 1, false).first,
            Some((2, 1)),
            "a line feed that differs is on the line it would end"
        );
    }

    #[test]
    fn every_difference_is_listed_by_number_and_octal_values() {
        let every = compared(b"abcd", b"aXcY", 2, true);
        assert_eq!(every.first, Some((2, 1)));
        assert_eq!(
            String::from_utf8(every.listed).unwrap(),
            "2 142 130\n4 144 131\n"
        );
        assert_eq!(every.bytes, 4, "it goes on to the end");
        let none = compared(b"same", b"same", 1, true);
        assert_eq!(none.listed, b"");
        let wide = compared(&[0xff, 0], &[0, 0xff], 1, true);
        assert_eq!(
            String::from_utf8(wide.listed).unwrap(),
            "1 377 0\n2 0 377\n"
        );
    }

    #[test]
    fn pieces_cut_anywhere_compare_as_wholes() {
        let a = b"line one\nline two\nline three\n";
        let mut b = a.to_vec();
        b[20] = b'X';
        for cut in [1, 2, 3, 7, 64] {
            let comparison = compared(a, &b, cut, false);
            assert_eq!(comparison.first, Some((21, 3)), "pieces of {cut}");
        }
    }

    #[test]
    fn what_cmp_says_names_both_files_or_the_shorter() {
        assert_eq!(differ("/a", "/b", (7, 2)), "/a /b differ: char 7, line 2\n");
        assert_eq!(differ("-", "/b", (1, 1)), "- /b differ: char 1, line 1\n");
        assert_eq!(ended("/short"), "cmp: EOF on /short\n");
    }

    /// A text in memory, given in the pieces it was cut into.
    struct Cut(alloc::collections::VecDeque<Vec<u8>>);

    impl Cut {
        fn into(text: &[u8], size: usize) -> Cut {
            Cut(text.chunks(size).map(<[u8]>::to_vec).collect())
        }
    }

    impl Pieces for Cut {
        fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
            let Some(piece) = self.0.pop_front() else {
                return Ok(false);
            };
            held.extend_from_slice(&piece);
            Ok(true)
        }
    }

    /// How two texts cut into pieces of different sizes compare, and
    /// which ended first.
    fn read_both(a: &[u8], b: &[u8], every: bool) -> (Option<(u64, u64)>, Option<usize>, Vec<u8>) {
        let mut sources = [Cut::into(a, 3), Cut::into(b, 5)];
        let (comparison, shorter) = compare(&mut sources, every).unwrap();
        (comparison.first, shorter, comparison.listed)
    }

    #[test]
    fn two_texts_read_in_pieces_of_their_own_compare_whole() {
        assert_eq!(
            read_both(b"same text\n", b"same text\n", false),
            (None, None, Vec::new())
        );
        assert_eq!(read_both(b"", b"", false), (None, None, Vec::new()));
        assert_eq!(
            read_both(b"same text\n", b"same tExt\n", false).0,
            Some((7, 1))
        );
        assert_eq!(
            read_both(b"one\ntwo\nthree", b"one\ntwo\nthreX", false).0,
            Some((13, 3))
        );
    }

    #[test]
    fn the_one_that_ends_first_is_named_once_all_before_matched() {
        assert_eq!(
            read_both(b"abc", b"abcdef", false),
            (None, Some(0), Vec::new())
        );
        assert_eq!(
            read_both(b"abcdef", b"abc", false),
            (None, Some(1), Vec::new())
        );
        assert_eq!(read_both(b"", b"x", false), (None, Some(0), Vec::new()));
        assert_eq!(
            read_both(b"aXc", b"abcdef", false),
            (Some((2, 1)), None, Vec::new()),
            "a difference before the end is said instead"
        );
        assert_eq!(
            read_both(b"aXc", b"abcdef", true),
            (Some((2, 1)), Some(0), b"2 130 142\n".to_vec()),
            "with -l both"
        );
    }

    #[test]
    fn every_difference_is_found_across_pieces_of_their_own() {
        let a = b"0123456789abcdef";
        let b = b"0X23456789aXcdeX";
        let (first, shorter, listed) = read_both(a, b, true);
        assert_eq!((first, shorter), (Some((2, 1)), None));
        assert_eq!(
            String::from_utf8(listed).unwrap(),
            "2 61 130\n12 142 130\n16 146 130\n"
        );
    }

    #[test]
    fn a_line_feed_that_differs_is_listed_in_octal_too() {
        let every = compared(b"a\nb c", b"a b\nc", 1, true);
        assert_eq!(
            String::from_utf8(every.listed).unwrap(),
            "2 12 40\n4 40 12\n"
        );
        assert_eq!(every.first, Some((2, 1)));
        assert_eq!(every.lines, 1, "lines are counted in the first file");
        let long = alloc::vec![b'x'; 300];
        let mut other = long.clone();
        other[299] = b'y';
        assert_eq!(compared(&long, &other, 128, false).first, Some((300, 1)));
    }

    #[test]
    fn feeding_says_whether_to_read_on() {
        let mut first = Comparison::default();
        assert!(first.feed(b"ab", b"ab", false));
        assert!(
            !first.feed(b"c", b"x", false),
            "the first difference stops it"
        );
        let mut every = Comparison::default();
        assert!(every.feed(b"c", b"x", true), "with -l every one goes on");
        assert!(every.feed(b"", b"", true));
        assert_eq!((first.bytes, every.bytes), (3, 1));
        assert_eq!(first.first, Some((3, 1)));
    }

    #[test]
    fn both_files_may_be_the_input_or_named_by_any_path() {
        assert_eq!(asked(&words("- -")), Ok((Say::First, "-", "-")));
        assert_eq!(
            asked(&words("-s -- -a")),
            Err(String::from("cmp: compares two files\n"))
        );
        assert_eq!(asked(&words("-s -- -a -b")), Ok((Say::Nothing, "-a", "-b")));
        assert_eq!(differ("a b", "c", (1, 1)), "a b c differ: char 1, line 1\n");
    }
}
