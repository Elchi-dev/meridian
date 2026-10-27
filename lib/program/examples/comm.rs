//! comm: the lines of two sorted files in three columns, as POSIX's comm
//! has it: those only in the first, those only in the second, and those
//! in both, each column after the first one tab further in. `-1`, `-2`
//! and `-3` leave a column out; `-` is the standard input. Lines compare
//! by their bytes, the order sort puts them in.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::abi::MESSAGE_BYTES;
use program::text::{self, LineReader, Pieces, Source};
use program::{Error, Handle};

program::main!(comm);

/// Which columns comm writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Columns {
    left_out: [bool; 3],
}

impl Columns {
    /// The line comm writes for `line`, found in the column `column`,
    /// counted from 0: one tab for each column before it that is written.
    fn line(self, column: usize, line: &[u8]) -> Option<Vec<u8>> {
        if self.left_out[column] {
            return None;
        }
        let tabs = self.left_out[..column].iter().filter(|out| !**out).count();
        let mut written = alloc::vec![b'\t'; tabs];
        written.extend_from_slice(line);
        written.push(b'\n');
        Some(written)
    }
}

fn asked(arguments: &[String]) -> Result<(Columns, &str, &str), String> {
    let mut columns = Columns::default();
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
            let column = match letter {
                '1' => 0,
                '2' => 1,
                '3' => 2,
                _ => return Err(format!("comm: -{letter} is no option\n")),
            };
            columns.left_out[column] = true;
        }
        words = rest;
    }
    match words {
        [first, second] => Ok((columns, first, second)),
        _ => Err(String::from("comm: compares two files\n")),
    }
}

/// Gives `out` the line comm writes for each line of the two sorted
/// texts `readers` read, each in its column, in order.
///
/// # Errors
///
/// Which of them could not be read, and why.
fn merged<P: Pieces>(
    columns: Columns,
    readers: &mut [LineReader<P>; 2],
    mut out: impl FnMut(&[u8]),
) -> Result<(), (usize, Error)> {
    let next =
        |side: usize, readers: &mut [LineReader<P>; 2]| readers[side].line().map_err(|e| (side, e));
    let mut first = next(0, readers)?;
    let mut second = next(1, readers)?;
    loop {
        let column = match (&first, &second) {
            (None, None) => return Ok(()),
            (Some(a), Some(b)) if a == b => 2,
            (Some(a), Some(b)) if a < b => 0,
            (Some(_), None) => 0,
            _ => 1,
        };
        let line = if column == 1 { &second } else { &first };
        if let Some(written) = line.as_deref().and_then(|line| columns.line(column, line)) {
            out(&written);
        }
        if column != 1 {
            first = next(0, readers)?;
        }
        if column != 0 {
            second = next(1, readers)?;
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn comm(_: Handle) -> u64 {
    let (columns, first, second) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let paths = [first, second];
    let first = match Source::open(first) {
        Ok(source) => source,
        Err(e) => {
            let _ = program::write_error(&text::trouble("comm", first, e));
            return 2;
        }
    };
    let second = match Source::open(second) {
        Ok(source) => source,
        Err(e) => {
            let _ = program::write_error(&text::trouble("comm", second, e));
            first.close();
            return 2;
        }
    };
    let mut readers = [LineReader::new(first), LineReader::new(second)];
    let mut held = Vec::new();
    let mut failed = false;
    let read = merged(columns, &mut readers, |line| {
        held.extend_from_slice(line);
        if held.len() >= MESSAGE_BYTES {
            failed |= program::write_bytes(&held).is_err();
            held.clear();
        }
    });
    failed |= program::write_bytes(&held).is_err();
    for reader in readers {
        reader.into_pieces().close();
    }
    if let Err((side, e)) = read {
        let _ = program::write_error(&text::trouble("comm", paths[side], e));
        return 2;
    }
    u64::from(failed)
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

    /// A text in memory, given in the pieces it was cut into.
    struct Cut(alloc::collections::VecDeque<Vec<u8>>);

    impl Pieces for Cut {
        fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
            let Some(piece) = self.0.pop_front() else {
                return Ok(false);
            };
            held.extend_from_slice(&piece);
            Ok(true)
        }
    }

    /// What comm asked with `arguments` writes for `a` and `b`, read in
    /// pieces of 3 and 5 bytes.
    fn comm_of(arguments: &str, a: &str, b: &str) -> String {
        let words = words(&(String::from(arguments) + " /a /b"));
        let (columns, ..) = asked(&words).unwrap();
        let cut = |text: &str, size| {
            LineReader::new(Cut(text
                .as_bytes()
                .chunks(size)
                .map(<[u8]>::to_vec)
                .collect()))
        };
        let mut readers = [cut(a, 3), cut(b, 5)];
        let mut out = Vec::new();
        merged(columns, &mut readers, |line| out.extend_from_slice(line)).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn lines_go_into_the_column_of_where_they_are() {
        assert_eq!(
            comm_of("", "a\nb\nd\n", "b\nc\nd\ne\n"),
            "a\n\t\tb\n\tc\n\t\td\n\te\n"
        );
        assert_eq!(comm_of("", "", ""), "");
        assert_eq!(comm_of("", "x\n", ""), "x\n");
        assert_eq!(comm_of("", "", "x\n"), "\tx\n");
        assert_eq!(comm_of("", "same\n", "same\n"), "\t\tsame\n");
        assert_eq!(
            comm_of("", "a\nb", "b"),
            "a\n\t\tb\n",
            "last lines without their line feed"
        );
    }

    #[test]
    fn columns_left_out_take_their_tabs_with_them() {
        let (a, b) = ("a\nb\nd\n", "b\nc\nd\ne\n");
        assert_eq!(comm_of("-1", a, b), "\tb\nc\n\td\ne\n");
        assert_eq!(comm_of("-2", a, b), "a\n\tb\n\td\n");
        assert_eq!(comm_of("-3", a, b), "a\n\tc\n\te\n");
        assert_eq!(comm_of("-12", a, b), "b\nd\n", "only the lines in both");
        assert_eq!(comm_of("-1 -3", a, b), "c\ne\n", "only those in the second");
        assert_eq!(comm_of("-23", a, b), "a\n");
        assert_eq!(comm_of("-123", a, b), "");
    }

    #[test]
    fn lines_compare_by_their_bytes() {
        assert_eq!(comm_of("-3", "B\na\n", "a\n"), "B\n", "capitals first");
        assert_eq!(comm_of("", "a\nab\n", "ab\n"), "a\n\t\tab\n");
        assert_eq!(comm_of("-12", "ä\nö\n", "ö\n"), "ö\n");
        assert_eq!(
            comm_of("", "\n\na\n", "\na\n"),
            "\t\t\n\n\t\ta\n",
            "empty lines too, one pair at a time"
        );
    }

    #[test]
    fn its_words_are_columns_to_leave_out_and_two_files() {
        assert_eq!(asked(&words("/a /b")), Ok((Columns::default(), "/a", "/b")));
        assert_eq!(
            asked(&words("-13 - /b")),
            Ok((
                Columns {
                    left_out: [true, false, true]
                },
                "-",
                "/b"
            ))
        );
        assert_eq!(asked(&words("-- -1 /b")).map(|(_, a, _)| a), Ok("-1"));
        assert_eq!(
            asked(&words("/a")),
            Err(String::from("comm: compares two files\n"))
        );
        assert_eq!(
            asked(&words("-4 /a /b")),
            Err(String::from("comm: -4 is no option\n"))
        );
    }

    #[test]
    fn each_line_comes_as_often_as_counting_says() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut roll = |sides: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % 1024).unwrap() % sides
        };
        for _ in 0..200 {
            let mut lists: [Vec<&str>; 2] = Default::default();
            for list in &mut lists {
                *list = (0..roll(8))
                    .map(|_| ["a", "ab", "b", "c", "c2"][roll(5)])
                    .collect();
                list.sort_unstable();
            }
            let text = |list: &[&str]| {
                list.iter().fold(String::new(), |mut text, line| {
                    text.push_str(line);
                    text.push('\n');
                    text
                })
            };
            let out = comm_of("", &text(&lists[0]), &text(&lists[1]));
            for line in ["a", "ab", "b", "c", "c2"] {
                let count = |list: &[&str]| list.iter().filter(|l| **l == line).count();
                let (a, b) = (count(&lists[0]), count(&lists[1]));
                let both = a.min(b);
                let in_column = |tabs: &str| {
                    out.lines()
                        .filter(|l| *l == alloc::format!("{tabs}{line}"))
                        .count()
                };
                assert_eq!(
                    (in_column(""), in_column("\t"), in_column("\t\t")),
                    (a - both, b - both, both),
                    "{line} in {lists:?}"
                );
            }
            let order: Vec<&str> = out.lines().map(|l| l.trim_start_matches('\t')).collect();
            assert!(
                order.windows(2).all(|w| w[0] <= w[1]),
                "in order: {order:?}"
            );
        }
    }
}
