//! An awk program's tokens, as POSIX's grammar has them. A `/` begins a
//! regular expression where an operand may begin, and divides after
//! one; a name with `(` right after it calls a function; a newline is a
//! token of its own, which the parser takes as an end of a statement or
//! passes over where the grammar lets a line go on.

use alloc::string::String;
use alloc::vec::Vec;

use crate::value;

/// The functions awk has built in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Atan2,
    Close,
    Cos,
    Exp,
    Fflush,
    Gsub,
    Index,
    Int,
    Length,
    Log,
    Match,
    Rand,
    Sin,
    Split,
    Sprintf,
    Sqrt,
    Srand,
    Sub,
    Substr,
    System,
    Tolower,
    Toupper,
}

/// The words awk keeps for itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    Begin,
    Break,
    Continue,
    Delete,
    Do,
    Else,
    End,
    Exit,
    For,
    Function,
    Getline,
    If,
    In,
    Next,
    Nextfile,
    Print,
    Printf,
    Return,
    While,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Newline,
    Semicolon,
    OpenBrace,
    CloseBrace,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    Comma,
    Number(f64),
    String(Vec<u8>),
    /// A regular expression, as `regex` reads it: the escapes awk has
    /// for it already turned into what they stand for.
    Regex(Vec<u8>),
    Name(String),
    /// A name with `(` right after it.
    FuncName(String),
    Builtin(Builtin),
    Keyword(Keyword),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Not,
    Greater,
    Less,
    Pipe,
    Question,
    Colon,
    Tilde,
    NoMatch,
    Dollar,
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    ModAssign,
    PowAssign,
    Incr,
    Decr,
    And,
    Or,
    Equal,
    NotEqual,
    LessEqual,
    GreaterEqual,
    Append,
}

/// What is wrong with a program, and on which of its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub what: String,
}

/// A token and the line it is on, counted from 1.
pub type Placed = (Token, usize);

const KEYWORDS: [(&str, Keyword); 19] = [
    ("BEGIN", Keyword::Begin),
    ("END", Keyword::End),
    ("break", Keyword::Break),
    ("continue", Keyword::Continue),
    ("delete", Keyword::Delete),
    ("do", Keyword::Do),
    ("else", Keyword::Else),
    ("exit", Keyword::Exit),
    ("for", Keyword::For),
    ("function", Keyword::Function),
    ("getline", Keyword::Getline),
    ("if", Keyword::If),
    ("in", Keyword::In),
    ("next", Keyword::Next),
    ("nextfile", Keyword::Nextfile),
    ("print", Keyword::Print),
    ("printf", Keyword::Printf),
    ("return", Keyword::Return),
    ("while", Keyword::While),
];

const BUILTINS: [(&str, Builtin); 22] = [
    ("atan2", Builtin::Atan2),
    ("close", Builtin::Close),
    ("cos", Builtin::Cos),
    ("exp", Builtin::Exp),
    ("fflush", Builtin::Fflush),
    ("gsub", Builtin::Gsub),
    ("index", Builtin::Index),
    ("int", Builtin::Int),
    ("length", Builtin::Length),
    ("log", Builtin::Log),
    ("match", Builtin::Match),
    ("rand", Builtin::Rand),
    ("sin", Builtin::Sin),
    ("split", Builtin::Split),
    ("sprintf", Builtin::Sprintf),
    ("sqrt", Builtin::Sqrt),
    ("srand", Builtin::Srand),
    ("sub", Builtin::Sub),
    ("substr", Builtin::Substr),
    ("system", Builtin::System),
    ("tolower", Builtin::Tolower),
    ("toupper", Builtin::Toupper),
];

/// Operators, the longer first so that each is taken whole.
const OPERATORS: [(&str, Token); 39] = [
    ("&&", Token::And),
    ("||", Token::Or),
    ("==", Token::Equal),
    ("!=", Token::NotEqual),
    ("<=", Token::LessEqual),
    (">=", Token::GreaterEqual),
    (">>", Token::Append),
    ("!~", Token::NoMatch),
    ("++", Token::Incr),
    ("--", Token::Decr),
    ("+=", Token::AddAssign),
    ("-=", Token::SubAssign),
    ("*=", Token::MulAssign),
    ("/=", Token::DivAssign),
    ("%=", Token::ModAssign),
    ("^=", Token::PowAssign),
    ("{", Token::OpenBrace),
    ("}", Token::CloseBrace),
    ("(", Token::OpenParen),
    (")", Token::CloseParen),
    ("[", Token::OpenBracket),
    ("]", Token::CloseBracket),
    (";", Token::Semicolon),
    (",", Token::Comma),
    ("+", Token::Plus),
    ("-", Token::Minus),
    ("*", Token::Star),
    ("/", Token::Slash),
    ("%", Token::Percent),
    ("^", Token::Caret),
    ("!", Token::Not),
    (">", Token::Greater),
    ("<", Token::Less),
    ("|", Token::Pipe),
    ("?", Token::Question),
    (":", Token::Colon),
    ("~", Token::Tilde),
    ("$", Token::Dollar),
    ("=", Token::Assign),
];

/// Whether a `/` after `token` divides, as it does after an operand,
/// rather than beginning a regular expression.
fn ends_operand(token: Option<&Token>) -> bool {
    matches!(
        token,
        Some(
            Token::Number(_)
                | Token::String(_)
                | Token::Regex(_)
                | Token::Name(_)
                | Token::Builtin(_)
                | Token::Keyword(Keyword::Getline)
                | Token::CloseParen
                | Token::CloseBracket
                | Token::Incr
                | Token::Decr
        )
    )
}

/// The byte an escape stands for in a string or a regular expression:
/// `\` then `rest`, and how many bytes of `rest` it takes; none for an
/// escape awk has no meaning for.
fn escape(rest: &[u8]) -> Option<(u8, usize)> {
    let octal = rest
        .iter()
        .take(3)
        .take_while(|b| (b'0'..=b'7').contains(b))
        .count();
    if octal > 0 {
        let n = rest[..octal]
            .iter()
            .fold(0u32, |n, &b| n * 8 + u32::from(b - b'0'));
        return Some((u8::try_from(n & 0xff).unwrap_or(0), octal));
    }
    let byte = match rest.first()? {
        b'"' => b'"',
        b'/' => b'/',
        b'a' => 0x07,
        b'b' => 0x08,
        b'f' => 0x0c,
        b'n' => b'\n',
        b'r' => b'\r',
        b't' => b'\t',
        b'v' => 0x0b,
        _ => return None,
    };
    Some((byte, 1))
}

/// `bytes` with the escapes a string in a program takes turned into what
/// they stand for, as an assignment among awk's operands has its value.
#[must_use]
pub fn unescaped(bytes: &[u8]) -> Vec<u8> {
    let mut taken = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'\\' {
            taken.push(bytes[at]);
            at += 1;
            continue;
        }
        let rest = &bytes[at + 1..];
        if let Some((byte, len)) = escape(rest) {
            taken.push(byte);
            at += 1 + len;
            continue;
        }
        match rest.first() {
            Some(b'\\') | None => taken.push(b'\\'),
            Some(&other) => taken.extend_from_slice(&[b'\\', other]),
        }
        at += 2;
    }
    taken
}

/// The characters a regular expression gives a meaning of their own,
/// which an octal escape that writes one of them stands for as itself.
const SPECIAL: &[u8] = b"\\^$.[]|()*+?{}";

struct Lexer<'s> {
    text: &'s [u8],
    at: usize,
    line: usize,
    tokens: Vec<Placed>,
}

impl Lexer<'_> {
    fn error(&self, what: &str) -> Error {
        Error {
            line: self.line,
            what: String::from(what),
        }
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.text.get(self.at + ahead).copied()
    }

    /// The string whose `"` is at hand, its escapes taken.
    fn string(&mut self) -> Result<Token, Error> {
        self.at += 1;
        let mut bytes = Vec::new();
        loop {
            match self.peek(0) {
                None | Some(b'\n') => return Err(self.error("a string goes on past its line")),
                Some(b'"') => break,
                Some(b'\\') => {
                    let rest = &self.text[self.at + 1..];
                    if let Some((byte, len)) = escape(rest) {
                        bytes.push(byte);
                        self.at += 1 + len;
                        continue;
                    }
                    match rest.first() {
                        Some(b'\\') => bytes.push(b'\\'),
                        Some(b'\n') => self.line += 1,
                        // An escape awk has no meaning for keeps its
                        // backslash, as a regular expression made of the
                        // string may want it.
                        Some(&other) => bytes.extend_from_slice(&[b'\\', other]),
                        None => return Err(self.error("a string goes on past its line")),
                    }
                    self.at += 2;
                    continue;
                }
                Some(byte) => bytes.push(byte),
            }
            self.at += 1;
        }
        self.at += 1;
        Ok(Token::String(bytes))
    }

    /// The regular expression whose `/` is at hand, as `regex` reads it.
    fn regex(&mut self) -> Result<Token, Error> {
        self.at += 1;
        let mut bytes = Vec::new();
        let mut bracket = false;
        loop {
            let unended = || self.error("a regular expression goes on past its line");
            match self.peek(0) {
                None | Some(b'\n') => return Err(unended()),
                Some(b'/') if !bracket => break,
                Some(b'\\') => {
                    let rest = &self.text[self.at + 1..];
                    if let Some((byte, len)) = escape(rest) {
                        if SPECIAL.contains(&byte) && rest[0].is_ascii_digit() {
                            bytes.push(b'\\');
                        }
                        bytes.push(byte);
                        self.at += 1 + len;
                        continue;
                    }
                    let &next = rest.first().ok_or_else(unended)?;
                    if next == b'\n' {
                        return Err(unended());
                    }
                    bytes.extend_from_slice(&[b'\\', next]);
                    self.at += 2;
                    continue;
                }
                Some(b'[') if !bracket => {
                    bracket = true;
                    bytes.push(b'[');
                    self.at += 1;
                    // A ] first in the bracket, after any ^, is one of its
                    // characters.
                    for lead in *b"^]" {
                        if self.peek(0) == Some(lead) {
                            bytes.push(lead);
                            self.at += 1;
                        }
                    }
                    continue;
                }
                Some(b'[') if matches!(self.peek(1), Some(b':' | b'.' | b'=')) => {
                    // A class such as [:alpha:] ends with its own ], which
                    // does not end the bracket.
                    let close = [self.peek(1).unwrap_or(b':'), b']'];
                    let from = self.at;
                    let end = self.text[from + 2..]
                        .windows(2)
                        .position(|pair| pair == close)
                        .ok_or_else(unended)?;
                    bytes.extend_from_slice(&self.text[from..from + 2 + end + 2]);
                    self.at = from + 2 + end + 2;
                    continue;
                }
                Some(b']') if bracket => bracket = false,
                Some(_) => {}
            }
            bytes.push(self.text[self.at]);
            self.at += 1;
        }
        self.at += 1;
        Ok(Token::Regex(bytes))
    }

    /// The number whose first digit or point is at hand.
    fn number(&mut self) -> Token {
        let len = value::number_len(&self.text[self.at..]);
        let written = &self.text[self.at..self.at + len];
        self.at += len;
        Token::Number(value::leading(written))
    }

    /// The name, keyword or built-in function whose first letter is at
    /// hand.
    fn word(&mut self) -> Token {
        let from = self.at;
        while self
            .peek(0)
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.at += 1;
        }
        let word = core::str::from_utf8(&self.text[from..self.at]).unwrap_or_default();
        if let Some(&(_, keyword)) = KEYWORDS.iter().find(|(w, _)| *w == word) {
            return Token::Keyword(keyword);
        }
        if let Some(&(_, builtin)) = BUILTINS.iter().find(|(w, _)| *w == word) {
            return Token::Builtin(builtin);
        }
        if self.peek(0) == Some(b'(') {
            Token::FuncName(String::from(word))
        } else {
            Token::Name(String::from(word))
        }
    }

    /// The next token and its line; none at the end.
    fn next(&mut self) -> Result<Option<Placed>, Error> {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b' ' | b'\t' | b'\r'), _) => self.at += 1,
                (Some(b'\\'), Some(b'\n')) => {
                    self.at += 2;
                    self.line += 1;
                }
                (Some(b'\\'), Some(b'\r')) if self.peek(2) == Some(b'\n') => {
                    self.at += 3;
                    self.line += 1;
                }
                (Some(b'#'), _) => {
                    while self.peek(0).is_some_and(|b| b != b'\n') {
                        self.at += 1;
                    }
                }
                _ => break,
            }
        }
        let Some(byte) = self.peek(0) else {
            return Ok(None);
        };
        let line = self.line;
        let token = match byte {
            b'\n' => {
                self.at += 1;
                self.line += 1;
                Token::Newline
            }
            b'"' => self.string()?,
            b'/' if !ends_operand(self.tokens.last().map(|(token, _)| token)) => self.regex()?,
            b'0'..=b'9' => self.number(),
            b'.' if self.peek(1).is_some_and(|b| b.is_ascii_digit()) => self.number(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.word(),
            _ => {
                let rest = &self.text[self.at..];
                let Some((written, token)) = OPERATORS
                    .iter()
                    .find(|(written, _)| rest.starts_with(written.as_bytes()))
                else {
                    let shown = core::str::from_utf8(rest)
                        .ok()
                        .and_then(|text| text.chars().next())
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    return Err(self.error(&alloc::format!("{shown} means nothing here")));
                };
                self.at += written.len();
                token.clone()
            }
        };
        Ok(Some((token, line)))
    }
}

/// The tokens of the program `text`, each with its line.
///
/// # Errors
///
/// For a string or a regular expression that goes on past its line, and
/// a character that is no token.
pub fn tokens(text: &str) -> Result<Vec<Placed>, Error> {
    let mut lexer = Lexer {
        text: text.as_bytes(),
        at: 0,
        line: 1,
        tokens: Vec::new(),
    };
    while let Some(placed) = lexer.next()? {
        lexer.tokens.push(placed);
    }
    Ok(lexer.tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Token as T;

    fn lexed(text: &str) -> Vec<Token> {
        tokens(text)
            .unwrap()
            .into_iter()
            .map(|(token, _)| token)
            .collect()
    }

    fn name(word: &str) -> Token {
        T::Name(String::from(word))
    }

    fn string(text: &[u8]) -> Token {
        T::String(text.to_vec())
    }

    fn regex(text: &[u8]) -> Token {
        T::Regex(text.to_vec())
    }

    #[test]
    fn words_are_names_keywords_builtins_or_calls() {
        assert_eq!(
            lexed("BEGIN getline length foo f(x) g (y) in_x"),
            [
                T::Keyword(Keyword::Begin),
                T::Keyword(Keyword::Getline),
                T::Builtin(Builtin::Length),
                name("foo"),
                T::FuncName("f".into()),
                T::OpenParen,
                name("x"),
                T::CloseParen,
                name("g"),
                T::OpenParen,
                name("y"),
                T::CloseParen,
                name("in_x"),
            ]
        );
        for (word, keyword) in KEYWORDS {
            assert_eq!(lexed(word), [T::Keyword(keyword)]);
        }
        for (word, builtin) in BUILTINS {
            assert_eq!(lexed(word), [T::Builtin(builtin)]);
        }
    }

    #[test]
    fn numbers_take_points_and_powers() {
        assert_eq!(
            lexed("1 2.5 .5 5. 1e3 1E-2 1e 0x"),
            [
                T::Number(1.0),
                T::Number(2.5),
                T::Number(0.5),
                T::Number(5.0),
                T::Number(1000.0),
                T::Number(0.01),
                T::Number(1.0),
                name("e"),
                T::Number(0.0),
                name("x"),
            ]
        );
        assert_eq!(lexed("a.5"), [name("a"), T::Number(0.5)]);
    }

    #[test]
    fn operators_are_taken_longest_first() {
        assert_eq!(
            lexed("a+=b++ - --c>=d>>e!~f!g&&h||i==j!=k<=l^=m"),
            [
                name("a"),
                T::AddAssign,
                name("b"),
                T::Incr,
                T::Minus,
                T::Decr,
                name("c"),
                T::GreaterEqual,
                name("d"),
                T::Append,
                name("e"),
                T::NoMatch,
                name("f"),
                T::Not,
                name("g"),
                T::And,
                name("h"),
                T::Or,
                name("i"),
                T::Equal,
                name("j"),
                T::NotEqual,
                name("k"),
                T::LessEqual,
                name("l"),
                T::PowAssign,
                name("m"),
            ]
        );
        for (written, token) in OPERATORS {
            // Alone a slash begins a regular expression.
            if !written.starts_with('/') {
                assert_eq!(lexed(written), [token], "{written}");
            }
        }
        assert_eq!(
            lexed("x /= 2"),
            [name("x"), T::DivAssign, T::Number(2.0)],
            "after an operand it divides"
        );
        assert_eq!(
            lexed("$1 ? : ~ | % ="),
            [
                T::Dollar,
                T::Number(1.0),
                T::Question,
                T::Colon,
                T::Tilde,
                T::Pipe,
                T::Percent,
                T::Assign
            ]
        );
        assert_eq!(
            lexed("{ } [ ] ; , * <"),
            [
                T::OpenBrace,
                T::CloseBrace,
                T::OpenBracket,
                T::CloseBracket,
                T::Semicolon,
                T::Comma,
                T::Star,
                T::Less
            ]
        );
    }

    #[test]
    fn a_slash_divides_after_an_operand_and_begins_a_regex_else() {
        assert_eq!(
            lexed("a / b / c"),
            [name("a"), T::Slash, name("b"), T::Slash, name("c")]
        );
        assert_eq!(lexed("/a/ ~ /b/"), [regex(b"a"), T::Tilde, regex(b"b")]);
        assert_eq!(lexed("x ~ /=/"), [name("x"), T::Tilde, regex(b"=")]);
        for before in [
            "1", "\"s\"", "/r/", "n", "length", "getline", ")", "]", "i++", "i--",
        ] {
            let found = lexed(&alloc::format!("{before} / 2 /"));
            assert!(found.contains(&T::Slash), "{before}: {found:?}");
        }
        for before in [
            "(", ",", "!", "&&", "||", "~", "{", ";", "\n", "print", "$", "=", "-",
        ] {
            let found = lexed(&alloc::format!("{before} /2/"));
            assert_eq!(found.last(), Some(&regex(b"2")), "{before}: {found:?}");
        }
    }

    #[test]
    fn strings_take_their_escapes() {
        assert_eq!(
            lexed(r#""a\"b\\c\/d\n\t\a\b\f\r\v\101\0\1234" "\q\." "#),
            [
                string(b"a\"b\\c/d\n\t\x07\x08\x0c\r\x0bA\0S4"),
                string(b"\\q\\."),
            ]
        );
        assert_eq!(
            lexed("\"a\\\nb\""),
            [string(b"ab")],
            "a backslash at its end goes on"
        );
        assert_eq!(
            lexed(r#""\777""#),
            [string(b"\xff")],
            "an octal escape is a byte"
        );
        assert_eq!(lexed("\"\""), [string(b"")]);
    }

    #[test]
    fn an_operand_s_value_takes_the_escapes_a_string_does() {
        assert_eq!(unescaped(br"a\tb\\c\101\q"), b"a\tb\\cA\\q");
        assert_eq!(unescaped(br#"\"\/"#), b"\"/");
        assert_eq!(
            unescaped(br"end\"),
            b"end\\",
            "a backslash at the end stays"
        );
        assert_eq!(unescaped(b""), b"");
        assert_eq!(unescaped(b"\xff"), b"\xff");
    }

    #[test]
    fn regexes_take_their_escapes_and_brackets() {
        assert_eq!(
            lexed(r#"/a\/b\.c\\d\n\t\"/"#),
            [regex(b"a/b\\.c\\\\d\n\t\"")]
        );
        assert_eq!(
            lexed(r"/\056\101/"),
            [regex(b"\\.A")],
            "an escape that writes . means a point"
        );
        assert_eq!(lexed("/[/]x/"), [regex(b"[/]x")]);
        assert_eq!(lexed("/[]/]/"), [regex(b"[]/]")]);
        assert_eq!(lexed("/[^]/]/"), [regex(b"[^]/]")]);
        assert_eq!(lexed("/[[:alpha:]/]+/"), [regex(b"[[:alpha:]/]+")]);
        assert_eq!(lexed("/[[.-.]/]/"), [regex(b"[[.-.]/]")]);
    }

    #[test]
    fn newlines_comments_and_continued_lines() {
        let placed = tokens("a # one\n\nb \\\n c\\\r\n d\n").unwrap();
        assert_eq!(
            placed,
            [
                (name("a"), 1),
                (T::Newline, 1),
                (T::Newline, 2),
                (name("b"), 3),
                (name("c"), 4),
                (name("d"), 5),
                (T::Newline, 5),
            ]
        );
        assert_eq!(lexed("# all of it"), Vec::<Token>::new());
    }

    #[test]
    fn what_cannot_be_read_says_its_line() {
        let wrong = |text: &str| tokens(text).unwrap_err();
        assert_eq!(
            wrong("x\n\"abc\nd\""),
            Error {
                line: 2,
                what: "a string goes on past its line".into()
            }
        );
        assert_eq!(wrong("\"abc").what, "a string goes on past its line");
        assert_eq!(wrong("\"abc\\").what, "a string goes on past its line");
        assert_eq!(wrong("x\n\n/ab").line, 3);
        assert_eq!(
            wrong("/ab\n/").what,
            "a regular expression goes on past its line"
        );
        assert_eq!(
            wrong("/ab\\").what,
            "a regular expression goes on past its line"
        );
        assert_eq!(
            wrong("/ab\\\n/").what,
            "a regular expression goes on past its line"
        );
        assert_eq!(
            wrong("/[[:alpha/").what,
            "a regular expression goes on past its line"
        );
        assert_eq!(
            wrong("/a[/").what,
            "a regular expression goes on past its line",
            "a / in a bracket ends nothing"
        );
        assert_eq!(wrong("a @ b").what, "@ means nothing here");
        assert_eq!(wrong("a & b").what, "& means nothing here");
        assert_eq!(wrong("é").what, "é means nothing here");
    }

    #[test]
    fn lines_count_through_strings_and_continued_lines() {
        let placed = tokens("a\n\"x\\\ny\"\nb").unwrap();
        assert_eq!(
            placed,
            [
                (name("a"), 1),
                (T::Newline, 1),
                (string(b"xy"), 2),
                (T::Newline, 3),
                (name("b"), 4),
            ],
            "a string that goes on to the next line counts it"
        );
        let placed = tokens("a\r\nb").unwrap();
        assert_eq!(
            placed,
            [(name("a"), 1), (T::Newline, 1), (name("b"), 2)],
            "a carriage return is a blank"
        );
        assert_eq!(tokens("\n\n\n\"x").unwrap_err().line, 4);
    }

    #[test]
    fn numbers_stop_where_they_stop_being_numbers() {
        assert_eq!(lexed("1.e5"), [T::Number(100_000.0)]);
        assert_eq!(lexed("0001"), [T::Number(1.0)]);
        assert_eq!(
            lexed("1e+x"),
            [T::Number(1.0), name("e"), T::Plus, name("x")]
        );
        assert_eq!(lexed("1..2"), [T::Number(1.0), T::Number(0.2)]);
        assert_eq!(lexed("12abc"), [T::Number(12.0), name("abc")]);
        assert_eq!(lexed("1e400"), [T::Number(f64::INFINITY)]);
        assert_eq!(lexed("1e-400"), [T::Number(0.0)]);
        assert_eq!(
            tokens(".").unwrap_err().what,
            ". means nothing here",
            "a point alone is no number"
        );
    }

    #[test]
    fn a_slash_after_keywords_and_operators_begins_a_regex() {
        for before in [
            "print", "printf", "return", "in", "&&", "||", "?", ":", ",", "~", "!~", "+", "*", "<",
            "==", "(", "[",
        ] {
            let found = lexed(&format!("{before} /a/"));
            assert_eq!(found.last(), Some(&regex(b"a")), "{before}: {found:?}");
        }
        for before in ["x", "1", "\"s\"", ")", "]", "$x", "a[1]", "length"] {
            let found = lexed(&format!("{before} /a/ 2"));
            assert!(found.contains(&T::Slash), "{before}: {found:?}");
        }
    }

    #[test]
    fn calls_are_names_right_before_a_parenthesis() {
        assert_eq!(lexed("f(x)")[0], T::FuncName("f".into()));
        assert_eq!(lexed("f (x)")[0], name("f"), "with a blank it is a name");
        assert_eq!(
            lexed("length (x)")[0],
            T::Builtin(Builtin::Length),
            "a builtin is one either way"
        );
        assert_eq!(lexed("_a1(")[0], T::FuncName("_a1".into()));
        assert_eq!(
            lexed("in(")[0],
            T::Keyword(Keyword::In),
            "a keyword stays one"
        );
    }

    #[test]
    fn escapes_of_three_octal_digits_and_fewer() {
        assert_eq!(lexed(r#""\0\07\101\1012""#), [string(b"\0\x07AA2")]);
        assert_eq!(
            lexed(r#""\8\9""#),
            [string(b"\\8\\9")],
            "8 and 9 are no octal digits"
        );
        assert_eq!(
            lexed(r#""\400""#),
            [string(b"\x00")],
            "a byte is the low eight bits"
        );
        assert_eq!(lexed(r"/\101\/\./"), [regex(b"A/\\.")]);
        assert_eq!(
            lexed(r"/\052/"),
            [regex(b"\\*")],
            "an octal * is a star, not a repetition"
        );
        assert_eq!(
            lexed(r"/\q/"),
            [regex(b"\\q")],
            "an escape awk does not know goes to the regex"
        );
    }

    #[test]
    fn a_whole_program_reads_into_its_tokens() {
        let text = "BEGIN { FS = \":\" }\n$3 >= 1000 { n[$1]++ }\nEND { for (u in n) printf(\"%s %d\\n\", u, n[u]) }\n";
        let found = lexed(text);
        assert_eq!(
            found,
            [
                T::Keyword(Keyword::Begin),
                T::OpenBrace,
                name("FS"),
                T::Assign,
                string(b":"),
                T::CloseBrace,
                T::Newline,
                T::Dollar,
                T::Number(3.0),
                T::GreaterEqual,
                T::Number(1000.0),
                T::OpenBrace,
                name("n"),
                T::OpenBracket,
                T::Dollar,
                T::Number(1.0),
                T::CloseBracket,
                T::Incr,
                T::CloseBrace,
                T::Newline,
                T::Keyword(Keyword::End),
                T::OpenBrace,
                T::Keyword(Keyword::For),
                T::OpenParen,
                name("u"),
                T::Keyword(Keyword::In),
                name("n"),
                T::CloseParen,
                T::Keyword(Keyword::Printf),
                T::OpenParen,
                string(b"%s %d\n"),
                T::Comma,
                name("u"),
                T::Comma,
                name("n"),
                T::OpenBracket,
                name("u"),
                T::CloseBracket,
                T::CloseParen,
                T::CloseBrace,
                T::Newline,
            ]
        );
    }
}
