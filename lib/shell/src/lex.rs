//! A script read as words and operators, sections 1 to 3 of the
//! specification: quotes, escapes, variables and comments resolved into
//! the parts of each word, positions kept for the errors that follow.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{Code, Error, Pos};

/// A piece of a word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// Characters as they are, and whether they stood unquoted and
    /// unescaped, where `*` and `?` match names of files.
    Text { text: String, plain: bool },
    /// A variable's value: `$name` or `${name}`, or `$?`, `$#` and `$1`
    /// on, named `?`, `#` and `1`.
    Variable(String),
}

/// A word of a script and where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<Part>,
    pub at: Pos,
}

impl Word {
    /// Its text when it is all plain characters, as keywords and the
    /// names `let`, `for` and `fn` make are written.
    #[must_use]
    pub fn plain(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [Part::Text { text, plain: true }] => Some(text),
            _ => None,
        }
    }
}

/// A word or an operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Word(Word),
    /// `|`
    Pipe,
    /// `&&`, as `and`.
    And,
    /// `||`, as `or`.
    Or,
    /// `;`
    Semicolon,
    /// A line's end.
    Newline,
    /// `>`
    Out,
    /// `>>`
    Append,
    /// `<`
    In,
    /// `{`
    Open,
    /// `}`
    Close,
}

/// The script's characters with where each one is.
struct Chars<'a> {
    rest: core::iter::Peekable<core::str::Chars<'a>>,
    at: Pos,
}

impl Chars<'_> {
    fn peek(&mut self) -> Option<char> {
        self.rest.peek().copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.rest.next()?;
        if c == '\n' {
            self.at = Pos {
                line: self.at.line + 1,
                column: 1,
            };
        } else {
            self.at.column += 1;
        }
        Some(c)
    }

    /// Takes a line's end at a carriage return, which has to be one with
    /// a line feed after it.
    fn carriage_return(&mut self) -> Result<(), Error> {
        let at = self.at;
        self.next();
        if self.peek() == Some('\n') {
            Ok(())
        } else {
            Err(Error {
                at,
                code: Code::LoneCarriageReturn,
            })
        }
    }
}

/// What ends a word outside quotes.
fn ends_word(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\r' | '|' | ';' | '>' | '<' | '{' | '}'
    )
}

fn starts_name(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn in_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The words and operators of `script`, each with where it starts.
///
/// # Errors
///
/// At a carriage return without a line feed, a quote not closed on its
/// line, and a `${` without a name and a `}`.
pub fn tokens(script: &str) -> Result<Vec<(Token, Pos)>, Error> {
    let mut chars = Chars {
        rest: script.chars().peekable(),
        at: Pos { line: 1, column: 1 },
    };
    let mut out = Vec::new();
    while let Some(c) = chars.peek() {
        let at = chars.at;
        let token = match c {
            ' ' | '\t' => {
                chars.next();
                continue;
            }
            '\r' => {
                chars.carriage_return()?;
                continue;
            }
            '\\' if continues(&mut chars) => continue,
            '#' => {
                while chars.peek().is_some_and(|c| c != '\n' && c != '\r') {
                    chars.next();
                }
                continue;
            }
            '\n' => Token::Newline,
            '|' | '&' if doubled(&chars, c) => {
                chars.next();
                chars.next();
                out.push((if c == '|' { Token::Or } else { Token::And }, at));
                continue;
            }
            '|' => Token::Pipe,
            ';' => Token::Semicolon,
            '<' => Token::In,
            '{' => Token::Open,
            '}' => Token::Close,
            '>' => {
                chars.next();
                if chars.peek() == Some('>') {
                    chars.next();
                    out.push((Token::Append, at));
                } else {
                    out.push((Token::Out, at));
                }
                continue;
            }
            _ => {
                out.push((Token::Word(word(&mut chars)?), at));
                continue;
            }
        };
        chars.next();
        out.push((token, at));
    }
    Ok(out)
}

/// Whether `c` comes next twice.
fn doubled(chars: &Chars<'_>, c: char) -> bool {
    let mut ahead = chars.rest.clone();
    ahead.next() == Some(c) && ahead.next() == Some(c)
}

/// Whether a `\` and a line's end come next, which join this line to the
/// next; they are taken when they do.
fn continues(chars: &mut Chars<'_>) -> bool {
    let mut ahead = chars.rest.clone();
    ahead.next();
    let joins = match ahead.next() {
        Some('\n') => 2,
        Some('\r') if ahead.next() == Some('\n') => 3,
        _ => return false,
    };
    for _ in 0..joins {
        chars.next();
    }
    true
}

/// Adds `c` to the parts of a word: to its last text when that is as
/// plain as `c`, as a text of its own otherwise.
fn push(parts: &mut Vec<Part>, c: char, plain: bool) {
    if let Some(Part::Text { text, plain: last }) = parts.last_mut()
        && *last == plain
    {
        text.push(c);
        return;
    }
    parts.push(Part::Text {
        text: c.into(),
        plain,
    });
}

/// The word that starts at the next character.
fn word(chars: &mut Chars<'_>) -> Result<Word, Error> {
    let at = chars.at;
    let mut parts = Vec::new();
    while let Some(c) = chars.peek() {
        match c {
            c if ends_word(c) => break,
            '&' if doubled(chars, '&') => break,
            '\\' if continues(chars) => {}
            '\\' => {
                chars.next();
                match chars.next() {
                    Some(escaped) => push(&mut parts, escaped, false),
                    None => push(&mut parts, '\\', false),
                }
            }
            '\'' => single(chars, &mut parts)?,
            '"' => double(chars, &mut parts)?,
            '$' => variable(chars, &mut parts, true)?,
            c => {
                chars.next();
                push(&mut parts, c, true);
            }
        }
    }
    Ok(Word { parts, at })
}

/// A `'...'` quote, its characters as they are.
fn single(chars: &mut Chars<'_>, parts: &mut Vec<Part>) -> Result<(), Error> {
    let at = chars.at;
    chars.next();
    let mut text = String::new();
    loop {
        match chars.next() {
            Some('\'') => break,
            Some('\n' | '\r') | None => {
                return Err(Error {
                    at,
                    code: Code::UnterminatedQuote,
                });
            }
            Some(c) => text.push(c),
        }
    }
    quoted(parts, text);
    Ok(())
}

/// Adds the quoted `text` to a word, even an empty one, which still makes
/// the word.
fn quoted(parts: &mut Vec<Part>, text: String) {
    if let Some(Part::Text {
        text: last,
        plain: false,
    }) = parts.last_mut()
    {
        last.push_str(&text);
    } else {
        parts.push(Part::Text { text, plain: false });
    }
}

/// A `"..."` quote, with its escapes and variables.
fn double(chars: &mut Chars<'_>, parts: &mut Vec<Part>) -> Result<(), Error> {
    let at = chars.at;
    chars.next();
    quoted(parts, String::new());
    loop {
        match chars.peek() {
            Some('"') => {
                chars.next();
                return Ok(());
            }
            Some('\n' | '\r') | None => {
                return Err(Error {
                    at,
                    code: Code::UnterminatedQuote,
                });
            }
            Some('$') => variable(chars, parts, false)?,
            Some('\\') => {
                chars.next();
                let c = match chars.peek() {
                    Some(c @ ('"' | '\\' | '$')) => c,
                    Some('n') => '\n',
                    Some('t') => '\t',
                    _ => {
                        push(parts, '\\', false);
                        continue;
                    }
                };
                chars.next();
                push(parts, c, false);
            }
            Some(c) => {
                chars.next();
                push(parts, c, false);
            }
        }
    }
}

/// A `$` and the variable it starts, or a `$` that starts none, plain
/// outside quotes when `plain`.
fn variable(chars: &mut Chars<'_>, parts: &mut Vec<Part>, plain: bool) -> Result<(), Error> {
    let at = chars.at;
    chars.next();
    let name = match chars.peek() {
        Some('{') => {
            chars.next();
            let mut name = String::new();
            while let Some(c) = chars.peek().filter(|&c| in_name(c)) {
                chars.next();
                name.push(c);
            }
            let named = name.chars().next().is_some_and(starts_name);
            if !named || chars.next() != Some('}') {
                return Err(Error {
                    at,
                    code: Code::BadVariable,
                });
            }
            name
        }
        Some(c @ ('?' | '#')) => {
            chars.next();
            c.into()
        }
        Some(c) if c.is_ascii_digit() => {
            let mut digits = String::new();
            while let Some(d) = chars.peek().filter(char::is_ascii_digit) {
                chars.next();
                digits.push(d);
            }
            digits
        }
        Some(c) if starts_name(c) => {
            let mut name = String::new();
            while let Some(c) = chars.peek().filter(|&c| in_name(c)) {
                chars.next();
                name.push(c);
            }
            name
        }
        _ => {
            push(parts, '$', plain);
            return Ok(());
        }
    };
    parts.push(Part::Variable(name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tokens of `script` written short: plain text as it is, quoted
    /// or escaped text in brackets, variables as `${name}`, operators as
    /// written and a line's end as `NL`.
    fn short(script: &str) -> Vec<String> {
        tokens(script)
            .unwrap()
            .into_iter()
            .map(|(token, _)| match token {
                Token::Word(w) => w
                    .parts
                    .iter()
                    .map(|p| match p {
                        Part::Text { text, plain: true } => text.clone(),
                        Part::Text { text, plain: false } => format!("[{text}]"),
                        Part::Variable(name) => format!("${{{name}}}"),
                    })
                    .collect(),
                Token::Pipe => "|".into(),
                Token::And => "&&".into(),
                Token::Or => "||".into(),
                Token::Semicolon => ";".into(),
                Token::Newline => "NL".into(),
                Token::Out => ">".into(),
                Token::Append => ">>".into(),
                Token::In => "<".into(),
                Token::Open => "{".into(),
                Token::Close => "}".into(),
            })
            .collect()
    }

    fn error(script: &str) -> (usize, usize, Code) {
        let e = tokens(script).unwrap_err();
        (e.at.line, e.at.column, e.code)
    }

    #[test]
    fn words_end_at_spaces_and_operators() {
        assert_eq!(
            short("ls -l /System|grep a;echo b>f>>g<h{x}"),
            [
                "ls", "-l", "/System", "|", "grep", "a", ";", "echo", "b", ">", "f", ">>", "g",
                "<", "h", "{", "x", "}"
            ]
        );
        assert_eq!(short("  a\t\tb  "), ["a", "b"]);
        assert_eq!(short(""), Vec::<String>::new());
        assert_eq!(short("a\nb\n"), ["a", "NL", "b", "NL"]);
        assert_eq!(
            short("a\r\nb"),
            ["a", "NL", "b"],
            "a carriage return and a line feed"
        );
        assert_eq!(short("> >> >>> <"), [">", ">>", ">>", ">", "<"]);
    }

    #[test]
    fn each_token_knows_where_it_starts() {
        let at: Vec<(usize, usize)> = tokens("echo a\n  b \u{e9}t\u{e9} |\tc\n}")
            .unwrap()
            .into_iter()
            .map(|(_, p)| (p.line, p.column))
            .collect();
        assert_eq!(
            at,
            [
                (1, 1),
                (1, 6),
                (1, 7),
                (2, 3),
                (2, 5),
                (2, 9),
                (2, 11),
                (2, 12),
                (3, 1)
            ]
        );
        let Ok(words) = tokens("x 'a b' \"c\" $v") else {
            panic!("these are words");
        };
        let starts: Vec<usize> = words.iter().map(|(_, p)| p.column).collect();
        assert_eq!(starts, [1, 3, 9, 13]);
        let Token::Word(w) = &words[1].0 else {
            panic!("a word");
        };
        assert_eq!(w.at, words[1].1, "the word keeps its start too");
    }

    #[test]
    fn comments_run_to_the_end_of_their_line() {
        assert_eq!(
            short("# all of it\necho a # the rest\nb"),
            ["NL", "echo", "a", "NL", "b"]
        );
        assert_eq!(
            short("echo a#b"),
            ["echo", "a#b"],
            "inside a word, # is a character"
        );
        assert_eq!(short("echo '#' \"#\""), ["echo", "[#]", "[#]"]);
        assert_eq!(short("a # x\r\nb"), ["a", "NL", "b"]);
        assert_eq!(short("#"), Vec::<String>::new());
    }

    #[test]
    fn quotes_hold_what_they_hold() {
        assert_eq!(short(r"echo '$x \n *'"), ["echo", r"[$x \n *]"]);
        assert_eq!(
            short(r#"echo "a \" \\ \$ \n \t \q""#),
            ["echo", "[a \" \\ $ \n \t \\q]"]
        );
        assert_eq!(short(r#"a"b"'c'd"#), ["a[bc]d"], "parts make one word");
        assert_eq!(short(r#"say "hi $name!""#), ["say", "[hi ]${name}[!]"]);
        assert_eq!(short(r#""$a$b""#), ["[]${a}${b}"]);
        assert_eq!(
            short("echo '' \"\""),
            ["echo", "[]", "[]"],
            "empty quotes still make words"
        );
        assert_eq!(short(r#""a;b|c{d}>e""#), ["[a;b|c{d}>e]"]);
    }

    #[test]
    fn variables_take_their_names() {
        assert_eq!(
            short("$name ${name}x $n_1-2"),
            ["${name}", "${name}x", "${n_1}-2"]
        );
        assert_eq!(
            short("$? $# $1 $12x $0"),
            ["${?}", "${#}", "${1}", "${12}x", "${0}"]
        );
        assert_eq!(
            short("$ $- a$ $/"),
            ["$", "$-", "a$", "$/"],
            "a $ that starts none"
        );
        assert_eq!(short("\"$\" \"$-\""), ["[$]", "[$-]"]);
        assert_eq!(short("${_a}"), ["${_a}"]);
    }

    #[test]
    fn a_backslash_takes_the_next_character_as_it_is() {
        assert_eq!(short(r"a\*b \ c \; \\"), [r"a[*]b", "[ ]c", "[;]", r"[\]"]);
        assert_eq!(short(r"end\"), [r"end[\]"], "at the end, itself");
        assert_eq!(short("echo a\\\nb"), ["echo", "ab"], "a line's end joins");
        assert_eq!(short("echo a \\\n b"), ["echo", "a", "b"]);
        assert_eq!(short("echo a \\\r\n b"), ["echo", "a", "b"]);
        assert_eq!(short("\\\necho"), ["echo"]);
        assert_eq!(
            short("a\\\rb"),
            ["a[\r]b"],
            "a carriage return alone is a character to escape"
        );
    }

    #[test]
    fn plain_words_are_the_ones_keywords_are_made_of() {
        let words: Vec<Option<String>> = tokens(r#"if 'if' i\f "if" $if i"f" if2"#)
            .unwrap()
            .into_iter()
            .map(|(t, _)| match t {
                Token::Word(w) => w.plain().map(String::from),
                _ => None,
            })
            .collect();
        assert_eq!(
            words,
            [
                Some("if".into()),
                None,
                None,
                None,
                None,
                None,
                Some("if2".into())
            ]
        );
    }

    #[test]
    fn quotes_close_on_their_line() {
        assert_eq!(error("echo 'abc"), (1, 6, Code::UnterminatedQuote));
        assert_eq!(error("echo 'abc\ndef'"), (1, 6, Code::UnterminatedQuote));
        assert_eq!(error("x\necho \"abc"), (2, 6, Code::UnterminatedQuote));
        assert_eq!(error("echo \"a\nb\""), (1, 6, Code::UnterminatedQuote));
        assert_eq!(error("echo \"a\r\nb\""), (1, 6, Code::UnterminatedQuote));
        assert_eq!(error("echo 'a\r\nb'"), (1, 6, Code::UnterminatedQuote));
        assert_eq!(error("echo \"\\"), (1, 6, Code::UnterminatedQuote));
    }

    #[test]
    fn a_braced_variable_needs_a_name_and_its_brace() {
        assert_eq!(error("echo ${"), (1, 6, Code::BadVariable));
        assert_eq!(error("echo ${}"), (1, 6, Code::BadVariable));
        assert_eq!(error("echo ${1x}"), (1, 6, Code::BadVariable));
        assert_eq!(error("echo ${a b}"), (1, 6, Code::BadVariable));
        assert_eq!(error("echo \"${a\""), (1, 7, Code::BadVariable));
    }

    #[test]
    fn a_carriage_return_comes_with_a_line_feed() {
        assert_eq!(error("a\rb"), (1, 2, Code::LoneCarriageReturn));
        assert_eq!(error("a\n\r"), (2, 1, Code::LoneCarriageReturn));
        assert_eq!(error("a # comment\rb"), (1, 12, Code::LoneCarriageReturn));
    }

    /// Scripts and their tokens, written short, one case a line: the start
    /// of the conformance suite the specification asks for.
    const READ_AS: &[(&str, &[&str])] = &[
        ("a", &["a"]),
        ("a b c", &["a", "b", "c"]),
        ("a\tb", &["a", "b"]),
        ("a|b", &["a", "|", "b"]),
        ("a | b", &["a", "|", "b"]),
        ("a;b", &["a", ";", "b"]),
        ("a>b", &["a", ">", "b"]),
        ("a>>b", &["a", ">>", "b"]),
        ("a<b", &["a", "<", "b"]),
        ("a{b}", &["a", "{", "b", "}"]),
        ("{}", &["{", "}"]),
        ("a\n", &["a", "NL"]),
        ("\n\n", &["NL", "NL"]),
        ("a=b", &["a=b"]),
        ("a,b.c:d", &["a,b.c:d"]),
        ("-x --y=z", &["-x", "--y=z"]),
        ("~/a", &["~/a"]),
        ("a*b?c", &["a*b?c"]),
        ("'a*b'", &["[a*b]"]),
        ("\"a*b\"", &["[a*b]"]),
        ("a\\*b", &["a[*]b"]),
        ("'a''b'", &["[ab]"]),
        ("'a'\"b\"", &["[ab]"]),
        ("x'a'y", &["x[a]y"]),
        ("'a b'c", &["[a b]c"]),
        ("\"\"x", &["[]x"]),
        ("x\"\"", &["x[]"]),
        ("'\"'", &["[\"]"]),
        ("\"'\"", &["[']"]),
        ("'\\'", &["[\\]"]),
        ("\"\\\\\"", &["[\\]"]),
        ("\"\\a\"", &["[\\a]"]),
        ("\"a\\\"b\"", &["[a\"b]"]),
        ("\"\\$x\"", &["[$x]"]),
        ("\"$x\"", &["[]${x}"]),
        ("\"a$x b\"", &["[a]${x}[ b]"]),
        ("\"${x}y\"", &["[]${x}[y]"]),
        ("\"$\"", &["[$]"]),
        ("\"a $\"", &["[a $]"]),
        ("$x", &["${x}"]),
        ("$x$y", &["${x}${y}"]),
        ("$x.y", &["${x}.y"]),
        ("$x/y", &["${x}/y"]),
        ("a$x", &["a${x}"]),
        ("${x}y", &["${x}y"]),
        ("$x_y", &["${x_y}"]),
        ("$X9", &["${X9}"]),
        ("$9x", &["${9}x"]),
        ("$123", &["${123}"]),
        ("$?x", &["${?}x"]),
        ("$##", &["${#}#"]),
        ("$", &["$"]),
        ("a $ b", &["a", "$", "b"]),
        ("$-", &["$-"]),
        ("$'a'", &["$[a]"]),
        ("$\"a\"", &["$[a]"]),
        ("\\$x", &["[$]x"]),
        ("a\\ b", &["a[ ]b"]),
        ("\\|", &["[|]"]),
        ("\\#", &["[#]"]),
        ("\\\\", &["[\\]"]),
        ("a#", &["a#"]),
        ("a #", &["a"]),
        ("#a\nb", &["NL", "b"]),
        ("a#b c", &["a#b", "c"]),
        ("a ;# x", &["a", ";"]),
        ("a\\\nb\\\nc", &["abc"]),
        ("a \\\nb", &["a", "b"]),
        ("\u{e9}t\u{e9}", &["\u{e9}t\u{e9}"]),
        ("'\u{1F600}'", &["[\u{1F600}]"]),
        ("a\r\n\r\nb", &["a", "NL", "NL", "b"]),
        ("a&&b", &["a", "&&", "b"]),
        ("a || b", &["a", "||", "b"]),
        ("a&b", &["a&b"]),
        ("a & b", &["a", "&", "b"]),
        ("a&", &["a&"]),
        ("|||", &["||", "|"]),
        ("&&&", &["&&", "&"]),
        ("|", &["|"]),
        ("||", &["||"]),
        ("&&", &["&&"]),
        ("&", &["&"]),
        ("a|", &["a", "|"]),
        ("a &&", &["a", "&&"]),
        ("&&a", &["&&", "a"]),
        ("a\\&&b", &["a[&]&b"]),
        ("'&&' \\&&", &["[&&]", "[&]&"]),
    ];

    #[test]
    fn the_suite_reads_as_written() {
        for (script, want) in READ_AS {
            assert_eq!(short(script), *want, "{script:?}");
        }
    }

    /// Scripts that do not read, with the line, column and code of their
    /// first error.
    const REFUSED: &[(&str, usize, usize, Code)] = &[
        ("'", 1, 1, Code::UnterminatedQuote),
        ("\"", 1, 1, Code::UnterminatedQuote),
        ("a 'b", 1, 3, Code::UnterminatedQuote),
        ("a \"b", 1, 3, Code::UnterminatedQuote),
        ("'a' 'b", 1, 5, Code::UnterminatedQuote),
        ("\n\n  'x", 3, 3, Code::UnterminatedQuote),
        ("\u{e9} 'x", 1, 3, Code::UnterminatedQuote),
        ("\"a\\\"", 1, 1, Code::UnterminatedQuote),
        ("${", 1, 1, Code::BadVariable),
        ("a${b", 1, 2, Code::BadVariable),
        ("${?}", 1, 1, Code::BadVariable),
        ("${1}", 1, 1, Code::BadVariable),
        ("${-}", 1, 1, Code::BadVariable),
        ("${a-b}", 1, 1, Code::BadVariable),
        ("\"x ${}\"", 1, 4, Code::BadVariable),
        ("\r", 1, 1, Code::LoneCarriageReturn),
        ("ab\rc", 1, 3, Code::LoneCarriageReturn),
        ("a\r\n\rb", 2, 1, Code::LoneCarriageReturn),
        ("'a' \r", 1, 5, Code::LoneCarriageReturn),
    ];

    #[test]
    fn the_suite_refuses_what_it_should_where_it_should() {
        for &(script, line, column, code) in REFUSED {
            assert_eq!(error(script), (line, column, code), "{script:?}");
        }
    }

    #[test]
    fn a_word_is_plain_only_when_all_of_it_is() {
        let plain = |script: &str| match tokens(script).unwrap().remove(0).0 {
            Token::Word(w) => w.plain().map(String::from),
            _ => None,
        };
        assert_eq!(plain("abc"), Some("abc".into()));
        assert_eq!(plain("a*"), Some("a*".into()));
        assert_eq!(plain("a'b'"), None, "part of it quoted");
        assert_eq!(plain("a$b"), None, "part of it a variable");
        assert_eq!(plain("''"), None);
        assert_eq!(plain("\\a"), None);
        assert_eq!(
            plain("a\\\nb"),
            Some("ab".into()),
            "a joined line keeps it plain"
        );
    }

    #[test]
    fn cases_read_the_same_side_by_side() {
        for (a, want_a) in READ_AS {
            for (b, want_b) in READ_AS {
                let mut want: Vec<&str> = want_a.to_vec();
                want.push("NL");
                want.extend_from_slice(want_b);
                assert_eq!(short(&format!("{a}\n{b}")), want, "{a:?} then {b:?}");
            }
        }
    }

    #[test]
    fn cases_keep_their_columns_a_line_further_down() {
        for (script, _) in READ_AS {
            let here = tokens(script).unwrap();
            let below = tokens(&format!("\n{script}")).unwrap();
            assert_eq!(below.len(), here.len() + 1, "{script:?}");
            for ((t, at), (u, moved)) in here.iter().zip(&below[1..]) {
                let same = match (t, u) {
                    (Token::Word(a), Token::Word(b)) => a.parts == b.parts,
                    (a, b) => a == b,
                };
                assert!(same, "{script:?}");
                assert_eq!(
                    (moved.line, moved.column),
                    (at.line + 1, at.column),
                    "{script:?}"
                );
            }
            let right = tokens(&format!("  {script}")).unwrap();
            for ((_, at), (_, moved)) in here.iter().zip(&right) {
                let shift = if at.line == 1 { 2 } else { 0 };
                assert_eq!(
                    (moved.line, moved.column),
                    (at.line, at.column + shift),
                    "{script:?}"
                );
            }
        }
    }

    #[test]
    fn refusals_move_with_their_scripts() {
        for &(script, line, column, code) in REFUSED {
            assert_eq!(
                error(&format!("ok\n{script}")),
                (line + 1, column, code),
                "{script:?}"
            );
            let shift = if line == 1 { 3 } else { 0 };
            assert_eq!(
                error(&format!("ok {script}")),
                (line, column + shift, code),
                "{script:?}"
            );
        }
    }
}
