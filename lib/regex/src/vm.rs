//! The Pike VM: an expression compiled to a few instructions and run over
//! a text once, every way through the program at the same time. For each
//! instruction it keeps only the thread that started furthest left, so
//! the match it finds is POSIX's: of those that start leftmost, the
//! longest.

use alloc::vec::Vec;

use crate::parse::Node;
use crate::same;
use crate::set::Set;

/// How many instructions a program may have once its intervals are
/// written out; an expression that needs more is refused.
pub const MOST_OPS: usize = 1 << 16;

/// One instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Char(char),
    Any,
    Set(Set),
    /// Goes on only at the start of the text.
    Start,
    /// Goes on only at its end.
    End,
    /// Goes on at both.
    Split(usize, usize),
    Jump(usize),
    Match,
}

/// Why an expression has no program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// It needs more than [`MOST_OPS`] instructions.
    TooBig,
    /// It has a back-reference, which no program of these instructions
    /// can match.
    BackReference,
}

/// An expression compiled.
#[derive(Debug)]
pub struct Program {
    ops: Vec<Op>,
    either_case: bool,
}

/// The program for `tree`, which matches characters in either case when
/// `either_case`.
///
/// # Errors
///
/// For a tree that needs more than [`MOST_OPS`] instructions or holds a
/// back-reference.
pub fn compile(tree: &Node, either_case: bool) -> Result<Program, Refused> {
    let mut ops = Vec::new();
    emit(tree, &mut ops)?;
    push(&mut ops, Op::Match)?;
    Ok(Program { ops, either_case })
}

fn push(ops: &mut Vec<Op>, op: Op) -> Result<usize, Refused> {
    if ops.len() == MOST_OPS {
        return Err(Refused::TooBig);
    }
    ops.push(op);
    Ok(ops.len() - 1)
}

fn emit(node: &Node, ops: &mut Vec<Op>) -> Result<(), Refused> {
    match node {
        Node::Empty => {}
        Node::Char(c) => {
            push(ops, Op::Char(*c))?;
        }
        Node::Any => {
            push(ops, Op::Any)?;
        }
        Node::Set(set) => {
            push(ops, Op::Set(set.clone()))?;
        }
        Node::Start => {
            push(ops, Op::Start)?;
        }
        Node::End => {
            push(ops, Op::End)?;
        }
        Node::Group(_, inside) => emit(inside, ops)?,
        Node::Concat(items) => {
            for item in items {
                emit(item, ops)?;
            }
        }
        Node::Alt(branches) => {
            let mut ends = Vec::new();
            let Some((last, rest)) = branches.split_last() else {
                return Ok(());
            };
            for branch in rest {
                let split = push(ops, Op::Split(0, 0))?;
                emit(branch, ops)?;
                ends.push(push(ops, Op::Jump(0))?);
                ops[split] = Op::Split(split + 1, ops.len());
            }
            emit(last, ops)?;
            for end in ends {
                ops[end] = Op::Jump(ops.len());
            }
        }
        Node::Repeat(inside, least, most) => {
            for _ in 0..*least {
                emit(inside, ops)?;
            }
            match most {
                None => {
                    let split = push(ops, Op::Split(0, 0))?;
                    emit(inside, ops)?;
                    push(ops, Op::Jump(split))?;
                    ops[split] = Op::Split(split + 1, ops.len());
                }
                Some(most) => {
                    // Once one more is left out, so are the rest.
                    let mut splits = Vec::new();
                    for _ in *least..*most {
                        splits.push(push(ops, Op::Split(0, 0))?);
                        emit(inside, ops)?;
                    }
                    for split in splits {
                        ops[split] = Op::Split(split + 1, ops.len());
                    }
                }
            }
        }
        Node::Back(_) => return Err(Refused::BackReference),
    }
    Ok(())
}

/// The threads at one place in the text: each instruction they wait at
/// and where they started, those that started further left first.
struct Threads {
    list: Vec<(usize, usize)>,
    /// For each instruction, the place a thread last reached it plus one,
    /// so that a later thread there at the same place is dropped.
    seen: Vec<usize>,
}

impl Program {
    /// The leftmost-longest match among those that start at `from` or
    /// after, in `chars`, a text as its characters, none for a byte that
    /// is no UTF-8: the place of its first character and of the one after
    /// its last. `^` holds only where the text starts.
    #[must_use]
    pub fn find(&self, chars: &[Option<char>], from: usize) -> Option<(usize, usize)> {
        let mut now = Threads {
            list: Vec::new(),
            seen: alloc::vec![0; self.ops.len()],
        };
        let mut next = Threads {
            list: Vec::new(),
            seen: now.seen.clone(),
        };
        let mut stack = Vec::new();
        let mut best: Option<(usize, usize)> = None;
        for at in from..=chars.len() {
            if best.is_none() {
                self.add(&mut now, &mut stack, (0, at), at, chars.len());
            } else if now.list.is_empty() {
                break;
            }
            next.list.clear();
            for &(pc, start) in &now.list {
                if best.is_some_and(|(first, _)| start > first) {
                    break;
                }
                let takes = match (&self.ops[pc], chars.get(at)) {
                    (Op::Match, _) => {
                        best = Some((start, at));
                        continue;
                    }
                    (Op::Char(want), Some(&c)) => same(c, *want, self.either_case),
                    (Op::Any, Some(_)) => true,
                    (Op::Set(set), Some(&c)) => set.takes(c, self.either_case),
                    // At the end of the text nothing more is taken.
                    _ => continue,
                };
                if takes {
                    self.add(&mut next, &mut stack, (pc + 1, start), at + 1, chars.len());
                }
            }
            core::mem::swap(&mut now, &mut next);
        }
        best
    }

    /// Adds a thread at `(pc, start)` to `threads`, at the place `at` in a
    /// text of `end` characters, following jumps, splits and anchors to
    /// the instructions that wait for a character or match.
    fn add(
        &self,
        threads: &mut Threads,
        stack: &mut Vec<usize>,
        (pc, start): (usize, usize),
        at: usize,
        end: usize,
    ) {
        stack.push(pc);
        while let Some(pc) = stack.pop() {
            if threads.seen[pc] == at + 1 {
                continue;
            }
            threads.seen[pc] = at + 1;
            match self.ops[pc] {
                Op::Jump(to) => stack.push(to),
                Op::Split(first, second) => {
                    stack.push(second);
                    stack.push(first);
                }
                Op::Start if at == 0 => stack.push(pc + 1),
                Op::End if at == end => stack.push(pc + 1),
                Op::Start | Op::End => {}
                _ => threads.list.push((pc, start)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Syntax;
    use crate::parse::parse;

    fn program(pattern: &str, syntax: Syntax, either_case: bool) -> Program {
        compile(&parse(pattern, syntax).unwrap().tree, either_case).unwrap()
    }

    fn chars(text: &str) -> Vec<Option<char>> {
        text.chars().map(Some).collect()
    }

    /// Where the extended `pattern` matches `text` first.
    fn ere(pattern: &str, text: &str) -> Option<(usize, usize)> {
        program(pattern, Syntax::Extended, false).find(&chars(text), 0)
    }

    fn bre(pattern: &str, text: &str) -> Option<(usize, usize)> {
        program(pattern, Syntax::Basic, false).find(&chars(text), 0)
    }

    #[test]
    fn a_text_is_found_where_it_starts_first() {
        assert_eq!(ere("abc", "xabcxabc"), Some((1, 4)));
        assert_eq!(ere("abc", "abd"), None);
        assert_eq!(ere("abc", "ab"), None, "the text ends first");
        assert_eq!(ere("", "xyz"), Some((0, 0)));
        assert_eq!(ere("", ""), Some((0, 0)));
        assert_eq!(ere("a", ""), None);
        assert_eq!(ere("ü", "aüb"), Some((1, 2)), "counted in characters");
        assert_eq!(bre("a.c", "xxabc"), Some((2, 5)));
    }

    #[test]
    fn of_the_leftmost_matches_the_longest_wins() {
        assert_eq!(ere("a|ab", "ab"), Some((0, 2)), "not the first branch");
        assert_eq!(ere("ab|a", "ab"), Some((0, 2)));
        assert_eq!(
            ere("ab|bcde", "abcde"),
            Some((0, 2)),
            "leftmost over longer"
        );
        assert_eq!(ere("(a|ab)(c|bcd)", "abcd"), Some((0, 4)));
        assert_eq!(
            ere("x*", "axx"),
            Some((0, 0)),
            "an empty match at the start is leftmost"
        );
        assert_eq!(ere("x+", "axxb"), Some((1, 3)));
        assert_eq!(ere("(a|b)*c", "ababac"), Some((0, 6)));
        assert_eq!(ere("b+|a+b", "aaabbb"), Some((0, 4)));
        assert_eq!(ere("a*b|a*", "aaa"), Some((0, 3)));
        assert_eq!(
            ere("xy|bc|abcd", "zabcde"),
            Some((1, 5)),
            "a later start that matched first gives way"
        );
    }

    #[test]
    fn anchors_hold_at_the_ends_of_the_text_only() {
        assert_eq!(ere("^a", "aa"), Some((0, 1)));
        assert_eq!(ere("^a", "ba"), None);
        assert_eq!(ere("a$", "aa"), Some((1, 2)));
        assert_eq!(ere("a$", "ab"), None);
        assert_eq!(ere("^$", ""), Some((0, 0)));
        assert_eq!(ere("^$", "a"), None);
        assert_eq!(ere("$", "abc"), Some((3, 3)));
        assert_eq!(ere("a^b", "ab"), None, "^ inside holds nowhere");
        assert_eq!(ere("(^|x)a", "xa"), Some((0, 2)));
        assert_eq!(ere("(^|x)a", "ya"), None);
        let from = |pattern, text, from| {
            program(pattern, Syntax::Extended, false).find(&chars(text), from)
        };
        assert_eq!(
            from("^a", "aa", 1),
            None,
            "^ is the text's start, not where the search starts"
        );
        assert_eq!(from("a", "aba", 1), Some((2, 3)));
        assert_eq!(from("b*", "abb", 1), Some((1, 3)));
        assert_eq!(from("$", "ab", 2), Some((2, 2)));
        assert_eq!(from("a", "ab", 2), None);
        assert_eq!(from("", "ab", 3), None, "past the end nothing is found");
    }

    #[test]
    fn repetitions_count_and_end() {
        assert_eq!(ere("a{2,3}", "aaaa"), Some((0, 3)));
        assert_eq!(ere("a{2}", "a"), None);
        assert_eq!(ere("a{2}", "baab"), Some((1, 3)));
        assert_eq!(ere("a{0}", "a"), Some((0, 0)));
        assert_eq!(ere("a{0}b", "ab"), Some((1, 2)));
        assert_eq!(ere("(ab){2,}", "xabababy"), Some((1, 7)));
        assert_eq!(ere("a?b", "ab"), Some((0, 2)));
        assert_eq!(ere("ba?", "baa"), Some((0, 2)));
        assert_eq!(
            ere("(a*)*", "b"),
            Some((0, 0)),
            "a loop that takes nothing ends"
        );
        assert_eq!(ere("(a*)+b", "aab"), Some((0, 3)));
        assert_eq!(ere("(|a)*b", "aab"), Some((0, 3)));
        assert_eq!(ere("(a*)*b", &"a".repeat(2000)), None);
        assert_eq!(bre("a\\{2,\\}", "caaab"), Some((1, 4)));
        assert_eq!(bre("a**", "aa"), Some((0, 2)));
    }

    #[test]
    fn sets_and_dots_take_one_character() {
        assert_eq!(ere("[0-9]+", "ab123c"), Some((2, 5)));
        assert_eq!(ere(".+", "héllo"), Some((0, 5)));
        assert_eq!(ere("[^a]", "aab"), Some((2, 3)));
        assert_eq!(ere("a.c", "ac"), None);
        let any = program("a.c", Syntax::Extended, false);
        assert_eq!(
            any.find(&[Some('a'), None, Some('c')], 0),
            Some((0, 3)),
            "a byte that is no UTF-8 is a character"
        );
        let find = |pattern, text: &[Option<char>]| {
            program(pattern, Syntax::Extended, false).find(text, 0)
        };
        assert_eq!(find("[^b]", &[None]), Some((0, 1)));
        assert_eq!(find("[a-z]", &[None]), None);
        assert_eq!(find("x", &[None, Some('x')]), Some((1, 2)));
    }

    #[test]
    fn either_case_matches_characters_in_the_other_case() {
        let either = |pattern, text| program(pattern, Syntax::Extended, true).find(&chars(text), 0);
        assert_eq!(either("abc", "xABC"), Some((1, 4)));
        assert_eq!(ere("abc", "xABC"), None);
        assert_eq!(either("[a-c]+", "BCA"), Some((0, 3)));
        assert_eq!(either("Ü", "ü"), Some((0, 1)));
        assert_eq!(
            either("s", "ſ"),
            Some((0, 1)),
            "the long s is S in upper case"
        );
        assert_eq!(either("k", "K"), Some((0, 1)));
        assert_eq!(either("x", "y"), None);
        assert_eq!(program("x", Syntax::Extended, true).find(&[None], 0), None);
    }

    #[test]
    fn a_program_holds_its_instructions_in_order() {
        let ops = |pattern| program(pattern, Syntax::Extended, false).ops;
        let a = Op::Char('a');
        let b = Op::Char('b');
        assert_eq!(ops("ab"), [a.clone(), b.clone(), Op::Match]);
        assert_eq!(
            ops("a|b"),
            [
                Op::Split(1, 3),
                a.clone(),
                Op::Jump(4),
                b.clone(),
                Op::Match
            ]
        );
        assert_eq!(
            ops("a|b|c"),
            [
                Op::Split(1, 3),
                a.clone(),
                Op::Jump(7),
                Op::Split(4, 6),
                b,
                Op::Jump(7),
                Op::Char('c'),
                Op::Match
            ]
        );
        assert_eq!(
            ops("a*"),
            [Op::Split(1, 3), a.clone(), Op::Jump(0), Op::Match]
        );
        assert_eq!(
            ops("a{1,3}"),
            [
                a.clone(),
                Op::Split(2, 5),
                a.clone(),
                Op::Split(4, 5),
                a.clone(),
                Op::Match
            ]
        );
        assert_eq!(ops("^(a)$"), [Op::Start, a, Op::End, Op::Match]);
        assert_eq!(ops("()"), [Op::Match]);
    }

    #[test]
    fn a_program_too_big_or_with_back_references_is_refused() {
        let compiled =
            |pattern: &str, syntax| compile(&parse(pattern, syntax).unwrap().tree, false);
        assert_eq!(
            compiled("((a{255}){255}){2}", Syntax::Extended).unwrap_err(),
            Refused::TooBig
        );
        assert!(compiled("(a{255}){255}", Syntax::Extended).is_ok());
        let most = "a".repeat(MOST_OPS - 1);
        assert_eq!(
            compiled(&most, Syntax::Basic).map(|p| p.ops.len()).ok(),
            Some(MOST_OPS),
            "the last fits with its match"
        );
        assert_eq!(
            compiled(&(most + "a"), Syntax::Basic).unwrap_err(),
            Refused::TooBig
        );
        assert_eq!(
            compiled("\\(a\\)\\1", Syntax::Basic).unwrap_err(),
            Refused::BackReference
        );
    }

    #[test]
    fn posix_matches_the_leftmost_then_the_longest() {
        for (pattern, text, want) in [
            ("a+", "baaab", Some((1, 4))),
            ("(a|b)+", "cabbad", Some((1, 5))),
            ("(wee|week)(knights|night)", "weeknights", Some((0, 10))),
            ("(a|ab)(bc|c)", "abc", Some((0, 3))),
            ("(ab|a)(bc|c)?", "abc", Some((0, 3))),
            ("a(b|c)*d", "abccbd", Some((0, 6))),
            ("a(b|c)*d", "abcx", None),
            ("x(a|aa)*y", "xaaay", Some((0, 5))),
            ("[[:digit:]]+\\.[[:digit:]]*", "pi is 3.14!", Some((6, 10))),
            ("[[:alpha:]_][[:alnum:]_]*", "  _x9 = 1", Some((2, 5))),
            ("(..)*", "abcde", Some((0, 4))),
            ("(a*)(b*)c", "aabbbc", Some((0, 6))),
            ("a{3}|a{2}b", "aab", Some((0, 3))),
            ("(a|b|c|d|e)f", "ef", Some((0, 2))),
            ("ab*c|abd", "abd", Some((0, 3))),
            ("(^a|b)c", "ac bc", Some((0, 2))),
            ("(b$|a)", "ab", Some((0, 1))),
            ("b$", "ab", Some((1, 2))),
            ("[^[:space:]]+", " \tword\n", Some((2, 6))),
            ("é+", "caféé", Some((3, 5))),
        ] {
            assert_eq!(ere(pattern, text), want, "{pattern} in {text:?}");
        }
    }

    #[test]
    fn a_basic_expression_matches_as_its_extended_twin() {
        for (basic, extended, text) in [
            ("\\(ab\\)*c", "(ab)*c", "xababcy"),
            ("a\\{2,3\\}", "a{2,3}", "baaaab"),
            ("^a.*b$", "^a.*b$", "axxb"),
            ("[ab]*\\.", "[ab]*\\.", "abba."),
            ("x\\{0,\\}y", "x*y", "zxxy"),
        ] {
            assert_eq!(
                bre(basic, text),
                ere(extended, text),
                "{basic} and {extended}"
            );
            assert!(bre(basic, text).is_some(), "{basic} in {text}");
        }
    }

    #[test]
    fn a_search_goes_on_from_where_the_last_match_ended() {
        let every = |pattern, text| {
            let program = program(pattern, Syntax::Extended, false);
            let chars = chars(text);
            let mut found = Vec::new();
            let mut from = 0;
            while let Some((start, end)) = program.find(&chars, from) {
                found.push((start, end));
                from = end.max(start + 1);
            }
            found
        };
        assert_eq!(every("[0-9]+", "12 345 6"), [(0, 2), (3, 6), (7, 8)]);
        assert_eq!(
            every("x*", "axb"),
            [(0, 0), (1, 2), (2, 2), (3, 3)],
            "empty matches between, and one at the end"
        );
        assert_eq!(every("^a", "aaa"), [(0, 1)]);
        assert_eq!(every("a$", "aaa"), [(2, 3)]);
    }
}
