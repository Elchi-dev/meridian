//! A bc program's text as its tokens: numbers, strings, names, keywords
//! and operators, with the lines they are on. Comments, blanks and
//! newlines a backslash escapes are none.

use alloc::string::String;
use alloc::vec::Vec;

/// The operators that take two numbers, and those assignment combines
/// with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
}

/// The comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// The digits and point of a number as written, read in `ibase` when
    /// the program runs.
    Number(Vec<u8>),
    String(Vec<u8>),
    Name(String),
    Auto,
    Break,
    Continue,
    Define,
    Else,
    For,
    Halt,
    Ibase,
    If,
    Length,
    Obase,
    Print,
    Quit,
    Return,
    Scale,
    Sqrt,
    While,
    Op(Op),
    /// `=`, or an operator and `=`.
    Assign(Option<Op>),
    Rel(Rel),
    Increment,
    Decrement,
    Not,
    And,
    Or,
    Open,
    Close,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    Comma,
    Semicolon,
    Newline,
}

/// A token, the line it begins on, counted from 1, and where its bytes
/// begin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spot {
    pub token: Token,
    pub line: usize,
    pub at: usize,
}

/// Why the text has no tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wrong {
    /// It ends inside a string or a comment, which more text may close,
    /// and where that begins.
    Unfinished(usize),
    /// A character no token begins with, on its line, and where that
    /// line begins and ends.
    Character(char, usize, usize, usize),
}

fn keyword(name: &str) -> Option<Token> {
    Some(match name {
        "auto" => Token::Auto,
        "break" => Token::Break,
        "continue" => Token::Continue,
        "define" => Token::Define,
        "else" => Token::Else,
        "for" => Token::For,
        "halt" => Token::Halt,
        "ibase" => Token::Ibase,
        "if" => Token::If,
        "length" => Token::Length,
        "obase" => Token::Obase,
        "print" => Token::Print,
        "quit" => Token::Quit,
        "return" => Token::Return,
        "scale" => Token::Scale,
        "sqrt" => Token::Sqrt,
        "while" => Token::While,
        _ => return None,
    })
}

/// How many newlines `bytes` holds.
fn newlines(bytes: &[u8]) -> usize {
    bytes.iter().map(|&b| usize::from(b == b'\n')).sum()
}

/// The tokens of `text`, the first on line `line`.
///
/// # Errors
///
/// When it ends inside a string or comment, or holds a character no
/// token begins with.
pub fn tokens(text: &[u8], mut line: usize) -> Result<Vec<Spot>, Wrong> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&c) = text.get(at) {
        let next = text.get(at + 1).copied();
        let (start, from) = (line, at);
        let token = match c {
            b' ' | b'\t' | b'\r' => {
                at += 1;
                continue;
            }
            b'\\' if next == Some(b'\n') => {
                at += 2;
                line += 1;
                continue;
            }
            b'#' => {
                while text.get(at).is_some_and(|&c| c != b'\n') {
                    at += 1;
                }
                continue;
            }
            b'/' if next == Some(b'*') => {
                let end = text[at + 2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .ok_or(Wrong::Unfinished(at))?;
                line += newlines(&text[at..at + 2 + end]);
                at += end + 4;
                continue;
            }
            b'\n' => {
                at += 1;
                line += 1;
                Token::Newline
            }
            b'"' => {
                let len = text[at + 1..]
                    .iter()
                    .position(|&b| b == b'"')
                    .ok_or(Wrong::Unfinished(at))?;
                let string = text[at + 1..at + 1 + len].to_vec();
                line += newlines(&string);
                at += len + 2;
                Token::String(string)
            }
            b'0'..=b'9' | b'A'..=b'F' | b'.' => {
                let from = at;
                let mut point = false;
                while let Some(&d) = text.get(at) {
                    match d {
                        b'0'..=b'9' | b'A'..=b'F' => at += 1,
                        b'.' if !point => {
                            point = true;
                            at += 1;
                        }
                        // A newline a backslash escapes may part a number.
                        b'\\' if text.get(at + 1) == Some(&b'\n') => {
                            at += 2;
                            line += 1;
                        }
                        _ => break,
                    }
                }
                let digits: Vec<u8> = text[from..at]
                    .iter()
                    .copied()
                    .filter(|&b| b != b'\\' && b != b'\n')
                    .collect();
                Token::Number(digits)
            }
            b'a'..=b'z' => {
                let from = at;
                while text
                    .get(at)
                    .is_some_and(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    at += 1;
                }
                let name = String::from_utf8_lossy(&text[from..at]).into_owned();
                keyword(&name).unwrap_or(Token::Name(name))
            }
            _ => {
                let (token, len) = operator(c, next).ok_or_else(|| unknown(text, at, line))?;
                at += len;
                token
            }
        };
        out.push(Spot {
            token,
            line: start,
            at: from,
        });
    }
    Ok(out)
}

/// The character at `at` on line `line`, which no token begins with, as
/// a mistake that takes its line.
fn unknown(text: &[u8], at: usize, line: usize) -> Wrong {
    let shown = String::from_utf8_lossy(&text[at..]);
    let begins = text[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |n| n + 1);
    let end = text[at..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(text.len(), |n| at + n + 1);
    Wrong::Character(shown.chars().next().unwrap_or('?'), line, begins, end)
}

/// The operator `c`, with `next` after it, begins, and how many bytes it
/// takes.
fn operator(c: u8, next: Option<u8>) -> Option<(Token, usize)> {
    let op = |c| match c {
        b'+' => Some(Op::Add),
        b'-' => Some(Op::Sub),
        b'*' => Some(Op::Mul),
        b'/' => Some(Op::Div),
        b'%' => Some(Op::Rem),
        b'^' => Some(Op::Pow),
        _ => None,
    };
    let two = match (c, next) {
        (b'+', Some(b'+')) => Some(Token::Increment),
        (b'-', Some(b'-')) => Some(Token::Decrement),
        (b'=', Some(b'=')) => Some(Token::Rel(Rel::Equal)),
        (b'!', Some(b'=')) => Some(Token::Rel(Rel::NotEqual)),
        (b'<', Some(b'=')) => Some(Token::Rel(Rel::LessEqual)),
        (b'>', Some(b'=')) => Some(Token::Rel(Rel::GreaterEqual)),
        (b'&', Some(b'&')) => Some(Token::And),
        (b'|', Some(b'|')) => Some(Token::Or),
        (_, Some(b'=')) => op(c).map(|o| Token::Assign(Some(o))),
        _ => None,
    };
    if let Some(token) = two {
        return Some((token, 2));
    }
    let one = match c {
        b'=' => Token::Assign(None),
        b'<' => Token::Rel(Rel::Less),
        b'>' => Token::Rel(Rel::Greater),
        b'!' => Token::Not,
        b'(' => Token::Open,
        b')' => Token::Close,
        b'[' => Token::OpenBracket,
        b']' => Token::CloseBracket,
        b'{' => Token::OpenBrace,
        b'}' => Token::CloseBrace,
        b',' => Token::Comma,
        b';' => Token::Semicolon,
        _ => Token::Op(op(c)?),
    };
    Some((one, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Result<Vec<Token>, Wrong> {
        tokens(text.as_bytes(), 1).map(|spots| spots.into_iter().map(|s| s.token).collect())
    }

    fn number(text: &str) -> Token {
        Token::Number(text.as_bytes().to_vec())
    }

    #[test]
    fn numbers_take_digits_capitals_and_one_point() {
        assert_eq!(kinds("12.5"), Ok(alloc::vec![number("12.5")]));
        assert_eq!(kinds(".5"), Ok(alloc::vec![number(".5")]));
        assert_eq!(kinds("FF.A"), Ok(alloc::vec![number("FF.A")]));
        assert_eq!(kinds("1.2.3"), Ok(alloc::vec![number("1.2"), number(".3")]));
        assert_eq!(
            kinds("12\\\n34"),
            Ok(alloc::vec![number("1234")]),
            "parted by an escaped newline"
        );
        assert_eq!(kinds("1G"), Err(Wrong::Character('G', 1, 0, 2)));
        assert_eq!(kinds("."), Ok(alloc::vec![number(".")]));
    }

    #[test]
    fn names_are_small_letters_and_keywords_are_known() {
        assert_eq!(
            kinds("abc x_1 if while scale ibase obase"),
            Ok(alloc::vec![
                Token::Name("abc".into()),
                Token::Name("x_1".into()),
                Token::If,
                Token::While,
                Token::Scale,
                Token::Ibase,
                Token::Obase
            ])
        );
        for (word, token) in [
            ("auto", Token::Auto),
            ("break", Token::Break),
            ("continue", Token::Continue),
            ("define", Token::Define),
            ("else", Token::Else),
            ("for", Token::For),
            ("halt", Token::Halt),
            ("length", Token::Length),
            ("print", Token::Print),
            ("quit", Token::Quit),
            ("return", Token::Return),
            ("sqrt", Token::Sqrt),
        ] {
            assert_eq!(kinds(word), Ok(alloc::vec![token]), "{word}");
        }
        assert_eq!(kinds("ifs"), Ok(alloc::vec![Token::Name("ifs".into())]));
    }

    #[test]
    fn operators_take_the_longest_they_can() {
        assert_eq!(
            kinds("+ ++ += - -- -= * *= / /= % %= ^ ^= = =="),
            Ok(alloc::vec![
                Token::Op(Op::Add),
                Token::Increment,
                Token::Assign(Some(Op::Add)),
                Token::Op(Op::Sub),
                Token::Decrement,
                Token::Assign(Some(Op::Sub)),
                Token::Op(Op::Mul),
                Token::Assign(Some(Op::Mul)),
                Token::Op(Op::Div),
                Token::Assign(Some(Op::Div)),
                Token::Op(Op::Rem),
                Token::Assign(Some(Op::Rem)),
                Token::Op(Op::Pow),
                Token::Assign(Some(Op::Pow)),
                Token::Assign(None),
                Token::Rel(Rel::Equal)
            ])
        );
        assert_eq!(
            kinds("< <= > >= != ! && || ( ) [ ] { } , ;"),
            Ok(alloc::vec![
                Token::Rel(Rel::Less),
                Token::Rel(Rel::LessEqual),
                Token::Rel(Rel::Greater),
                Token::Rel(Rel::GreaterEqual),
                Token::Rel(Rel::NotEqual),
                Token::Not,
                Token::And,
                Token::Or,
                Token::Open,
                Token::Close,
                Token::OpenBracket,
                Token::CloseBracket,
                Token::OpenBrace,
                Token::CloseBrace,
                Token::Comma,
                Token::Semicolon
            ])
        );
        assert_eq!(kinds("a+++b").map(|t| t.len()), Ok(4), "a ++ + b");
        assert_eq!(kinds("&"), Err(Wrong::Character('&', 1, 0, 1)));
        assert_eq!(kinds("|"), Err(Wrong::Character('|', 1, 0, 1)));
    }

    #[test]
    fn comments_blanks_and_escaped_newlines_are_none() {
        assert_eq!(
            kinds("1 /* a\nb */ 2 # c\n3\t\r"),
            Ok(alloc::vec![
                number("1"),
                number("2"),
                Token::Newline,
                number("3")
            ])
        );
        assert_eq!(
            kinds("1 \\\n+ 2"),
            Ok(alloc::vec![number("1"), Token::Op(Op::Add), number("2")])
        );
        assert_eq!(kinds("1 /* open"), Err(Wrong::Unfinished(2)));
        assert_eq!(
            kinds("/ *"),
            Ok(alloc::vec![Token::Op(Op::Div), Token::Op(Op::Mul)])
        );
    }

    #[test]
    fn strings_keep_what_is_in_them() {
        assert_eq!(
            kinds("\"a\nb\\n\" 1"),
            Ok(alloc::vec![Token::String(b"a\nb\\n".to_vec()), number("1")])
        );
        assert_eq!(kinds("\"open"), Err(Wrong::Unfinished(0)));
        assert_eq!(kinds("\"\""), Ok(alloc::vec![Token::String(Vec::new())]));
    }

    #[test]
    fn tokens_know_their_lines() {
        let spots = tokens(b"1\n/* \n */ 2 \"x\ny\" 3\\\n4\n@", 5);
        assert_eq!(spots, Err(Wrong::Character('@', 10, 23, 24)));
        let spots = tokens(b"1\n/* \n */ 2 \"x\ny\" 3\\\n4", 5).unwrap();
        let lines: Vec<usize> = spots.iter().map(|s| s.line).collect();
        assert_eq!(lines, [5, 5, 7, 7, 8]);
        let starts: Vec<usize> = spots.iter().map(|s| s.at).collect();
        assert_eq!(starts, [0, 1, 10, 12, 18]);
        assert_eq!(spots[4].token, number("34"));
    }
}
