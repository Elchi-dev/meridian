//! An expression read into the tree of what it is made of, by POSIX's
//! rules for basic and extended regular expressions.
//!
//! Where POSIX leaves a choice open, this reads `^` right after `\(` and
//! `$` right before `\)` as anchors, refuses a repetition with nothing to
//! repeat, and refuses escapes other tools give meanings POSIX does not,
//! as `\+` in a basic expression or `\w`, so a script written for them
//! fails here rather than matching something else. `\n` is a line feed.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::set::{self, Set};
use crate::{Code, Error, Syntax};

/// How many times an interval may name at most, POSIX's `RE_DUP_MAX`.
pub const MOST: u32 = 255;

/// How deep groups may nest, and how many repetitions one item may
/// take, so that matching it stays within a program's stack.
pub const MOST_NESTING: usize = 16;
pub const MOST_REPETITIONS: usize = 4;

/// What an expression is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// Matches the empty text.
    Empty,
    Char(char),
    /// `.`: any character.
    Any,
    Set(Set),
    /// `^`: the start of the text.
    Start,
    /// `$`: the end of the text.
    End,
    /// The subexpression numbered first, counted from 1 by where it opens.
    Group(usize, Box<Node>),
    /// Each after the other.
    Concat(Vec<Node>),
    /// Any one of them.
    Alt(Vec<Node>),
    /// The node at least the first count of times and at most the second,
    /// without end for none.
    Repeat(Box<Node>, u32, Option<u32>),
    /// `\n`: the text the subexpression numbered n matched.
    Back(usize),
}

/// An expression read, and how many subexpressions it has.
#[derive(Debug, PartialEq, Eq)]
pub struct Parsed {
    pub tree: Node,
    pub groups: usize,
}

/// Reads `pattern` as `syntax` says.
///
/// # Errors
///
/// Where the expression breaks POSIX's rules, or uses an escape POSIX
/// gives no meaning.
pub fn parse(pattern: &str, syntax: Syntax) -> Result<Parsed, Error> {
    let mut reader = Reader {
        chars: pattern.chars().collect(),
        at: 0,
        groups: 0,
        closed: Vec::new(),
        nesting: 0,
    };
    let tree = match syntax {
        Syntax::Basic => reader.basic(None)?,
        Syntax::Extended => reader.extended(None)?,
    };
    Ok(Parsed {
        tree,
        groups: reader.groups,
    })
}

struct Reader {
    chars: Vec<char>,
    at: usize,
    groups: usize,
    /// The subexpressions that ended so far, which a back-reference may
    /// name.
    closed: Vec<usize>,
    /// How many groups are open around the reader.
    nesting: usize,
}

/// `items` one after the other, as one node.
fn concat(mut items: Vec<Node>) -> Node {
    match items.len() {
        0 => Node::Empty,
        1 => items.remove(0),
        _ => Node::Concat(items),
    }
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn ahead(&self, n: usize) -> Option<char> {
        self.chars.get(self.at + n).copied()
    }

    /// A basic expression up to its end, or to the `\)` that ends the
    /// group whose `\(` is at `open`.
    fn basic(&mut self, open: Option<usize>) -> Result<Node, Error> {
        let mut items = Vec::new();
        if self.peek() == Some('^') {
            self.at += 1;
            items.push(Node::Start);
        }
        loop {
            let at = self.at;
            let Some(c) = self.peek() else {
                return match open {
                    Some(open) => fail(open, Code::Parenthesis),
                    None => Ok(concat(items)),
                };
            };
            let next = self.ahead(1);
            if c == '\\' && next == Some(')') {
                if open.is_none() {
                    return fail(at, Code::Parenthesis);
                }
                self.at += 2;
                return Ok(concat(items));
            }
            let at_end = match (next, self.ahead(2)) {
                (None, _) => true,
                (Some('\\'), Some(')')) => open.is_some(),
                _ => false,
            };
            self.at += 1;
            let node = match c {
                '$' if at_end => Node::End,
                '.' => Node::Any,
                '[' => self.bracket(at)?,
                '*' => match items.pop() {
                    // At the start, or after `\(` or a leading `^`, it is
                    // a character.
                    None => Node::Char('*'),
                    Some(Node::Start) => {
                        items.push(Node::Start);
                        Node::Char('*')
                    }
                    Some(item) => repeat(item, (0, None), at)?,
                },
                '\\' if next == Some('{') => {
                    self.at += 1;
                    let item = repeated(items.pop(), at)?;
                    let counts = self.interval(at, Syntax::Basic)?;
                    repeat(item, counts, at)?
                }
                '\\' => self.basic_escape(at)?,
                c => Node::Char(c),
            };
            items.push(node);
        }
    }

    /// What the backslash at `at` and the character after it stand for
    /// in a basic expression, but for `\)` and `\{`.
    fn basic_escape(&mut self, at: usize) -> Result<Node, Error> {
        let Some(c) = self.peek() else {
            return fail(at, Code::TrailingBackslash);
        };
        self.at += 1;
        match c {
            '(' => self.group(at, Syntax::Basic),
            '1'..='9' => self.back(c, at),
            'n' => Ok(Node::Char('\n')),
            '+' | '?' | '|' | '<' | '>' | '`' | '\'' => fail(at, Code::Escape),
            c if c.is_alphanumeric() => fail(at, Code::Escape),
            c => Ok(Node::Char(c)),
        }
    }

    /// An extended expression up to its end, or to the `)` that ends the
    /// group whose `(` is at `open`.
    fn extended(&mut self, open: Option<usize>) -> Result<Node, Error> {
        let mut branches = Vec::new();
        let mut items = Vec::new();
        loop {
            let at = self.at;
            let Some(c) = self.peek() else {
                if let Some(open) = open {
                    return fail(open, Code::Parenthesis);
                }
                branches.push(concat(items));
                break;
            };
            self.at += 1;
            let node = match c {
                ')' if open.is_none() => return fail(at, Code::Parenthesis),
                ')' => {
                    branches.push(concat(items));
                    break;
                }
                '|' => {
                    branches.push(concat(core::mem::take(&mut items)));
                    continue;
                }
                '*' | '+' | '?' | '{' => {
                    let item = repeated(items.pop(), at)?;
                    let counts = match c {
                        '*' => (0, None),
                        '+' => (1, None),
                        '?' => (0, Some(1)),
                        _ => self.interval(at, Syntax::Extended)?,
                    };
                    repeat(item, counts, at)?
                }
                '(' => self.group(at, Syntax::Extended)?,
                '^' => Node::Start,
                '$' => Node::End,
                '.' => Node::Any,
                '[' => self.bracket(at)?,
                '\\' => self.extended_escape(at)?,
                c => Node::Char(c),
            };
            items.push(node);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Node::Alt(branches)
        })
    }

    /// What the backslash at `at` and the character after it stand for
    /// in an extended expression.
    fn extended_escape(&mut self, at: usize) -> Result<Node, Error> {
        let Some(c) = self.peek() else {
            return fail(at, Code::TrailingBackslash);
        };
        self.at += 1;
        match c {
            '1'..='9' => self.back(c, at),
            'n' => Ok(Node::Char('\n')),
            '<' | '>' | '`' | '\'' => fail(at, Code::Escape),
            c if c.is_alphanumeric() => fail(at, Code::Escape),
            c => Ok(Node::Char(c)),
        }
    }

    /// The group whose opening is at `at`, read up to its end.
    fn group(&mut self, at: usize, syntax: Syntax) -> Result<Node, Error> {
        if self.nesting == MOST_NESTING {
            return fail(at, Code::TooDeep);
        }
        self.nesting += 1;
        self.groups += 1;
        let number = self.groups;
        let inside = match syntax {
            Syntax::Basic => self.basic(Some(at))?,
            Syntax::Extended => self.extended(Some(at))?,
        };
        self.closed.push(number);
        self.nesting -= 1;
        Ok(Node::Group(number, Box::new(inside)))
    }

    /// The back-reference `\digit` at `at`, to a group that ended before.
    fn back(&self, digit: char, at: usize) -> Result<Node, Error> {
        let number = digit as usize - '0' as usize;
        if !self.closed.contains(&number) {
            return fail(at, Code::BackReference);
        }
        Ok(Node::Back(number))
    }

    fn bracket(&mut self, at: usize) -> Result<Node, Error> {
        let (set, after) = set::bracket(&self.chars, at)?;
        self.at = after;
        Ok(Node::Set(set))
    }

    /// The counts of the interval whose opening is at `at`, read past its
    /// `}`, or `\}` in a basic expression.
    fn interval(&mut self, at: usize, syntax: Syntax) -> Result<(u32, Option<u32>), Error> {
        let least = self.number().ok_or(Error {
            at,
            code: Code::Interval,
        })?;
        let most = if self.peek() == Some(',') {
            self.at += 1;
            self.number()
        } else {
            Some(least)
        };
        let end = match syntax {
            Syntax::Basic => ['\\', '}'].as_slice(),
            Syntax::Extended => ['}'].as_slice(),
        };
        let closed = self.chars[self.at..].starts_with(end);
        let counted = least <= MOST && most.is_none_or(|most| least <= most && most <= MOST);
        if !closed || !counted {
            return fail(at, Code::Interval);
        }
        self.at += end.len();
        Ok((least, most))
    }

    /// The decimal number at the reader, none when no digit is there; one
    /// too big for a count comes out bigger than [`MOST`].
    fn number(&mut self) -> Option<u32> {
        let start = self.at;
        let mut value: u32 = 0;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(digit);
            self.at += 1;
        }
        (self.at > start).then_some(value)
    }
}

/// `item` repeated `counts` times, by the repetition at `at`, unless it
/// is repeated [`MOST_REPETITIONS`] times over already.
fn repeat(item: Node, (least, most): (u32, Option<u32>), at: usize) -> Result<Node, Error> {
    let mut repetitions = 0;
    let mut inside = &item;
    while let Node::Repeat(next, ..) = inside {
        repetitions += 1;
        inside = next;
    }
    if repetitions == MOST_REPETITIONS {
        return fail(at, Code::TooDeep);
    }
    Ok(Node::Repeat(Box::new(item), least, most))
}

/// What a repetition at `at` repeats: the item before it, but not an
/// anchor and not nothing.
fn repeated(item: Option<Node>, at: usize) -> Result<Node, Error> {
    match item {
        None | Some(Node::Start | Node::End) => fail(at, Code::NothingToRepeat),
        Some(item) => Ok(item),
    }
}

fn fail<T>(at: usize, code: Code) -> Result<T, Error> {
    Err(Error { at, code })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(text: &str) -> Node {
        parse(text, Syntax::Basic).unwrap().tree
    }

    fn extended(text: &str) -> Node {
        parse(text, Syntax::Extended).unwrap().tree
    }

    fn failed(text: &str, syntax: Syntax) -> (usize, Code) {
        let e = parse(text, syntax).unwrap_err();
        (e.at, e.code)
    }

    fn chars(text: &str) -> Node {
        concat(text.chars().map(Node::Char).collect())
    }

    fn c(one: char) -> Node {
        Node::Char(one)
    }

    fn cat<const N: usize>(items: [Node; N]) -> Node {
        Node::Concat(Vec::from(items))
    }

    fn star(node: Node) -> Node {
        Node::Repeat(Box::new(node), 0, None)
    }

    fn group(number: usize, node: Node) -> Node {
        Node::Group(number, Box::new(node))
    }

    #[test]
    fn ordinary_characters_stand_for_themselves() {
        assert_eq!(basic("abc"), cat([c('a'), c('b'), c('c')]));
        assert_eq!(basic("a"), c('a'));
        assert_eq!(basic(""), Node::Empty);
        assert_eq!(extended(""), Node::Empty);
        assert_eq!(
            basic("ü+?|(){}"),
            chars("ü+?|(){}"),
            "special only in extended ones"
        );
        assert_eq!(extended("ü]}"), chars("ü]}"));
        assert_eq!(basic("a.b"), cat([c('a'), Node::Any, c('b')]));
        assert_eq!(extended("."), Node::Any);
    }

    #[test]
    fn a_star_in_a_basic_expression_repeats_what_comes_before_it() {
        assert_eq!(basic("a*"), star(c('a')));
        assert_eq!(basic("ab*"), cat([c('a'), star(c('b'))]));
        assert_eq!(basic("a**"), star(star(c('a'))));
        assert_eq!(basic(".*"), star(Node::Any));
        assert_eq!(basic("*a"), chars("*a"), "at the start a character");
        assert_eq!(basic("**"), star(c('*')));
        assert_eq!(basic("^*"), cat([Node::Start, c('*')]), "after a leading ^");
        assert_eq!(basic("\\(*a\\)"), group(1, chars("*a")), "after \\(");
        assert_eq!(basic("\\(^*\\)"), group(1, cat([Node::Start, c('*')])));
        assert_eq!(basic("^a*"), cat([Node::Start, star(c('a'))]));
    }

    #[test]
    fn anchors_in_a_basic_expression_hold_only_at_its_ends() {
        assert_eq!(basic("^a$"), cat([Node::Start, c('a'), Node::End]));
        assert_eq!(basic("a^b"), chars("a^b"));
        assert_eq!(basic("a$b"), chars("a$b"));
        assert_eq!(basic("$a"), chars("$a"));
        assert_eq!(basic("^"), Node::Start);
        assert_eq!(basic("$"), Node::End);
        assert_eq!(basic("^^"), cat([Node::Start, c('^')]));
        assert_eq!(basic("$$"), cat([c('$'), Node::End]));
        assert_eq!(
            basic("x\\(^a$\\)y"),
            cat([
                c('x'),
                group(1, cat([Node::Start, c('a'), Node::End])),
                c('y')
            ]),
            "a group's ends too"
        );
        assert_eq!(basic("a$*"), cat([c('a'), star(c('$'))]));
    }

    #[test]
    fn groups_are_numbered_by_where_they_open() {
        let want = cat([group(1, cat([c('a'), group(2, c('b'))])), group(3, c('c'))]);
        let parsed = parse("\\(a\\(b\\)\\)\\(c\\)", Syntax::Basic).unwrap();
        assert_eq!((parsed.tree, parsed.groups), (want.clone(), 3));
        let parsed = parse("(a(b))(c)", Syntax::Extended).unwrap();
        assert_eq!((parsed.tree, parsed.groups), (want, 3));
        assert_eq!(extended("()"), group(1, Node::Empty));
        assert_eq!(basic("\\(\\)"), group(1, Node::Empty));
        assert_eq!(parse("abc", Syntax::Basic).unwrap().groups, 0);
        assert_eq!(basic("\\(a\\)*"), star(group(1, c('a'))));
        assert_eq!(extended("(a)*"), star(group(1, c('a'))));
    }

    #[test]
    fn a_back_reference_names_a_group_that_ended_before_it() {
        assert_eq!(basic("\\(a\\)\\1"), cat([group(1, c('a')), Node::Back(1)]));
        assert_eq!(
            extended("(a)(b)\\2\\1"),
            cat([
                group(1, c('a')),
                group(2, c('b')),
                Node::Back(2),
                Node::Back(1)
            ])
        );
        assert_eq!(
            basic("\\(a\\)\\(b\\1\\)"),
            cat([group(1, c('a')), group(2, cat([c('b'), Node::Back(1)]))])
        );
        assert_eq!(
            basic("\\(\\(a\\)\\2\\)"),
            group(1, cat([group(2, c('a')), Node::Back(2)])),
            "an inner group ends first"
        );
        assert_eq!(failed("\\1", Syntax::Basic), (0, Code::BackReference));
        assert_eq!(
            failed("\\(a\\1\\)", Syntax::Basic),
            (3, Code::BackReference),
            "its own group has not ended"
        );
        assert_eq!(
            failed("\\(a\\)\\2", Syntax::Basic),
            (5, Code::BackReference)
        );
        assert_eq!(failed("(a)\\2", Syntax::Extended), (3, Code::BackReference));
    }

    #[test]
    fn an_interval_counts_what_comes_before_it() {
        let a = |least, most| Node::Repeat(Box::new(c('a')), least, most);
        assert_eq!(basic("a\\{2\\}"), a(2, Some(2)));
        assert_eq!(basic("a\\{2,\\}"), a(2, None));
        assert_eq!(basic("a\\{2,5\\}"), a(2, Some(5)));
        assert_eq!(basic("a\\{0,255\\}"), a(0, Some(255)));
        assert_eq!(basic("a\\{3,3\\}"), a(3, Some(3)));
        assert_eq!(extended("a{3}"), a(3, Some(3)));
        assert_eq!(extended("a{0,}"), a(0, None));
        assert_eq!(extended("a+"), a(1, None));
        assert_eq!(extended("a?"), a(0, Some(1)));
        assert_eq!(extended("a*"), a(0, None));
        assert_eq!(
            extended("a+?"),
            Node::Repeat(Box::new(a(1, None)), 0, Some(1))
        );
        assert_eq!(
            extended("(ab){1,2}c"),
            cat([
                Node::Repeat(Box::new(group(1, chars("ab"))), 1, Some(2)),
                c('c')
            ])
        );
        assert_eq!(basic("a\\{1\\}b"), cat([a(1, Some(1)), c('b')]));
    }

    #[test]
    fn an_interval_read_wrong_says_where_it_starts() {
        for text in [
            "a\\{2",
            "a\\{2}",
            "a\\{,2\\}",
            "a\\{3,2\\}",
            "a\\{256\\}",
            "a\\{1,256\\}",
            "a\\{99999999999\\}",
            "a\\{2 \\}",
            "a\\{\\}",
            "a\\{x\\}",
        ] {
            assert_eq!(failed(text, Syntax::Basic), (1, Code::Interval), "{text}");
        }
        for text in ["a{2", "a{x}", "a{}", "a{2\\}", "a{1,0}", "a{256,}"] {
            assert_eq!(
                failed(text, Syntax::Extended),
                (1, Code::Interval),
                "{text}"
            );
        }
        assert_eq!(failed("\\{2\\}", Syntax::Basic), (0, Code::NothingToRepeat));
        assert_eq!(
            failed("^\\{2\\}", Syntax::Basic),
            (1, Code::NothingToRepeat)
        );
    }

    #[test]
    fn an_extended_expression_takes_any_of_its_branches() {
        assert_eq!(extended("a|b"), Node::Alt(Vec::from([c('a'), c('b')])));
        assert_eq!(
            extended("ab|cd|e"),
            Node::Alt(Vec::from([chars("ab"), chars("cd"), c('e')]))
        );
        assert_eq!(
            extended("a(b|c)d"),
            cat([
                c('a'),
                group(1, Node::Alt(Vec::from([c('b'), c('c')]))),
                c('d')
            ])
        );
        assert_eq!(extended("a|"), Node::Alt(Vec::from([c('a'), Node::Empty])));
        assert_eq!(
            extended("|"),
            Node::Alt(Vec::from([Node::Empty, Node::Empty]))
        );
        assert_eq!(
            extended("^a|b$"),
            Node::Alt(Vec::from([
                cat([Node::Start, c('a')]),
                cat([c('b'), Node::End])
            ]))
        );
        assert_eq!(
            extended("a^b$c"),
            cat([c('a'), Node::Start, c('b'), Node::End, c('c')]),
            "anchors anywhere"
        );
    }

    #[test]
    fn escapes_stand_for_the_character_unless_posix_leaves_them_open() {
        assert_eq!(basic("\\.\\*\\[\\]\\\\\\^\\$"), chars(".*[]\\^$"));
        assert_eq!(basic("\\/\\-\\}"), chars("/-}"));
        assert_eq!(basic("a\\nb"), chars("a\nb"));
        assert_eq!(extended("\\(\\)\\|\\+\\?\\{\\}\\.\\*"), chars("()|+?{}.*"));
        assert_eq!(extended("\\n"), c('\n'));
        assert_eq!(extended("\\/"), c('/'));
        for text in [
            "\\+", "\\?", "\\|", "\\<", "\\>", "\\w", "\\s", "\\b", "\\0", "\\`", "\\'",
        ] {
            assert_eq!(failed(text, Syntax::Basic), (0, Code::Escape), "{text}");
        }
        for text in ["\\<", "\\>", "\\w", "\\B", "\\0", "\\`", "\\'", "\\ä"] {
            assert_eq!(failed(text, Syntax::Extended), (0, Code::Escape), "{text}");
        }
        assert_eq!(failed("ab\\", Syntax::Basic), (2, Code::TrailingBackslash));
        assert_eq!(failed("\\", Syntax::Extended), (0, Code::TrailingBackslash));
    }

    #[test]
    fn parentheses_pair() {
        assert_eq!(failed("\\(a", Syntax::Basic), (0, Code::Parenthesis));
        assert_eq!(failed("a\\)", Syntax::Basic), (1, Code::Parenthesis));
        assert_eq!(failed("\\(\\(a\\)", Syntax::Basic), (0, Code::Parenthesis));
        assert_eq!(
            failed("a$\\)", Syntax::Basic),
            (2, Code::Parenthesis),
            "$ before a \\) that ends nothing is a character"
        );
        assert_eq!(failed("(a", Syntax::Extended), (0, Code::Parenthesis));
        assert_eq!(failed("a)", Syntax::Extended), (1, Code::Parenthesis));
        assert_eq!(failed("((a)", Syntax::Extended), (0, Code::Parenthesis));
        assert_eq!(failed("(a))", Syntax::Extended), (3, Code::Parenthesis));
        assert_eq!(failed("x(a|(b)", Syntax::Extended), (1, Code::Parenthesis));
    }

    #[test]
    fn a_repetition_needs_something_to_repeat() {
        for (text, at) in [
            ("*a", 0),
            ("+", 0),
            ("?", 0),
            ("{1}", 0),
            ("a|*b", 2),
            ("(*a)", 1),
            ("^*", 1),
            ("$?", 1),
            ("a$+", 2),
        ] {
            assert_eq!(
                failed(text, Syntax::Extended),
                (at, Code::NothingToRepeat),
                "{text}"
            );
        }
    }

    #[test]
    fn brackets_are_read_as_sets() {
        let set = |items: &[set::Item]| {
            Node::Set(Set {
                negated: false,
                items: Vec::from(items),
            })
        };
        assert_eq!(
            basic("[ab]c"),
            cat([set(&[set::Item::One('a'), set::Item::One('b')]), c('c')])
        );
        assert_eq!(
            extended("[a|b]+"),
            Node::Repeat(
                Box::new(set(&[
                    set::Item::One('a'),
                    set::Item::One('|'),
                    set::Item::One('b')
                ])),
                1,
                None
            )
        );
        assert_eq!(
            basic("a[]*]"),
            cat([c('a'), set(&[set::Item::One(']'), set::Item::One('*')])])
        );
        assert_eq!(basic("[.]*"), star(set(&[set::Item::One('.')])));
        assert_eq!(failed("[a", Syntax::Basic), (0, Code::OpenBracket));
        assert_eq!(failed("x[", Syntax::Extended), (1, Code::OpenBracket));
        assert_eq!(failed("x[b-a]", Syntax::Extended), (4, Code::Range));
    }

    #[test]
    fn both_syntaxes_read_the_same_expression_into_the_same_tree() {
        for (basic_text, extended_text) in [
            ("\\(a\\)*", "(a)*"),
            ("a\\{2,3\\}", "a{2,3}"),
            ("a\\{2,\\}", "a{2,}"),
            ("a\\{0,1\\}", "a?"),
            ("a\\{1,\\}", "a+"),
            ("^\\(ab\\)\\1$", "^(ab)\\1$"),
            ("[[:digit:]]*x", "[[:digit:]]*x"),
            ("a+b?c|d", "a\\+b\\?c\\|d"),
            ("(x){y}", "\\(x\\)\\{y\\}"),
            ("\\(\\(a\\)b\\)", "((a)b)"),
            (".\\{3\\}", ".{3}"),
            ("\\.\\*", "\\.\\*"),
            ("a\\nb", "a\\nb"),
        ] {
            assert_eq!(
                parse(basic_text, Syntax::Basic),
                parse(extended_text, Syntax::Extended),
                "{basic_text} and {extended_text}"
            );
        }
    }

    #[test]
    fn a_place_counts_characters_not_bytes() {
        assert_eq!(failed("ä\\(", Syntax::Basic), (1, Code::Parenthesis));
        assert_eq!(failed("ü€[", Syntax::Extended), (2, Code::OpenBracket));
        assert_eq!(failed("日本*{", Syntax::Extended), (3, Code::Interval));
        assert_eq!(failed("äö\\", Syntax::Basic), (2, Code::TrailingBackslash));
        assert_eq!(failed("é(x))", Syntax::Extended), (4, Code::Parenthesis));
    }

    #[test]
    fn groups_count_across_branches_and_repetitions() {
        let parsed = parse("(a)|(b)", Syntax::Extended).unwrap();
        assert_eq!(parsed.groups, 2);
        assert_eq!(
            parsed.tree,
            Node::Alt(Vec::from([group(1, c('a')), group(2, c('b'))]))
        );
        assert_eq!(
            basic("\\(a*\\)\\{2\\}"),
            Node::Repeat(Box::new(group(1, star(c('a')))), 2, Some(2))
        );
        assert_eq!(
            extended("((a|b)*c)+"),
            Node::Repeat(
                Box::new(group(
                    1,
                    cat([
                        star(group(2, Node::Alt(Vec::from([c('a'), c('b')])))),
                        c('c')
                    ])
                )),
                1,
                None
            )
        );
        let nine = "\\(a\\)".repeat(9) + "\\9";
        let parsed = parse(&nine, Syntax::Basic).unwrap();
        assert_eq!(parsed.groups, 9);
        assert!(
            matches!(&parsed.tree, Node::Concat(items) if items.last() == Some(&Node::Back(9)))
        );
    }

    #[test]
    fn a_basic_interval_repeats_any_one_item_before_it() {
        let repeat = |node, least, most| Node::Repeat(Box::new(node), least, most);
        assert_eq!(basic(".\\{3\\}"), repeat(Node::Any, 3, Some(3)));
        assert_eq!(
            basic("ab\\{2\\}"),
            cat([c('a'), repeat(c('b'), 2, Some(2))]),
            "only the last character"
        );
        assert_eq!(basic("a*\\{2\\}"), repeat(star(c('a')), 2, Some(2)));
        assert_eq!(
            basic("\\(a\\)\\1\\{2\\}"),
            cat([group(1, c('a')), repeat(Node::Back(1), 2, Some(2))])
        );
        assert_eq!(basic("a\\{1\\}*"), star(repeat(c('a'), 1, Some(1))));
        assert_eq!(
            basic("\\(^a\\{2\\}\\)"),
            group(1, cat([Node::Start, repeat(c('a'), 2, Some(2))]))
        );
    }

    #[test]
    fn a_dollar_ends_a_group_as_it_ends_the_expression() {
        assert_eq!(
            basic("\\(a$\\)b"),
            cat([group(1, cat([c('a'), Node::End])), c('b')])
        );
        assert_eq!(basic("\\($\\)"), group(1, Node::End));
        assert_eq!(
            basic("\\(a$b\\)"),
            group(1, chars("a$b")),
            "inside it a character"
        );
        assert_eq!(
            basic("a\\$"),
            chars("a$"),
            "escaped, a character at the end too"
        );
        assert_eq!(basic("\\^a"), chars("^a"));
    }

    #[test]
    fn an_extended_group_holds_its_own_anchors_and_branches() {
        assert_eq!(extended("(^a)"), group(1, cat([Node::Start, c('a')])));
        assert_eq!(
            extended("(a|^b)c"),
            cat([
                group(
                    1,
                    Node::Alt(Vec::from([c('a'), cat([Node::Start, c('b')])]))
                ),
                c('c')
            ])
        );
        assert_eq!(
            extended("(|a)"),
            group(1, Node::Alt(Vec::from([Node::Empty, c('a')])))
        );
        assert_eq!(extended("a(b)"), cat([c('a'), group(1, c('b'))]));
        assert_eq!(
            extended("(a)(b)"),
            cat([group(1, c('a')), group(2, c('b'))])
        );
    }

    #[test]
    fn an_interval_counts_up_to_255_at_either_end() {
        let a = |least, most| Node::Repeat(Box::new(c('a')), least, most);
        assert_eq!(basic("a\\{255\\}"), a(255, Some(255)));
        assert_eq!(basic("a\\{255,\\}"), a(255, None));
        assert_eq!(extended("a{255,255}"), a(255, Some(255)));
        assert_eq!(extended("a{0}"), a(0, Some(0)));
        assert_eq!(extended("a{007}"), a(7, Some(7)), "leading zeros");
        for text in ["a{256}", "a{255,256}", "a{256,256}"] {
            assert_eq!(
                failed(text, Syntax::Extended),
                (1, Code::Interval),
                "{text}"
            );
        }
    }

    #[test]
    fn a_bracket_is_read_whole_wherever_it_stands() {
        let lower = Node::Set(Set {
            negated: false,
            items: Vec::from([set::Item::Range('a', 'z')]),
        });
        assert_eq!(basic("\\([a-z]*\\)"), group(1, star(lower.clone())));
        assert_eq!(
            extended("x|[a-z]{2}"),
            Node::Alt(Vec::from([
                c('x'),
                Node::Repeat(Box::new(lower.clone()), 2, Some(2))
            ]))
        );
        assert_eq!(
            basic("^[a-z]$"),
            cat([Node::Start, lower, Node::End]),
            "a ] before $ ends the set, and $ the expression"
        );
        assert_eq!(
            failed("[(]\\)", Syntax::Basic),
            (3, Code::Parenthesis),
            "a ( in a set opens no group for the \\) to end"
        );
    }

    #[test]
    fn groups_nest_and_repetitions_pile_up_only_so_far() {
        let nested = |depth| "(".repeat(depth) + "a" + &")".repeat(depth);
        assert!(parse(&nested(MOST_NESTING), Syntax::Extended).is_ok());
        assert_eq!(
            failed(&nested(MOST_NESTING + 1), Syntax::Extended),
            (MOST_NESTING, Code::TooDeep),
            "the group one too deep"
        );
        assert_eq!(
            failed(&"(".repeat(10_000), Syntax::Extended),
            (MOST_NESTING, Code::TooDeep),
            "long before the stack would end"
        );
        assert_eq!(
            failed(&"\\(".repeat(MOST_NESTING + 1), Syntax::Basic),
            (2 * MOST_NESTING, Code::TooDeep)
        );
        assert!(
            parse(&"(a)".repeat(100), Syntax::Extended).is_ok(),
            "side by side they do not nest"
        );
        assert!(parse("a****", Syntax::Basic).is_ok());
        assert_eq!(failed("a*****", Syntax::Basic), (5, Code::TooDeep));
        assert_eq!(failed("a+*?{2}+", Syntax::Extended), (7, Code::TooDeep));
        assert_eq!(
            failed("a\\{2\\}\\{2\\}***", Syntax::Basic),
            (13, Code::TooDeep)
        );
        assert!(parse("(a*)*(b+)+", Syntax::Extended).is_ok());
    }
}
