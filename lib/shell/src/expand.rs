//! Words expanded into what a command gets, sections 2 and 5 of the
//! specification: each variable replaced by its value, and a word with an
//! unquoted `*` or `?` turned into a pattern, matched against names as the
//! tree of files compares them.

use alloc::string::String;
use alloc::vec::Vec;

use crate::lex::{Part, Word};
use crate::{Code, Error};

/// A piece of a pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    /// A character that has to be there, in either case.
    Char(char),
    /// `?`: any one character.
    One,
    /// `*`: any run of characters, none too.
    Any,
}

/// What a word stands for once expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expanded {
    /// One word, as it is.
    Text(String),
    /// The names of the files that match, as many words.
    Pattern(Vec<Piece>),
}

/// `word` expanded, each variable's value found by `value`.
///
/// # Errors
///
/// At the word when a variable in it was never set.
pub fn expand(word: &Word, value: impl Fn(&str) -> Option<String>) -> Result<Expanded, Error> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut wild = false;
    for part in &word.parts {
        let (chars, plain) = match part {
            Part::Text { text, plain } => (text.clone(), *plain),
            Part::Variable(name) => match value(name) {
                Some(value) => (value, false),
                None => {
                    return Err(Error {
                        at: word.at,
                        code: Code::UnsetVariable,
                    });
                }
            },
        };
        for c in chars.chars() {
            pieces.push(match c {
                '*' if plain => Piece::Any,
                '?' if plain => Piece::One,
                c => Piece::Char(c),
            });
            wild |= plain && matches!(c, '*' | '?');
        }
        text.push_str(&chars);
    }
    Ok(if wild {
        Expanded::Pattern(pieces)
    } else {
        Expanded::Text(text)
    })
}

/// `c` in capitals, when its capital is one character, as the tree of
/// files compares names.
fn capital(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

/// Whether the name `name` matches `pattern`, compared in capitals. `.`
/// and `..` match no pattern.
#[must_use]
pub fn matches(pattern: &[Piece], name: &str) -> bool {
    if name == "." || name == ".." {
        return false;
    }
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    // Where the last `*` was, and how far into the name it reaches.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some(Piece::Any) => {
                star = Some((p, n));
                p += 1;
            }
            Some(Piece::One) => {
                p += 1;
                n += 1;
            }
            Some(&Piece::Char(c)) if capital(c) == capital(name[n]) => {
                p += 1;
                n += 1;
            }
            _ => {
                let Some((at, reach)) = star else {
                    return false;
                };
                star = Some((at, reach + 1));
                p = at + 1;
                n = reach + 1;
            }
        }
    }
    pattern[p..].iter().all(|&piece| piece == Piece::Any)
}

/// The parts of `pattern` between its slashes, each with the text it has
/// when it holds no `*` or `?`, so only the parts that do need a listing.
#[must_use]
pub fn segments(pattern: &[Piece]) -> Vec<(Vec<Piece>, Option<String>)> {
    pattern
        .split(|&p| p == Piece::Char('/'))
        .map(|segment| {
            let text = segment
                .iter()
                .map(|&p| match p {
                    Piece::Char(c) => Some(c),
                    Piece::One | Piece::Any => None,
                })
                .collect();
            (segment.to_vec(), text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pos;
    use crate::lex::{Token, tokens};

    fn word(text: &str) -> Word {
        match tokens(text).unwrap().remove(0).0 {
            Token::Word(w) => w,
            other => panic!("{other:?} is no word"),
        }
    }

    fn values(name: &str) -> Option<String> {
        match name {
            "x" => Some("a*b".into()),
            "dir" => Some("/System".into()),
            "empty" => Some(String::new()),
            "?" => Some("0".into()),
            "1" => Some("first".into()),
            _ => None,
        }
    }

    /// The pattern `text` writes, `*` and `?` as wildcards.
    fn pattern(text: &str) -> Vec<Piece> {
        text.chars()
            .map(|c| match c {
                '*' => Piece::Any,
                '?' => Piece::One,
                c => Piece::Char(c),
            })
            .collect()
    }

    #[test]
    fn variables_become_their_values_and_stay_one_word() {
        let text = |w: &str| expand(&word(w), values).unwrap();
        assert_eq!(text("plain"), Expanded::Text("plain".into()));
        assert_eq!(text("$dir/a"), Expanded::Text("/System/a".into()));
        assert_eq!(text("\"$dir\"/b"), Expanded::Text("/System/b".into()));
        assert_eq!(text("${dir}x"), Expanded::Text("/Systemx".into()));
        assert_eq!(
            text("$x"),
            Expanded::Text("a*b".into()),
            "a value never makes a pattern"
        );
        assert_eq!(text("a$empty"), Expanded::Text("a".into()));
        assert_eq!(text("$?$1"), Expanded::Text("0first".into()));
        assert_eq!(text("'*'"), Expanded::Text("*".into()), "nor does a quote");
        assert_eq!(text("\\?"), Expanded::Text("?".into()), "nor an escape");
        assert_eq!(text("''"), Expanded::Text(String::new()));
    }

    #[test]
    fn an_unquoted_star_or_question_mark_makes_a_pattern() {
        let expanded = |w: &str| expand(&word(w), values).unwrap();
        assert_eq!(expanded("*.txt"), Expanded::Pattern(pattern("*.txt")));
        assert_eq!(expanded("a?c"), Expanded::Pattern(pattern("a?c")));
        assert_eq!(
            expanded("$x*"),
            Expanded::Pattern(vec![
                Piece::Char('a'),
                Piece::Char('*'),
                Piece::Char('b'),
                Piece::Any
            ]),
            "the value's star stays a character"
        );
        assert_eq!(
            expanded("'*'?"),
            Expanded::Pattern(vec![Piece::Char('*'), Piece::One])
        );
        assert_eq!(expanded("$dir/*"), Expanded::Pattern(pattern("/System/*")));
    }

    #[test]
    fn a_variable_never_set_stops_the_word() {
        let e = expand(&word("  a$nope"), values).unwrap_err();
        assert_eq!(e.code, Code::UnsetVariable);
        assert_eq!(e.at, Pos { line: 1, column: 3 }, "where the word starts");
        assert_eq!(
            expand(&word("\"$nope\""), values).unwrap_err().code,
            Code::UnsetVariable
        );
        assert_eq!(
            expand(&word("*$nope"), values).unwrap_err().code,
            Code::UnsetVariable
        );
    }

    #[test]
    fn names_match_as_the_tree_compares_them() {
        let cases = [
            ("*.txt", "a.txt", true),
            ("*.txt", "A.TXT", true),
            ("*.TXT", "notes.txt", true),
            ("*.txt", "a.txt.bak", false),
            ("*.txt", "txt", false),
            ("?", "a", true),
            ("?", "ab", false),
            ("??", "ab", true),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("a*b*c", "abc", true),
            ("a*b*c", "aXbYc", true),
            ("a*b*c", "acb", false),
            ("*a", "bba", true),
            ("*a", "bab", false),
            ("**", "x", true),
            ("*x*x*", "xx", true),
            ("*x*x*", "x", false),
            ("*x*x*", "axbxc", true),
            ("x*", "x", true),
            ("*", ".", false),
            ("*", "..", false),
            (".*", ".x", true),
            ("\u{e9}*", "\u{c9}t\u{e9}", true),
            ("\u{df}", "SS", false),
            ("\u{df}", "\u{df}", true),
            ("abc", "abc", true),
            ("abc", "abd", false),
            ("abc", "ab", false),
            ("ab", "abc", false),
        ];
        for (p, name, want) in cases {
            assert_eq!(matches(&pattern(p), name), want, "{p} against {name}");
        }
        let star = [Piece::Char('*')];
        assert!(
            matches(&star, "*") && !matches(&star, "a"),
            "a quoted star is a star"
        );
    }

    #[test]
    fn patterns_split_at_their_slashes() {
        let split = |text: &str| -> Vec<Option<String>> {
            segments(&pattern(text))
                .into_iter()
                .map(|(_, t)| t)
                .collect()
        };
        assert_eq!(
            split("/System/*.meri"),
            [Some(String::new()), Some("System".into()), None]
        );
        assert_eq!(split("a/b"), [Some("a".into()), Some("b".into())]);
        assert_eq!(split("*/x"), [None, Some("x".into())]);
        assert_eq!(split("?"), [None]);
        let parts = segments(&pattern("/a/b?"));
        assert_eq!(parts[2].0, [Piece::Char('b'), Piece::One]);
    }

    #[test]
    fn capitals_are_single_characters_or_the_character_itself() {
        assert_eq!(capital('a'), 'A');
        assert_eq!(capital('Z'), 'Z');
        assert_eq!(capital('\u{e9}'), '\u{c9}');
        assert_eq!(capital('\u{df}'), '\u{df}', "its capital is two letters");
        assert_eq!(capital('1'), '1');
    }

    #[test]
    fn a_parsed_command_expands_word_by_word() {
        let statements = crate::parse::script("cp $dir/*.meri \"$dir/back up\" $x").unwrap();
        let [crate::parse::Statement::List(list)] = statements.as_slice() else {
            panic!("one command");
        };
        let expanded: Vec<Expanded> = list.first.commands[0]
            .words
            .iter()
            .map(|w| expand(w, values).unwrap())
            .collect();
        assert_eq!(
            expanded,
            [
                Expanded::Text("cp".into()),
                Expanded::Pattern(pattern("/System/*.meri")),
                Expanded::Text("/System/back up".into()),
                Expanded::Text("a*b".into()),
            ]
        );
    }
}
