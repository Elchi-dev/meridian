//! Bracket expressions: `[abc]`, ranges as `a-z`, classes as
//! `[:digit:]`, and `[^...]` for every character they leave out, read as
//! POSIX has them in its own locale.

use alloc::vec::Vec;

use crate::{Code, Error, lower, upper};

/// The characters a bracket expression holds, or with `^` all others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Set {
    pub negated: bool,
    pub items: Vec<Item>,
}

/// One part of a bracket expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    One(char),
    /// Every character from the first to the second, by their values.
    Range(char, char),
    Class(Class),
}

/// A character class, as POSIX's own locale has it: ASCII.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
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

const CLASSES: [(&str, Class); 12] = [
    ("alnum", Class::Alnum),
    ("alpha", Class::Alpha),
    ("blank", Class::Blank),
    ("cntrl", Class::Cntrl),
    ("digit", Class::Digit),
    ("graph", Class::Graph),
    ("lower", Class::Lower),
    ("print", Class::Print),
    ("punct", Class::Punct),
    ("space", Class::Space),
    ("upper", Class::Upper),
    ("xdigit", Class::Xdigit),
];

impl Class {
    /// Whether the class holds `c`.
    #[must_use]
    pub fn holds(self, c: char) -> bool {
        match self {
            Class::Alnum => c.is_ascii_alphanumeric(),
            Class::Alpha => c.is_ascii_alphabetic(),
            Class::Blank => c == ' ' || c == '\t',
            Class::Cntrl => c.is_ascii_control(),
            Class::Digit => c.is_ascii_digit(),
            Class::Graph => c.is_ascii_graphic(),
            Class::Lower => c.is_ascii_lowercase(),
            Class::Print => c.is_ascii_graphic() || c == ' ',
            Class::Punct => c.is_ascii_punctuation(),
            Class::Space => c.is_ascii_whitespace() || c == '\u{b}',
            Class::Upper => c.is_ascii_uppercase(),
            Class::Xdigit => c.is_ascii_hexdigit(),
        }
    }
}

impl Set {
    /// Whether the set takes the character `c`, in either case when
    /// `either_case`; none stands for a byte that is no UTF-8, which only
    /// a set with `^` takes.
    #[must_use]
    pub fn takes(&self, c: Option<char>, either_case: bool) -> bool {
        let inside = c.is_some_and(|c| {
            self.holds(c) || either_case && (self.holds(lower(c)) || self.holds(upper(c)))
        });
        inside != self.negated
    }

    fn holds(&self, c: char) -> bool {
        self.items.iter().any(|item| match *item {
            Item::One(one) => one == c,
            Item::Range(first, last) => (first..=last).contains(&c),
            Item::Class(class) => class.holds(c),
        })
    }
}

/// The bracket expression whose `[` is `chars[open]`, and where the
/// expression goes on after its `]`.
///
/// # Errors
///
/// For one without its `]`, a range that ends before it starts, and a
/// class, an equivalence class or a collating element this locale does
/// not have, which is any of more than one character.
pub fn bracket(chars: &[char], open: usize) -> Result<(Set, usize), Error> {
    let fail = |at, code| Err(Error { at, code });
    let mut at = open + 1;
    let negated = chars.get(at) == Some(&'^');
    if negated {
        at += 1;
    }
    let mut items = Vec::new();
    // A `]` right at the start is one of the characters.
    let mut first = true;
    loop {
        let Some(&c) = chars.get(at) else {
            return fail(open, Code::OpenBracket);
        };
        if c == ']' && !first {
            return Ok((Set { negated, items }, at + 1));
        }
        first = false;
        let (start, after) = match element(chars, at)? {
            Element::Class(class, after) => {
                items.push(Item::Class(class));
                at = after;
                continue;
            }
            Element::One(c, after) => (c, after),
        };
        at = after;
        let ranged = chars.get(at) == Some(&'-') && chars.get(at + 1).is_some_and(|&c| c != ']');
        if !ranged {
            items.push(Item::One(start));
            continue;
        }
        let Element::One(end, after) = element(chars, at + 1)? else {
            return fail(at + 1, Code::Range);
        };
        if end < start {
            return fail(at + 1, Code::Range);
        }
        items.push(Item::Range(start, end));
        at = after;
    }
}

/// What one element of a bracket expression stands for, and where the
/// next starts.
enum Element {
    One(char, usize),
    Class(Class, usize),
}

/// The element at `chars[at]`: a character, `[:class:]`, or `[=c=]` and
/// `[.c.]` for the one character `c`.
fn element(chars: &[char], at: usize) -> Result<Element, Error> {
    let c = chars[at];
    let kind = match chars.get(at + 1) {
        Some(&kind @ (':' | '=' | '.')) if c == '[' => kind,
        _ => return Ok(Element::One(c, at + 1)),
    };
    let inside = at + 2;
    let end = (inside..chars.len().saturating_sub(1))
        .find(|&i| chars[i] == kind && chars[i + 1] == ']')
        .ok_or(Error {
            at,
            code: Code::OpenBracket,
        })?;
    let name = &chars[inside..end];
    let after = end + 2;
    match (kind, name) {
        (':', _) => CLASSES
            .iter()
            .find(|(known, _)| known.chars().eq(name.iter().copied()))
            .map(|&(_, class)| Element::Class(class, after))
            .ok_or(Error {
                at,
                code: Code::Class,
            }),
        (_, &[one]) => Ok(Element::One(one, after)),
        _ => Err(Error {
            at,
            code: Code::Class,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Result<(Set, usize), Error> {
        let chars: Vec<char> = text.chars().collect();
        bracket(&chars, 0)
    }

    fn set(text: &str) -> Set {
        read(text).unwrap().0
    }

    fn failed(text: &str) -> (usize, Code) {
        let e = read(text).unwrap_err();
        (e.at, e.code)
    }

    #[test]
    fn a_set_holds_its_characters_and_ranges() {
        assert_eq!(
            read("[ab]x"),
            Ok((
                Set {
                    negated: false,
                    items: Vec::from([Item::One('a'), Item::One('b')]),
                },
                4
            ))
        );
        assert_eq!(set("[a-z]").items, [Item::Range('a', 'z')]);
        assert_eq!(set("[a-a]").items, [Item::Range('a', 'a')]);
        assert_eq!(
            set("[0-9A-F]").items,
            [Item::Range('0', '9'), Item::Range('A', 'F')]
        );
        assert!(set("[^a]").negated);
        assert_eq!(set("[^a]").items, [Item::One('a')]);
        assert_eq!(set("[ä-ü]").items, [Item::Range('ä', 'ü')]);
    }

    #[test]
    fn a_bracket_or_dash_where_nothing_else_fits_is_a_character() {
        assert_eq!(set("[]a]").items, [Item::One(']'), Item::One('a')]);
        assert_eq!(set("[^]a]").items, [Item::One(']'), Item::One('a')]);
        assert_eq!(set("[-a]").items, [Item::One('-'), Item::One('a')]);
        assert_eq!(set("[a-]").items, [Item::One('a'), Item::One('-')]);
        assert_eq!(set("[]-a]").items, [Item::Range(']', 'a')]);
        assert_eq!(
            set("[\\n]").items,
            [Item::One('\\'), Item::One('n')],
            "no escapes"
        );
        assert_eq!(set("[[]").items, [Item::One('[')]);
        assert_eq!(
            set("[a[b]").items,
            [Item::One('a'), Item::One('['), Item::One('b')]
        );
        assert_eq!(read("[]]").map(|(_, after)| after), Ok(3));
    }

    #[test]
    fn classes_and_one_character_elements_are_read_by_name() {
        assert_eq!(set("[[:digit:]]").items, [Item::Class(Class::Digit)]);
        assert_eq!(
            set("[[:alpha:]_]").items,
            [Item::Class(Class::Alpha), Item::One('_')]
        );
        for (name, class) in CLASSES {
            assert_eq!(
                set(&alloc::format!("[[:{name}:]]")).items,
                [Item::Class(class)]
            );
        }
        assert_eq!(set("[[=a=]]").items, [Item::One('a')]);
        assert_eq!(set("[[.-.]]").items, [Item::One('-')]);
        assert_eq!(set("[[.a.]-c]").items, [Item::Range('a', 'c')]);
        assert_eq!(set("[a-[.c.]]").items, [Item::Range('a', 'c')]);
        assert_eq!(set("[[.].]]").items, [Item::One(']')]);
    }

    #[test]
    fn a_set_read_wrong_says_where() {
        assert_eq!(failed("[a"), (0, Code::OpenBracket));
        assert_eq!(failed("["), (0, Code::OpenBracket));
        assert_eq!(failed("[^"), (0, Code::OpenBracket));
        assert_eq!(failed("[]"), (0, Code::OpenBracket), "the ] is a character");
        assert_eq!(failed("[z-a]"), (3, Code::Range));
        assert_eq!(failed("[a-[:digit:]]"), (3, Code::Range));
        assert_eq!(failed("[[:digits:]]"), (1, Code::Class));
        assert_eq!(failed("[[:DIGIT:]]"), (1, Code::Class));
        assert_eq!(failed("[[=ab=]]"), (1, Code::Class));
        assert_eq!(failed("[[.ch.]]"), (1, Code::Class));
        assert_eq!(failed("[[..]]"), (1, Code::Class));
        assert_eq!(
            failed("[[.a.b.]]"),
            (1, Code::Class),
            "the element ends at the first .] only"
        );
        assert_eq!(failed("[[:a]:]]"), (1, Code::Class));
        assert_eq!(failed("[[:alpha]"), (1, Code::OpenBracket));
        assert_eq!(failed("[[:"), (1, Code::OpenBracket));
    }

    #[test]
    fn a_set_takes_what_it_holds_and_with_a_caret_the_rest() {
        let digits = set("[0-9x]");
        assert!(digits.takes(Some('0'), false));
        assert!(digits.takes(Some('9'), false));
        assert!(digits.takes(Some('x'), false));
        assert!(!digits.takes(Some('/'), false), "just below the range");
        assert!(!digits.takes(Some(':'), false), "just above");
        assert!(!digits.takes(None, false));
        let others = set("[^0-9]");
        assert!(!others.takes(Some('5'), false));
        assert!(others.takes(Some('a'), false));
        assert!(others.takes(None, false), "a byte that is no UTF-8");
    }

    #[test]
    fn either_case_takes_a_character_whose_other_case_is_held() {
        let lower = set("[a-c]");
        assert!(!lower.takes(Some('B'), false));
        assert!(lower.takes(Some('B'), true));
        let upper = set("[[:upper:]]");
        assert!(!upper.takes(Some('q'), false));
        assert!(upper.takes(Some('q'), true));
        assert!(set("[ü]").takes(Some('Ü'), true));
        assert!(!set("[ü]").takes(Some('u'), true));
        assert!(
            !set("[^a]").takes(Some('A'), true),
            "A is a, which it leaves out"
        );
    }

    #[test]
    fn each_class_holds_what_posix_puts_in_it() {
        let held = |class: Class| -> alloc::string::String {
            (0u8..128)
                .map(char::from)
                .filter(|&c| class.holds(c))
                .collect()
        };
        assert_eq!(held(Class::Digit), "0123456789");
        assert_eq!(held(Class::Xdigit), "0123456789ABCDEFabcdef");
        assert_eq!(held(Class::Blank), "\t ");
        assert_eq!(held(Class::Space), "\t\n\u{b}\u{c}\r ");
        assert_eq!(held(Class::Upper).len(), 26);
        assert_eq!(held(Class::Lower).len(), 26);
        assert_eq!(held(Class::Alpha).len(), 52);
        assert_eq!(held(Class::Alnum).len(), 62);
        assert_eq!(held(Class::Punct).len(), 32);
        assert_eq!(held(Class::Graph).len(), 94);
        assert_eq!(held(Class::Print).len(), 95);
        assert_eq!(held(Class::Cntrl).len(), 33);
        assert!(!Class::Alpha.holds('é'), "POSIX's locale is ASCII");
    }

    #[test]
    fn a_range_goes_by_the_values_of_its_ends() {
        let punctuation = set("[+--]");
        assert_eq!(punctuation.items, [Item::Range('+', '-')]);
        assert!(punctuation.takes(Some(','), false), "between + and -");
        assert!(!punctuation.takes(Some('.'), false));
        let wide = set("[α-ω]");
        assert!(wide.takes(Some('λ'), false));
        assert!(!wide.takes(Some('a'), false));
        assert!(!wide.takes(Some('Λ'), false));
        assert!(wide.takes(Some('Λ'), true), "Λ is λ in the other case");
    }

    #[test]
    fn either_case_reaches_across_ranges_and_classes() {
        let upper = set("[A-Z]");
        assert!(upper.takes(Some('q'), true));
        assert!(!upper.takes(Some('q'), false));
        assert!(!upper.takes(Some('1'), true));
        let not_lower = set("[^[:lower:]]");
        assert!(not_lower.takes(Some('Q'), false));
        assert!(
            !not_lower.takes(Some('Q'), true),
            "Q is q in the other case"
        );
        assert!(not_lower.takes(Some('1'), true));
        let sharp = set("[ß]");
        assert!(sharp.takes(Some('ß'), true));
        assert!(!sharp.takes(Some('S'), true), "SS is not one character");
    }

    #[test]
    fn a_set_holds_its_items_in_the_order_written() {
        assert_eq!(
            set("[a-cx[:digit:]-]").items,
            [
                Item::Range('a', 'c'),
                Item::One('x'),
                Item::Class(Class::Digit),
                Item::One('-')
            ]
        );
        assert_eq!(
            set("[^-a-c]").items,
            [Item::One('-'), Item::Range('a', 'c')]
        );
        assert_eq!(
            read("[[:space:][:punct:]]x").map(|(set, after)| (set.items.len(), after)),
            Ok((2, 20))
        );
    }

    #[test]
    fn no_class_holds_a_character_past_ascii() {
        for (name, class) in CLASSES {
            for c in ['é', 'ß', 'Ä', '٣', '\u{a0}', '\u{2028}', '日'] {
                assert!(!class.holds(c), "{name} holds {c:?}");
            }
        }
    }
}
