//! The specification and the crate must not drift apart: every example in
//! `docs/formats/meri.md` is a valid document, the dump example dumps as
//! shown, and the error codes are the ones its table lists.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]

mod common;

use common::{dump, repo};
use meri::{Code, Error, Pos};

const CODES: [Code; 27] = [
    Code::InvalidUtf8,
    Code::ControlCharacter,
    Code::LoneCarriageReturn,
    Code::UnexpectedCharacter,
    Code::ExpectedEquals,
    Code::ExpectedBrace,
    Code::ExpectedValue,
    Code::ExpectedNewline,
    Code::ExpectedComma,
    Code::DuplicateKey,
    Code::NameConflict,
    Code::DuplicateLabel,
    Code::UnclosedBlock,
    Code::UnexpectedClose,
    Code::UnterminatedString,
    Code::InvalidEscape,
    Code::InvalidUnicodeEscape,
    Code::TextOpening,
    Code::TextIndent,
    Code::UnterminatedText,
    Code::UnterminatedList,
    Code::InvalidNumber,
    Code::NumberOverflow,
    Code::UnitNotAllowed,
    Code::TooDeep,
    Code::WrongType,
    Code::OutOfRange,
];

fn spec() -> String {
    std::fs::read_to_string(repo().join("docs/formats/meri.md")).unwrap()
}

/// The fenced blocks of the specification: language, body and the prose
/// right before.
fn fences(spec: &str) -> Vec<(String, String, String)> {
    let parts: Vec<&str> = spec.split("```").collect();
    let mut out = Vec::new();
    for i in (1..parts.len() - 1).step_by(2) {
        let (lang, body) = parts[i].split_once('\n').unwrap();
        out.push((
            lang.trim().to_string(),
            body.to_string(),
            parts[i - 1].to_string(),
        ));
    }
    out
}

#[test]
fn every_example_in_the_specification_is_valid() {
    let spec = spec();
    let examples: Vec<String> = fences(&spec)
        .into_iter()
        .filter(|(lang, _, _)| lang == "meri")
        .map(|(_, body, _)| body)
        .collect();
    assert!(
        examples.len() >= 4,
        "found only {} examples",
        examples.len()
    );
    for body in examples {
        if let Err(err) = dump(body.as_bytes()) {
            panic!("{err} in the example\n{body}");
        }
    }
}

#[test]
fn the_dump_example_dumps_as_the_specification_shows() {
    let spec = spec();
    let fences = fences(&spec);
    let pair = fences
        .windows(2)
        .find(|w| w[0].0 == "meri" && w[1].2.contains("dumps to"))
        .expect("the specification has an example followed by \"dumps to\"");
    assert_eq!(dump(pair[0].1.as_bytes()).unwrap(), pair[1].1);
}

#[test]
fn the_codes_are_the_ones_the_specification_names() {
    let spec = spec();
    let (_, errors) = spec.split_once("## 12. Errors").unwrap();
    let (table, _) = errors.split_once("## 13.").unwrap();
    let rows: Vec<&str> = table
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split_once('`').map(|(code, _)| code))
        .collect();
    let names: Vec<&str> = CODES.iter().map(|c| c.name()).collect();
    assert_eq!(rows, names[..25]);
    for code in [Code::WrongType, Code::OutOfRange] {
        assert!(spec.contains(&format!("`{}`", code.name())), "{code:?}");
    }
}

#[test]
fn every_code_has_its_own_message() {
    let mut messages: Vec<&str> = CODES.iter().map(|c| c.message()).collect();
    for m in &messages {
        assert!(m.len() > 20, "{m}");
        assert!(!m.contains(['\u{2013}', '\u{2014}']), "{m}");
    }
    messages.sort_unstable();
    messages.dedup();
    assert_eq!(messages.len(), CODES.len());
}

#[test]
fn an_error_shows_its_position_and_message() {
    let err = Error {
        code: Code::DuplicateKey,
        at: Pos { line: 4, col: 5 },
    };
    assert_eq!(
        err.to_string(),
        "4:5: this key is already set in the block; remove one of them"
    );
    assert_eq!(Pos { line: 12, col: 1 }.to_string(), "12:1");
}
