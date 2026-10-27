//! POSIX's regular expressions, basic and extended, as decision record
//! 0023 describes: an expression read into a tree ([`parse`]), its
//! bracket expressions ([`set`]), the Pike VM that finds where it
//! matches ([`vm`]), and the places its parts reach, which say where its
//! subexpressions matched ([`reach`]).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

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
        }
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
}
