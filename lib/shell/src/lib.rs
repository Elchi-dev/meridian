//! Meridian's shell language, as `docs/formats/shell.md` describes it:
//! a script read into its words and operators ([`lex`]), and those into
//! the commands, lists and blocks they stand for ([`parse`]). Running them
//! is the shell's.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::fmt;

pub mod lex;
pub mod parse;

/// A place in a script: its line and its column, both counted from 1, the
/// column in Unicode scalar values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
}

/// What is wrong with a script, as the specification names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    /// A carriage return without a line feed after it.
    LoneCarriageReturn,
    /// A quote not closed on its line.
    UnterminatedQuote,
    /// `${` without a name and a `}`.
    BadVariable,
    /// A redirection without the path it leads to.
    MissingPath,
    /// A `|` without a command on one of its sides, or `and`, `or` or
    /// `not` without one after it.
    MissingCommand,
    /// `let`, `for` or `fn` without the name of what it makes.
    ExpectedName,
    /// `let NAME` without `=`.
    ExpectedEquals,
    /// `let NAME =` without the value.
    ExpectedValue,
    /// `for NAME` without `in`.
    ExpectedIn,
    /// A keyword whose block does not start on its line.
    ExpectedBrace,
    /// A `{` where no block starts.
    UnexpectedOpen,
    /// A `}` without a block.
    UnexpectedClose,
    /// A block still open at the end of the script.
    UnclosedBlock,
    /// `else` that does not follow an `if` block.
    UnexpectedElse,
    /// More after a statement on its line.
    ExpectedEnd,
}

impl Code {
    /// The name the specification gives it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Code::LoneCarriageReturn => "lone-carriage-return",
            Code::UnterminatedQuote => "unterminated-quote",
            Code::BadVariable => "bad-variable",
            Code::MissingPath => "missing-path",
            Code::MissingCommand => "missing-command",
            Code::ExpectedName => "expected-name",
            Code::ExpectedEquals => "expected-equals",
            Code::ExpectedValue => "expected-value",
            Code::ExpectedIn => "expected-in",
            Code::ExpectedBrace => "expected-brace",
            Code::UnexpectedOpen => "unexpected-open",
            Code::UnexpectedClose => "unexpected-close",
            Code::UnclosedBlock => "unclosed-block",
            Code::UnexpectedElse => "unexpected-else",
            Code::ExpectedEnd => "expected-end",
        }
    }

    /// What to do about it.
    #[must_use]
    pub fn help(self) -> &'static str {
        match self {
            Code::LoneCarriageReturn => {
                "end lines with a line feed, or a carriage return and a line feed"
            }
            Code::UnterminatedQuote => "close the quote that starts here on the same line",
            Code::BadVariable => "write ${name}, with a name of letters, digits and _",
            Code::MissingPath => "put the path of the file after the redirection",
            Code::MissingCommand => "put a command here",
            Code::ExpectedName => "name what this makes, with letters, digits and _",
            Code::ExpectedEquals => "write let NAME = VALUE",
            Code::ExpectedValue => "put the value after =",
            Code::ExpectedIn => "write for NAME in WORDS",
            Code::ExpectedBrace => "start the block with { on this line",
            Code::UnexpectedOpen => "quote the { to use it in a word",
            Code::UnexpectedClose => "remove the }, or open its block before it",
            Code::UnclosedBlock => "close the block this starts with }",
            Code::UnexpectedElse => "put else right after the } of an if block",
            Code::ExpectedEnd => "end the statement here, with ; or a new line",
        }
    }
}

/// A script's first error: where it is and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub at: Pos,
    pub code: Code,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}",
            self.at.line,
            self.at.column,
            self.code.name(),
            self.code.help()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODES: [Code; 15] = [
        Code::LoneCarriageReturn,
        Code::UnterminatedQuote,
        Code::BadVariable,
        Code::MissingPath,
        Code::MissingCommand,
        Code::ExpectedName,
        Code::ExpectedEquals,
        Code::ExpectedValue,
        Code::ExpectedIn,
        Code::ExpectedBrace,
        Code::UnexpectedOpen,
        Code::UnexpectedClose,
        Code::UnclosedBlock,
        Code::UnexpectedElse,
        Code::ExpectedEnd,
    ];

    #[test]
    fn every_code_has_the_name_the_specification_gives_it() {
        let spec = include_str!("../../../docs/formats/shell.md");
        for code in CODES {
            let name = code.name();
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{name}"
            );
            assert!(
                spec.contains(&format!("`{name}`")),
                "the specification names {name}"
            );
        }
        let mut names: Vec<&str> = CODES.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CODES.len(), "each its own");
    }

    #[test]
    fn every_code_says_what_to_do() {
        let mut helps: Vec<&str> = CODES.iter().map(|c| c.help()).collect();
        for help in &helps {
            assert!(help.len() > 10 && !help.ends_with('.'), "{help}");
        }
        helps.sort_unstable();
        helps.dedup();
        assert_eq!(helps.len(), CODES.len(), "each its own");
    }

    #[test]
    fn an_error_reads_as_its_place_name_and_help() {
        let e = Error {
            at: Pos { line: 3, column: 9 },
            code: Code::UnterminatedQuote,
        };
        assert_eq!(
            e.to_string(),
            "3:9: unterminated-quote: close the quote that starts here on the same line"
        );
    }

    #[test]
    fn each_error_reads_in_full() {
        let said: Vec<String> = CODES
            .iter()
            .enumerate()
            .map(|(i, &code)| {
                Error {
                    at: Pos {
                        line: i + 1,
                        column: 2 * i + 1,
                    },
                    code,
                }
                .to_string()
            })
            .collect();
        assert_eq!(
            said,
            [
                "1:1: lone-carriage-return: end lines with a line feed, or a carriage return and a line feed",
                "2:3: unterminated-quote: close the quote that starts here on the same line",
                "3:5: bad-variable: write ${name}, with a name of letters, digits and _",
                "4:7: missing-path: put the path of the file after the redirection",
                "5:9: missing-command: put a command here",
                "6:11: expected-name: name what this makes, with letters, digits and _",
                "7:13: expected-equals: write let NAME = VALUE",
                "8:15: expected-value: put the value after =",
                "9:17: expected-in: write for NAME in WORDS",
                "10:19: expected-brace: start the block with { on this line",
                "11:21: unexpected-open: quote the { to use it in a word",
                "12:23: unexpected-close: remove the }, or open its block before it",
                "13:25: unclosed-block: close the block this starts with }",
                "14:27: unexpected-else: put else right after the } of an if block",
                "15:29: expected-end: end the statement here, with ; or a new line",
            ]
        );
    }
}
