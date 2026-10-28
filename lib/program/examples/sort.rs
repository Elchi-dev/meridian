//! sort: the lines of its files, or of its standard input, all together
//! in order, as POSIX's sort writes them in the C locale: by their bytes,
//! or by the keys `-k` names, each a part of the line from a field and a
//! character to another, fields parted by `-t` or by runs of blanks.
//! `-n` compares numbers, `-f` capitals as small letters, `-d` only
//! blanks, letters and digits, `-i` only printable characters, `-b`
//! leaves out the blanks a field starts with, and `-r` turns the order
//! round, for all keys or attached to one for it alone. Lines whose keys
//! compare the same come in the order of their bytes, unless `-u` keeps
//! only the first of them. `-c` says whether the input is in order and
//! where it is not, `-C` only ends with 1 then, `-o` writes into a file,
//! which may be one of the inputs, and `-m` merges sorted files, which
//! sorting them all does as well. `-` is the standard input.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use abi::file::{CREATE_ALWAYS, WRITE};
use program::text::{self, Lines};
use program::{Error, Handle};

program::main!(sort);

/// Which bytes of a key count: with `-d` only blanks, letters and
/// digits, with `-i` only printable ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Kept {
    dictionary: bool,
    printable: bool,
}

/// How a key compares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Order {
    /// Leaves out the blanks a field starts with, at the start and at the
    /// end of a key.
    blanks: (bool, bool),
    kept: Kept,
    fold: bool,
    numeric: bool,
    reverse: bool,
}

impl Order {
    /// Takes the type letter `letter`, whose b leaves out blanks before
    /// a key's start or its end as `ends` says; whether it is one.
    fn take(&mut self, letter: char, ends: (bool, bool)) -> bool {
        match letter {
            'b' => {
                self.blanks.0 |= ends.0;
                self.blanks.1 |= ends.1;
            }
            'd' => self.kept.dictionary = true,
            'f' => self.fold = true,
            'i' => self.kept.printable = true,
            'n' => self.numeric = true,
            'r' => self.reverse = true,
            _ => return false,
        }
        true
    }
}

/// A key: from a field and a character, counted from 1, to a field and a
/// character, 0 for the field's last, or else the end of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    start: (usize, usize),
    end: Option<(usize, usize)>,
    /// Its own order, none when it takes the one given for all.
    order: Option<Order>,
}

/// Whether the input is only checked, and whether that is said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Check {
    Loud,
    Quiet,
}

/// What sort's words ask for.
#[derive(Debug, PartialEq)]
struct Asked<'a> {
    order: Order,
    keys: Vec<Key>,
    separator: Option<String>,
    unique: bool,
    check: Option<Check>,
    output: Option<&'a str>,
    files: &'a [String],
}

/// A position as `-k` takes it: a field, and a character after a dot,
/// and the type letters after them.
fn position<'v>(text: &'v str, value: &str) -> Result<(usize, Option<usize>, &'v str), String> {
    let digits = |s: &'v str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let wrong = || format!("sort: {value} is no key\n");
    let field_len = digits(text);
    let field = text[..field_len].parse::<usize>().map_err(|_| wrong())?;
    let rest = &text[field_len..];
    let Some(after_dot) = rest.strip_prefix('.') else {
        return Ok((field, None, rest));
    };
    let char_len = digits(after_dot);
    let character = after_dot[..char_len]
        .parse::<usize>()
        .map_err(|_| wrong())?;
    Ok((field, Some(character), &after_dot[char_len..]))
}

/// The key `-k` names.
fn key(value: &str) -> Result<Key, String> {
    let wrong = || format!("sort: {value} is no key\n");
    let (start, end) = match value.split_once(',') {
        Some((start, end)) => (start, Some(end)),
        None => (value, None),
    };
    let mut order = Order::default();
    let mut own = false;
    let (field, character, letters) = position(start, value)?;
    if field == 0 || character == Some(0) {
        return Err(wrong());
    }
    for letter in letters.chars() {
        own = true;
        if !order.take(letter, (true, false)) {
            return Err(wrong());
        }
    }
    let start = (field, character.unwrap_or(1));
    let end = match end {
        Some(end) => {
            let (field, character, letters) = position(end, value)?;
            if field == 0 {
                return Err(wrong());
            }
            for letter in letters.chars() {
                own = true;
                if !order.take(letter, (false, true)) {
                    return Err(wrong());
                }
            }
            Some((field, character.unwrap_or(0)))
        }
        None => None,
    };
    Ok(Key {
        start,
        end,
        order: own.then_some(order),
    })
}

/// What sort's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        order: Order::default(),
        keys: Vec::new(),
        separator: None,
        unique: false,
        check: None,
        output: None,
        files: &[],
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
        for (at, letter) in letters.char_indices() {
            if asked.order.take(letter, (true, true)) {
                continue;
            }
            match letter {
                'u' => asked.unique = true,
                'c' => asked.check = Some(Check::Loud),
                'C' => asked.check = Some(Check::Quiet),
                'm' => {}
                'k' | 't' | 'o' => {
                    let after = &letters[at + letter.len_utf8()..];
                    let value = if after.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("sort: -{letter} wants its word\n"))?;
                        words = rest;
                        value.as_str()
                    } else {
                        after
                    };
                    match letter {
                        'k' => asked.keys.push(key(value)?),
                        't' if value.chars().count() == 1 => {
                            asked.separator = Some(String::from(value));
                        }
                        't' => return Err(format!("sort: -t takes one character, not {value}\n")),
                        _ => asked.output = Some(value),
                    }
                    break;
                }
                _ => return Err(format!("sort: -{letter} is no option\n")),
            }
        }
    }
    if asked.check.is_some() && words.len() > 1 {
        return Err(String::from("sort: checks one file at most\n"));
    }
    asked.files = words;
    Ok(asked)
}

/// Whether `byte` is a blank.
fn blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

/// Where the character after `count` characters from `at` starts in
/// `line`, at most its end.
fn characters_on(line: &[u8], at: usize, count: usize) -> usize {
    let mut at = at;
    for _ in 0..count {
        if at >= line.len() {
            return line.len();
        }
        at += 1;
        while line.get(at).is_some_and(|b| b & 0xc0 == 0x80) {
            at += 1;
        }
    }
    at
}

/// Where field `field`, counted from 1, starts and ends in `line`: parted
/// by `separator`, or else each blanks and then what is not blank. A field
/// past the last starts and ends at the line's end.
fn field_at(line: &[u8], field: usize, separator: Option<&[u8]>) -> (usize, usize) {
    let mut start = 0;
    for n in 1.. {
        let end = if let Some(separator) = separator {
            line[start..]
                .windows(separator.len())
                .position(|w| w == separator)
                .map_or(line.len(), |at| start + at)
        } else {
            let blanks = line[start..].iter().take_while(|&&b| blank(b)).count();
            let word = line[start + blanks..]
                .iter()
                .take_while(|&&b| !blank(b))
                .count();
            start + blanks + word
        };
        if n == field {
            return (start, end);
        }
        if end >= line.len() {
            return (line.len(), line.len());
        }
        start = end + separator.map_or(0, <[u8]>::len);
    }
    (line.len(), line.len())
}

/// The part of `line` `key` names, compared as `order` says.
fn key_of<'l>(line: &'l [u8], key: &Key, order: Order, separator: Option<&[u8]>) -> &'l [u8] {
    let skip = |at: usize, end: usize, skipping: bool| {
        if skipping {
            at + line[at..end].iter().take_while(|&&b| blank(b)).count()
        } else {
            at
        }
    };
    let (field_start, field_end) = field_at(line, key.start.0, separator);
    let first = skip(field_start, field_end, order.blanks.0);
    let start = characters_on(line, first, key.start.1 - 1);
    let end = match key.end {
        None => line.len(),
        Some((field, 0)) => field_at(line, field, separator).1,
        // Counted from the field's start, an end past the field goes on
        // into the line, as GNU's sort has it.
        Some((field, character)) => {
            let (from, to) = field_at(line, field, separator);
            characters_on(line, skip(from, to, order.blanks.1), character)
        }
    };
    line.get(start..end).unwrap_or_default()
}

/// A number as `-n` reads one from the start of `text`: blanks, a minus,
/// digits and a point with digits after it; whether it is below zero,
/// and its whole and its fraction, without zeros that change nothing.
fn number(text: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut rest = &text[text.iter().take_while(|&&b| blank(b)).count()..];
    let negative = rest.first() == Some(&b'-');
    if negative {
        rest = &rest[1..];
    }
    let whole_len = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    let whole = &rest[..whole_len];
    let whole = &whole[whole.iter().take_while(|&&b| b == b'0').count()..];
    let fraction = match rest[whole_len..].strip_prefix(b".") {
        Some(after) => {
            let digits = &after[..after.iter().take_while(|b| b.is_ascii_digit()).count()];
            &digits[..digits.len() - digits.iter().rev().take_while(|&&b| b == b'0').count()]
        }
        None => &[],
    };
    let zero = whole.is_empty() && fraction.is_empty();
    (negative && !zero, whole, fraction)
}

/// How two numbers compare.
fn numerically(a: &[u8], b: &[u8]) -> Ordering {
    let (a_negative, a_whole, a_fraction) = number(a);
    let (b_negative, b_whole, b_fraction) = number(b);
    let size = a_whole
        .len()
        .cmp(&b_whole.len())
        .then_with(|| a_whole.cmp(b_whole))
        .then_with(|| a_fraction.cmp(b_fraction));
    match (a_negative, b_negative) {
        (false, false) => size,
        (true, true) => size.reverse(),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    }
}

/// How two keys compare as `order` says.
fn compare_keys(a: &[u8], b: &[u8], order: Order) -> Ordering {
    let compared = if order.numeric {
        numerically(a, b)
    } else {
        let kept = |byte: &&u8| {
            (!order.kept.dictionary || blank(**byte) || byte.is_ascii_alphanumeric())
                && (!order.kept.printable || (b' '..=b'~').contains(*byte))
        };
        let folded = |byte: &u8| {
            if order.fold {
                byte.to_ascii_uppercase()
            } else {
                *byte
            }
        };
        a.iter()
            .filter(kept)
            .map(folded)
            .cmp(b.iter().filter(kept).map(folded))
    };
    if order.reverse {
        compared.reverse()
    } else {
        compared
    }
}

/// How sort orders two lines: by each key in turn, and then, unless only
/// the first of lines alike is kept, by their bytes.
fn compare(a: &[u8], b: &[u8], asked: &Asked) -> Ordering {
    let separator = asked.separator.as_deref().map(str::as_bytes);
    let whole = Key {
        start: (1, 1),
        end: None,
        order: None,
    };
    let keys = if asked.keys.is_empty() {
        core::slice::from_ref(&whole)
    } else {
        &asked.keys
    };
    for key in keys {
        let order = key.order.unwrap_or(asked.order);
        let compared = compare_keys(
            key_of(a, key, order, separator),
            key_of(b, key, order, separator),
            order,
        );
        if compared != Ordering::Equal {
            return compared;
        }
    }
    if asked.unique {
        return Ordering::Equal;
    }
    let bytes = a.cmp(b);
    if asked.order.reverse {
        bytes.reverse()
    } else {
        bytes
    }
}

/// The lines of every text given, each line's last one too.
#[derive(Default)]
struct All {
    lines: Vec<Vec<u8>>,
}

impl All {
    /// Takes in the whole of one text, piece by piece from `pieces`.
    fn take<E>(
        &mut self,
        pieces: impl FnOnce(&mut dyn FnMut(&[u8]) -> bool) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut cut = Lines::default();
        let all = &mut self.lines;
        pieces(&mut |piece| {
            cut.feed(piece, |line| {
                all.push(line.to_vec());
                true
            })
        })?;
        cut.finish(|line| all.push(line.to_vec()));
        Ok(())
    }

    /// Gives `out` the lines in order, each with its end, the first of
    /// lines alike alone when only that is kept.
    fn sorted(mut self, asked: &Asked, out: &mut impl FnMut(&[u8])) {
        self.lines.sort_by(|a, b| compare(a, b, asked));
        let mut last: Option<&Vec<u8>> = None;
        for line in &self.lines {
            if asked.unique
                && last.is_some_and(|last| compare(last, line, asked) == Ordering::Equal)
            {
                continue;
            }
            out(line);
            out(b"\n");
            last = Some(line);
        }
    }

    /// The first line, counted from 1, that comes too early after the
    /// one before it: one that would sort before it, or with `-u` one
    /// that is alike.
    fn disorder(&self, asked: &Asked) -> Option<(usize, &[u8])> {
        self.lines.windows(2).enumerate().find_map(|(at, pair)| {
            let order = compare(&pair[0], &pair[1], asked);
            let wrong = order == Ordering::Greater || asked.unique && order == Ordering::Equal;
            wrong.then_some((at + 2, pair[1].as_slice()))
        })
    }
}

/// Writes `all` in order with `write`; answers whether every write went
/// through.
#[cfg_attr(test, allow(dead_code))]
fn written(all: All, asked: &Asked, write: impl FnMut(&[u8]) -> Result<(), Error>) -> bool {
    let mut out = text::Out::new(write);
    all.sorted(asked, &mut |bytes| out.put(bytes));
    out.finish()
}

#[cfg_attr(test, allow(dead_code))]
fn sort(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let mut all = All::default();
    for path in text::sources(asked.files) {
        let path = path.filter(|&p| p != "-");
        if let Err(e) = all.take(|each| text::pieces(path, each)) {
            let _ = program::write_error(&text::trouble("sort", path.unwrap_or("-"), e));
            return 2;
        }
    }
    if let Some(check) = asked.check {
        let Some((line, text)) = all.disorder(&asked) else {
            return 0;
        };
        if check == Check::Loud {
            let name = asked.files.first().map_or("-", String::as_str);
            let text = String::from_utf8_lossy(text);
            let _ = program::write_error(&format!("sort: {name}:{line}: disorder: {text}\n"));
        }
        return 1;
    }
    let whole = match asked.output {
        Some(path) => match program::open_with(path, WRITE, CREATE_ALWAYS) {
            Ok(file) => {
                let mut at = 0;
                let whole = written(all, &asked, |bytes| {
                    program::write_file(file, at, bytes)?;
                    at += bytes.len() as u64;
                    Ok(())
                });
                let _ = program::close(file);
                whole
            }
            Err(e) => {
                let _ = program::write_error(&text::trouble("sort", path, e));
                return 2;
            }
        },
        None => written(all, &asked, program::write_bytes),
    };
    u64::from(!whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    /// The lines of `texts`, each read in pieces of three bytes.
    fn all_of(texts: &[&str]) -> All {
        let mut all = All::default();
        for text in texts {
            let fed: Result<(), ()> = all.take(|each| {
                for piece in text.as_bytes().chunks(3) {
                    if !each(piece) {
                        break;
                    }
                }
                Ok(())
            });
            assert_eq!(fed, Ok(()));
        }
        all
    }

    /// What sort asked with `options` writes for `texts`.
    fn sorted_of(options: &str, texts: &[&str]) -> String {
        let given = words(options);
        let asked = asked(&given).unwrap();
        let mut out = Vec::new();
        all_of(texts).sorted(&asked, &mut |bytes| out.extend_from_slice(bytes));
        String::from_utf8(out).unwrap()
    }

    fn sorted(texts: &[&str], reversed: bool) -> String {
        sorted_of(if reversed { "-r" } else { "" }, texts)
    }

    #[test]
    fn lines_come_in_the_order_of_their_bytes() {
        assert_eq!(sorted(&["b\na\nc\n"], false), "a\nb\nc\n");
        assert_eq!(sorted(&["b\na\nc\n"], true), "c\nb\na\n");
        assert_eq!(sorted(&["B\na\nA\n"], false), "A\nB\na\n", "capitals first");
        assert_eq!(sorted(&["two\none", "three\n"], false), "one\nthree\ntwo\n");
        assert_eq!(
            sorted(&["a\n\nb\n"], false),
            "\na\nb\n",
            "an empty line first"
        );
        assert_eq!(sorted(&["ab\na\n"], false), "a\nab\n");
        assert_eq!(sorted(&[], false), "");
        assert_eq!(sorted(&["b\na\nb\na\n"], false), "a\na\nb\nb\n");
        assert_eq!(sorted(&["b\na\nb\n"], true), "b\nb\na\n");
        assert_eq!(
            sorted(&["\u{e9}\nz\ne\n"], false),
            "e\nz\n\u{e9}\n",
            "by their bytes, not their letters"
        );
        assert_eq!(sorted(&["last", "first\n"], false), "first\nlast\n");
    }

    #[test]
    fn a_text_that_stops_early_keeps_what_came() {
        let mut all = All::default();
        let fed: Result<(), &str> = all.take(|each| {
            let _ = each(b"b\na");
            Err("the disk could not be read")
        });
        assert_eq!(fed, Err("the disk could not be read"));
        let mut out = Vec::new();
        all.sorted(&asked(&[]).unwrap(), &mut |bytes| {
            out.extend_from_slice(bytes);
        });
        assert_eq!(out, b"b\n");
    }

    #[test]
    fn keys_name_fields_and_characters() {
        assert_eq!(
            sorted_of("-k 2n", &["b 2\na 10\nc 1\n"]),
            "c 1\nb 2\na 10\n"
        );
        assert_eq!(
            sorted_of("-k 2", &["x  b\nx a\n"]),
            "x  b\nx a\n",
            "blanks before a field belong to it"
        );
        assert_eq!(sorted_of("-k 2b", &["x  b\nx a\n"]), "x a\nx  b\n");
        assert_eq!(sorted_of("-b -k 2", &["x  b\nx a\n"]), "x a\nx  b\n");
        assert_eq!(
            sorted_of("-b -k 2,2.1", &["y  a\nx  b\n"]),
            "y  a\nx  b\n",
            "-b given for all keys leaves out blanks before their ends too"
        );
        assert_eq!(sorted_of("-k 2,2.1", &["y  a\nx  b\n"]), "x  b\ny  a\n");
        assert_eq!(
            sorted_of("-b", &[" b\na\n"]),
            "a\n b\n",
            "-b without keys too"
        );
        assert_eq!(sorted_of("", &[" b\na\n"]), " b\na\n");
        assert_eq!(
            sorted_of("-t : -k 2,2nr", &["a:3\nb:1\nc:2\n"]),
            "a:3\nc:2\nb:1\n"
        );
        assert_eq!(
            sorted_of("-k 1.2,1.2", &["abc\nabd\naac\n"]),
            "aac\nabc\nabd\n"
        );
        assert_eq!(
            sorted_of("-k 2,2 -k 1,1r", &["a 1\nb 1\nc 0\n"]),
            "c 0\nb 1\na 1\n",
            "the second key when the first compares the same"
        );
        assert_eq!(sorted_of("-t, -k3", &["a,b\nb,a,c\n"]), "a,b\nb,a,c\n");
        assert_eq!(
            sorted_of("-k 1.2,1.9 -k 2r", &["ab z\nab a\n"]),
            "ab a\nab z\n"
        );
    }

    #[test]
    fn ties_go_by_the_bytes_turned_round_only_by_an_r_for_all() {
        assert_eq!(sorted_of("-k 2,2 -r", &["a 1\nb 1\n"]), "b 1\na 1\n");
        assert_eq!(sorted_of("-k 2,2r", &["b 1\na 1\n"]), "a 1\nb 1\n");
        assert_eq!(sorted_of("-r -n", &["1\n2\n"]), "2\n1\n");
    }

    #[test]
    fn orders_for_folding_dictionaries_and_printables() {
        assert_eq!(sorted_of("-f", &["a\nB\nb\nA\n"]), "A\na\nB\nb\n");
        assert_eq!(
            sorted_of("-fu", &["a\nB\nb\nA\n"]),
            "a\nB\n",
            "the first of those alike"
        );
        assert_eq!(sorted_of("-d", &["a-b\nab\na b\n"]), "a b\na-b\nab\n");
        assert_eq!(sorted_of("-i", &["a\u{1}c\nab\n"]), "ab\na\u{1}c\n");
        assert_eq!(sorted_of("-u", &["b\na\nb\n"]), "a\nb\n");
    }

    #[test]
    fn numbers_compare_by_their_value() {
        assert_eq!(
            sorted_of("-n", &["10\n9\n-1\n-10\n0\n-0\n1.5\n1.50\n.5\nx\n"]),
            "-10\n-1\n-0\n0\nx\n.5\n1.5\n1.50\n9\n10\n"
        );
        assert_eq!(number(b"  -007.500x"), (true, &b"7"[..], &b"5"[..]));
        assert_eq!(
            number(b"-0.0"),
            (false, &b""[..], &b""[..]),
            "minus zero is zero"
        );
        assert_eq!(number(b""), (false, &b""[..], &b""[..]));
        assert_eq!(
            numerically(b"100000000000000000000000", b"99"),
            Ordering::Greater
        );
        assert_eq!(numerically(b"-2", b"-10"), Ordering::Greater);
        assert_eq!(numerically(b"1.05", b"1.5"), Ordering::Less);
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..2000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let [a0, a1, a2, a3, b0, b1, b2, b3] = seed.to_le_bytes();
            let a = i64::from(i32::from_le_bytes([a0, a1, a2, a3])) >> (seed % 24);
            let b = i64::from(i32::from_le_bytes([b0, b1, b2, b3])) >> (seed % 20);
            let texts = (alloc::format!("{a}"), alloc::format!("{b}"));
            assert_eq!(
                numerically(texts.0.as_bytes(), texts.1.as_bytes()),
                a.cmp(&b),
                "{a} {b}"
            );
        }
    }

    #[test]
    fn fields_and_keys_of_a_line() {
        assert_eq!(field_at(b"a  b c", 1, None), (0, 1));
        assert_eq!(
            field_at(b"a  b c", 2, None),
            (1, 4),
            "with the blanks before it"
        );
        assert_eq!(field_at(b"a  b c", 3, None), (4, 6));
        assert_eq!(field_at(b"a  b c", 4, None), (6, 6));
        assert_eq!(field_at(b"a::b", 2, Some(b":")), (2, 2));
        assert_eq!(field_at(b"a::b", 3, Some(b":")), (3, 4));
        assert_eq!(field_at(b"a::b", 4, Some(b":")), (4, 4));
        let key = |text: &str| super::key(text).unwrap();
        let order = Order::default();
        assert_eq!(key_of(b"ab cd ef", &key("2"), order, None), b" cd ef");
        assert_eq!(key_of(b"ab cd ef", &key("2,2"), order, None), b" cd");
        assert_eq!(key_of(b"ab cd ef", &key("2.2,2.3"), order, None), b"cd");
        assert_eq!(
            key_of(
                b"ab cd ef",
                &key("2.2b,2.3b"),
                Order {
                    blanks: (true, true),
                    ..order
                },
                None
            ),
            b"d "
        );
        assert_eq!(key_of(b"ab", &key("3"), order, None), b"");
        assert_eq!(
            key_of(b"ab cd", &key("2,1"), order, None),
            b"",
            "an end before the start"
        );
        assert_eq!(
            key_of("\u{e9}x y".as_bytes(), &key("1.2,1.2"), order, None),
            b"x"
        );
        assert_eq!(
            key_of(b"ab cd", &key("1.2,1.4"), order, None),
            b"b c",
            "an end past its field goes on into the line"
        );
        assert_eq!(key_of(b"ab cd", &key("1.2,1.9"), order, None), b"b cd");
    }

    #[test]
    fn keys_as_k_takes_them() {
        let own = |f: fn(&mut Order)| {
            let mut order = Order::default();
            f(&mut order);
            Some(order)
        };
        assert_eq!(
            key("2"),
            Ok(Key {
                start: (2, 1),
                end: None,
                order: None
            })
        );
        assert_eq!(
            key("2,3"),
            Ok(Key {
                start: (2, 1),
                end: Some((3, 0)),
                order: None
            })
        );
        assert_eq!(
            key("1.2,4.5"),
            Ok(Key {
                start: (1, 2),
                end: Some((4, 5)),
                order: None
            })
        );
        assert_eq!(key("2n").unwrap().order, own(|o| o.numeric = true));
        assert_eq!(
            key("2,2nr").unwrap().order,
            own(|o| {
                o.numeric = true;
                o.reverse = true;
            })
        );
        assert_eq!(
            key("1b,1").unwrap().order,
            own(|o| o.blanks = (true, false))
        );
        assert_eq!(
            key("1,1b").unwrap().order,
            own(|o| o.blanks = (false, true))
        );
        assert_eq!(
            key("3df").unwrap().order,
            own(|o| {
                o.kept.dictionary = true;
                o.fold = true;
            })
        );
        assert_eq!(key("1i").unwrap().order, own(|o| o.kept.printable = true));
        for wrong in ["0", "1.0", "x", "2q", "1,0", ",1", "1.", "", "1,2.x"] {
            assert_eq!(
                key(wrong),
                Err(alloc::format!("sort: {wrong} is no key\n")),
                "{wrong}"
            );
        }
    }

    #[test]
    fn options_and_the_files() {
        let given = words("-nru -t : -k 2 -k3,3 -o out -m a b");
        let asked = super::asked(&given).unwrap();
        assert!(asked.order.numeric && asked.order.reverse && asked.unique);
        assert_eq!(asked.separator.as_deref(), Some(":"));
        assert_eq!(asked.keys.len(), 2);
        assert_eq!(asked.output, Some("out"));
        assert_eq!(asked.files, ["a", "b"]);
        assert_eq!(asked.check, None);
        let given = words("-b");
        assert_eq!(super::asked(&given).unwrap().order.blanks, (true, true));
        let given = words("-c f");
        assert_eq!(super::asked(&given).unwrap().check, Some(Check::Loud));
        let given = words("-C");
        assert_eq!(super::asked(&given).unwrap().check, Some(Check::Quiet));
        let given = words("a -r");
        assert_eq!(
            super::asked(&given).unwrap().files,
            ["a", "-r"],
            "an option comes first"
        );
        let wrong = |options: &str| super::asked(&words(options)).unwrap_err();
        assert_eq!(wrong("-x"), "sort: -x is no option\n");
        assert_eq!(wrong("-k"), "sort: -k wants its word\n");
        assert_eq!(wrong("-k 0"), "sort: 0 is no key\n");
        assert_eq!(wrong("-t ab"), "sort: -t takes one character, not ab\n");
        assert_eq!(wrong("-c a b"), "sort: checks one file at most\n");
    }

    #[test]
    fn a_check_finds_the_first_line_out_of_order() {
        let plain = asked(&[]).unwrap();
        assert_eq!(
            all_of(&["b\na\nc\n"]).disorder(&plain),
            Some((2, &b"a"[..]))
        );
        assert_eq!(all_of(&["a\nb\nb\n"]).disorder(&plain), None);
        assert_eq!(all_of(&[""]).disorder(&plain), None);
        let given = words("-u");
        let unique = asked(&given).unwrap();
        assert_eq!(
            all_of(&["a\nb\nb\n"]).disorder(&unique),
            Some((3, &b"b"[..]))
        );
        let given = words("-n");
        let numeric = asked(&given).unwrap();
        assert_eq!(all_of(&["2\n10\n"]).disorder(&numeric), None);
        assert_eq!(all_of(&["2\n10\n"]).disorder(&plain), Some((2, &b"10"[..])));
    }

    #[test]
    fn random_lines_come_out_in_order_with_every_option() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let options = [
            "",
            "-r",
            "-n",
            "-f",
            "-d",
            "-k 2",
            "-k 2b,2n",
            "-t : -k 2r",
            "-u",
            "-fu -k 1,1",
        ];
        for _ in 0..200 {
            let mut lines = Vec::new();
            for _ in 0..seed % 20 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let parts = ["a", "B", "-3", "10", " 2", ":", "b:1", "x y", ""];
                let first = parts[usize::try_from(seed % 9).unwrap()];
                let second = parts[usize::try_from((seed >> 8) % 9).unwrap()];
                lines.push(alloc::format!("{first} {second}"));
            }
            let text = lines.iter().fold(String::new(), |all, l| all + l + "\n");
            for option in options {
                let given = words(option);
                let asked = super::asked(&given).unwrap();
                let out = sorted_of(option, &[&text]);
                let all = all_of(&[&out]);
                assert_eq!(all.disorder(&asked), None, "{option} {text:?}");
                if !asked.unique {
                    let mut sorted_in: Vec<&str> = text.lines().collect();
                    let mut sorted_out: Vec<&str> = out.lines().collect();
                    sorted_in.sort_unstable();
                    sorted_out.sort_unstable();
                    assert_eq!(sorted_in, sorted_out, "the same lines, {option}");
                }
            }
        }
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// Random lines of three fields parted by colons, from a few values.
    fn random_fields(seed: &mut u64) -> Vec<String> {
        let values = ["a", "b", "B", "", "10", "9", "-2", "ab"];
        (0..shuffled(seed) % 16)
            .map(|_| {
                let mut fields = [""; 3];
                for field in &mut fields {
                    *field = values[usize::try_from(shuffled(seed) % 8).unwrap()];
                }
                fields.join(":")
            })
            .collect()
    }

    fn text_of(lines: &[String]) -> String {
        lines.iter().fold(String::new(), |all, l| all + l + "\n")
    }

    fn field(line: &str, n: usize) -> &str {
        line.split(':').nth(n - 1).unwrap_or("")
    }

    #[test]
    fn t_keys_sort_by_the_field_they_name_and_then_the_line() {
        let mut seed = 0x0123_4567_89ab_cdef_u64;
        for _ in 0..200 {
            let lines = random_fields(&mut seed);
            for n in 1..=3 {
                let mut want = lines.clone();
                want.sort_by(|a, b| field(a, n).cmp(field(b, n)).then_with(|| a.cmp(b)));
                assert_eq!(
                    sorted_of(&alloc::format!("-t : -k {n},{n}"), &[&text_of(&lines)]),
                    text_of(&want)
                );
                let mut back = lines.clone();
                back.sort_by(|a, b| field(b, n).cmp(field(a, n)).then_with(|| a.cmp(b)));
                assert_eq!(
                    sorted_of(&alloc::format!("-t : -k {n},{n}r"), &[&text_of(&lines)]),
                    text_of(&back),
                    "an r of the key's own turns only the key round"
                );
            }
        }
    }

    #[test]
    fn numeric_keys_sort_as_the_numbers_they_hold() {
        let mut seed = 0x6c07_8965_5d0c_2bb3_u64;
        for _ in 0..200 {
            let lines: Vec<String> = (0..shuffled(&mut seed) % 16)
                .map(|n| {
                    let value = i64::try_from(shuffled(&mut seed) % 2001).unwrap() - 1000;
                    alloc::format!("{n}:{value}")
                })
                .collect();
            let mut want = lines.clone();
            let number = |line: &str| field(line, 2).parse::<i64>().unwrap();
            want.sort_by(|a, b| number(a).cmp(&number(b)).then_with(|| a.cmp(b)));
            assert_eq!(sorted_of("-t : -k 2n", &[&text_of(&lines)]), text_of(&want));
        }
    }

    #[test]
    fn folding_sorts_as_the_capitals_and_then_the_bytes() {
        let mut seed = 0x1234_5678_9abc_def1_u64;
        for _ in 0..200 {
            let lines: Vec<String> = (0..shuffled(&mut seed) % 16)
                .map(|_| {
                    (0..shuffled(&mut seed) % 4)
                        .map(|_| {
                            ['a', 'A', 'b', 'B', '_']
                                [usize::try_from(shuffled(&mut seed) % 5).unwrap()]
                        })
                        .collect()
                })
                .collect();
            let mut want = lines.clone();
            want.sort_by(|a, b| {
                a.to_ascii_uppercase()
                    .cmp(&b.to_ascii_uppercase())
                    .then_with(|| a.cmp(b))
            });
            assert_eq!(sorted_of("-f", &[&text_of(&lines)]), text_of(&want));
        }
    }

    #[test]
    fn unique_keeps_the_first_line_of_each_key_as_it_came() {
        let mut seed = 0xfeed_beef_dead_cafe_u64;
        for _ in 0..200 {
            let lines = random_fields(&mut seed);
            let mut want = lines.clone();
            want.sort_by(|a, b| field(a, 1).cmp(field(b, 1)));
            want.dedup_by(|later, first| field(later, 1) == field(first, 1));
            assert_eq!(
                sorted_of("-u -t : -k 1,1", &[&text_of(&lines)]),
                text_of(&want)
            );
        }
    }

    #[test]
    fn characters_count_whole_and_stop_at_the_end() {
        let text = "a\u{e9}\u{1f600}b".as_bytes();
        assert_eq!(characters_on(text, 0, 0), 0);
        assert_eq!(characters_on(text, 0, 1), 1);
        assert_eq!(characters_on(text, 0, 2), 3);
        assert_eq!(characters_on(text, 0, 3), 7);
        assert_eq!(characters_on(text, 0, 4), 8);
        assert_eq!(characters_on(text, 0, 99), 8);
        assert_eq!(characters_on(text, 8, 1), 8);
        assert_eq!(characters_on(b"", 0, 5), 0);
    }

    #[test]
    fn written_lines_go_out_through_the_writer() {
        let given = words("-r");
        let asked = asked(&given).unwrap();
        let mut out = Vec::new();
        let whole = written(all_of(&["a\nc\nb\n"]), &asked, |bytes| {
            out.extend_from_slice(bytes);
            Ok(())
        });
        assert!(whole);
        assert_eq!(out, b"c\nb\na\n");
        let refused = written(all_of(&["a\n"]), &asked, |_| Err(Error::Closed));
        assert!(!refused, "a write that went wrong");
    }

    /// The fields of `line` as POSIX's sort has them without -t: each the
    /// blanks before it and then what is not blank.
    fn blank_fields(line: &str) -> Vec<String> {
        let mut fields = Vec::new();
        let mut field = String::new();
        let mut in_word = false;
        for c in line.chars() {
            let blank = c == ' ' || c == '\t';
            if blank && in_word {
                fields.push(core::mem::take(&mut field));
                in_word = false;
            }
            in_word |= !blank;
            field.push(c);
        }
        fields.push(field);
        fields
    }

    #[test]
    fn keys_of_blank_parted_fields_take_the_blanks_before_them() {
        let mut seed = 0x5851_f42d_4c95_7f2d_u64;
        for _ in 0..200 {
            let lines: Vec<String> = (0..shuffled(&mut seed) % 12)
                .map(|_| {
                    (0..3)
                        .map(|_| {
                            let blanks =
                                " ".repeat(usize::try_from(shuffled(&mut seed) % 3).unwrap());
                            let word = ["a", "b", "ab", "Z"]
                                [usize::try_from(shuffled(&mut seed) % 4).unwrap()];
                            blanks + word
                        })
                        .collect()
                })
                .collect();
            for n in 1..=3 {
                let key = |line: &str| blank_fields(line).get(n - 1).cloned().unwrap_or_default();
                let mut want = lines.clone();
                want.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.cmp(b)));
                assert_eq!(
                    sorted_of(&alloc::format!("-k {n},{n}"), &[&text_of(&lines)]),
                    text_of(&want)
                );
                let trimmed = |line: &str| key(line).trim_start().to_string();
                let mut want = lines.clone();
                want.sort_by(|a, b| trimmed(a).cmp(&trimmed(b)).then_with(|| a.cmp(b)));
                assert_eq!(
                    sorted_of(&alloc::format!("-k {n}b,{n}"), &[&text_of(&lines)]),
                    text_of(&want)
                );
            }
        }
    }

    #[test]
    fn a_check_finds_exactly_the_first_pair_out_of_order() {
        let mut seed = 0x3c6e_f372_fe94_f82b_u64;
        let plain = asked(&[]).unwrap();
        for _ in 0..300 {
            let lines = random_fields(&mut seed);
            let all = all_of(&[&text_of(&lines)]);
            let first = lines
                .windows(2)
                .position(|pair| pair[0] > pair[1])
                .map(|at| at + 2);
            assert_eq!(all.disorder(&plain).map(|(line, _)| line), first);
            let sorted = sorted_of("", &[&text_of(&lines)]);
            assert_eq!(all_of(&[&sorted]).disorder(&plain), None);
        }
    }

    #[test]
    fn dictionary_and_printable_orders_leave_out_what_they_say() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..200 {
            let lines: Vec<String> = (0..shuffled(&mut seed) % 12)
                .map(|_| {
                    (0..shuffled(&mut seed) % 5)
                        .map(|_| {
                            ['a', 'b', '-', ' ', '\u{7}', '1']
                                [usize::try_from(shuffled(&mut seed) % 6).unwrap()]
                        })
                        .collect()
                })
                .collect();
            let dictionary = |line: &str| -> String {
                line.chars()
                    .filter(|c| *c == ' ' || c.is_ascii_alphanumeric())
                    .collect()
            };
            let mut want = lines.clone();
            want.sort_by(|a, b| dictionary(a).cmp(&dictionary(b)).then_with(|| a.cmp(b)));
            assert_eq!(sorted_of("-d", &[&text_of(&lines)]), text_of(&want));
            let printable = |line: &str| -> String {
                line.chars().filter(|c| (' '..='~').contains(c)).collect()
            };
            let mut want = lines.clone();
            want.sort_by(|a, b| printable(a).cmp(&printable(b)).then_with(|| a.cmp(b)));
            assert_eq!(sorted_of("-i", &[&text_of(&lines)]), text_of(&want));
        }
    }
}
