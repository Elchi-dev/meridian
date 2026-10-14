//! Reads `.meri` documents, the format of Meridian's settings files, as
//! `docs/formats/meri.md` specifies it. Nothing here allocates, so the
//! loader can read `boot.meri` before there is a heap: [`Reader`] hands out
//! a document one [`Event`] at a time.

#![cfg_attr(not(test), no_std)]

mod number;
mod reader;
mod text;

use core::fmt;

pub use number::{Float, Number, Quantity};
pub use reader::{Event, MAX_DEPTH, Reader, Value};
pub use text::{Chars, Str};

/// Where something is in a document. Lines and columns count from 1, and
/// a column counts characters, not bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// The kinds of problems, as section 12 of the specification names them.
/// The last two are not about the document itself but about a value a
/// program asked for, from section 13.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    InvalidUtf8,
    ControlCharacter,
    LoneCarriageReturn,
    UnexpectedCharacter,
    ExpectedEquals,
    ExpectedBrace,
    ExpectedValue,
    ExpectedNewline,
    ExpectedComma,
    DuplicateKey,
    NameConflict,
    DuplicateLabel,
    UnclosedBlock,
    UnexpectedClose,
    UnterminatedString,
    InvalidEscape,
    InvalidUnicodeEscape,
    TextOpening,
    TextIndent,
    UnterminatedText,
    UnterminatedList,
    InvalidNumber,
    NumberOverflow,
    UnitNotAllowed,
    TooDeep,
    WrongType,
    OutOfRange,
}

impl Code {
    /// The name the specification and the conformance suite use, such as
    /// `duplicate-key`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Code::InvalidUtf8 => "invalid-utf8",
            Code::ControlCharacter => "control-character",
            Code::LoneCarriageReturn => "lone-carriage-return",
            Code::UnexpectedCharacter => "unexpected-character",
            Code::ExpectedEquals => "expected-equals",
            Code::ExpectedBrace => "expected-brace",
            Code::ExpectedValue => "expected-value",
            Code::ExpectedNewline => "expected-newline",
            Code::ExpectedComma => "expected-comma",
            Code::DuplicateKey => "duplicate-key",
            Code::NameConflict => "name-conflict",
            Code::DuplicateLabel => "duplicate-label",
            Code::UnclosedBlock => "unclosed-block",
            Code::UnexpectedClose => "unexpected-close",
            Code::UnterminatedString => "unterminated-string",
            Code::InvalidEscape => "invalid-escape",
            Code::InvalidUnicodeEscape => "invalid-unicode-escape",
            Code::TextOpening => "text-opening",
            Code::TextIndent => "text-indent",
            Code::UnterminatedText => "unterminated-text",
            Code::UnterminatedList => "unterminated-list",
            Code::InvalidNumber => "invalid-number",
            Code::NumberOverflow => "number-overflow",
            Code::UnitNotAllowed => "unit-not-allowed",
            Code::TooDeep => "too-deep",
            Code::WrongType => "wrong-type",
            Code::OutOfRange => "out-of-range",
        }
    }

    /// What is wrong and what to do about it, for people.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Code::InvalidUtf8 => "the file is not valid UTF-8 here; save it as UTF-8",
            Code::ControlCharacter => {
                "control characters other than tab are not allowed; write them as an escape inside a string, such as \\u{1}"
            }
            Code::LoneCarriageReturn => {
                "a carriage return must be followed by a line feed; save the file with LF or CR LF line endings"
            }
            Code::UnexpectedCharacter => {
                "this cannot start a setting or a value; a line holds a setting such as name = value, or a block such as name { }"
            }
            Code::ExpectedEquals => "expected = or { after the name",
            Code::ExpectedBrace => "expected { after the label of the block",
            Code::ExpectedValue => "expected a value here",
            Code::ExpectedNewline => {
                "only a comment may follow a setting on its line; start a new line for the next one"
            }
            Code::ExpectedComma => "expected , or ] after the list item",
            Code::DuplicateKey => "this key is already set in the block; remove one of them",
            Code::NameConflict => {
                "this name is already used in the block for the other kind of item; a name is either a setting or blocks, so rename one of them"
            }
            Code::DuplicateLabel => {
                "a block with this name and label already exists; give this one another label"
            }
            Code::UnclosedBlock => "this block is never closed; add a } for it",
            Code::UnexpectedClose => "this } closes no block; remove it",
            Code::UnterminatedString => {
                "this string is never closed on its line; add a \" at its end, or use \"\"\" for text over several lines"
            }
            Code::InvalidEscape => {
                "this is not an escape; the escapes are \\\\, \\\", \\n, \\r, \\t, \\0 and \\u{...}"
            }
            Code::InvalidUnicodeEscape => {
                "\\u needs 1 to 6 hexadecimal digits in braces naming a Unicode character, such as \\u{e9}"
            }
            Code::TextOpening => {
                "nothing may follow the opening \"\"\" on its line; start the text on the next line"
            }
            Code::TextIndent => {
                "this line is indented less than the closing \"\"\"; indent it at least as far, with the same spaces or tabs"
            }
            Code::UnterminatedText => {
                "this text is never closed; end it with \"\"\" on a line of its own"
            }
            Code::UnterminatedList => "this list is never closed; add a ] at its end",
            Code::InvalidNumber => {
                "this number is malformed; check for a misplaced _ or . or a unit followed by more"
            }
            Code::NumberOverflow => "this number is too large for 64 bits",
            Code::UnitNotAllowed => {
                "hexadecimal, octal and binary numbers cannot carry a unit; write the value in decimal"
            }
            Code::TooDeep => "blocks and lists nest more than 64 levels deep here",
            Code::WrongType => "this value has the wrong type for the setting",
            Code::OutOfRange => {
                "this value is out of range; it must come to a whole number that fits in 64 bits, and a size cannot be negative"
            }
        }
    }
}

/// A problem in a document, or in a value a program asked for. Reading
/// stops at the first one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub code: Code,
    pub at: Pos,
}

impl Error {
    pub(crate) fn new(code: Code, at: Pos) -> Error {
        Error { code, at }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.code.message())
    }
}
