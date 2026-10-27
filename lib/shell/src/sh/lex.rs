//! A script of the sh dialect read as tokens, as POSIX's token
//! recognition does: operators by the longest that fits, quotes, escapes,
//! comments, and the start and end of each expansion a `$` or a backquote
//! brings into a word, which is read into its parts when it is expanded.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{Code, Error, Pos};

/// What a `$` or a backquote brings into a word, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expansion {
    /// `$name`, or one of the special parameters, `$?` or `$1` on.
    Parameter(String),
    /// What stands between `${` and `}`.
    Braced(String),
    /// What stands between `$(` and `)`, or between backquotes.
    Command(String),
    /// What stands between `$((` and `))`.
    Arithmetic(String),
}

/// A piece of a word, and whether it stood in quotes, where what it brings
/// is neither split into fields nor matched against names of files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text { text: String, quoted: bool },
    Expand { expansion: Expansion, quoted: bool },
}

/// A word of a script and where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<Part>,
    pub at: Pos,
}

impl Word {
    /// Its text when it is all unquoted characters, as reserved words,
    /// names and assignments are written.
    #[must_use]
    pub fn plain(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [
                Part::Text {
                    text,
                    quoted: false,
                },
            ] => Some(text),
            _ => None,
        }
    }
}

/// The operators of the dialect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `&`
    Background,
    /// `&&`
    AndIf,
    /// `|`
    Pipe,
    /// `||`
    OrIf,
    /// `;`
    Semi,
    /// `;;`
    DoubleSemi,
    /// `;&`
    SemiAnd,
    /// `<`
    Less,
    /// `<<`
    DoubleLess,
    /// `<<-`
    DoubleLessDash,
    /// `<&`
    LessAnd,
    /// `<>`
    LessGreat,
    /// `>`
    Great,
    /// `>>`
    DoubleGreat,
    /// `>&`
    GreatAnd,
    /// `>|`
    Clobber,
    /// `(`
    Open,
    /// `)`
    Close,
}

/// How deep expansions may stand inside each other. Reading each one
/// takes some of a program's stack, which is 64 KiB, and scripts rarely
/// go past three or four.
const DEEPEST: usize = 16;

/// The operators as they are written.
const OPERATORS: [(&str, Op); 18] = [
    ("<<-", Op::DoubleLessDash),
    ("&&", Op::AndIf),
    ("||", Op::OrIf),
    (";;", Op::DoubleSemi),
    (";&", Op::SemiAnd),
    ("<<", Op::DoubleLess),
    ("<&", Op::LessAnd),
    ("<>", Op::LessGreat),
    (">>", Op::DoubleGreat),
    (">&", Op::GreatAnd),
    (">|", Op::Clobber),
    ("&", Op::Background),
    ("|", Op::Pipe),
    (";", Op::Semi),
    ("<", Op::Less),
    (">", Op::Great),
    ("(", Op::Open),
    (")", Op::Close),
];

/// A token of a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Word(Word),
    /// Digits right before `<` or `>`, which name the stream it redirects.
    IoNumber(u32),
    Op(Op),
    Newline,
}

/// The script's characters with where each one is.
struct Chars {
    all: Vec<char>,
    at: usize,
    pos: Pos,
    /// How many expansions the one being read stands inside.
    depth: usize,
}

impl Chars {
    fn new(script: &str) -> Chars {
        Chars {
            all: script.chars().collect(),
            at: 0,
            pos: Pos { line: 1, column: 1 },
            depth: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.all.get(self.at).copied()
    }

    fn ahead(&self, n: usize) -> Option<char> {
        self.all.get(self.at + n).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += 1;
        if c == '\n' {
            self.pos = Pos {
                line: self.pos.line + 1,
                column: 1,
            };
        } else {
            self.pos.column += 1;
        }
        Some(c)
    }

    /// The character `n` on from here, past lines a backslash joins.
    fn past_joins(&self, n: usize) -> Option<char> {
        let mut at = self.at;
        for _ in 0..=n {
            while self.all.get(at) == Some(&'\\') && self.all.get(at + 1) == Some(&'\n') {
                at += 2;
            }
            at += 1;
        }
        self.all.get(at - 1).copied()
    }

    /// The characters from `from` up to `to` as written.
    fn text(&self, from: usize, to: usize) -> String {
        self.all[from..to].iter().collect()
    }

    /// Takes a blank, a space or a tab, and says whether there was one.
    fn blank(&mut self) -> bool {
        let blank = matches!(self.peek(), Some(' ' | '\t'));
        if blank {
            self.next();
        }
        blank
    }

    /// Takes a backslash and a line's end after it, which join two lines,
    /// and says whether they were there.
    fn joins(&mut self) -> bool {
        let joins = self.peek() == Some('\\') && self.ahead(1) == Some('\n');
        if joins {
            self.next();
            self.next();
        }
        joins
    }
}

/// What is left open at the end of the script, at `at`.
fn unclosed(at: Pos) -> Error {
    Error {
        at,
        code: Code::Unclosed,
    }
}

fn starts_name(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn in_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The tokens of `script`, each with where it starts.
///
/// # Errors
///
/// At a quote, a `${`, a `$(`, a `$((` or a backquote that the script
/// leaves open.
pub fn tokens(script: &str) -> Result<Vec<(Token, Pos)>, Error> {
    let mut chars = Chars::new(script);
    let mut out = Vec::new();
    while let Some(token) = token(&mut chars)? {
        out.push(token);
    }
    Ok(out)
}

/// The next token and where it starts, taken with the blanks and comments
/// before it; none at the end.
fn token(chars: &mut Chars) -> Result<Option<(Token, Pos)>, Error> {
    loop {
        while chars.joins() || chars.blank() {}
        if chars.peek() != Some('#') {
            break;
        }
        while chars.peek().is_some_and(|c| c != '\n') {
            chars.next();
        }
    }
    let at = chars.pos;
    let token = match chars.peek() {
        None => return Ok(None),
        Some('\n') => {
            chars.next();
            Token::Newline
        }
        Some(_) => {
            if let Some(op) = operator(chars) {
                return Ok(Some((Token::Op(op), at)));
            }
            let word = word(chars)?;
            let digits = word
                .plain()
                .filter(|t| t.bytes().all(|b| b.is_ascii_digit()));
            match digits.and_then(|t| t.parse().ok()) {
                Some(n) if matches!(chars.peek(), Some('<' | '>')) => Token::IoNumber(n),
                _ => Token::Word(word),
            }
        }
    };
    Ok(Some((token, at)))
}

/// The operator written `text`.
fn written(text: &str) -> Option<Op> {
    OPERATORS
        .iter()
        .find(|&&(t, _)| t == text)
        .map(|&(_, op)| op)
}

/// The operator that starts here, the longest that fits, taken; lines a
/// backslash joins may cut it, as they may cut anything else.
fn operator(chars: &mut Chars) -> Option<Op> {
    let mut text = String::from(chars.peek()?);
    let mut op = written(&text)?;
    chars.next();
    loop {
        let Some(c) = chars.past_joins(0) else {
            return Some(op);
        };
        text.push(c);
        match written(&text) {
            Some(longer) => {
                op = longer;
                while chars.joins() {}
                chars.next();
            }
            None => return Some(op),
        }
    }
}

/// Puts `c` on the end of the parts, joined to text quoted as it is.
fn push(parts: &mut Vec<Part>, c: char, quoted: bool) {
    match parts.last_mut() {
        Some(Part::Text { text, quoted: was }) if *was == quoted => text.push(c),
        _ => parts.push(Part::Text {
            text: c.into(),
            quoted,
        }),
    }
}

/// A word, up to a blank, a line's end or an operator outside quotes.
fn word(chars: &mut Chars) -> Result<Word, Error> {
    let at = chars.pos;
    let mut parts = Vec::new();
    loop {
        if chars.joins() || special(chars, &mut parts)? {
            continue;
        }
        match chars.peek() {
            None | Some(' ' | '\t' | '\n' | '&' | '|' | ';' | '<' | '>' | '(' | ')') => break,
            Some(c) => {
                chars.next();
                push(&mut parts, c, false);
            }
        }
    }
    Ok(Word { parts, at })
}

/// Takes the escape, quote or expansion that starts here onto the parts,
/// and says whether one did.
fn special(chars: &mut Chars, parts: &mut Vec<Part>) -> Result<bool, Error> {
    match chars.peek() {
        Some('\\') => {
            chars.next();
            match chars.next() {
                Some(c) => push(parts, c, true),
                None => push(parts, '\\', false),
            }
        }
        Some('\'') => {
            let open = chars.pos;
            let before = parts.len();
            chars.next();
            loop {
                match chars.next() {
                    Some('\'') => break,
                    Some(c) => push(parts, c, true),
                    None => return Err(unclosed(open)),
                }
            }
            quote_ended(parts, before);
        }
        Some('"') => double(chars, parts)?,
        Some('$') if chars.past_joins(1) == Some('\'') => dollar_single(chars, parts)?,
        Some('$' | '`') => match dollar(chars)? {
            Some(expansion) => parts.push(Part::Expand {
                expansion,
                quoted: false,
            }),
            None => push(parts, '$', false),
        },
        _ => return Ok(false),
    }
    Ok(true)
}

/// Marks a quote that ended having added nothing to a word that ends in
/// no quoted text, as a quote alone still makes a word: `''` is one.
fn quote_ended(parts: &mut Vec<Part>, before: usize) {
    if parts.len() == before && !matches!(parts.last(), Some(Part::Text { quoted: true, .. })) {
        parts.push(Part::Text {
            text: String::new(),
            quoted: true,
        });
    }
}

/// What stands in dollar-single-quotes, its `$` next, with each escape
/// read as the bytes it stands for and the bytes read as UTF-8, any that
/// make no character as U+FFFD. A zero byte ends the text, and what
/// follows it up to the closing quote is dropped, as other shells do.
fn dollar_single(chars: &mut Chars, parts: &mut Vec<Part>) -> Result<(), Error> {
    let open = chars.pos;
    let before = parts.len();
    chars.next();
    while chars.joins() {}
    chars.next();
    let mut bytes = Vec::new();
    let mut cut = false;
    loop {
        let more = match chars.next() {
            None => return Err(unclosed(open)),
            Some('\'') => break,
            Some('\\') => escape(chars),
            Some(c) => String::from(c).into_bytes(),
        };
        cut |= more.contains(&0);
        if !cut {
            bytes.extend(more);
        }
    }
    for c in String::from_utf8_lossy(&bytes).chars() {
        push(parts, c, true);
    }
    quote_ended(parts, before);
    Ok(())
}

/// The bytes the escape after a `\` in dollar-single-quotes stands for,
/// taken; one POSIX does not give stays as it is written.
fn escape(chars: &mut Chars) -> Vec<u8> {
    let Some(c) = chars.next() else {
        return b"\\".to_vec();
    };
    let byte = match c {
        '"' | '\'' | '\\' => c as u8,
        'a' => 0x07,
        'b' => 0x08,
        'e' => 0x1b,
        'f' => 0x0c,
        'n' => b'\n',
        'r' => b'\r',
        't' => b'\t',
        'v' => 0x0b,
        'c' => match chars.peek() {
            Some('\\') if chars.ahead(1) == Some('\\') => {
                chars.next();
                chars.next();
                0x1c
            }
            Some('?') => {
                chars.next();
                0x7f
            }
            Some(x @ ('@'..='[' | ']'..='_' | 'a'..='z')) => {
                chars.next();
                x as u8 & 0x1f
            }
            _ => return b"\\c".to_vec(),
        },
        'x' => match more_digits(chars, 16, 2, 0) {
            (_, 0) => return b"\\x".to_vec(),
            (n, _) => n.to_le_bytes()[0],
        },
        // Three octal digits reach 511; other shells keep the low byte.
        '0'..='7' => more_digits(chars, 8, 2, c as u32 - '0' as u32)
            .0
            .to_le_bytes()[0],
        c => return format!("\\{c}").into_bytes(),
    };
    alloc::vec![byte]
}

/// `value` with up to `most` more digits in `radix` after it, taken, and
/// how many there were.
fn more_digits(chars: &mut Chars, radix: u32, most: usize, value: u32) -> (u32, usize) {
    let mut value = value;
    for taken in 0..most {
        let Some(d) = chars.peek().and_then(|c| c.to_digit(radix)) else {
            return (value, taken);
        };
        chars.next();
        value = value * radix + d;
    }
    (value, most)
}

/// What stands in double quotes, its `"` next: characters as they are but
/// for a `\` before `$`, a backquote, `"`, `\` or a line's end, and
/// expansions.
fn double(chars: &mut Chars, parts: &mut Vec<Part>) -> Result<(), Error> {
    let open = chars.pos;
    chars.next();
    let before = parts.len();
    loop {
        if chars.joins() {
            continue;
        }
        match chars.peek() {
            None => return Err(unclosed(open)),
            Some('"') => {
                chars.next();
                break;
            }
            Some('\\') if matches!(chars.ahead(1), Some('$' | '`' | '"' | '\\')) => {
                chars.next();
                if let Some(c) = chars.next() {
                    push(parts, c, true);
                }
            }
            Some('$' | '`') => match dollar(chars)? {
                Some(expansion) => parts.push(Part::Expand {
                    expansion,
                    quoted: true,
                }),
                None => push(parts, '$', true),
            },
            Some(c) => {
                chars.next();
                push(parts, c, true);
            }
        }
    }
    quote_ended(parts, before);
    Ok(())
}

/// The expansion a `$` or a backquote starts, its first character next;
/// none for a `$` that starts none, which is taken as a `$`.
fn dollar(chars: &mut Chars) -> Result<Option<Expansion>, Error> {
    let open = chars.pos;
    if chars.next() == Some('`') {
        let mut text = String::new();
        loop {
            match chars.next() {
                None => return Err(unclosed(open)),
                Some('`') => break,
                Some('\\') if matches!(chars.peek(), Some('$' | '`' | '\\')) => {
                    text.extend(chars.next());
                }
                Some(c) => text.push(c),
            }
        }
        return Ok(Some(Expansion::Command(text)));
    }
    while chars.joins() {}
    let expansion = match chars.peek() {
        Some('{' | '(') => {
            if chars.depth == DEEPEST {
                return Err(Error {
                    at: open,
                    code: Code::TooNested,
                });
            }
            chars.depth += 1;
            let expansion = inside(chars, open)?;
            chars.depth -= 1;
            expansion
        }
        Some(c) if starts_name(c) => {
            let mut name = String::new();
            loop {
                while chars.joins() {}
                let Some(c) = chars.peek().filter(|&c| in_name(c)) else {
                    break;
                };
                chars.next();
                name.push(c);
            }
            Expansion::Parameter(name)
        }
        Some(c @ ('@' | '*' | '#' | '?' | '-' | '$' | '!' | '0'..='9')) => {
            chars.next();
            Expansion::Parameter(c.into())
        }
        _ => return Ok(None),
    };
    Ok(Some(expansion))
}

/// The expansion the `${`, `$((` or `$(` at `open` starts, its bracket
/// next.
fn inside(chars: &mut Chars, open: Pos) -> Result<Expansion, Error> {
    if chars.peek() == Some('{') {
        chars.next();
        let from = chars.at;
        bracketed(chars, open, '{', '}')?;
        return Ok(Expansion::Braced(chars.text(from, chars.at - 1)));
    }
    let back = (chars.at, chars.pos, chars.depth);
    let mut arithmetic = Ok(());
    if chars.past_joins(1) == Some('(') {
        chars.next();
        while chars.joins() {}
        chars.next();
        let from = chars.at;
        arithmetic = bracketed(chars, open, '(', ')');
        if arithmetic.is_ok() && chars.past_joins(0) == Some(')') {
            let to = chars.at - 1;
            while chars.joins() {}
            chars.next();
            return Ok(Expansion::Arithmetic(chars.text(from, to)));
        }
        // Not closed by "))", so it was "$(" with a subshell inside, as
        // POSIX says to read it then.
        (chars.at, chars.pos, chars.depth) = back;
    }
    chars.next();
    let from = chars.at;
    if let Err(e) = command(chars, open) {
        return Err(arithmetic.err().unwrap_or(e));
    }
    Ok(Expansion::Command(chars.text(from, chars.at - 1)))
}

/// Takes the commands up to the `)` that closes the `$(` at `at`, read as
/// tokens so that quotes, comments and expansions in them are taken
/// whole, and the `)`.
fn command(chars: &mut Chars, at: Pos) -> Result<(), Error> {
    let mut opens = Vec::new();
    loop {
        match token(chars)? {
            None => return Err(unclosed(opens.last().copied().unwrap_or(at))),
            Some((Token::Op(Op::Open), here)) => opens.push(here),
            Some((Token::Op(Op::Close), _)) if opens.pop().is_none() => return Ok(()),
            Some(_) => {}
        }
    }
}

/// Takes what stands up to the `close` that matches the bracket opened at
/// `at`, with quotes and expansions in it taken whole and other `open`s
/// and `close`s in pairs, and the `close`.
fn bracketed(chars: &mut Chars, at: Pos, open: char, close: char) -> Result<(), Error> {
    let mut opens = Vec::new();
    let mut parts = Vec::new();
    loop {
        if chars.joins() || special(chars, &mut parts)? {
            continue;
        }
        let here = chars.pos;
        match chars.next() {
            None => return Err(unclosed(opens.last().copied().unwrap_or(at))),
            Some(c) if c == open => opens.push(here),
            Some(c) if c == close && opens.pop().is_none() => return Ok(()),
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    /// The tokens of `script` written short: a word as its parts, quoted
    /// text in brackets and expansions as `$kind{text}`, quoted ones with a
    /// `"` before them.
    fn short(script: &str) -> Vec<String> {
        tokens(script)
            .unwrap()
            .into_iter()
            .map(|(token, _)| match token {
                Token::Word(w) => w
                    .parts
                    .iter()
                    .map(|p| match p {
                        Part::Text {
                            text,
                            quoted: false,
                        } => text.clone(),
                        Part::Text { text, quoted: true } => format!("[{text}]"),
                        Part::Expand { expansion, quoted } => {
                            let q = if *quoted { "\"" } else { "" };
                            match expansion {
                                Expansion::Parameter(n) => format!("{q}$p{{{n}}}"),
                                Expansion::Braced(t) => format!("{q}$b{{{t}}}"),
                                Expansion::Command(t) => format!("{q}$c{{{t}}}"),
                                Expansion::Arithmetic(t) => format!("{q}$a{{{t}}}"),
                            }
                        }
                    })
                    .collect(),
                Token::IoNumber(n) => format!("io{n}"),
                Token::Op(op) => OPERATORS
                    .iter()
                    .find(|(_, o)| *o == op)
                    .map(|(t, _)| String::from(*t))
                    .unwrap(),
                Token::Newline => "NL".into(),
            })
            .collect()
    }

    fn error(script: &str) -> (usize, usize, Code) {
        let e = tokens(script).unwrap_err();
        (e.at.line, e.at.column, e.code)
    }

    #[test]
    fn operators_are_taken_by_the_longest_that_fits() {
        assert_eq!(
            short("a&&b||c;;d;&e<<-f<<g<&h<>i>>j>&k>|l&m|n;o<p>q(r)"),
            [
                "a", "&&", "b", "||", "c", ";;", "d", ";&", "e", "<<-", "f", "<<", "g", "<&", "h",
                "<>", "i", ">>", "j", ">&", "k", ">|", "l", "&", "m", "|", "n", ";", "o", "<", "p",
                ">", "q", "(", "r", ")"
            ]
        );
        assert_eq!(short("a <<<b"), ["a", "<<", "<", "b"]);
        assert_eq!(short("a >>> b"), ["a", ">>", ">", "b"]);
    }

    #[test]
    fn words_end_at_blanks_and_lines_and_comments_start_words() {
        assert_eq!(short("  echo\ta  b\n c"), ["echo", "a", "b", "NL", "c"]);
        assert_eq!(short("a # b c\nd"), ["a", "NL", "d"]);
        assert_eq!(short("a#b"), ["a#b"], "a # inside a word is a #");
        assert_eq!(short("a \\\nb"), ["a", "b"], "a backslash joins lines");
        assert_eq!(short("ab\\\ncd"), ["abcd"]);
        assert_eq!(short(""), Vec::<String>::new());
        assert_eq!(short("a \\"), ["a", "\\"], "a backslash that ends it");
        assert_eq!(short(" \\\n\t\\\n"), Vec::<String>::new());
    }

    #[test]
    fn quotes_keep_what_they_hold_as_it_is() {
        assert_eq!(short("'a b' \"c d\""), ["[a b]", "[c d]"]);
        assert_eq!(short("'$x \\ `'"), ["[$x \\ `]"]);
        assert_eq!(short("a'b'c"), ["a[b]c"]);
        assert_eq!(
            short("\\a\\ b"),
            ["[a ]b"],
            "a backslash quotes one character"
        );
        assert_eq!(short("'' \"\""), ["[]", "[]"], "an empty quote is a word");
        assert_eq!(short("'a\nb'"), ["[a\nb]"], "over lines");
        assert_eq!(short("\"a\\\"b\\\\c\\d\\$e\""), ["[a\"b\\c\\d$e]"]);
        assert_eq!(short("\"a\\\nb\""), ["[ab]"]);
    }

    #[test]
    fn a_dollar_brings_what_it_starts_into_its_word() {
        assert_eq!(
            short("$x ${y:-z} $(ls -l) $((1+2)) `pwd` $1 $? $@ $#"),
            [
                "$p{x}",
                "$b{y:-z}",
                "$c{ls -l}",
                "$a{1+2}",
                "$c{pwd}",
                "$p{1}",
                "$p{?}",
                "$p{@}",
                "$p{#}"
            ]
        );
        assert_eq!(short("a$x.b"), ["a$p{x}.b"]);
        assert_eq!(short("$12"), ["$p{1}2"], "one digit");
        assert_eq!(short("\"$x and ${y}\""), ["\"$p{x}[ and ]\"$b{y}"]);
        assert_eq!(short("a$ $"), ["a$", "$"], "a $ that starts nothing");
        assert_eq!(short("\"$\""), ["[$]"]);
        assert_eq!(short("$(echo $(pwd))"), ["$c{echo $(pwd)}"], "nested");
        assert_eq!(short("$(echo ')')"), ["$c{echo ')'}"], "quotes inside");
        assert_eq!(short("${x:-'}'}"), ["$b{x:-'}'}"]);
        assert_eq!(short("`echo \\`a\\``"), ["$c{echo `a`}"]);
        assert_eq!(short("$((2*(3+4)))"), ["$a{2*(3+4)}"]);
        assert_eq!(short("$((a) )"), ["$c{(a) }"], "a subshell, not arithmetic");
        assert_eq!(short("$((a);(b))"), ["$c{(a);(b)}"]);
        assert_eq!(short("$(echo \\))"), ["$c{echo \\)}"], "an escaped )");
        assert_eq!(short("$(echo \"\\\")\")"), ["$c{echo \"\\\")\"}"]);
        assert_eq!(short("`echo \\a`"), ["$c{echo \\a}"], "kept before others");
    }

    #[test]
    fn digits_right_before_a_redirection_name_its_stream() {
        assert_eq!(
            short("a 2>e 10<f"),
            ["a", "io2", ">", "e", "io10", "<", "f"]
        );
        assert_eq!(
            short("a 2 >e"),
            ["a", "2", ">", "e"],
            "apart they are a word"
        );
        assert_eq!(short("a x2>e"), ["a", "x2", ">", "e"]);
        assert_eq!(short("a '2'>e"), ["a", "[2]", ">", "e"]);
        assert_eq!(short("echo 2"), ["echo", "2"]);
    }

    #[test]
    fn what_is_left_open_at_the_end_is_an_error_where_it_opened() {
        assert_eq!(error("a 'b"), (1, 3, Code::Unclosed));
        assert_eq!(error("a \"b\nc"), (1, 3, Code::Unclosed));
        assert_eq!(error("x ${a"), (1, 3, Code::Unclosed));
        assert_eq!(error("x $(a"), (1, 3, Code::Unclosed));
        assert_eq!(error("x $((a)"), (1, 3, Code::Unclosed));
        assert_eq!(error("x `a"), (1, 3, Code::Unclosed));
        assert_eq!(error("\"`a\""), (1, 2, Code::Unclosed));
    }

    #[test]
    fn each_token_knows_where_it_starts() {
        assert_eq!(
            places("echo a\n  b>c"),
            [(1, 1), (1, 6), (1, 7), (2, 3), (2, 4), (2, 5)]
        );
        assert_eq!(places("é ä\nb"), [(1, 1), (1, 3), (1, 4), (2, 1)]);
        assert_eq!(
            places("'a\nb' c"),
            [(1, 1), (2, 4)],
            "after lines in quotes"
        );
        assert_eq!(places("a \\\n  b"), [(1, 1), (2, 3)], "after joined lines");
        assert_eq!(
            places("2>e a&&b"),
            [(1, 1), (1, 2), (1, 3), (1, 5), (1, 6), (1, 8)]
        );
        for (token, at) in tokens("x 'y' $z").unwrap() {
            if let Token::Word(word) = token {
                assert_eq!(word.at, at, "a word is where its first character is");
            }
        }
    }

    /// Where each token of `script` starts.
    fn places(script: &str) -> Vec<(usize, usize)> {
        tokens(script)
            .unwrap()
            .into_iter()
            .map(|(_, p)| (p.line, p.column))
            .collect()
    }

    /// The one word `script` is.
    fn only_word(script: &str) -> Word {
        match tokens(script).unwrap().as_slice() {
            [(Token::Word(word), _)] => word.clone(),
            other => panic!("{script:?} is {other:?}"),
        }
    }

    /// The tokens of each line of `script`, as [`short`] writes them, with
    /// a space between each two.
    fn lines(script: &str) -> Vec<String> {
        short(script)
            .split(|t| t == "NL")
            .map(|line| line.join(" "))
            .collect()
    }

    #[test]
    fn lines_a_backslash_joins_may_cut_anything() {
        assert_eq!(short("a &\\\n& b"), ["a", "&&", "b"]);
        assert_eq!(short("a >\\\n> b"), ["a", ">>", "b"]);
        assert_eq!(short("a <\\\n<\\\n- b"), ["a", "<<-", "b"]);
        assert_eq!(short("2\\\n>e"), ["io2", ">", "e"]);
        assert_eq!(short("$\\\nx"), ["$p{x}"]);
        assert_eq!(short("$ab\\\ncd"), ["$p{abcd}"]);
        assert_eq!(short("$\\\n{x}"), ["$b{x}"]);
        assert_eq!(short("a\\\n"), ["a"]);
        assert_eq!(short("&\\\n"), ["&"]);
        assert_eq!(short("&\\\nx"), ["&", "x"]);
        assert_eq!(short("'a\\\nb'"), ["[a\\\nb]"], "but not in single quotes");
        assert_eq!(short("a\\\\\nb"), ["a[\\]", "NL", "b"], "nor after a \\\\");
    }

    #[test]
    fn every_special_parameter_is_one_character_and_names_are_longer() {
        assert_eq!(
            short("$@ $* $# $? $- $$ $! $0 $9"),
            [
                "$p{@}", "$p{*}", "$p{#}", "$p{?}", "$p{-}", "$p{$}", "$p{!}", "$p{0}", "$p{9}"
            ]
        );
        assert_eq!(short("$$$"), ["$p{$}$"]);
        assert_eq!(
            short("$a_b1 $_x $X.y $x-y $1a"),
            ["$p{a_b1}", "$p{_x}", "$p{X}.y", "$p{x}-y", "$p{1}a"]
        );
        assert_eq!(short("$% $/ $= $é"), ["$%", "$/", "$=", "$é"], "no names");
    }

    #[test]
    fn expansions_in_double_quotes_are_quoted_and_others_are_not() {
        assert_eq!(
            short("\"$(a)${b}$((1))`c`$d\""),
            ["\"$c{a}\"$b{b}\"$a{1}\"$c{c}\"$p{d}"]
        );
        assert_eq!(short("$(a)\"$(a)\""), ["$c{a}\"$c{a}"]);
        assert_eq!(short("\"a$\" \"$\"x"), ["[a$]", "[$]x"]);
        assert_eq!(
            only_word("a'b'$c\"d\"").parts,
            [
                Part::Text {
                    text: "a".into(),
                    quoted: false
                },
                Part::Text {
                    text: "b".into(),
                    quoted: true
                },
                Part::Expand {
                    expansion: Expansion::Parameter("c".into()),
                    quoted: false
                },
                Part::Text {
                    text: "d".into(),
                    quoted: true
                },
            ]
        );
        assert_eq!(
            short("\"a\"'b'"),
            ["[ab]"],
            "quoted text joins whichever quote"
        );
    }

    #[test]
    fn double_quotes_keep_a_backslash_before_what_they_do_not_treat() {
        assert_eq!(short("\"\\a\\'\\n\""), ["[\\a\\'\\n]"]);
        assert_eq!(short("\"a'b\" 'a\"b'"), ["[a'b]", "[a\"b]"]);
        assert_eq!(short("\"`a`\""), ["\"$c{a}"]);
        assert_eq!(short("\"\\`a\\`\""), ["[`a`]"]);
    }

    #[test]
    fn a_plain_word_is_unquoted_text_alone() {
        assert_eq!(only_word("abc").plain(), Some("abc"));
        assert_eq!(only_word("a=b").plain(), Some("a=b"));
        for script in ["'abc'", "a$b", "\"\"", "\\a", "$a", "a\"b\""] {
            assert_eq!(only_word(script).plain(), None, "{script}");
        }
    }

    #[test]
    fn operators_inside_expansions_and_quotes_do_not_end_words() {
        assert_eq!(short("$(a|b;c&d) x"), ["$c{a|b;c&d}", "x"]);
        assert_eq!(short("${a:-b c}"), ["$b{a:-b c}"]);
        assert_eq!(short("'a|b' \"c;d\""), ["[a|b]", "[c;d]"]);
        assert_eq!(short("a\\|b"), ["a[|]b"]);
        assert_eq!(short("'a'|b"), ["[a]", "|", "b"]);
        assert_eq!(short("a>'b c'"), ["a", ">", "[b c]"]);
        assert_eq!(short("${a:-${b}} ${a:-{}}"), ["$b{a:-${b}}", "$b{a:-{}}"]);
        assert_eq!(short("$(a\nb)"), ["$c{a\nb}"], "no line's end inside");
        assert_eq!(short("`a $(b`"), ["$c{a $(b}"], "read when it runs");
    }

    #[test]
    fn arithmetic_needs_both_closing_parentheses_together() {
        assert_eq!(short("$(( (1) ))"), ["$a{ (1) }"]);
        assert_eq!(short("$((a)b)"), ["$c{(a)b}"]);
        assert_eq!(short("$((1)\n)"), ["$c{(1)\n}"]);
        assert_eq!(short("$(($(echo 1)+1))"), ["$a{$(echo 1)+1}"]);
    }

    #[test]
    fn comments_run_to_the_end_of_their_line() {
        assert_eq!(short("# all\necho"), ["NL", "echo"]);
        assert_eq!(short("a;#b\nc"), ["a", ";", "NL", "c"]);
        assert_eq!(short("a|#b"), ["a", "|"]);
        assert_eq!(short("echo '#' \\#x \"#\""), ["echo", "[#]", "[#]x", "[#]"]);
        assert_eq!(short("a #"), ["a"]);
        assert_eq!(short("a # 'b $(c"), ["a"], "nothing opens in a comment");
        assert_eq!(short("# a \\\nb"), ["NL", "b"], "nor goes on");
    }

    #[test]
    fn digits_before_every_redirection_name_its_stream() {
        assert_eq!(
            lines("2>>a 0<&3 1>&2 2<>f 2>|f 2<<E 3<<-E"),
            ["io2 >> a io0 <& 3 io1 >& 2 io2 <> f io2 >| f io2 << E io3 <<- E"]
        );
        assert_eq!(short("12345>x"), ["io12345", ">", "x"]);
        assert_eq!(
            short("99999999999>x"),
            ["99999999999", ">", "x"],
            "too many"
        );
        assert_eq!(short("2|x 2&"), ["2", "|", "x", "2", "&"]);
        assert_eq!(short("a 2\\>e"), ["a", "2[>]e"]);
        assert_eq!(short("\"2\">e"), ["[2]", ">", "e"]);
    }

    #[test]
    fn more_left_open_is_an_error_where_it_opened() {
        assert_eq!(error("echo \"$(a\""), (1, 10, Code::Unclosed));
        assert_eq!(error("a\n  'b"), (2, 3, Code::Unclosed));
        assert_eq!(error("a \\\n 'b"), (2, 2, Code::Unclosed));
        assert_eq!(error("$(echo ')"), (1, 8, Code::Unclosed));
        assert_eq!(error("${a\"}"), (1, 4, Code::Unclosed));
        assert_eq!(error("'ok' \"also\" 'not"), (1, 13, Code::Unclosed));
        assert_eq!(error("x\n\n\""), (3, 1, Code::Unclosed));
        assert_eq!(error("é '"), (1, 3, Code::Unclosed));
        assert_eq!(error("$((a)"), (1, 1, Code::Unclosed));
    }

    #[test]
    fn blanks_and_lines_end_words() {
        assert_eq!(short("a\t\tb"), ["a", "b"]);
        assert_eq!(short("a\n\nb\n"), ["a", "NL", "NL", "b", "NL"]);
        assert_eq!(short("a;\nb"), ["a", ";", "NL", "b"]);
        assert_eq!(
            short("a\r\nb"),
            ["a\r", "NL", "b"],
            "a carriage return is a character"
        );
        assert_eq!(short(&"ab ".repeat(1000)).len(), 1000);
    }

    #[test]
    fn dollar_single_quotes_read_their_escapes() {
        assert_eq!(short("$'a\\tb\\n'"), ["[a\tb\n]"]);
        assert_eq!(
            short("$'\\a\\b\\e\\f\\r\\v'"),
            ["[\u{7}\u{8}\u{1b}\u{c}\r\u{b}]"]
        );
        assert_eq!(
            short("$'\\'\\\"\\\\'"),
            ["['\"\\]"],
            "a quote that does not end it"
        );
        assert_eq!(
            short("$'\\x41\\x4a2\\x7e'"),
            ["[AJ2~]"],
            "two hex digits at most"
        );
        assert_eq!(
            short("$'\\101\\1012\\7'"),
            ["[AA2\u{7}]"],
            "three octal at most"
        );
        assert_eq!(short("$'\\xc3\\xa4'"), ["[ä]"], "bytes, read as UTF-8");
        assert_eq!(short("$'\\xff' $'\\777'"), ["[\u{fffd}]", "[\u{fffd}]"]);
        assert_eq!(
            short("$'\\cA\\ca\\c?\\c\\\\\\c['"),
            ["[\u{1}\u{1}\u{7f}\u{1c}\u{1b}]"]
        );
        assert_eq!(
            short("$'\\q\\x\\c1'"),
            ["[\\q\\x\\c1]"],
            "others as written"
        );
        assert_eq!(short("$'a\\0b\\'c' d"), ["[a]", "d"], "a zero byte ends it");
        assert_eq!(short("$'' a$'b'c"), ["[]", "a[b]c"]);
        assert_eq!(short("\"$'a'\""), ["[$'a']"], "nothing in double quotes");
        assert_eq!(short("$(echo $'a\\')')"), ["$c{echo $'a\\')'}"]);
        assert_eq!(short("$(echo 'a\\')"), ["$c{echo 'a\\'}"]);
        assert_eq!(error("x $'a\\'"), (1, 3, Code::Unclosed));
        assert_eq!(error("$'\\"), (1, 1, Code::Unclosed));
    }

    /// The character at `at` in `script`.
    fn at(script: &str, at: Pos) -> Option<char> {
        script
            .split_inclusive('\n')
            .nth(at.line - 1)?
            .chars()
            .nth(at.column - 1)
    }

    #[test]
    fn every_short_script_reads_or_is_refused_where_something_opens() {
        let alphabet = [
            'a', '1', ' ', '\n', '\\', '\'', '"', '`', '$', '(', ')', '{', '}', '#', '&', '|', ';',
            '<', '>', '-',
        ];
        let n = alphabet.len();
        let mut script = String::new();
        for len in 0..=4u32 {
            for mut i in 0..n.pow(len) {
                script.clear();
                for _ in 0..len {
                    script.push(alphabet[i % n]);
                    i /= n;
                }
                match tokens(&script) {
                    Ok(read) => {
                        let places: Vec<Pos> = read.iter().map(|&(_, p)| p).collect();
                        assert!(places.windows(2).all(|w| w[0] < w[1]), "{script:?}");
                        for (token, p) in &read {
                            assert!(at(&script, *p).is_some(), "{script:?} at {p:?}");
                            let Token::Word(word) = token else { continue };
                            if let Some(text) = word.plain().filter(|_| !script.contains('\\')) {
                                let line = script.split('\n').nth(p.line - 1).unwrap();
                                let rest: String = line.chars().skip(p.column - 1).collect();
                                assert!(rest.starts_with(text), "{script:?}: {text:?}");
                            }
                        }
                    }
                    Err(e) => {
                        assert_eq!(e.code, Code::Unclosed, "{script:?}");
                        let opens = at(&script, e.at);
                        assert!(
                            matches!(opens, Some('\'' | '"' | '`' | '$' | '(' | '{')),
                            "{script:?} at {:?}",
                            e.at
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn each_operator_reads_alone_and_nothing_longer_does() {
        for (text, op) in OPERATORS {
            assert_eq!(
                tokens(text).unwrap(),
                [(Token::Op(op), Pos { line: 1, column: 1 })]
            );
            assert_eq!(written(text), Some(op));
        }
        for text in ["&&&", "|&", "<<<", ">>>", "&;", "((", "))", ">-", ";|", ""] {
            assert_eq!(written(text), None, "{text}");
        }
        assert_eq!(short("a <<- b a << -b"), ["a", "<<-", "b", "a", "<<", "-b"]);
        assert_eq!(short("((a))"), ["(", "(", "a", ")", ")"]);
    }

    #[test]
    fn reserved_words_braces_and_patterns_are_words_here() {
        assert_eq!(
            short("if { a; } ! b then fi"),
            ["if", "{", "a", ";", "}", "!", "b", "then", "fi"]
        );
        assert_eq!(short("{a,b} a{b}c"), ["{a,b}", "a{b}c"]);
        assert_eq!(short("a=b c=$(d) =e"), ["a=b", "c=$c{d}", "=e"]);
        assert_eq!(short("~ ~a/b x~"), ["~", "~a/b", "x~"]);
        assert_eq!(short("*.txt [ab]? a-z"), ["*.txt", "[ab]?", "a-z"]);
        assert_eq!(short("a!b a=b=c"), ["a!b", "a=b=c"]);
    }

    #[test]
    fn places_count_characters_as_written() {
        assert_eq!(places("$'\\x41' b"), [(1, 1), (1, 9)], "not as read");
        assert_eq!(places("$(a\nb) c"), [(1, 1), (2, 4)]);
        assert_eq!(places("\ta\t2>b"), [(1, 2), (1, 4), (1, 5), (1, 6)]);
        assert_eq!(places("# x\n|"), [(1, 4), (2, 1)]);
        assert_eq!(places("$'\n' x"), [(1, 1), (2, 3)]);
        assert_eq!(error("\"${a"), (1, 2, Code::Unclosed));
        assert_eq!(error("\"$(a"), (1, 2, Code::Unclosed));
        assert_eq!(error("$(a \"b)"), (1, 5, Code::Unclosed));
        assert_eq!(error("$(( \"a ))"), (1, 5, Code::Unclosed));
        assert_eq!(error("a\n$'b\\'\n"), (2, 1, Code::Unclosed));
    }

    #[test]
    fn a_quote_that_adds_nothing_still_makes_a_word() {
        let text = |text: &str, quoted| Part::Text {
            text: text.into(),
            quoted,
        };
        let x = |quoted| Part::Expand {
            expansion: Expansion::Parameter("x".into()),
            quoted,
        };
        assert_eq!(only_word("''").parts, [text("", true)]);
        assert_eq!(only_word("\"\"").parts, [text("", true)]);
        assert_eq!(only_word("$''").parts, [text("", true)]);
        assert_eq!(only_word("$x''").parts, [x(false), text("", true)]);
        assert_eq!(only_word("x\"\"").parts, [text("x", false), text("", true)]);
        assert_eq!(only_word("'a'\"b\"").parts, [text("ab", true)]);
        assert_eq!(only_word("'a'''").parts, [text("a", true)]);
        assert_eq!(only_word("\"$x\"").parts, [x(true)]);
    }

    #[test]
    fn what_an_expansion_holds_is_read_as_the_shell_reads_it() {
        assert_eq!(
            short("$(echo \"$(echo \")\")\")"),
            ["$c{echo \"$(echo \")\")\"}"]
        );
        assert_eq!(short("${a:-$(echo })}"), ["$b{a:-$(echo })}"]);
        assert_eq!(short("$(echo ${a:-)})"), ["$c{echo ${a:-)}}"]);
        assert_eq!(short("$(echo # )\n)"), ["$c{echo # )\n}"]);
        assert_eq!(short("$(echo a#)"), ["$c{echo a#}"]);
        assert_eq!(
            short("$(( $((1)) + `echo ))` ))"),
            ["$a{ $((1)) + `echo ))` }"]
        );
        assert_eq!(short("${a:-\"}\"}"), ["$b{a:-\"}\"}"]);
        assert_eq!(short("$(echo \\$'a\\')"), ["$c{echo \\$'a\\'}"]);
    }

    #[test]
    fn an_unclosed_error_is_at_the_innermost_opening() {
        for (script, at) in [
            ("$(echo \"a", (1, 8)),
            ("$(echo 'a", (1, 8)),
            ("$(echo `a", (1, 8)),
            ("$(echo $'a", (1, 8)),
            ("${x:-'a", (1, 6)),
            ("${x:-\"a", (1, 6)),
            ("${x:-$(a", (1, 6)),
            ("$(a ${b", (1, 5)),
            ("$((1 + \"", (1, 8)),
            ("\"${a", (1, 2)),
            ("\"$(a", (1, 2)),
            ("\"`a", (1, 2)),
            ("\"$((a", (1, 2)),
            ("$((a", (1, 1)),
            ("a$(b $(c", (1, 6)),
            ("$(a (b", (1, 5)),
            ("${a {b", (1, 5)),
            ("$(a) $(b", (1, 6)),
            ("'a' \"b", (1, 5)),
            ("$(a\n  'b", (2, 3)),
        ] {
            assert_eq!(error(script), (at.0, at.1, Code::Unclosed), "{script:?}");
        }
    }

    /// `x` inside `n` of `open` and `close`.
    fn nest(open: &str, close: &str, n: usize) -> String {
        format!("{}x{}", open.repeat(n), close.repeat(n))
    }

    #[test]
    fn expansions_stand_inside_each_other_sixteen_deep() {
        assert_eq!(
            short(&nest("$(", ")", 16)),
            [format!("$c{{{}}}", nest("$(", ")", 15))]
        );
        assert!(tokens(&nest("${a:-", "}", 16)).is_ok());
        assert!(tokens(&nest("$((", "))", 16)).is_ok());
        assert!(tokens(&nest("\"$(", ")\"", 16)).is_ok());
        assert_eq!(error(&nest("$(", ")", 17)), (1, 33, Code::TooNested));
        assert_eq!(error(&nest("${a:-", "}", 17)), (1, 81, Code::TooNested));
        assert_eq!(error(&nest("$((", "))", 17)), (1, 49, Code::TooNested));
        assert_eq!(error(&nest("\"$(", ")\"", 17)), (1, 50, Code::TooNested));
        assert!(
            tokens(&"$(a)".repeat(100)).is_ok(),
            "side by side, not inside"
        );
        assert!(tokens(&nest("$(a) $(", ")", 16)).is_ok());
    }

    #[test]
    fn the_specification_and_the_help_give_the_same_depth() {
        let spec = include_str!("../../../../docs/formats/shell.md");
        assert!(
            spec.contains(&format!("inside each other {DEEPEST} deep")),
            "the specification says {DEEPEST}"
        );
        assert!(spec.contains("(`too-nested`)"));
        let help = Code::TooNested.help();
        assert!(help.contains(&format!("past {DEEPEST} ")), "{help}");
    }

    #[test]
    fn arithmetic_that_turns_out_a_command_gives_back_the_depth_it_took() {
        // As arithmetic the # is a character and the ${ is left open; as a
        // command the # starts a comment, so it reads.
        let turns = "$(( #${\n) )";
        assert_eq!(short(turns), ["$c{( #${\n) }"]);
        assert!(tokens(&format!("{turns} {}", nest("$(", ")", 16))).is_ok());
        assert!(tokens(&format!("{turns}{turns}{}", nest("$(", ")", 16))).is_ok());
        let deep = format!("{}{turns}{}", "$(".repeat(15), ")".repeat(15));
        assert!(tokens(&deep).is_ok());
    }

    #[test]
    fn a_script_reads_as_sh_reads_it() {
        let script = r#"#!/bin/sh
set -e
for f in *.txt; do
  case "$f" in
    a*) echo "starts with a: $f" ;;
    *) cat "$f" >> all.txt 2>/dev/null || true ;;
  esac
done
if [ -n "${HOME:-}" ]; then cd "$HOME"; fi
x=$(( $(wc -l < all.txt) + 1 ))
"#;
        assert_eq!(
            lines(script),
            [
                "",
                "set -e",
                "for f in *.txt ; do",
                "case \"$p{f} in",
                "a* ) echo [starts with a: ]\"$p{f} ;;",
                "* ) cat \"$p{f} >> all.txt io2 > /dev/null || true ;;",
                "esac",
                "done",
                "if [ -n \"$b{HOME:-} ] ; then cd \"$p{HOME} ; fi",
                "x=$a{ $(wc -l < all.txt) + 1 }",
                "",
            ]
        );
    }

    #[test]
    fn functions_loops_and_subshells_read_as_sh_reads_them() {
        let script = r#"usage() {
	echo "usage: ${0##*/} [-v] file..." >&2
	exit 2
}
while getopts v opt; do
	case $opt in
	v) verbose=1 ;;
	?) usage ;;
	esac
done
shift $((OPTIND - 1))
[ $# -gt 0 ] || usage
for f; do
	(cd "${f%/*}" && ls -l "${f##*/}") &
done
echo "${#}" files, last was "${f:-none}"
"#;
        assert_eq!(
            lines(script),
            [
                "usage ( ) {",
                "echo [usage: ]\"$b{0##*/}[ [-v] file...] >& 2",
                "exit 2",
                "}",
                "while getopts v opt ; do",
                "case $p{opt} in",
                "v ) verbose=1 ;;",
                "? ) usage ;;",
                "esac",
                "done",
                "shift $a{OPTIND - 1}",
                "[ $p{#} -gt 0 ] || usage",
                "for f ; do",
                "( cd \"$b{f%/*} && ls -l \"$b{f##*/} ) &",
                "done",
                "echo \"$b{#} files, last was \"$b{f:-none}",
                "",
            ]
        );
    }

    #[test]
    fn substitutions_and_streams_read_as_sh_reads_them() {
        let script = r#"IFS=$'\n\t'
today=`date +%F` count=$(ls | wc -l)
exec 3<>/dev/tty 4>&-
printf '%s\n' "$(basename "$(pwd)")" >|log"#;
        assert_eq!(
            lines(script),
            [
                "IFS=[\n\t]",
                "today=$c{date +%F} count=$c{ls | wc -l}",
                "exec io3 <> /dev/tty io4 >& -",
                "printf [%s\\n] \"$c{basename \"$(pwd)\"} >| log",
            ]
        );
    }

    /// The tokens of `script` as [`short`] writes them, but for the lines a
    /// backslash joins inside expansions, which are joined when they run.
    fn joined(script: &str) -> Vec<String> {
        short(script)
            .into_iter()
            .map(|t| t.replace("\\\n", ""))
            .collect()
    }

    #[test]
    fn a_backslash_and_a_line_end_between_any_two_characters_change_nothing() {
        let scripts = [
            "echo a&&b||c;d",
            "x=${a:-b}$(c)$((1+2))$$ $? $ab",
            "a 2>>f <g 10<&3",
            "\"a $b `c` d\" $'e'",
            "case a in (b) c;; esac <<-x",
            "f(){ g; }",
        ];
        for script in scripts {
            let want = short(script);
            for (i, _) in script.char_indices().skip(1) {
                let cut = format!("{}\\\n{}", &script[..i], &script[i..]);
                assert_eq!(joined(&cut), want, "{cut:?}");
            }
        }
    }

    #[test]
    fn a_blank_or_a_line_end_between_two_pieces_keeps_their_tokens_apart() {
        let pieces = [
            "a", "'b c'", "\"$x\"", "$(d e)", "${f:-g}", "$((1+2))", "|", "&&", ";", "2>", "<<-",
            "`h`", "$'\\t'", "((", "x=y",
        ];
        for a in pieces {
            for b in pieces {
                for between in [" ", "\t", "\n"] {
                    let script = format!("{a}{between}{b}");
                    let mut want = short(a);
                    if between == "\n" {
                        want.push("NL".into());
                    }
                    let split = want.len();
                    want.extend(short(b));
                    assert_eq!(short(&script), want, "{script:?}");
                    let shift = |(line, column): (usize, usize)| {
                        if between == "\n" {
                            (line + 1, column)
                        } else {
                            (line, column + a.chars().count() + 1)
                        }
                    };
                    let moved: Vec<(usize, usize)> = places(b).into_iter().map(shift).collect();
                    assert_eq!(places(&script)[split..], moved[..], "{script:?}");
                }
            }
        }
    }

    #[test]
    fn dollar_single_quote_escapes_stop_where_their_digits_do() {
        assert_eq!(short("$'\\1234 \\x414 \\0101'"), ["[S4 A4 \u{8}1]"]);
        assert_eq!(
            short("$'\\c' $'\\x' $'\\xg' $'\\8\\9'"),
            ["[\\c]", "[\\x]", "[\\xg]", "[\\8\\9]"]
        );
        assert_eq!(short("$'\\c\\''"), ["[\\c']"], "the \\' is its own escape");
        assert_eq!(
            short("$'\\cz\\cZ\\c_\\c@x'"),
            ["[\u{1a}\u{1a}\u{1f}]"],
            "\\c@ is a zero byte"
        );
    }

    #[test]
    fn traps_reads_and_groups_read_as_sh_reads_them() {
        let script = r#"tmp=$(mktemp) || exit 1
trap 'rm -f "$tmp"' EXIT
: "${OUT:=out.txt}"
while IFS= read -r line; do
	n=$((n + ${#line}))
	case $line in
	a*|b*) echo short ;&
	c*) echo also ;;
	esac
done < "$1"
{ echo "$n"; date; } >"$OUT" 2>&1
! grep -q x "$OUT" && sleep 1 & wait $!
"#;
        assert_eq!(
            lines(script),
            [
                "tmp=$c{mktemp} || exit 1",
                "trap [rm -f \"$tmp\"] EXIT",
                ": \"$b{OUT:=out.txt}",
                "while IFS= read -r line ; do",
                "n=$a{n + ${#line}}",
                "case $p{line} in",
                "a* | b* ) echo short ;&",
                "c* ) echo also ;;",
                "esac",
                "done < \"$p{1}",
                "{ echo \"$p{n} ; date ; } > \"$p{OUT} io2 >& 1",
                "! grep -q x \"$p{OUT} && sleep 1 & wait $p{!}",
                "",
            ]
        );
    }

    #[test]
    fn characters_count_lines_and_columns_as_they_are_taken() {
        let mut chars = Chars::new("a\\\nb \tc\n\\d");
        assert!(!chars.joins() && !chars.blank(), "neither at a");
        assert_eq!(chars.next(), Some('a'));
        assert_eq!(chars.past_joins(0), Some('b'));
        assert_eq!(chars.past_joins(1), Some(' '));
        assert!(chars.joins());
        assert!(!chars.joins(), "once");
        assert_eq!(chars.pos, Pos { line: 2, column: 1 });
        assert_eq!(chars.next(), Some('b'));
        assert!(chars.blank() && chars.blank() && !chars.blank());
        assert_eq!(chars.pos, Pos { line: 2, column: 4 });
        assert_eq!(chars.next(), Some('c'));
        assert_eq!(chars.next(), Some('\n'));
        assert_eq!(chars.pos, Pos { line: 3, column: 1 });
        assert!(!chars.joins(), "a backslash before no line's end");
        assert_eq!(chars.past_joins(1), Some('d'));
        assert_eq!(chars.past_joins(2), None);
        assert_eq!(
            (chars.next(), chars.next(), chars.next()),
            (Some('\\'), Some('d'), None)
        );
        assert_eq!(chars.pos, Pos { line: 3, column: 3 });
    }
}
