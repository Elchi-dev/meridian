//! Records and fields: where a record of the input ends, as `RS` says,
//! and how a record splits into fields, as `FS` says.

use alloc::string::String;
use alloc::vec::Vec;

use regex::{Regex, Syntax, Text};

/// The characters a regular expression gives a meaning of their own.
const SPECIAL: &[u8] = b"\\^$.[]|()*+?{}";

/// `bytes` as a regular expression that matches them and nothing else.
fn literal(bytes: &[u8]) -> String {
    let mut pattern = String::new();
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c.is_ascii() && SPECIAL.contains(&(c as u8)) {
                pattern.push('\\');
            }
            pattern.push(c);
        }
    }
    pattern
}

/// Reads `pattern`, an extended regular expression.
fn regex(pattern: &[u8]) -> Result<Regex, regex::Error> {
    Regex::new(&String::from_utf8_lossy(pattern), Syntax::Extended, false)
}

/// The first non-empty match of `regex` in `text` from the byte `from`
/// on, as the bytes it spans.
fn non_empty(regex: &Regex, text: &Text, bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut at = from;
    while at <= bytes.len() {
        let found = regex.find(text, at).ok()??;
        if found.end > found.start {
            return Some((found.start, found.end));
        }
        // An empty match ends nothing; look again a character on.
        at = found.start + char_len(&bytes[found.start..]).max(1);
    }
    None
}

/// How many bytes the character at the start of `bytes` takes, a byte
/// that is no UTF-8 one.
fn char_len(bytes: &[u8]) -> usize {
    bytes.utf8_chunks().next().map_or(0, |chunk| {
        chunk.valid().chars().next().map_or(1, char::len_utf8)
    })
}

/// What ends a record.
#[derive(Debug)]
pub enum Ends {
    /// One byte, a line feed unless `RS` names another.
    Byte(u8),
    /// One empty line or more, as `RS=""` asks: the input in paragraphs.
    Paragraph,
    /// A match of `RS` read as a regular expression, when it is longer.
    Regex(Regex),
}

impl Ends {
    /// What `rs`, the value of `RS`, says ends a record.
    ///
    /// # Errors
    ///
    /// When `rs` is longer than one byte and no regular expression.
    pub fn new(rs: &[u8]) -> Result<Ends, regex::Error> {
        Ok(match rs {
            [] => Ends::Paragraph,
            [byte] => Ends::Byte(*byte),
            _ => Ends::Regex(regex(rs)?),
        })
    }

    /// Where the first record in `data` is: from where it starts to
    /// where it ends, and how many bytes it takes with what ends it.
    /// None when no record is left, or, before the input `ended`, when
    /// what is still to come could change where the record ends.
    #[must_use]
    pub fn first(&self, data: &[u8], ended: bool) -> Option<(usize, usize, usize)> {
        match self {
            Ends::Byte(byte) => {
                if let Some(at) = data.iter().position(|b| b == byte) {
                    return Some((0, at, at + 1));
                }
                (ended && !data.is_empty()).then_some((0, data.len(), data.len()))
            }
            Ends::Paragraph => {
                let lead = data.iter().take_while(|&&b| b == b'\n').count();
                let rest = &data[lead..];
                if let Some(at) = rest.windows(2).position(|pair| pair == b"\n\n") {
                    let after = at + rest[at..].iter().take_while(|&&b| b == b'\n').count();
                    return Some((lead, lead + at, lead + after));
                }
                if !ended || rest.is_empty() {
                    return None;
                }
                let end = rest.len() - usize::from(rest.ends_with(b"\n"));
                Some((lead, lead + end, data.len()))
            }
            Ends::Regex(regex) => {
                let text = Text::new(data);
                match non_empty(regex, &text, data, 0) {
                    // A match that reaches what is read so far might go on.
                    Some((_, end)) if end == data.len() && !ended => None,
                    Some((start, end)) => Some((0, start, end)),
                    None => (ended && !data.is_empty()).then_some((0, data.len(), data.len())),
                }
            }
        }
    }
}

/// How a record splits into fields.
#[derive(Debug)]
pub enum Splitter {
    /// At runs of blanks and line feeds, none at either end, as the
    /// default `FS`, a blank, asks.
    Blanks,
    Byte(u8),
    /// Into its characters, as an empty `FS` asks.
    Chars,
    Regex(Regex),
}

impl Splitter {
    /// How `fs`, the value of `FS`, splits a record; in `paragraph` mode a
    /// line feed parts fields too.
    ///
    /// # Errors
    ///
    /// When `fs` is longer than one byte and no regular expression.
    pub fn new(fs: &[u8], paragraph: bool) -> Result<Splitter, regex::Error> {
        Ok(match fs {
            b" " => Splitter::Blanks,
            [] => Splitter::Chars,
            [byte] if !paragraph || *byte == b'\n' => Splitter::Byte(*byte),
            [_] => Splitter::Regex(regex(alloc::format!("{}|\n", literal(fs)).as_bytes())?),
            _ if paragraph => {
                // Read alone first, so that a wrong one is told as written.
                regex(fs)?;
                let mut pattern = Vec::from(&b"("[..]);
                pattern.extend_from_slice(fs);
                pattern.extend_from_slice(b")|\n");
                Splitter::Regex(regex(&pattern)?)
            }
            _ => Splitter::Regex(regex(fs)?),
        })
    }

    /// The fields of `text`.
    #[must_use]
    pub fn split<'t>(&self, text: &'t [u8]) -> Vec<&'t [u8]> {
        match self {
            Splitter::Blanks => text
                .split(|&b| matches!(b, b' ' | b'\t' | b'\n'))
                .filter(|field| !field.is_empty())
                .collect(),
            _ if text.is_empty() => Vec::new(),
            Splitter::Byte(byte) => text.split(|b| b == byte).collect(),
            Splitter::Chars => {
                let mut fields = Vec::new();
                let mut at = 0;
                while at < text.len() {
                    let len = char_len(&text[at..]);
                    fields.push(&text[at..at + len]);
                    at += len;
                }
                fields
            }
            Splitter::Regex(regex) => {
                let searched = Text::new(text);
                let mut fields = Vec::new();
                let mut at = 0;
                while let Some((start, end)) = non_empty(regex, &searched, text, at) {
                    fields.push(&text[at..start]);
                    at = end;
                }
                fields.push(&text[at..]);
                fields
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The records `ends` finds in `data`, which has all come.
    fn records(rs: &[u8], data: &[u8]) -> Vec<Vec<u8>> {
        let ends = Ends::new(rs).unwrap();
        let mut found = Vec::new();
        let mut at = 0;
        while let Some((start, end, taken)) = ends.first(&data[at..], true) {
            found.push(data[at + start..at + end].to_vec());
            at += taken;
        }
        found
    }

    fn fields<'t>(fs: &[u8], paragraph: bool, text: &'t [u8]) -> Vec<&'t [u8]> {
        Splitter::new(fs, paragraph).unwrap().split(text)
    }

    #[test]
    fn records_end_at_a_byte() {
        assert_eq!(
            records(b"\n", b"a\nb\n\nc"),
            [b"a".to_vec(), b"b".to_vec(), b"".to_vec(), b"c".to_vec()]
        );
        assert_eq!(
            records(b"\n", b"a\n"),
            [b"a".to_vec()],
            "the last line feed ends the last"
        );
        assert_eq!(records(b";", b"x;y"), [b"x".to_vec(), b"y".to_vec()]);
        assert_eq!(records(b"\n", b""), Vec::<Vec<u8>>::new());
        let ends = Ends::new(b"\n").unwrap();
        assert_eq!(ends.first(b"abc", false), None, "more may come");
        assert_eq!(ends.first(b"abc", true), Some((0, 3, 3)));
        assert_eq!(ends.first(b"ab\ncd", false), Some((0, 2, 3)));
        assert_eq!(ends.first(b"", true), None);
    }

    #[test]
    fn paragraphs_end_at_empty_lines() {
        assert_eq!(
            records(b"", b"\n\na\nb\n\n\nc\n"),
            [b"a\nb".to_vec(), b"c".to_vec()],
            "lines before the first are none and the last line feed ends the last"
        );
        assert_eq!(records(b"", b"a\n\n"), [b"a".to_vec()]);
        assert_eq!(records(b"", b"\n\n\n"), Vec::<Vec<u8>>::new());
        let ends = Ends::new(b"").unwrap();
        assert_eq!(
            ends.first(b"a\n", false),
            None,
            "another line feed may come"
        );
        assert_eq!(ends.first(b"a\n\nb", false), Some((0, 1, 3)));
        assert_eq!(ends.first(b"\n\n", false), None);
        assert_eq!(ends.first(b"a\nb", true), Some((0, 3, 3)));
        assert_eq!(ends.first(b"\na", true), Some((1, 2, 2)));
    }

    #[test]
    fn a_longer_rs_is_a_regular_expression() {
        assert_eq!(
            records(b"[0-9]+", b"a12b3c"),
            [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
        );
        assert_eq!(
            records(b"x*", b"axxb"),
            [b"a".to_vec(), b"b".to_vec()],
            "an empty match ends nothing"
        );
        let ends = Ends::new(b"ab+").unwrap();
        assert_eq!(ends.first(b"1ab", false), None, "the match may go on");
        assert_eq!(ends.first(b"1ab", true), Some((0, 1, 3)));
        assert_eq!(ends.first(b"1abb2", false), Some((0, 1, 4)));
        assert_eq!(ends.first(b"12", false), None);
        assert_eq!(ends.first(b"12", true), Some((0, 2, 2)));
        assert!(Ends::new(b"a(").is_err());
    }

    #[test]
    fn blanks_part_fields_and_none_lead_or_trail() {
        assert_eq!(
            fields(b" ", false, b"  a \t b\nc  "),
            [&b"a"[..], b"b", b"c"]
        );
        assert_eq!(fields(b" ", false, b""), Vec::<&[u8]>::new());
        assert_eq!(fields(b" ", false, b" \t "), Vec::<&[u8]>::new());
        assert_eq!(fields(b" ", true, b"a\nb"), [&b"a"[..], b"b"]);
    }

    #[test]
    fn one_byte_parts_fields_as_it_is_and_more_is_a_regex() {
        assert_eq!(fields(b":", false, b"a::b:"), [&b"a"[..], b"", b"b", b""]);
        assert_eq!(
            fields(b"|", false, b"a|b"),
            [&b"a"[..], b"b"],
            "a special character stands for itself"
        );
        assert_eq!(fields(b"\t", false, b"a\t\tb"), [&b"a"[..], b"", b"b"]);
        assert_eq!(fields(b":", false, b""), Vec::<&[u8]>::new());
        assert_eq!(fields(b"[,;]+", false, b"a,;b;c"), [&b"a"[..], b"b", b"c"]);
        assert_eq!(fields(b", ", false, b",a, b"), [&b",a"[..], b"b"]);
        assert_eq!(
            fields(b"x*", false, b"axxb"),
            [&b"a"[..], b"b"],
            "an empty match parts nothing"
        );
        assert_eq!(fields(b"[0-9]", false, b"1a2"), [&b""[..], b"a", b""]);
        assert!(Splitter::new(b"a(", false).is_err());
    }

    #[test]
    fn an_empty_fs_splits_into_characters() {
        assert_eq!(
            fields(b"", false, b"a\xc3\xa4\xffb"),
            [&b"a"[..], "ä".as_bytes(), b"\xff", b"b"]
        );
        assert_eq!(fields(b"", false, b""), Vec::<&[u8]>::new());
    }

    #[test]
    fn in_paragraphs_a_line_feed_parts_fields_too() {
        assert_eq!(fields(b":", true, b"a:b\nc"), [&b"a"[..], b"b", b"c"]);
        assert_eq!(
            fields(b".", true, b"a.b\nc"),
            [&b"a"[..], b"b", b"c"],
            "the byte as itself"
        );
        assert_eq!(fields(b"[0-9]+", true, b"a1b\nc"), [&b"a"[..], b"b", b"c"]);
        assert_eq!(fields(b"\n", true, b"a\nb"), [&b"a"[..], b"b"]);
        assert_eq!(
            fields("ä".as_bytes(), true, "aäb\nc".as_bytes()),
            [&b"a"[..], b"b", b"c"]
        );
        assert!(Splitter::new(b"a(", true).is_err());
        assert_eq!(literal(b"a.b"), "a\\.b");
    }

    #[test]
    fn a_regex_rs_finds_its_first_non_empty_match() {
        let ends = Ends::new(b"a*b").unwrap();
        assert_eq!(ends.first(b"xbyab", true), Some((0, 1, 2)));
        assert_eq!(
            ends.first(b"xaab", false),
            None,
            "it ends where the data does"
        );
        assert_eq!(ends.first(b"xaabz", false), Some((0, 1, 4)));
        let ends = Ends::new(b"\n\n+").unwrap();
        assert_eq!(ends.first(b"a\n\n\nb", false), Some((0, 1, 4)));
        assert_eq!(ends.first(b"", true), None);
        let ends = Ends::new(b"^x").unwrap();
        assert_eq!(
            ends.first(b"xab", true),
            Some((0, 0, 1)),
            "^ holds at the start of what is left"
        );
    }

    #[test]
    fn a_regex_fs_parts_at_its_ends_too() {
        assert_eq!(fields(b"-+", false, b"-a--b-"), [&b""[..], b"a", b"b", b""]);
        assert_eq!(fields(b"(ab)+", false, b"xababy"), [&b"x"[..], b"y"]);
        assert_eq!(fields(b"[[:space:]]+", false, b"a \t b"), [&b"a"[..], b"b"]);
        assert_eq!(fields(b"b", false, b"abc"), [&b"a"[..], b"c"]);
        assert_eq!(fields(b"ab", false, b"ab"), [&b""[..], b""]);
        assert_eq!(fields(b"z", false, b"\xff"), [&b"\xff"[..]]);
    }

    #[test]
    fn blanks_are_only_spaces_tabs_and_line_feeds() {
        assert_eq!(fields(b" ", false, b"a\x0bb\rc"), [&b"a\x0bb\rc"[..]]);
        assert_eq!(fields(b" ", false, b"\n\na\n"), [&b"a"[..]]);
        assert_eq!(char_len(b""), 0);
        assert_eq!(char_len("ä".as_bytes()), 2);
        assert_eq!(char_len(b"\xffa"), 1);
    }

    #[test]
    fn records_found_piece_by_piece_are_those_found_at_once() {
        // As a reader does: more data each time none is found yet.
        let whole = b"ab;cd;;efg;h";
        for rs in [&b";"[..], b";+", b"", b"\n"] {
            let ends = Ends::new(rs).unwrap();
            let at_once = records(rs, whole);
            let mut found = Vec::new();
            let mut data = Vec::new();
            let mut rest = &whole[..];
            loop {
                let ended = rest.is_empty();
                match ends.first(&data, ended) {
                    Some((start, end, taken)) => {
                        found.push(data[start..end].to_vec());
                        data.drain(..taken);
                    }
                    None if ended => break,
                    None => {
                        data.push(rest[0]);
                        rest = &rest[1..];
                    }
                }
            }
            assert_eq!(found, at_once, "{:?}", String::from_utf8_lossy(rs));
        }
    }

    #[test]
    fn a_splitter_splits_the_same_text_alike_each_time() {
        let splitter = Splitter::new(b",", false).unwrap();
        assert_eq!(splitter.split(b"a,b"), splitter.split(b"a,b"));
        let splitter = Splitter::new(b"[,;]", true).unwrap();
        assert_eq!(splitter.split(b"a,b\nc;d"), [&b"a"[..], b"b", b"c", b"d"]);
        assert!(matches!(Splitter::new(b" ", true), Ok(Splitter::Blanks)));
        assert!(matches!(
            Splitter::new(b"\n", true),
            Ok(Splitter::Byte(b'\n'))
        ));
        assert!(matches!(
            Splitter::new(b"x", false),
            Ok(Splitter::Byte(b'x'))
        ));
        assert!(matches!(Splitter::new(b"x", true), Ok(Splitter::Regex(_))));
        assert!(matches!(Splitter::new(b"", true), Ok(Splitter::Chars)));
        assert!(matches!(Ends::new(b"\n"), Ok(Ends::Byte(b'\n'))));
        assert!(matches!(Ends::new(b""), Ok(Ends::Paragraph)));
        assert!(matches!(Ends::new(b"ab"), Ok(Ends::Regex(_))));
    }
}
