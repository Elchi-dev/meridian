//! Meridian's shell language, as `docs/formats/shell.md` describes it:
//! a script read into its words and operators ([`lex`]), and those into
//! the commands, lists and blocks they stand for ([`parse`]). Running them
//! is the shell's. The sh dialect is read by [`sh`].

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::fmt;

pub mod commands;
pub mod expand;
pub mod lex;
pub mod parse;
pub mod run;
pub mod sh;

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
    /// A variable that was never set, found while running.
    UnsetVariable,
    /// A pattern that matches no file, found while running.
    NoMatch,
    /// `return` with a word that is no number from 0 to 255.
    BadStatus,
    /// Functions that call each other too deep.
    TooDeep,
    /// In the sh dialect, a quote, an expansion or a backquote the script
    /// leaves open.
    Unclosed,
    /// In the sh dialect, compound commands, expansions, or parentheses and
    /// signs in arithmetic inside each other deeper than 16.
    TooNested,
    /// In the sh dialect, a reserved word or an operator where no command
    /// can start.
    Unexpected,
    /// In the sh dialect, `<<` or `<<-` without the word that ends its
    /// here-document.
    MissingDelimiter,
    /// In the sh dialect, `if` or `elif` without `then` after its condition.
    ExpectedThen,
    /// In the sh dialect, an `if` without its `fi`.
    ExpectedFi,
    /// In the sh dialect, `while`, `until` or `for` without `do`.
    ExpectedDo,
    /// In the sh dialect, a loop without its `done`.
    ExpectedDone,
    /// In the sh dialect, a `case` without its `esac`.
    ExpectedEsac,
    /// In the sh dialect, `case` without its word or `in`.
    ExpectedCaseIn,
    /// In the sh dialect, a case item without a pattern.
    ExpectedPattern,
    /// In the sh dialect, a subshell, a case item's patterns or `name(` without `)`.
    ExpectedCloseParen,
    /// In the sh dialect, a `{` without its `}`.
    ExpectedCloseBrace,
    /// In the sh dialect, `name()` without a compound command after it.
    ExpectedBody,
    /// In the sh dialect, what stands in `$((...))` that is not C's arithmetic.
    BadArithmetic,
    /// In the sh dialect, `/` or `%` by 0 in arithmetic.
    DivisionByZero,
    /// In the sh dialect, a variable in arithmetic whose value is no integer.
    NotANumber,
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
            Code::UnsetVariable => "unset-variable",
            Code::NoMatch => "no-match",
            Code::BadStatus => "bad-status",
            Code::TooDeep => "too-deep",
            Code::Unclosed => "unclosed",
            Code::TooNested => "too-nested",
            Code::Unexpected => "unexpected",
            Code::MissingDelimiter => "missing-delimiter",
            Code::ExpectedThen => "expected-then",
            Code::ExpectedFi => "expected-fi",
            Code::ExpectedDo => "expected-do",
            Code::ExpectedDone => "expected-done",
            Code::ExpectedEsac => "expected-esac",
            Code::ExpectedCaseIn => "expected-case-in",
            Code::ExpectedPattern => "expected-pattern",
            Code::ExpectedCloseParen => "expected-close-paren",
            Code::ExpectedCloseBrace => "expected-close-brace",
            Code::ExpectedBody => "expected-body",
            Code::BadArithmetic => "bad-arithmetic",
            Code::DivisionByZero => "division-by-zero",
            Code::NotANumber => "not-a-number",
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
            Code::UnsetVariable => "set the variable with let before this",
            Code::NoMatch => "no file matches the pattern; quote it to mean the word itself",
            Code::BadStatus => "return a number from 0 to 255",
            Code::TooDeep => "a function calls itself, or others call each other, too deep",
            Code::Unclosed => "close what starts here before the script ends",
            Code::TooNested => "this nests past 16 levels; move the inner part out",
            Code::Unexpected => "this does not belong here; quote it to mean the word itself",
            Code::MissingDelimiter => "put the word whose line ends the here-document after <<",
            Code::ExpectedThen => "write then after the condition",
            Code::ExpectedFi => "close the if with fi",
            Code::ExpectedDo => "write do before the body of the loop",
            Code::ExpectedDone => "close the loop with done",
            Code::ExpectedEsac => "close the case with esac",
            Code::ExpectedCaseIn => "write case WORD in, then the patterns",
            Code::ExpectedPattern => "put a pattern here, before ) or after |",
            Code::ExpectedCloseParen => "put a ) here to close what is open",
            Code::ExpectedCloseBrace => "close the { with }",
            Code::ExpectedBody => "give the function a body, such as { commands; }",
            Code::BadArithmetic => "write the arithmetic with numbers, names and C's operators",
            Code::DivisionByZero => "divide by something other than 0",
            Code::NotANumber => "give the variable a whole number before counting with it",
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

    const CODES: [Code; 36] = [
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
        Code::UnsetVariable,
        Code::NoMatch,
        Code::BadStatus,
        Code::TooDeep,
        Code::Unclosed,
        Code::TooNested,
        Code::Unexpected,
        Code::MissingDelimiter,
        Code::ExpectedThen,
        Code::ExpectedFi,
        Code::ExpectedDo,
        Code::ExpectedDone,
        Code::ExpectedEsac,
        Code::ExpectedCaseIn,
        Code::ExpectedPattern,
        Code::ExpectedCloseParen,
        Code::ExpectedCloseBrace,
        Code::ExpectedBody,
        Code::BadArithmetic,
        Code::DivisionByZero,
        Code::NotANumber,
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
                "16:31: unset-variable: set the variable with let before this",
                "17:33: no-match: no file matches the pattern; quote it to mean the word itself",
                "18:35: bad-status: return a number from 0 to 255",
                "19:37: too-deep: a function calls itself, or others call each other, too deep",
                "20:39: unclosed: close what starts here before the script ends",
                "21:41: too-nested: this nests past 16 levels; move the inner part out",
                "22:43: unexpected: this does not belong here; quote it to mean the word itself",
                "23:45: missing-delimiter: put the word whose line ends the here-document after <<",
                "24:47: expected-then: write then after the condition",
                "25:49: expected-fi: close the if with fi",
                "26:51: expected-do: write do before the body of the loop",
                "27:53: expected-done: close the loop with done",
                "28:55: expected-esac: close the case with esac",
                "29:57: expected-case-in: write case WORD in, then the patterns",
                "30:59: expected-pattern: put a pattern here, before ) or after |",
                "31:61: expected-close-paren: put a ) here to close what is open",
                "32:63: expected-close-brace: close the { with }",
                "33:65: expected-body: give the function a body, such as { commands; }",
                "34:67: bad-arithmetic: write the arithmetic with numbers, names and C's operators",
                "35:69: division-by-zero: divide by something other than 0",
                "36:71: not-a-number: give the variable a whole number before counting with it",
            ]
        );
    }
}
