//! POSIX's pattern matching notation: what `case` matches a word against,
//! what `${name#pattern}` and its kin take off a value, and what the names
//! of files are matched against.

use alloc::string::String;
use alloc::vec::Vec;

/// A class of characters named in brackets, as `[:alpha:]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Alnum,
    Alpha,
    Blank,
    Cntrl,
    Digit,
    Graph,
    Lower,
    Print,
    Punct,
    Space,
    Upper,
    Xdigit,
}

impl Class {
    fn named(name: &str) -> Option<Class> {
        Some(match name {
            "alnum" => Class::Alnum,
            "alpha" => Class::Alpha,
            "blank" => Class::Blank,
            "cntrl" => Class::Cntrl,
            "digit" => Class::Digit,
            "graph" => Class::Graph,
            "lower" => Class::Lower,
            "print" => Class::Print,
            "punct" => Class::Punct,
            "space" => Class::Space,
            "upper" => Class::Upper,
            "xdigit" => Class::Xdigit,
            _ => return None,
        })
    }

    fn holds(self, c: char) -> bool {
        let graph = !c.is_whitespace() && !c.is_control();
        match self {
            Class::Alnum => c.is_alphabetic() || c.is_ascii_digit(),
            Class::Alpha => c.is_alphabetic(),
            Class::Blank => c == ' ' || c == '\t',
            Class::Cntrl => c.is_control(),
            Class::Digit => c.is_ascii_digit(),
            Class::Graph => graph,
            Class::Lower => c.is_lowercase(),
            Class::Print => graph || c == ' ',
            Class::Punct => graph && !c.is_alphabetic() && !c.is_numeric(),
            Class::Space => c.is_whitespace(),
            Class::Upper => c.is_uppercase(),
            Class::Xdigit => c.is_ascii_hexdigit(),
        }
    }
}

/// What a bracket expression lists.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Char(char),
    /// From one character to another, both in, by their code points.
    Range(char, char),
    Class(Class),
}

/// What one character of a text has to be.
#[derive(Clone, Debug, PartialEq, Eq)]
enum One {
    Char(char),
    /// `?`: any character.
    Any,
    /// `[...]`: one of those listed, or with `!`, one of those not.
    Set {
        not: bool,
        items: Vec<Item>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    One(One),
    /// `*`: any run of characters, none too.
    Star,
}

impl One {
    fn takes(&self, c: char) -> bool {
        match self {
            One::Char(want) => *want == c,
            One::Any => true,
            One::Set { not, items } => {
                let listed = items.iter().any(|item| match item {
                    Item::Char(want) => *want == c,
                    Item::Range(from, to) => (*from..=*to).contains(&c),
                    Item::Class(class) => class.holds(c),
                });
                listed != *not
            }
        }
    }
}

/// A pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pieces: Vec<Piece>,
}

/// The bracket expression whose `[` is just before `from` in `written`,
/// and where what follows it starts; none when the `[` starts none.
fn set(written: &[(char, bool)], from: usize) -> Option<(One, usize)> {
    let unquoted = |i: usize, c: char| written.get(i) == Some(&(c, false));
    let mut i = from;
    let not = unquoted(i, '!') || unquoted(i, '^');
    if not {
        i += 1;
    }
    let mut items = Vec::new();
    loop {
        let &(c, quoted) = written.get(i)?;
        if c == ']' && !quoted && !items.is_empty() {
            return Some((One::Set { not, items }, i + 1));
        }
        if c == '[' && !quoted && matches!(written.get(i + 1), Some(&(':' | '=' | '.', false))) {
            let kind = written[i + 1].0;
            let start = i + 2;
            let end = (start..written.len().saturating_sub(1))
                .find(|&j| unquoted(j, kind) && unquoted(j + 1, ']'))?;
            let name: String = written[start..end].iter().map(|&(c, _)| c).collect();
            let item = if kind == ':' {
                Item::Class(Class::named(&name)?)
            } else {
                let mut chars = name.chars();
                let (Some(one), None) = (chars.next(), chars.next()) else {
                    return None;
                };
                Item::Char(one)
            };
            items.push(item);
            i = end + 2;
            continue;
        }
        match written.get(i + 2) {
            Some(&(to, _)) if unquoted(i + 1, '-') && !unquoted(i + 2, ']') => {
                items.push(Item::Range(c, to));
                i += 3;
            }
            _ => {
                items.push(Item::Char(c));
                i += 1;
            }
        }
    }
}

impl Pattern {
    /// The pattern the characters of an expanded word stand for, each with
    /// whether it was quoted, which makes it stand for itself.
    #[must_use]
    pub fn new(written: &[(char, bool)]) -> Pattern {
        let mut pieces = Vec::new();
        let mut i = 0;
        while let Some(&(c, quoted)) = written.get(i) {
            i += 1;
            let piece = match c {
                _ if quoted => Piece::One(One::Char(c)),
                '?' => Piece::One(One::Any),
                '*' => Piece::Star,
                '[' => match set(written, i) {
                    Some((one, next)) => {
                        i = next;
                        Piece::One(one)
                    }
                    None => Piece::One(One::Char('[')),
                },
                _ => Piece::One(One::Char(c)),
            };
            if !(piece == Piece::Star && pieces.last() == Some(&Piece::Star)) {
                pieces.push(piece);
            }
        }
        Pattern { pieces }
    }

    /// Whether it matches only the text it is written as, which a word
    /// without `*`, `?` or a bracket expression unquoted does.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.pieces
            .iter()
            .all(|p| matches!(p, Piece::One(One::Char(_))))
    }

    /// Whether it matches all of `text`.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        let text: Vec<char> = text.chars().collect();
        self.matches_chars(&text)
    }

    fn matches_chars(&self, text: &[char]) -> bool {
        let pieces = &self.pieces;
        let (mut p, mut t) = (0, 0);
        // Where to go on from after the last `*`, which takes one more
        // character each time what follows it fails.
        let mut star = None;
        while t < text.len() {
            match pieces.get(p) {
                Some(Piece::Star) => {
                    star = Some((p + 1, t));
                    p += 1;
                }
                Some(Piece::One(one)) if one.takes(text[t]) => {
                    p += 1;
                    t += 1;
                }
                _ => match star {
                    Some((after, from)) => {
                        p = after;
                        t = from + 1;
                        star = Some((after, from + 1));
                    }
                    None => return false,
                },
            }
        }
        pieces[p..].iter().all(|piece| *piece == Piece::Star)
    }

    /// Whether it matches the name of a file, which a `*`, a `?` or a
    /// bracket expression never starts matching at a `.`.
    #[must_use]
    pub fn matches_name(&self, name: &str) -> bool {
        if name.starts_with('.') && self.pieces.first() != Some(&Piece::One(One::Char('.'))) {
            return false;
        }
        self.matches(name)
    }

    /// `text` without the shortest start it matches, or the longest, as
    /// `${name#pattern}` and `${name##pattern}` take; all of it when none.
    #[must_use]
    pub fn without_start(&self, text: &str, longest: bool) -> String {
        let chars: Vec<char> = text.chars().collect();
        let ends: Vec<usize> = (0..=chars.len()).collect();
        let mut tried = ends.iter();
        let fits = |&&end: &&usize| self.matches_chars(&chars[..end]);
        let end = if longest {
            tried.rev().find(fits)
        } else {
            tried.find(fits)
        };
        end.map_or_else(|| text.into(), |&end| chars[end..].iter().collect())
    }

    /// `text` without the shortest end it matches, or the longest, as
    /// `${name%pattern}` and `${name%%pattern}` take; all of it when none.
    #[must_use]
    pub fn without_end(&self, text: &str, longest: bool) -> String {
        let chars: Vec<char> = text.chars().collect();
        let starts: Vec<usize> = (0..=chars.len()).collect();
        let mut tried = starts.iter();
        let fits = |&&start: &&usize| self.matches_chars(&chars[start..]);
        let start = if longest {
            tried.find(fits)
        } else {
            tried.rev().find(fits)
        };
        start.map_or_else(|| text.into(), |&start| chars[..start].iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh::lex::{self, Part, Token};

    /// The pattern a word of a script stands for, its quotes as the shell
    /// reads them.
    fn pattern(word: &str) -> Pattern {
        let tokens = lex::tokens(word).unwrap();
        let [(Token::Word(word), _)] = tokens.as_slice() else {
            panic!("{tokens:?}")
        };
        let written: Vec<(char, bool)> = word
            .parts
            .iter()
            .flat_map(|part| match part {
                Part::Text { text, quoted } => {
                    text.chars().map(|c| (c, *quoted)).collect::<Vec<_>>()
                }
                Part::Expand { .. } => panic!("{part:?}"),
            })
            .collect();
        Pattern::new(&written)
    }

    fn matching(word: &str, texts: &[&str]) -> Vec<bool> {
        let pattern = pattern(word);
        texts.iter().map(|t| pattern.matches(t)).collect()
    }

    #[test]
    fn a_question_mark_is_one_character_and_a_star_any_run() {
        assert_eq!(
            matching("a?c", &["abc", "ac", "abbc", "aäc"]),
            [true, false, false, true]
        );
        assert_eq!(matching("*", &["", "x", "a b"]), [true, true, true]);
        assert_eq!(matching("a*", &["a", "abc", "ba"]), [true, true, false]);
        assert_eq!(
            matching("*a*b*", &["xaybz", "ab", "ba"]),
            [true, true, false]
        );
        assert_eq!(
            matching("*.txt", &["notes.txt", ".txt", "notes.txt.bak"]),
            [true, true, false]
        );
        assert_eq!(matching("a**b", &["ab", "axyzb"]), [true, true]);
        assert_eq!(matching("*?", &["a", ""]), [true, false]);
        assert_eq!(matching("日本*", &["日本語", "日"]), [true, false]);
        assert_eq!(matching("''", &["", "a"]), [true, false]);
        let many = "a".repeat(60);
        assert!(!pattern("*a*a*a*a*a*b").matches(&many), "and soon");
    }

    #[test]
    fn quoted_characters_stand_for_themselves() {
        assert_eq!(matching("'*'", &["*", "x"]), [true, false]);
        assert_eq!(matching("\\?", &["?", "x"]), [true, false]);
        assert_eq!(matching("\"a*\"b*", &["a*bc", "axbc"]), [true, false]);
        assert_eq!(matching("'[a]'", &["[a]", "a"]), [true, false]);
        for (word, plain) in [
            ("abc", true),
            ("a*", false),
            ("'a*'", true),
            ("[a]", false),
            ("a[", true),
            ("x?", false),
            ("\\[a]", true),
        ] {
            assert_eq!(pattern(word).is_plain(), plain, "{word}");
        }
    }

    #[test]
    fn brackets_list_the_characters_they_match() {
        let all = ["a", "b", "c", "d", "ab", "]", "-", "!"];
        assert_eq!(
            matching("[abc]", &all),
            [true, true, true, false, false, false, false, false]
        );
        assert_eq!(
            matching("[!abc]", &all),
            [false, false, false, true, false, true, true, true]
        );
        assert_eq!(matching("[^abc]", &["a", "d"]), [false, true]);
        assert_eq!(matching("[a-c]", &["b", "d"]), [true, false]);
        assert_eq!(matching("[z-a]", &["a", "m", "z"]), [false, false, false]);
        assert_eq!(matching("[]a]", &["]", "a", "b"]), [true, true, false]);
        assert_eq!(matching("[!]a]", &["]", "a", "b"]), [false, false, true]);
        assert_eq!(matching("[a-]", &["-", "a", "b"]), [true, true, false]);
        assert_eq!(matching("[-a]", &["-", "a"]), [true, true]);
        assert_eq!(
            matching("[%--]", &["+", "%", "-", "."]),
            [true, true, true, false]
        );
        assert_eq!(
            matching("x[ab]y", &["xay", "xby", "xy"]),
            [true, true, false]
        );
    }

    #[test]
    fn brackets_name_classes_of_characters() {
        for (class, holds, not) in [
            ("alnum", "a7", "-"),
            ("alpha", "aä", "7"),
            ("blank", " \t", "\n"),
            ("cntrl", "\u{1}\u{7f}", "a"),
            ("digit", "09", "a"),
            ("graph", "a!", " "),
            ("lower", "aß", "A"),
            ("print", "a ", "\u{1}"),
            ("punct", "!-", "a"),
            ("space", " \n", "a"),
            ("upper", "AÄ", "a"),
            ("xdigit", "aF9", "g"),
        ] {
            let p = pattern(&format!("[[:{class}:]]"));
            for c in holds.chars() {
                assert!(p.matches(&c.to_string()), "{class} holds {c:?}");
            }
            assert!(!p.matches(not), "{class} leaves out {not:?}");
        }
        assert_eq!(
            matching("[[:upper:][:digit:]]", &["A", "1", "a"]),
            [true, true, false]
        );
        assert_eq!(matching("[![:space:]]", &["x", " "]), [true, false]);
        assert_eq!(matching("[[=a=]b]", &["a", "b", "c"]), [true, true, false]);
        assert_eq!(matching("[[.-.]a]", &["-", "a", "b"]), [true, true, false]);
    }

    #[test]
    fn a_bracket_that_starts_no_list_stands_for_itself() {
        assert_eq!(matching("[ab", &["[ab", "a"]), [true, false]);
        assert_eq!(matching("[", &["["]), [true]);
        assert_eq!(matching("a[]", &["a[]", "a"]), [true, false]);
        assert_eq!(
            matching("[[:nope:]]", &["[o]", "[[:nope:]]", "o"]),
            [true, false, false],
            "the first [ starts no list, the second one does"
        );
        assert_eq!(matching("[[=ab=]]", &["[=]", "a"]), [true, false]);
        assert_eq!(
            matching("[[=a=b=]]", &["[a]", "a]", "b"]),
            [true, false, false],
            "a name ends at =] only"
        );
    }

    #[test]
    fn quoted_characters_in_brackets_lose_their_meaning() {
        assert_eq!(matching("[\\]]", &["]", "\\"]), [true, false]);
        assert_eq!(
            matching("[a\\-z]", &["-", "a", "z", "b"]),
            [true, true, true, false]
        );
        assert_eq!(matching("['!'a]", &["!", "a", "b"]), [true, true, false]);
        assert_eq!(matching("[a'-'z]", &["b", "-"]), [false, true]);
    }

    #[test]
    fn a_name_starting_with_a_dot_needs_the_dot_written() {
        let names = [".hidden", "visible", ".", ".."];
        for (word, want) in [
            ("*", [false, true, false, false]),
            (".*", [true, false, true, true]),
            ("?hidden", [false, false, false, false]),
            ("[.]hidden", [false, false, false, false]),
            (".h*", [true, false, false, false]),
        ] {
            let p = pattern(word);
            let got: Vec<bool> = names.iter().map(|n| p.matches_name(n)).collect();
            assert_eq!(got, want, "{word}");
        }
        assert!(pattern("*").matches(".hidden"), "but any text may start so");
    }

    #[test]
    fn hidden_names_need_their_dot_however_the_rest_is_written() {
        let names = [".x", ".", "..", "a.c", ".a.c"];
        for (word, want) in [
            ("[!.]*", [false, false, false, true, false]),
            (".?", [true, false, true, false, false]),
            ("*.c", [false, false, false, true, false]),
            (".*.c", [false, false, false, false, true]),
            ("'.'*", [true, true, true, false, true]),
        ] {
            let p = pattern(word);
            let got: Vec<bool> = names.iter().map(|n| p.matches_name(n)).collect();
            assert_eq!(got, want, "{word}");
        }
    }

    #[test]
    fn starts_and_ends_come_off_shortest_or_longest() {
        let p = pattern("*X");
        assert_eq!(p.without_start("aXbXc", false), "bXc");
        assert_eq!(p.without_start("aXbXc", true), "c");
        let p = pattern("X*");
        assert_eq!(p.without_end("aXbXc", false), "aXb");
        assert_eq!(p.without_end("aXbXc", true), "a");
        assert_eq!(pattern("x*").without_start("abc", true), "abc", "no match");
        assert_eq!(pattern("*x").without_end("abc", false), "abc");
        assert_eq!(
            pattern("*").without_start("abc", false),
            "abc",
            "the empty start"
        );
        assert_eq!(pattern("*").without_start("abc", true), "");
        assert_eq!(pattern("*").without_end("abc", false), "abc");
        assert_eq!(pattern("*").without_end("abc", true), "");
        assert_eq!(pattern(".*").without_end("datei.tär", false), "datei");
    }

    #[test]
    fn paths_lose_their_folders_and_files_their_endings() {
        let path = "/usr/local/bin/sh";
        assert_eq!(pattern("*/").without_start(path, true), "sh");
        assert_eq!(pattern("*/").without_start(path, false), "usr/local/bin/sh");
        assert_eq!(pattern("/*").without_end(path, false), "/usr/local/bin");
        assert_eq!(pattern("/*").without_end(path, true), "");
        assert_eq!(
            pattern(".*").without_end("archive.tar.gz", false),
            "archive.tar"
        );
        assert_eq!(pattern(".*").without_end("archive.tar.gz", true), "archive");
        assert_eq!(
            pattern("[0-9]*").without_start("2026-10-27", false),
            "026-10-27"
        );
    }

    /// Whether `pattern` of letters, `*` and `?` matches `text`, as POSIX
    /// defines it, one case after the other.
    fn defined(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => (0..=text.len()).any(|k| defined(rest, &text[k..])),
            Some(('?', rest)) => !text.is_empty() && defined(rest, &text[1..]),
            Some((c, rest)) => text.first() == Some(c) && defined(rest, &text[1..]),
        }
    }

    /// Every string of up to `most` characters from `alphabet`.
    fn strings(alphabet: &[char], most: u32) -> Vec<Vec<char>> {
        let mut all = Vec::new();
        for len in 0..=most {
            for mut n in 0..alphabet.len().pow(len) {
                let mut s = Vec::new();
                for _ in 0..len {
                    s.push(alphabet[n % alphabet.len()]);
                    n /= alphabet.len();
                }
                all.push(s);
            }
        }
        all
    }

    #[test]
    fn every_short_pattern_matches_as_posix_defines_it() {
        let texts = strings(&['a', 'b'], 5);
        for p in strings(&['a', 'b', '*', '?'], 4) {
            let written: Vec<(char, bool)> = p.iter().map(|&c| (c, false)).collect();
            let pattern = Pattern::new(&written);
            for t in &texts {
                let text: String = t.iter().collect();
                assert_eq!(pattern.matches(&text), defined(&p, t), "{p:?} {text:?}");
            }
        }
    }

    #[test]
    fn every_short_start_and_end_comes_off_as_posix_defines_it() {
        let texts = strings(&['a', 'b'], 4);
        for p in strings(&['a', 'b', '*', '?'], 3) {
            let written: Vec<(char, bool)> = p.iter().map(|&c| (c, false)).collect();
            let pattern = Pattern::new(&written);
            for t in &texts {
                let text: String = t.iter().collect();
                let n = t.len();
                let starts: Vec<usize> = (0..=n).filter(|&k| defined(&p, &t[..k])).collect();
                let ends: Vec<usize> = (0..=n).filter(|&k| defined(&p, &t[k..])).collect();
                let rest = |from: usize| t[from..].iter().collect::<String>();
                let kept = |to: usize| t[..to].iter().collect::<String>();
                let first = |v: &[usize]| v.first().copied();
                let last = |v: &[usize]| v.last().copied();
                assert_eq!(
                    pattern.without_start(&text, false),
                    first(&starts).map_or(text.clone(), rest),
                    "{p:?} # {text:?}"
                );
                assert_eq!(
                    pattern.without_start(&text, true),
                    last(&starts).map_or(text.clone(), rest),
                    "{p:?} ## {text:?}"
                );
                assert_eq!(
                    pattern.without_end(&text, false),
                    last(&ends).map_or(text.clone(), kept),
                    "{p:?} % {text:?}"
                );
                assert_eq!(
                    pattern.without_end(&text, true),
                    first(&ends).map_or(text.clone(), kept),
                    "{p:?} %% {text:?}"
                );
            }
        }
    }

    #[test]
    fn capitals_are_other_characters() {
        assert_eq!(matching("abc", &["ABC", "abc"]), [false, true]);
        assert_eq!(matching("[a-z]", &["A", "q"]), [false, true]);
        assert_eq!(matching("*.TXT", &["a.txt", "a.TXT"]), [false, true]);
        assert_eq!(
            pattern("[[:lower:]]*").without_start("aBc", true),
            "",
            "the longest start any lower letter begins"
        );
    }

    #[test]
    fn a_long_text_matches_in_time() {
        let text = "b".repeat(20_000);
        assert!(!pattern("*a").matches(&text));
        assert!(pattern("*b").matches(&text));
        assert!(pattern("b*b*b*").matches(&text));
        assert_eq!(pattern("b").without_start(&text, false).len(), 19_999);
    }
}
