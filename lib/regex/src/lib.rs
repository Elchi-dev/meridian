//! POSIX's regular expressions, basic and extended, as decision record
//! 0023 describes: an expression read into a tree ([`parse`]), its
//! bracket expressions ([`set`]), the Pike VM that finds where it
//! matches ([`vm`]), the places its parts reach, which say where its
//! subexpressions matched ([`reach`]), and the search for expressions
//! with back-references ([`back`]). [`Regex`] puts them together.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

pub mod back;
#[cfg(test)]
mod dice;
pub mod parse;
pub mod reach;
pub mod set;
pub mod vm;

/// Which of POSIX's two syntaxes an expression is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syntax {
    /// Basic: `\(`, `\{` and back-references, as grep and sed take by
    /// default.
    Basic,
    /// Extended: `(`, `{`, `+`, `?` and `|`, as `grep -E` takes.
    Extended,
}

/// What is wrong with an expression, and the character it was found at,
/// counted from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub at: usize,
    pub code: Code,
}

/// The ways an expression can be wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    OpenBracket,
    Parenthesis,
    Interval,
    Range,
    Class,
    TrailingBackslash,
    NothingToRepeat,
    BackReference,
    Escape,
    TooDeep,
    TooBig,
}

impl Code {
    /// What a tool says about an expression wrong in this way.
    #[must_use]
    pub fn says(self) -> &'static str {
        match self {
            Code::OpenBracket => "a [ without the ] that ends it",
            Code::Parenthesis => "a parenthesis without its other half",
            Code::Interval => "an interval that is no count from 0 to 255",
            Code::Range => "a range that ends before it starts",
            Code::Class => "a class or collating element this system does not know",
            Code::TrailingBackslash => "a backslash with nothing after it",
            Code::NothingToRepeat => "a repetition with nothing before it",
            Code::BackReference => "a back-reference to no subexpression ended before it",
            Code::Escape => "an escape POSIX gives no meaning",
            Code::TooDeep => {
                "groups nested more than 16 deep, or one item repeated more than 4 times over"
            }
            Code::TooBig => "too big once its intervals are written out",
        }
    }
}

/// A search with back-references that would list more ways to match
/// than its budget allows, or go deeper than a program's stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooComplex;

impl TooComplex {
    /// What a tool says when a search went too far.
    #[must_use]
    pub fn says(self) -> &'static str {
        "the back-references leave too many ways to match this text"
    }
}

/// Where a match is in a text, in bytes: from `start` up to `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A text read into its characters once, to be searched as often as
/// needed. A byte that is no UTF-8 is a character of its own.
#[derive(Debug)]
pub struct Text<'b> {
    bytes: &'b [u8],
    chars: Vec<Option<char>>,
    /// Where the bytes of each character start, the text's length last.
    starts: Vec<usize>,
}

impl<'b> Text<'b> {
    #[must_use]
    pub fn new(bytes: &'b [u8]) -> Text<'b> {
        let mut chars = Vec::new();
        let mut starts = Vec::new();
        let mut at = 0;
        for chunk in bytes.utf8_chunks() {
            for (i, c) in chunk.valid().char_indices() {
                chars.push(Some(c));
                starts.push(at + i);
            }
            at += chunk.valid().len();
            for _ in chunk.invalid() {
                chars.push(None);
                starts.push(at);
                at += 1;
            }
        }
        starts.push(at);
        Text {
            bytes,
            chars,
            starts,
        }
    }

    /// The first character that starts at the byte `byte` or after it.
    fn index(&self, byte: usize) -> usize {
        self.starts.partition_point(|&start| start < byte)
    }

    /// The bytes from the character `start` up to the character `end`.
    fn span(&self, (start, end): (usize, usize)) -> Span {
        Span {
            start: self.starts[start],
            end: self.starts[end],
        }
    }

    /// The first byte of the character at `at`.
    fn byte(&self, at: usize) -> u8 {
        self.bytes[self.starts[at]]
    }

    fn chars(&self, either_case: bool) -> reach::Chars<'_> {
        reach::Chars {
            chars: &self.chars,
            either_case,
        }
    }
}

/// An expression ready to match.
#[derive(Debug)]
pub struct Regex {
    tree: parse::Node,
    groups: usize,
    either_case: bool,
    /// The VM's program, none for an expression with back-references.
    program: Option<vm::Program>,
}

impl Regex {
    /// Reads `pattern` as `syntax` says, to match characters in either
    /// case when `either_case`.
    ///
    /// # Errors
    ///
    /// Where the expression breaks POSIX's rules, and for one that is
    /// too big or nested too deep.
    pub fn new(pattern: &str, syntax: Syntax, either_case: bool) -> Result<Regex, Error> {
        let parsed = parse::parse(pattern, syntax)?;
        let program = match vm::compile(&parsed.tree, either_case) {
            Ok(program) => Some(program),
            Err(vm::Refused::BackReference) => None,
            Err(vm::Refused::TooBig) => {
                return Err(Error {
                    at: 0,
                    code: Code::TooBig,
                });
            }
        };
        Ok(Regex {
            tree: parsed.tree,
            groups: parsed.groups,
            either_case,
            program,
        })
    }

    /// How many subexpressions it has.
    #[must_use]
    pub fn groups(&self) -> usize {
        self.groups
    }

    /// The leftmost-longest match in `text` among those that start at the
    /// byte `from` or after. `^` holds only at the start of the text.
    ///
    /// # Errors
    ///
    /// For an expression with back-references that would take too long.
    pub fn find(&self, text: &Text, from: usize) -> Result<Option<Span>, TooComplex> {
        let start = text.index(from);
        let found = match &self.program {
            Some(program) => program.find(&text.chars, start),
            None => back::find(&self.tree, self.groups, text, self.either_case, start)?
                .and_then(|spans| spans[0]),
        };
        Ok(found.map(|found| text.span(found)))
    }

    /// The match [`Regex::find`] finds, first, and where each
    /// subexpression matched in it, by its number: none for one that
    /// took no part.
    ///
    /// # Errors
    ///
    /// For an expression with back-references that would take too long.
    pub fn captures(
        &self,
        text: &Text,
        from: usize,
    ) -> Result<Option<Vec<Option<Span>>>, TooComplex> {
        let start = text.index(from);
        let spans = match &self.program {
            Some(program) => program.find(&text.chars, start).map(|found| {
                reach::groups(
                    &self.tree,
                    self.groups,
                    &text.chars(self.either_case),
                    found,
                )
            }),
            None => back::find(&self.tree, self.groups, text, self.either_case, start)?,
        };
        Ok(spans.map(|spans| {
            spans
                .into_iter()
                .map(|span| span.map(|span| text.span(span)))
                .collect()
        }))
    }
}

/// Whether `c`, none for a byte that is no UTF-8, is the character
/// `want`, or `want` in the other case when `either_case`.
fn same(c: Option<char>, want: char, either_case: bool) -> bool {
    c.is_some_and(|c| {
        c == want || (either_case && (lower(c) == lower(want) || upper(c) == upper(want)))
    })
}

/// `c` in lower case, where that is one character, or else `c`.
fn lower(c: char) -> char {
    let mut lowered = c.to_lowercase();
    match (lowered.next(), lowered.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

/// `c` in upper case, where that is one character, or else `c`.
fn upper(c: char) -> char {
    let mut raised = c.to_uppercase();
    match (raised.next(), raised.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_changes_only_where_it_stays_one_character() {
        assert_eq!(lower('A'), 'a');
        assert_eq!(lower('Ü'), 'ü');
        assert_eq!(lower('a'), 'a');
        assert_eq!(lower('1'), '1');
        assert_eq!(upper('a'), 'A');
        assert_eq!(upper('ü'), 'Ü');
        assert_eq!(upper('ß'), 'ß', "SS would be two");
        assert_eq!(lower('İ'), 'İ', "i and a dot above would be two");
    }

    #[test]
    fn every_code_says_something_of_its_own() {
        let codes = [
            Code::OpenBracket,
            Code::Parenthesis,
            Code::Interval,
            Code::Range,
            Code::Class,
            Code::TrailingBackslash,
            Code::NothingToRepeat,
            Code::BackReference,
            Code::Escape,
            Code::TooDeep,
            Code::TooBig,
        ];
        for (i, a) in codes.iter().enumerate() {
            assert_ne!(a.says(), "");
            for b in &codes[i + 1..] {
                assert_ne!(a.says(), b.says());
            }
        }
        assert_eq!(Code::Range.says(), "a range that ends before it starts");
    }

    #[test]
    fn a_character_is_the_same_as_itself_or_in_its_other_case() {
        assert!(same(Some('a'), 'a', false));
        assert!(!same(Some('a'), 'b', false));
        assert!(!same(Some('A'), 'a', false));
        assert!(same(Some('A'), 'a', true));
        assert!(same(Some('a'), 'A', true));
        assert!(same(Some('ö'), 'Ö', true));
        assert!(!same(Some('a'), 'b', true));
        assert!(same(Some('ſ'), 's', true), "the long s is S in upper case");
        assert!(
            same(Some('K'), 'K', true),
            "the Kelvin sign is k in lower case"
        );
        assert!(!same(None, 'a', false), "a byte that is no UTF-8");
        assert!(!same(None, 'a', true));
    }

    #[test]
    fn a_text_is_read_into_characters_with_where_each_starts() {
        let text = Text::new("aü€\u{1F600}".as_bytes());
        assert_eq!(text.chars, [Some('a'), Some('ü'), Some('€'), Some('😀')]);
        assert_eq!(text.starts, [0, 1, 3, 6, 10]);
        let bytes = Text::new(&[b'a', 0xff, 0xfe, b'b', 0xc3]);
        assert_eq!(bytes.chars, [Some('a'), None, None, Some('b'), None]);
        assert_eq!(bytes.starts, [0, 1, 2, 3, 4, 5]);
        assert_eq!(bytes.byte(2), 0xfe);
        let empty = Text::new(b"");
        assert_eq!((empty.chars.len(), empty.starts.as_slice()), (0, &[0][..]));
    }

    #[test]
    fn a_byte_place_is_the_character_that_starts_there_or_next() {
        let text = Text::new("aü€".as_bytes());
        assert_eq!(text.index(0), 0);
        assert_eq!(text.index(1), 1);
        assert_eq!(text.index(2), 2, "inside ü, so the € after it");
        assert_eq!(text.index(3), 2);
        assert_eq!(text.index(6), 3, "the end");
        assert_eq!(text.index(7), 4, "past it");
        assert_eq!(text.span((1, 3)), Span { start: 1, end: 6 });
    }

    fn found(pattern: &str, syntax: Syntax, text: &str, from: usize) -> Option<Span> {
        Regex::new(pattern, syntax, false)
            .unwrap()
            .find(&Text::new(text.as_bytes()), from)
            .unwrap()
    }

    #[test]
    fn a_match_is_found_in_bytes() {
        let span = |start, end| Some(Span { start, end });
        assert_eq!(found("ü+", Syntax::Extended, "aüüb", 0), span(1, 5));
        assert_eq!(found("b", Syntax::Basic, "aüüb", 0), span(5, 6));
        assert_eq!(found("ü", Syntax::Basic, "üaü", 1), span(3, 5));
        assert_eq!(
            found("ü", Syntax::Basic, "üaü", 4),
            None,
            "inside the last ü"
        );
        assert_eq!(found("$", Syntax::Basic, "aü", 0), span(3, 3));
        assert_eq!(found("\\(ü\\)\\1", Syntax::Basic, "aüü", 0), span(1, 5));
        assert_eq!(found("x", Syntax::Basic, "aüü", 0), None);
        let either = Regex::new("Ü", Syntax::Basic, true).unwrap();
        assert_eq!(either.find(&Text::new("aü".as_bytes()), 0), Ok(span(1, 3)));
    }

    #[test]
    fn captures_say_where_each_subexpression_matched_in_bytes() {
        let captures = |pattern, text: &str| {
            Regex::new(pattern, Syntax::Basic, false)
                .unwrap()
                .captures(&Text::new(text.as_bytes()), 0)
                .unwrap()
        };
        let span = |start, end| Some(Span { start, end });
        assert_eq!(
            captures("\\(é*\\)-\\(.*\\)", "xéé-ö"),
            Some(Vec::from([span(1, 8), span(1, 5), span(6, 8)]))
        );
        assert_eq!(
            captures("\\(.\\)\\1", "aöö"),
            Some(Vec::from([span(1, 5), span(1, 3)])),
            "with a back-reference too"
        );
        assert_eq!(
            captures("a\\(x\\)*", "ab"),
            Some(Vec::from([span(0, 1), None]))
        );
        assert_eq!(captures("z", "ab"), None);
        assert_eq!(captures("\\(z\\)\\1", "ab"), None);
    }

    #[test]
    fn a_regex_knows_its_groups_and_refuses_what_it_cannot_hold() {
        assert_eq!(
            Regex::new("(a)(b(c))", Syntax::Extended, false)
                .unwrap()
                .groups(),
            3
        );
        assert_eq!(Regex::new("abc", Syntax::Basic, false).unwrap().groups(), 0);
        assert_eq!(
            Regex::new("((a{255}){255}){2}", Syntax::Extended, false).unwrap_err(),
            Error {
                at: 0,
                code: Code::TooBig
            }
        );
        assert_eq!(
            Regex::new("a\\{2", Syntax::Basic, false).unwrap_err(),
            Error {
                at: 1,
                code: Code::Interval
            }
        );
        assert!(
            Regex::new("((a{255}){255}){2}\\1", Syntax::Extended, false).is_err(),
            "too big for the VM before its back-reference is reached"
        );
    }

    #[test]
    fn a_search_goes_on_through_a_text_of_wide_characters() {
        let regex = Regex::new("[äöü]+", Syntax::Extended, false).unwrap();
        let line = "Bäume über Höhen";
        let text = Text::new(line.as_bytes());
        let mut found = Vec::new();
        let mut from = 0;
        while let Some(span) = regex.find(&text, from).unwrap() {
            found.push(&line[span.start..span.end]);
            from = span.end.max(span.start + 1);
        }
        assert_eq!(found, ["ä", "ü", "ö"]);
        let empty = Regex::new("x*", Syntax::Extended, false).unwrap();
        let text = Text::new("äb".as_bytes());
        let mut places = Vec::new();
        let mut from = 0;
        while let Some(span) = empty.find(&text, from).unwrap() {
            places.push(span.start);
            from = span.end + 1;
        }
        assert_eq!(places, [0, 2, 3], "after ä the next character is at byte 2");
    }

    #[test]
    fn captures_fold_case_and_take_bytes_that_are_no_utf8() {
        let span = |start, end| Some(Span { start, end });
        let either = Regex::new("(Ä+)(b)", Syntax::Extended, true).unwrap();
        assert_eq!(
            either.captures(&Text::new("xääB".as_bytes()), 0),
            Ok(Some(Vec::from([span(1, 6), span(1, 5), span(5, 6)])))
        );
        let any = Regex::new("a(.*)b", Syntax::Extended, false).unwrap();
        assert_eq!(
            any.captures(&Text::new(&[b'a', 0xff, 0xfe, b'b']), 0),
            Ok(Some(Vec::from([span(0, 4), span(1, 3)])))
        );
        let none = Regex::new("[^a]+", Syntax::Extended, false).unwrap();
        assert_eq!(
            none.find(&Text::new(&[0xff, 0xff, 0xff]), 0),
            Ok(span(0, 3))
        );
    }

    #[test]
    fn an_empty_expression_or_text_matches_at_its_start() {
        let span = |start, end| Some(Span { start, end });
        assert_eq!(found("", Syntax::Basic, "abc", 0), span(0, 0));
        assert_eq!(found("", Syntax::Extended, "", 0), span(0, 0));
        assert_eq!(found("^$", Syntax::Basic, "", 0), span(0, 0));
        assert_eq!(found("a*", Syntax::Basic, "", 0), span(0, 0));
        assert_eq!(found("a", Syntax::Basic, "", 0), None);
        assert_eq!(found("", Syntax::Basic, "ab", 2), span(2, 2));
        assert_eq!(found("", Syntax::Basic, "ab", 3), None);
    }

    #[test]
    fn both_engines_give_the_same_spans_in_bytes_from_any_start() {
        let mut dice = dice::Dice(0x1405_7b7e_f767_814f);
        for _ in 0..200 {
            let pattern = dice::pattern(&mut dice, 3);
            let either_case = dice.roll(2) == 0;
            let regex = Regex::new(&pattern, Syntax::Extended, either_case).unwrap();
            let parsed = parse::parse(&pattern, Syntax::Extended).unwrap();
            for _ in 0..4 {
                let words: alloc::string::String = dice::text(&mut dice)
                    .chars()
                    .map(|c| match c {
                        'c' => 'Ä',
                        'b' if either_case => 'B',
                        c => c,
                    })
                    .collect();
                let text = Text::new(words.as_bytes());
                for from in [0, 1, 2] {
                    let searched = back::find(
                        &parsed.tree,
                        parsed.groups,
                        &text,
                        either_case,
                        text.index(from),
                    )
                    .unwrap()
                    .map(|spans| {
                        spans
                            .into_iter()
                            .map(|span| span.map(|span| text.span(span)))
                            .collect::<Vec<_>>()
                    });
                    assert_eq!(
                        regex.captures(&text, from),
                        Ok(searched),
                        "{pattern} in {words:?} from {from}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_regex_refuses_an_expression_nested_too_deep_and_says_so() {
        let deep = "(".repeat(17) + &")".repeat(17);
        assert_eq!(
            Regex::new(&deep, Syntax::Extended, false).unwrap_err(),
            Error {
                at: 16,
                code: Code::TooDeep
            }
        );
        assert_eq!(
            Regex::new("a*****", Syntax::Basic, false).unwrap_err().code,
            Code::TooDeep
        );
        assert_eq!(
            Code::TooDeep.says(),
            "groups nested more than 16 deep, or one item repeated more than 4 times over"
        );
        assert_eq!(
            Code::TooBig.says(),
            "too big once its intervals are written out"
        );
        assert_eq!(
            TooComplex.says(),
            "the back-references leave too many ways to match this text"
        );
    }
}
