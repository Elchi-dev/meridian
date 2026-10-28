//! join: writes a line for each pair of lines of two files whose join
//! fields are the same, as POSIX's join does: the join field, then the
//! other fields of the first file's line and then of the second's. Both
//! files are sorted on their join fields by their bytes, and lines with
//! the same field pair each with each. Fields are parted by runs of
//! blanks, those a line starts with left out, or by each `-t` character,
//! and the output's by a space or that character. `-1` and `-2` name each
//! file's join field, `-a` also writes a file's lines that pair with
//! none, `-v` only those, and `-o` names the fields to write, `0` for the
//! join field and `F.N` for file F's field N, an empty or missing one
//! written as `-e` says. A `-` for a file is the standard input. A file
//! out of order is said, and join then ends with 1.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use program::text::{self, LineReader, Pieces, Source};
use program::{Error, Handle};

program::main!(join);

/// One field `-o` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    /// The join field.
    Join,
    /// A field of the first file's line, 0, or the second's, 1, counted
    /// from 0.
    Of(usize, usize),
}

/// What join's words ask for.
#[derive(Debug, PartialEq)]
struct Asked<'a> {
    /// Whether each file's lines that pair with none are written.
    unpaired: [bool; 2],
    /// Whether the lines that pair are left out.
    only_unpaired: bool,
    empty: Option<String>,
    list: Option<Vec<Field>>,
    separator: Option<String>,
    /// Each file's join field, counted from 0.
    fields: [usize; 2],
    files: [&'a str; 2],
}

/// A file's number as `-a` and `-v` take it, counted from 0.
fn file_number(letter: char, value: &str) -> Result<usize, String> {
    match value {
        "1" => Ok(0),
        "2" => Ok(1),
        _ => Err(format!("join: -{letter} takes 1 or 2, not {value}\n")),
    }
}

/// A field number as `-1` and `-2` take it, counted from 0.
fn field_number(letter: char, value: &str) -> Result<usize, String> {
    Some(value)
        .filter(|v| v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<usize>().ok())
        .and_then(|n| n.checked_sub(1))
        .ok_or_else(|| format!("join: -{letter} wants a field from 1, not {value}\n"))
}

/// The fields `-o` names, apart by commas or blanks.
fn field_list(value: &str) -> Result<Vec<Field>, String> {
    let mut list = Vec::new();
    for item in value.split([',', ' ', '\t']).filter(|i| !i.is_empty()) {
        if item == "0" {
            list.push(Field::Join);
            continue;
        }
        let field = item
            .split_once('.')
            .and_then(|(file, field)| {
                let file = match file {
                    "1" => 0,
                    "2" => 1,
                    _ => return None,
                };
                Some(Field::Of(file, field_number('o', field).ok()?))
            })
            .ok_or_else(|| {
                format!("join: {item} is no field; -o takes 0 or a file and a field, as 2.3\n")
            })?;
        list.push(field);
    }
    if list.is_empty() {
        return Err(String::from("join: -o wants fields\n"));
    }
    Ok(list)
}

/// What join's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        unpaired: [false; 2],
        only_unpaired: false,
        empty: None,
        list: None,
        separator: None,
        fields: [0; 2],
        files: ["", ""],
    };
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        words = rest;
        let letter = letters.chars().next().unwrap_or('-');
        if !"aveot12".contains(letter) {
            return Err(format!("join: -{letter} is no option\n"));
        }
        // Each option takes the rest of the word, or else the next.
        let after = &letters[letter.len_utf8()..];
        let value = if after.is_empty() {
            let (value, rest) = words
                .split_first()
                .ok_or_else(|| format!("join: -{letter} wants its word\n"))?;
            words = rest;
            value.as_str()
        } else {
            after
        };
        match letter {
            'a' => asked.unpaired[file_number('a', value)?] = true,
            'v' => {
                asked.unpaired[file_number('v', value)?] = true;
                asked.only_unpaired = true;
            }
            'e' => asked.empty = Some(String::from(value)),
            'o' => asked.list = Some(field_list(value)?),
            't' => {
                if value.chars().count() != 1 {
                    return Err(format!("join: -t takes one character, not {value}\n"));
                }
                asked.separator = Some(String::from(value));
            }
            '1' => asked.fields[0] = field_number('1', value)?,
            _ => asked.fields[1] = field_number('2', value)?,
        }
    }
    match words {
        [first, second] if !(first == "-" && second == "-") => {
            asked.files = [first, second];
            Ok(asked)
        }
        [_, _] => Err(String::from(
            "join: reads the standard input for one file only\n",
        )),
        _ => Err(String::from("join: joins two files\n")),
    }
}

/// The fields of `line`: parted by `separator`, or else by runs of
/// blanks, those it starts or ends with left out.
fn fields<'l>(line: &'l [u8], separator: Option<&[u8]>) -> Vec<&'l [u8]> {
    match separator {
        Some(separator) => {
            let mut fields = Vec::new();
            let mut rest = line;
            while let Some(at) = rest.windows(separator.len()).position(|w| w == separator) {
                fields.push(&rest[..at]);
                rest = &rest[at + separator.len()..];
            }
            fields.push(rest);
            fields
        }
        None => line
            .split(|&b| b == b' ' || b == b'\t')
            .filter(|f| !f.is_empty())
            .collect(),
    }
}

/// How join writes the lines it pairs.
struct Writing<'a> {
    asked: &'a Asked<'a>,
}

impl Writing<'_> {
    fn separator(&self) -> Option<&[u8]> {
        self.asked.separator.as_deref().map(str::as_bytes)
    }

    /// The join field of `line`, a line of the file `file`.
    fn key<'l>(&self, file: usize, line: &'l [u8]) -> &'l [u8] {
        fields(line, self.separator())
            .get(self.asked.fields[file])
            .copied()
            .unwrap_or(&[])
    }

    /// The line join writes for `lines`, a line of each file or of one
    /// when it pairs with none.
    fn line(&self, lines: [Option<&[u8]>; 2]) -> Vec<u8> {
        let split = lines.map(|line| line.map(|l| fields(l, self.separator())));
        let key = lines
            .iter()
            .enumerate()
            .find_map(|(file, line)| line.map(|l| self.key(file, l)))
            .unwrap_or(&[]);
        let mut written: Vec<&[u8]> = Vec::new();
        if let Some(list) = &self.asked.list {
            for &field in list {
                let value = match field {
                    Field::Join => key,
                    Field::Of(file, at) => split[file]
                        .as_ref()
                        .and_then(|f| f.get(at).copied())
                        .unwrap_or(&[]),
                };
                match &self.asked.empty {
                    Some(empty) if value.is_empty() => written.push(empty.as_bytes()),
                    _ => written.push(value),
                }
            }
        } else {
            written.push(key);
            for (file, fields) in split.iter().enumerate() {
                let Some(fields) = fields else {
                    continue;
                };
                let join = self.asked.fields[file];
                written.extend(
                    fields
                        .iter()
                        .enumerate()
                        .filter(|&(at, _)| at != join)
                        .map(|(_, f)| *f),
                );
            }
        }
        let mut line = written.join(self.separator().unwrap_or(b" "));
        line.push(b'\n');
        line
    }
}

/// One file's lines, read a group of lines with the same join field at a
/// time.
struct Side<P> {
    reader: LineReader<P>,
    /// The first line of the next group, read already.
    next: Option<Vec<u8>>,
    /// Whether a group came out of order.
    unsorted: bool,
}

impl<P: Pieces> Side<P> {
    fn new(pieces: P) -> Side<P> {
        Side {
            reader: LineReader::new(pieces),
            next: None,
            unsorted: false,
        }
    }

    /// The next lines with the same join field, none at the end of the
    /// file. A field before the last group's is out of order.
    fn group(&mut self, writing: &Writing, file: usize) -> Result<Vec<Vec<u8>>, Error> {
        let first = match self.next.take() {
            Some(line) => line,
            None => match self.reader.line()? {
                Some(line) => line,
                None => return Ok(Vec::new()),
            },
        };
        let mut group = Vec::from([first]);
        while let Some(line) = self.reader.line()? {
            let key = writing.key(file, &line);
            match key.cmp(writing.key(file, &group[0])) {
                Ordering::Equal => group.push(line),
                order => {
                    self.unsorted |= order == Ordering::Less;
                    self.next = Some(line);
                    break;
                }
            }
        }
        Ok(group)
    }
}

/// Gives `out` each line join writes for the two files `sides` read.
/// Answers which files came out of order.
///
/// # Errors
///
/// Which of them could not be read, and why.
fn joined<P: Pieces>(
    asked: &Asked,
    sides: &mut [Side<P>; 2],
    mut out: impl FnMut(&[u8]),
) -> Result<[bool; 2], (usize, Error)> {
    let writing = Writing { asked };
    let next = |file: usize, sides: &mut [Side<P>; 2]| {
        sides[file].group(&writing, file).map_err(|e| (file, e))
    };
    let mut groups = [next(0, sides)?, next(1, sides)?];
    loop {
        let order = match (groups[0].first(), groups[1].first()) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(a), Some(b)) => writing.key(0, a).cmp(writing.key(1, b)),
        };
        if order == Ordering::Equal {
            if !asked.only_unpaired {
                for a in &groups[0] {
                    for b in &groups[1] {
                        out(&writing.line([Some(a), Some(b)]));
                    }
                }
            }
            groups = [next(0, sides)?, next(1, sides)?];
            continue;
        }
        let file = usize::from(order == Ordering::Greater);
        if asked.unpaired[file] {
            for line in &groups[file] {
                let mut lines = [None, None];
                lines[file] = Some(line.as_slice());
                out(&writing.line(lines));
            }
        }
        groups[file] = next(file, sides)?;
    }
    Ok([sides[0].unsorted, sides[1].unsorted])
}

#[cfg_attr(test, allow(dead_code))]
fn join(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let open = |path: &str| {
        Source::open(path).map_err(|e| {
            let _ = program::write_error(&text::trouble("join", path, e));
        })
    };
    let Ok(first) = open(asked.files[0]) else {
        return 1;
    };
    let Ok(second) = open(asked.files[1]) else {
        first.close();
        return 1;
    };
    let mut sides = [Side::new(first), Side::new(second)];
    let mut out = text::standard_output();
    let joined = joined(&asked, &mut sides, |line| out.put(line));
    let written = out.finish();
    for side in sides {
        side.reader.into_pieces().close();
    }
    match joined {
        Ok(unsorted) => {
            for (file, _) in unsorted.iter().enumerate().filter(|(_, u)| **u) {
                let path = asked.files[file];
                let _ = program::write_error(&format!(
                    "join: {path} is not sorted on its join field\n"
                ));
            }
            u64::from(!written || unsorted.contains(&true))
        }
        Err((file, e)) => {
            let _ = program::write_error(&text::trouble("join", asked.files[file], e));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// What join asked with `options` writes for the texts `a` and `b`,
    /// read in pieces of 3 bytes, and which came out of order.
    fn joined_of(options: &str, a: &str, b: &str) -> (String, [bool; 2]) {
        let mut given = words(options);
        given.extend([String::from("/a"), String::from("/b")]);
        let asked = asked(&given).unwrap();
        let cut = |text: &str| Cut(text.as_bytes().chunks(3).map(<[u8]>::to_vec).collect());
        let mut sides = [Side::new(cut(a)), Side::new(cut(b))];
        let mut out = Vec::new();
        let unsorted = joined(&asked, &mut sides, |line| out.extend_from_slice(line)).unwrap();
        (String::from_utf8(out).unwrap(), unsorted)
    }

    fn join_of(options: &str, a: &str, b: &str) -> String {
        joined_of(options, a, b).0
    }

    const ONE: &str = "a 1\nb 2\nb 3\nd 4\n";
    const TWO: &str = "a x\nb y\nb z\nc w\n";

    #[test]
    fn lines_with_the_same_field_pair_each_with_each() {
        assert_eq!(join_of("", ONE, TWO), "a 1 x\nb 2 y\nb 2 z\nb 3 y\nb 3 z\n");
        assert_eq!(join_of("", "", TWO), "");
        assert_eq!(join_of("", "q 1\n", "r 2\n"), "", "nothing pairs");
        assert_eq!(
            join_of("", "a\n", "a\n"),
            "a\n",
            "a line of the join field alone"
        );
        assert_eq!(
            join_of("", "a 1", "a 2"),
            "a 1 2\n",
            "without a last line feed"
        );
    }

    #[test]
    fn a_and_v_write_the_lines_that_pair_with_none() {
        assert_eq!(
            join_of("-a 1", ONE, TWO),
            "a 1 x\nb 2 y\nb 2 z\nb 3 y\nb 3 z\nd 4\n"
        );
        assert_eq!(
            join_of("-a 1 -a2", ONE, TWO),
            "a 1 x\nb 2 y\nb 2 z\nb 3 y\nb 3 z\nc w\nd 4\n"
        );
        assert_eq!(join_of("-v 2", ONE, TWO), "c w\n");
        assert_eq!(join_of("-v1", ONE, TWO), "d 4\n");
        assert_eq!(join_of("-v 1 -v 2", ONE, TWO), "c w\nd 4\n");
        assert_eq!(join_of("-a 2", "", "x 1\ny 2\n"), "x 1\ny 2\n");
    }

    #[test]
    fn o_names_the_fields_and_e_fills_the_empty_ones() {
        assert_eq!(
            join_of("-o 1.2,2.2,0", ONE, TWO),
            "1 x a\n2 y b\n2 z b\n3 y b\n3 z b\n"
        );
        assert_eq!(
            join_of("-a 1 -e NONE -o 0,1.2,2.2", ONE, TWO),
            "a 1 x\nb 2 y\nb 2 z\nb 3 y\nb 3 z\nd 4 NONE\n"
        );
        assert_eq!(
            join_of("-a 2 -e NONE", ONE, TWO),
            "a 1 x\nb 2 y\nb 2 z\nb 3 y\nb 3 z\nc w\n",
            "without -o nothing is missing"
        );
        assert_eq!(join_of("-o 1.3,2.1", "a 1\n", "a x\n"), " a\n");
        assert_eq!(join_of("-o 2.1 -a 1 -e -", "a 1\n", "b 2\n"), "-\n");
        assert_eq!(
            join_of("-t : -o 1.2,2.2,1.5 -e X", "k:1:\n", "k::2\n"),
            "1:X:X\n",
            "a field there but empty is filled too"
        );
    }

    #[test]
    fn t_parts_fields_at_each_character() {
        assert_eq!(join_of("-t :", "k:1:\n", "k::2\n"), "k:1:::2\n");
        assert_eq!(join_of("-t ,", "a,,b\n", "a,c\n"), "a,,b,c\n");
        assert_eq!(
            join_of("-t \u{e9}", "k\u{e9}1\n", "k\u{e9}2\n"),
            "k\u{e9}1\u{e9}2\n"
        );
        assert_eq!(
            join_of("", "  a   1\n", "a 2\n"),
            "a 1 2\n",
            "blanks a line starts with part nothing"
        );
        assert_eq!(
            join_of("-t x", " x 1\n", "x 2\n"),
            "",
            "with -t a blank is a character"
        );
    }

    #[test]
    fn one_and_two_name_the_join_fields() {
        assert_eq!(
            join_of("-1 2 -2 1", "x a\ny b\n", "a 1\nb 2\n"),
            "a x 1\nb y 2\n"
        );
        assert_eq!(
            join_of("-1 3", "p q a\n", "a z\n"),
            "a p q z\n",
            "the join field comes first, and not again"
        );
        assert_eq!(join_of("-2 2", "k 1\n", "1 k\n"), "k 1 1\n");
        assert_eq!(
            join_of("-1 5", "a b\n", "\n"),
            " a b\n",
            "a missing join field is empty, as an empty line's"
        );
    }

    #[test]
    fn files_out_of_order_are_noticed() {
        let (out, unsorted) = joined_of("", "b 1\na 2\n", "a x\nb y\n");
        assert_eq!(unsorted, [true, false]);
        assert_eq!(out, "b 1 y\n");
        assert_eq!(joined_of("", ONE, TWO).1, [false, false]);
        assert_eq!(joined_of("", "a\n", "c\nb\n").1, [false, true]);
    }

    #[test]
    fn fields_part_at_blanks_or_at_a_separator() {
        let f = |line: &str, separator: Option<&str>| -> Vec<String> {
            fields(line.as_bytes(), separator.map(str::as_bytes))
                .iter()
                .map(|f| String::from_utf8(f.to_vec()).unwrap())
                .collect()
        };
        assert_eq!(f(" a \t b  ", None), ["a", "b"]);
        assert_eq!(f("", None), [""; 0]);
        assert_eq!(f("a::b:", Some(":")), ["a", "", "b", ""]);
        assert_eq!(f("", Some(":")), [""]);
        assert_eq!(f("a\u{e9}b", Some("\u{e9}")), ["a", "b"]);
    }

    #[test]
    fn options_and_the_two_files() {
        let given = words("-a1 -v 2 -e x -o 0,2.3 -t ; -1 4 -2 5 f -");
        let asked = asked(&given).unwrap();
        assert_eq!(asked.unpaired, [true, true]);
        assert!(asked.only_unpaired);
        assert_eq!(asked.empty.as_deref(), Some("x"));
        assert_eq!(asked.list, Some(alloc::vec![Field::Join, Field::Of(1, 2)]));
        assert_eq!(asked.separator.as_deref(), Some(";"));
        assert_eq!(asked.fields, [3, 4]);
        assert_eq!(asked.files, ["f", "-"]);
        let given = words("a b");
        let plain = super::asked(&given).unwrap();
        assert_eq!((plain.unpaired, plain.only_unpaired), ([false; 2], false));
        assert_eq!(
            (plain.fields, plain.list, plain.separator),
            ([0; 2], None, None)
        );
        let given = words("-- -a b");
        assert_eq!(super::asked(&given).unwrap().files, ["-a", "b"]);
        assert_eq!(
            field_list("1.1 2.2,0").unwrap(),
            [Field::Of(0, 0), Field::Of(1, 1), Field::Join]
        );
    }

    #[test]
    fn options_that_do_not_read() {
        let wrong = |options: &str| asked(&words(options)).unwrap_err();
        assert_eq!(wrong("a"), "join: joins two files\n");
        assert_eq!(wrong("a b c"), "join: joins two files\n");
        assert_eq!(
            wrong("- -"),
            "join: reads the standard input for one file only\n"
        );
        assert_eq!(wrong("-x a b"), "join: -x is no option\n");
        assert_eq!(wrong("-a"), "join: -a wants its word\n");
        assert_eq!(wrong("-a 3 a b"), "join: -a takes 1 or 2, not 3\n");
        assert_eq!(wrong("-v 0 a b"), "join: -v takes 1 or 2, not 0\n");
        assert_eq!(wrong("-1 0 a b"), "join: -1 wants a field from 1, not 0\n");
        assert_eq!(wrong("-2 x a b"), "join: -2 wants a field from 1, not x\n");
        assert_eq!(wrong("-t ab a b"), "join: -t takes one character, not ab\n");
        assert_eq!(
            wrong("-o 3.1 a b"),
            "join: 3.1 is no field; -o takes 0 or a file and a field, as 2.3\n"
        );
        assert_eq!(
            wrong("-o 1.0 a b"),
            "join: 1.0 is no field; -o takes 0 or a file and a field, as 2.3\n"
        );
        assert_eq!(wrong("-o , a b"), "join: -o wants fields\n");
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// A sorted text of random lines, a key of one letter and a value.
    fn random_sorted(seed: &mut u64, values: &str) -> Vec<String> {
        let mut lines: Vec<String> = (0..shuffled(seed) % 12)
            .map(|n| {
                let key = char::from(b'a' + u8::try_from(shuffled(seed) % 5).unwrap());
                alloc::format!("{key} {values}{n}")
            })
            .collect();
        lines.sort_by(|a, b| a[..1].cmp(&b[..1]));
        lines
    }

    #[test]
    fn random_files_join_as_every_pair_with_the_same_key() {
        let mut seed = 0x6c07_8965_5d0c_2bb3_u64;
        for _ in 0..300 {
            let a = random_sorted(&mut seed, "p");
            let b = random_sorted(&mut seed, "q");
            let text = |lines: &[String]| lines.iter().fold(String::new(), |all, l| all + l + "\n");
            let mut want = String::new();
            for x in &a {
                for y in b.iter().filter(|y| y[..1] == x[..1]) {
                    want += &[x, " ", &y[2..], "\n"].concat();
                }
            }
            let (got, unsorted) = joined_of("", &text(&a), &text(&b));
            assert_eq!(got, want);
            assert_eq!(unsorted, [false, false]);
            let alone = |mine: &[String], theirs: &[String]| {
                mine.iter()
                    .filter(|m| theirs.iter().all(|t| t[..1] != m[..1]))
                    .count()
            };
            let unpaired = join_of("-v 1 -v 2", &text(&a), &text(&b));
            assert_eq!(unpaired.lines().count(), alone(&a, &b) + alone(&b, &a));
            let every = join_of("-a 1 -a 2", &text(&a), &text(&b));
            assert_eq!(
                every.lines().count(),
                want.lines().count() + alone(&a, &b) + alone(&b, &a)
            );
        }
    }

    #[test]
    fn a_group_pairs_in_the_order_of_both_files() {
        assert_eq!(
            join_of("", "k 1\nk 2\nk 3\n", "k x\nk y\n"),
            "k 1 x\nk 1 y\nk 2 x\nk 2 y\nk 3 x\nk 3 y\n"
        );
        assert_eq!(
            join_of("", "a 0\nk 1\nk 2\nz 9\n", "k x\n"),
            "k 1 x\nk 2 x\n",
            "a group between others"
        );
        assert_eq!(
            join_of("-v 1", "k 1\nk 2\n", "k x\n"),
            "",
            "a whole group pairs"
        );
    }

    #[test]
    fn the_second_file_s_lines_alone_take_their_own_join_field() {
        assert_eq!(join_of("-a 2 -2 2", "", "x k y\n"), "k x y\n");
        assert_eq!(
            join_of("-a 2 -o 0,1.1,2.2 -e E", "", "k w\n"),
            "k E w\n",
            "the first file's fields are missing"
        );
        assert_eq!(join_of("-a 1 -o 2.1,0 -e E -1 2", "x k\n", ""), "E k\n");
        assert_eq!(
            join_of("-a 2 -o 1.1", "", "k w\n"),
            "\n",
            "missing and no -e"
        );
    }

    #[test]
    fn separators_and_join_fields_together() {
        assert_eq!(
            join_of("-t : -1 2 -2 3", "x:k:y\n", "p:q:k\n"),
            "k:x:y:p:q\n"
        );
        assert_eq!(join_of("-t : -o 2.3,1.1", "k:1\n", "k:2:3\n"), "3:k\n");
        assert_eq!(
            join_of("-t :", ":a\n:b\n", ":c\n"),
            ":a:c\n:b:c\n",
            "an empty join field pairs with an empty one"
        );
        assert_eq!(
            join_of("", "   \n", "\n"),
            "\n",
            "lines of blanks have an empty field"
        );
    }

    #[test]
    fn a_group_is_read_whole_whatever_the_pieces() {
        let text = "a 1\nb 2\nb 3\nb 4\nc 5\n";
        let given = words("x y");
        let plain = asked(&given).unwrap();
        let writing = Writing { asked: &plain };
        for size in 1..=text.len() {
            let pieces = Cut(text.as_bytes().chunks(size).map(<[u8]>::to_vec).collect());
            let mut side = Side::new(pieces);
            let groups: Vec<usize> = (0..4)
                .map(|_| side.group(&writing, 0).unwrap().len())
                .collect();
            assert_eq!(groups, [1, 3, 1, 0], "pieces of {size}");
            assert!(!side.unsorted);
        }
    }

    #[test]
    fn a_line_is_its_fields_apart_by_the_separator() {
        let given = words("-t ; x y");
        let asked = asked(&given).unwrap();
        let writing = Writing { asked: &asked };
        assert_eq!(writing.line([Some(b"k;1"), Some(b"k;2;3")]), b"k;1;2;3\n");
        assert_eq!(writing.line([Some(b"k;1"), None]), b"k;1\n");
        assert_eq!(writing.line([None, Some(b"k;2")]), b"k;2\n");
        assert_eq!(writing.key(1, b";x"), b"");
        assert_eq!(writing.key(0, b""), b"");
    }

    #[test]
    fn random_files_with_a_separator_join_as_every_pair() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..200 {
            let a = random_sorted(&mut seed, "p");
            let b = random_sorted(&mut seed, "q");
            let colons = |lines: &[String]| {
                lines
                    .iter()
                    .fold(String::new(), |all, l| all + &l.replace(' ', ":") + "\n")
            };
            let mut want = String::new();
            for x in &a {
                for y in b.iter().filter(|y| y[..1] == x[..1]) {
                    want += &[&x.replace(' ', ":"), ":", &y[2..], "\n"].concat();
                }
            }
            assert_eq!(join_of("-t :", &colons(&a), &colons(&b)), want);
        }
    }
}
