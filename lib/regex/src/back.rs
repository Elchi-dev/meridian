//! Expressions with back-references, which no automaton matches. Every
//! way each part can match is listed with where the subexpressions are in
//! it, the longest first and, of one length, in the order POSIX prefers:
//! the earlier part the longer. The first way of the whole expression is
//! its match. Budgets of ways and of depth end a search that would take
//! too long, hold too much or run past the stack.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::parse::Node;
use crate::{Text, TooComplex, same};

/// Where each subexpression matched, numbered from 1, the match itself
/// first; none for one that took no part.
pub type Spans = Vec<Option<(usize, usize)>>;

/// How many places a way keeps: the match's and those of the nine
/// subexpressions a back-reference can name. Where a later one matched
/// is not kept, so it is said to have taken no part.
const SLOTS: usize = 10;

/// The places of a way, in characters, held in the way itself, so that
/// listing ways asks the heap for nothing each.
type Slots = [Option<(u32, u32)>; SLOTS];

/// How many ways one search may list before it gives up.
pub const BUDGET: usize = 1 << 18;

/// How many different ways one part may have at once, as a program has a
/// heap of 1 MiB and parts inside each other hold theirs at the same
/// time; `\(.*\)\1` on a line gives a part a way for each of its
/// characters.
pub const MOST_WAYS: usize = 1 << 9;

/// How deep parts and rounds may lie inside each other in a search before
/// it gives up, so that it stays within a program's stack.
pub const MOST_DEPTH: usize = 400;

/// One way a part matches: where it ends, and the places then.
type Way = (usize, Slots);

struct Search<'s, 't> {
    text: &'s Text<'t>,
    either_case: bool,
    left: usize,
    depth: usize,
}

/// A place as a way holds it.
fn held(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

/// A place a way held.
fn place(at: u32) -> usize {
    usize::try_from(at).unwrap_or(usize::MAX)
}

/// The leftmost-longest match of `tree`, which has `groups`
/// subexpressions, among those starting at the character `from` or
/// after, with where each subexpression matched in it.
///
/// # Errors
///
/// When the search lists more than [`BUDGET`] ways in all, one part more
/// than [`MOST_WAYS`] at once, or goes [`MOST_DEPTH`] deep.
pub fn find(
    tree: &Node,
    groups: usize,
    text: &Text,
    either_case: bool,
    from: usize,
) -> Result<Option<Spans>, TooComplex> {
    let mut search = Search {
        text,
        either_case,
        left: BUDGET,
        depth: 0,
    };
    for start in from..=text.chars.len() {
        if let Some(&(end, slots)) = search.ways(tree, start, &[None; SLOTS])?.first() {
            let mut spans: Spans = (0..=groups)
                .map(|i| {
                    slots
                        .get(i)
                        .copied()
                        .flatten()
                        .map(|(start, end)| (place(start), place(end)))
                })
                .collect();
            spans[0] = Some((start, end));
            return Ok(Some(spans));
        }
    }
    Ok(None)
}

/// Adds `more` to `ways`, unless that makes more than [`MOST_WAYS`]
/// different ones. Ways alike may pile up first; settling them on the
/// way leaves the order they settle in at the end as it is.
fn gather(ways: &mut Vec<Way>, more: Vec<Way>) -> Result<(), TooComplex> {
    ways.extend(more);
    let crowded = ways.len() > MOST_WAYS;
    if crowded {
        *ways = settled(core::mem::take(ways));
        if ways.len() > MOST_WAYS {
            return Err(TooComplex);
        }
    }
    Ok(())
}

/// `ways` the longest first, keeping the order of ways of one length,
/// each once: of ways alike the first stays.
fn settled(mut ways: Vec<Way>) -> Vec<Way> {
    ways.sort_by_key(|way| core::cmp::Reverse(way.0));
    let mut seen = BTreeSet::new();
    let kept: Vec<bool> = ways.iter().map(|way| seen.insert(way)).collect();
    drop(seen);
    ways.into_iter()
        .zip(kept)
        .filter_map(|(way, keep)| keep.then_some(way))
        .collect()
}

impl Search<'_, '_> {
    /// Every way `node` matches from the character `at`, with `slots` so
    /// far: the longest first and, of one length, as POSIX prefers them.
    fn ways(&mut self, node: &Node, at: usize, slots: &Slots) -> Result<Vec<Way>, TooComplex> {
        self.deeper(|search| search.ways_inside(node, at, slots))
    }

    /// What `go` finds one level deeper, unless that is too deep.
    fn deeper(
        &mut self,
        go: impl FnOnce(&mut Self) -> Result<Vec<Way>, TooComplex>,
    ) -> Result<Vec<Way>, TooComplex> {
        if self.depth == MOST_DEPTH {
            return Err(TooComplex);
        }
        self.depth += 1;
        let ways = go(self);
        self.depth -= 1;
        ways
    }

    fn ways_inside(
        &mut self,
        node: &Node,
        at: usize,
        slots: &Slots,
    ) -> Result<Vec<Way>, TooComplex> {
        let either_case = self.either_case;
        let here = || Vec::from([(at, *slots)]);
        let ways = match node {
            Node::Empty => here(),
            Node::Char(want) => self.one(at, slots, |c| same(c, *want, either_case)),
            Node::Any => self.one(at, slots, |_| true),
            Node::Set(set) => self.one(at, slots, |c| set.takes(c, either_case)),
            Node::Start if at == 0 => here(),
            Node::End if at == self.text.chars.len() => here(),
            Node::Start | Node::End => Vec::new(),
            Node::Group(number, inside) => {
                let mut ways = self.ways(inside, at, slots)?;
                for (end, slots) in &mut ways {
                    if let Some(slot) = slots.get_mut(*number) {
                        *slot = Some((held(at), held(*end)));
                    }
                }
                ways
            }
            Node::Concat(items) => self.sequence(items, at, slots)?,
            Node::Alt(branches) => {
                let mut ways = Vec::new();
                for branch in branches {
                    let more = self.ways(branch, at, slots)?;
                    gather(&mut ways, more)?;
                }
                ways
            }
            Node::Repeat(inside, least, most)
                if matches!(**inside, Node::Char(_) | Node::Any | Node::Set(_)) =>
            {
                self.run(inside, (*least, *most), at, slots)?
            }
            Node::Repeat(inside, least, most) => {
                self.rounds(inside, (*least, *most), 0, at, slots)?
            }
            Node::Back(number) => match slots.get(*number).copied().flatten() {
                Some((start, end)) if self.repeats(place(start), place(end), at) => {
                    Vec::from([(at + place(end) - place(start), *slots)])
                }
                _ => Vec::new(),
            },
        };
        let ways = settled(ways);
        self.left = self.left.checked_sub(ways.len()).ok_or(TooComplex)?;
        Ok(ways)
    }

    /// The way past the character at `at`, when it `takes` it.
    fn one(&self, at: usize, slots: &Slots, takes: impl Fn(Option<char>) -> bool) -> Vec<Way> {
        match self.text.chars.get(at) {
            Some(&c) if takes(c) => Vec::from([(at + 1, *slots)]),
            _ => Vec::new(),
        }
    }

    /// Every way `items` match one after the other from `at`.
    fn sequence(
        &mut self,
        items: &[Node],
        at: usize,
        slots: &Slots,
    ) -> Result<Vec<Way>, TooComplex> {
        let Some((first, rest)) = items.split_first() else {
            return Ok(Vec::from([(at, *slots)]));
        };
        let mut ways = Vec::new();
        for (middle, slots) in self.ways(first, at, slots)? {
            let more = self.sequence(rest, middle, &slots)?;
            gather(&mut ways, more)?;
        }
        Ok(settled(ways))
    }

    /// Every way the one-character part `inside` repeats from `at`, as
    /// many times as `counts` allows, the longest first: a way for each
    /// length of the run of characters it takes, found without a round
    /// each, so a long line stays within the depth a search may go to.
    fn run(
        &self,
        inside: &Node,
        (least, most): (u32, Option<u32>),
        at: usize,
        slots: &Slots,
    ) -> Result<Vec<Way>, TooComplex> {
        let either_case = self.either_case;
        let takes = |c: Option<char>| match inside {
            Node::Char(want) => same(c, *want, either_case),
            Node::Set(set) => set.takes(c, either_case),
            _ => true,
        };
        let most = most.map_or(usize::MAX, place);
        let mut length = 0;
        while length < most && self.text.chars.get(at + length).is_some_and(|&c| takes(c)) {
            length += 1;
        }
        let lengths = place(least)..=length;
        if lengths.clone().count() > MOST_WAYS {
            return Err(TooComplex);
        }
        Ok(lengths.rev().map(|length| (at + length, *slots)).collect())
    }

    /// Every way `inside` matches round after round from `at`, `done`
    /// rounds in already, between the least and the most rounds `counts`
    /// allows: more rounds first, and stopping last.
    fn rounds(
        &mut self,
        inside: &Node,
        counts: (u32, Option<u32>),
        done: u32,
        at: usize,
        slots: &Slots,
    ) -> Result<Vec<Way>, TooComplex> {
        let (least, most) = counts;
        let mut ways = Vec::new();
        if most.is_none_or(|most| done < most) {
            for (end, slots) in self.ways(inside, at, slots)? {
                // A round that takes nothing adds nothing once the least
                // count is in.
                if end > at || done < least {
                    let more =
                        self.deeper(|search| search.rounds(inside, counts, done + 1, end, &slots))?;
                    gather(&mut ways, more)?;
                }
            }
        }
        if done >= least {
            ways.push((at, *slots));
        }
        Ok(settled(ways))
    }

    /// Whether the characters from `start` to `end` come again at `at`, in
    /// either case when the search says so; a byte that is no UTF-8 only
    /// as the same byte.
    fn repeats(&self, start: usize, end: usize, at: usize) -> bool {
        let chars = &self.text.chars;
        let Some(again) = chars.get(at..at + (end - start)) else {
            return false;
        };
        chars[start..end]
            .iter()
            .zip(again)
            .enumerate()
            .all(|(i, (&first, &second))| match first {
                Some(want) => same(second, want, self.either_case),
                None => second.is_none() && self.text.byte(start + i) == self.text.byte(at + i),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Syntax;
    use crate::dice::{Dice, pattern, text};
    use crate::parse::parse;
    use crate::reach::{self, Chars};
    use crate::vm::compile;

    /// The first match of `pattern`, read as `syntax`, in `bytes`, with its
    /// subexpressions.
    fn search(pattern: &str, syntax: Syntax, either_case: bool, bytes: &[u8]) -> Option<Spans> {
        let parsed = parse(pattern, syntax).unwrap();
        let text = Text::new(bytes);
        find(&parsed.tree, parsed.groups, &text, either_case, 0).unwrap()
    }

    fn bre(pattern: &str, text: &str) -> Option<Spans> {
        search(pattern, Syntax::Basic, false, text.as_bytes())
    }

    #[test]
    fn as_many_ways_as_the_most_are_kept_and_one_more_is_too_many() {
        let different = |n: u32| way(1, Some((n, n)));
        let most = u32::try_from(MOST_WAYS).unwrap();
        let mut ways = Vec::new();
        assert_eq!(
            gather(&mut ways, (0..most).map(different).collect()),
            Ok(())
        );
        assert_eq!(ways.len(), MOST_WAYS);
        assert_eq!(
            gather(&mut ways, Vec::from([different(0)])),
            Ok(()),
            "one alike settles away"
        );
        assert_eq!(ways.len(), MOST_WAYS);
        assert_eq!(
            gather(&mut ways, Vec::from([different(most)])),
            Err(TooComplex)
        );
    }

    #[test]
    fn a_back_reference_matches_what_its_group_matched() {
        assert_eq!(
            bre("\\(.\\)\\1", "abccd"),
            Some(Vec::from([Some((2, 4)), Some((2, 3))]))
        );
        assert_eq!(bre("\\(.\\)\\1", "abcd"), None);
        assert_eq!(
            bre("\\(a*\\)b\\1", "aabaa"),
            Some(Vec::from([Some((0, 5)), Some((0, 2))]))
        );
        assert_eq!(
            bre("\\(a*\\)b\\1", "aaba"),
            Some(Vec::from([Some((1, 4)), Some((1, 2))])),
            "the first start that can repeat it"
        );
        assert_eq!(
            bre("^\\(.*\\) \\1$", "hey hey"),
            Some(Vec::from([Some((0, 7)), Some((0, 3))]))
        );
        assert_eq!(bre("^\\(.*\\) \\1$", "hey you"), None);
        assert_eq!(
            bre("\\(a\\)\\(b\\)\\2\\1", "xabba"),
            Some(Vec::from([Some((1, 5)), Some((1, 2)), Some((2, 3))]))
        );
    }

    #[test]
    fn a_back_reference_to_a_group_that_took_no_part_matches_nothing() {
        let extended =
            |pattern, text: &str| search(pattern, Syntax::Extended, false, text.as_bytes());
        assert_eq!(extended("(a)?b\\1", "b"), None);
        assert_eq!(
            extended("(a)?b\\1", "aba"),
            Some(Vec::from([Some((0, 3)), Some((0, 1))]))
        );
        assert_eq!(
            extended("(a|b)\\1", "abba"),
            Some(Vec::from([Some((1, 3)), Some((1, 2))]))
        );
        assert_eq!(
            extended("(a*)+\\1", "aa"),
            Some(Vec::from([Some((0, 2)), Some((0, 1))])),
            "the rounds leave the reference an a to repeat"
        );
    }

    #[test]
    fn a_back_reference_repeats_in_either_case_and_bytes_as_they_are() {
        assert_eq!(
            search("(a)\\1", Syntax::Extended, true, b"aA"),
            Some(Vec::from([Some((0, 2)), Some((0, 1))]))
        );
        assert_eq!(search("(a)\\1", Syntax::Extended, false, b"aA"), None);
        assert_eq!(
            search("(.)\\1", Syntax::Extended, false, &[0xff, 0xff]),
            Some(Vec::from([Some((0, 2)), Some((0, 1))]))
        );
        assert_eq!(
            search("(.)\\1", Syntax::Extended, false, &[0xff, 0xfe]),
            None
        );
        assert_eq!(
            search("(.)\\1", Syntax::Extended, false, &[0xff, b'a']),
            None
        );
        assert_eq!(search("(.)\\1", Syntax::Extended, false, b"a\xff"), None);
        assert_eq!(
            search("(..)\\1", Syntax::Extended, false, "üxüx".as_bytes()),
            Some(Vec::from([Some((0, 4)), Some((0, 2))])),
            "counted in characters"
        );
    }

    #[test]
    fn a_search_starts_where_it_is_told() {
        let parsed = parse("\\(a\\)\\1", Syntax::Basic).unwrap();
        let text = Text::new(b"aabaa");
        let at = |from| find(&parsed.tree, parsed.groups, &text, false, from).unwrap();
        assert_eq!(at(0), Some(Vec::from([Some((0, 2)), Some((0, 1))])));
        assert_eq!(at(1), Some(Vec::from([Some((3, 5)), Some((3, 4))])));
        assert_eq!(at(4), None);
        assert_eq!(at(6), None, "past the end");
    }

    #[test]
    fn a_search_with_too_many_ways_gives_up() {
        let parsed = parse("\\(\\(a*\\)*\\)*\\1x", Syntax::Basic).unwrap();
        let long = "a".repeat(40);
        let text = Text::new(long.as_bytes());
        assert_eq!(
            find(&parsed.tree, parsed.groups, &text, false, 0),
            Err(TooComplex)
        );
        let deep = parse("\\(a\\)*\\1", Syntax::Basic).unwrap();
        let many = "a".repeat(MOST_DEPTH);
        let text = Text::new(many.as_bytes());
        assert_eq!(
            find(&deep.tree, deep.groups, &text, false, 0),
            Err(TooComplex),
            "a round for each of hundreds of characters"
        );
        let few = Text::new(b"aaaa");
        assert_eq!(
            find(&deep.tree, deep.groups, &few, false, 0),
            Ok(Some(Vec::from([Some((0, 4)), Some((2, 3))]))),
            "the longest whose last round the reference can repeat"
        );
    }

    #[test]
    fn without_back_references_the_search_finds_what_the_vm_and_places_find() {
        let mut dice = Dice(0x5851_f42d_4c95_7f2d);
        for _ in 0..400 {
            let pattern = pattern(&mut dice, 4);
            let parsed = parse(&pattern, Syntax::Extended).unwrap();
            let program = compile(&parsed.tree, false).unwrap();
            for _ in 0..6 {
                let words = text(&mut dice);
                let text = Text::new(words.as_bytes());
                let chars: Vec<Option<char>> = words.chars().map(Some).collect();
                let places = Chars {
                    chars: &chars,
                    either_case: false,
                };
                let found = program
                    .find(&chars, 0)
                    .map(|found| reach::groups(&parsed.tree, parsed.groups, &places, found));
                assert_eq!(
                    find(&parsed.tree, parsed.groups, &text, false, 0).unwrap(),
                    found,
                    "{pattern} in {words:?}"
                );
            }
        }
    }

    #[test]
    fn the_deepest_search_fits_in_the_stack_grep_and_sed_ask_for() {
        // They ask for 256 pages, 1 MiB.
        let deepest = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let parsed = parse("\\(a\\)*\\1", Syntax::Basic).unwrap();
                let long = "a".repeat(2 * MOST_DEPTH);
                find(
                    &parsed.tree,
                    parsed.groups,
                    &Text::new(long.as_bytes()),
                    false,
                    0,
                )
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(deepest, Err(TooComplex));
    }

    #[test]
    fn a_long_search_that_never_goes_deep_does_not_give_up() {
        let late = "a".repeat(500) + "bb";
        assert_eq!(
            bre("\\(b\\)\\1", &late),
            Some(Vec::from([Some((500, 502)), Some((500, 501))])),
            "five hundred starts, each a few levels deep"
        );
    }

    #[test]
    fn bytes_that_are_no_utf8_repeat_byte_for_byte() {
        let pair = |bytes: &[u8]| search("(..)\\1", Syntax::Extended, false, bytes);
        assert_eq!(
            pair(&[0xff, 0xfe, 0xff, 0xfe]),
            Some(Vec::from([Some((0, 4)), Some((0, 2))]))
        );
        assert_eq!(pair(&[0xff, 0xfe, 0xff, 0xff]), None);
        assert_eq!(pair(&[0xff, 0xfe, 0xfe, 0xfe]), None);
        assert_eq!(
            pair(&[b'x', 0xc3, b'x', 0xc3]),
            Some(Vec::from([Some((0, 4)), Some((0, 2))])),
            "a lone first byte of a character"
        );
    }

    #[test]
    fn of_one_length_the_earlier_part_takes_the_longer() {
        assert_eq!(
            bre("\\(a*\\)\\(a*\\)\\2", "aaaa"),
            Some(Vec::from([Some((0, 4)), Some((0, 4)), Some((4, 4))]))
        );
        assert_eq!(
            bre("\\(a*\\)\\(a*\\)b\\2", "aaaba"),
            Some(Vec::from([Some((0, 5)), Some((0, 2)), Some((2, 3))])),
            "the first gives up only what the reference needs"
        );
        assert_eq!(
            search("(a|ab)(c|bcd)(d*)\\3", Syntax::Extended, false, b"abcdd"),
            Some(Vec::from([
                Some((0, 5)),
                Some((0, 2)),
                Some((2, 3)),
                Some((3, 4))
            ]))
        );
    }

    #[test]
    fn a_part_has_ways_only_up_to_a_bound_that_keeps_the_heap_small() {
        let twice = "ab".repeat(200);
        assert_eq!(
            bre("^\\(.*\\)\\1$", &twice),
            Some(Vec::from([Some((0, 400)), Some((0, 200))])),
            "a line of 400 characters, a way for each"
        );
        let parsed = parse("\\(.*\\)\\1", Syntax::Basic).unwrap();
        let long = "a".repeat(MOST_WAYS + 10);
        let text = Text::new(long.as_bytes());
        assert_eq!(
            find(&parsed.tree, parsed.groups, &text, false, 0),
            Err(TooComplex),
            "more ends for .* than the bound"
        );
    }

    /// A way that ends at `end` with the first subexpression from `start`
    /// to `end`, or none.
    fn way(end: usize, group: Option<(u32, u32)>) -> Way {
        let mut slots = [None; SLOTS];
        slots[1] = group;
        (end, slots)
    }

    #[test]
    fn ways_settle_longest_first_each_once_in_the_order_they_came() {
        let a = way(2, Some((0, 1)));
        let b = way(5, None);
        let c = way(2, Some((0, 2)));
        assert_eq!(settled(Vec::from([a, b, c, a, b])), [b, a, c]);
        assert_eq!(
            settled(Vec::from([c, a])),
            [c, a],
            "of one length, the first first"
        );
        assert_eq!(settled(Vec::new()), []);
    }

    #[test]
    fn gathering_settles_before_it_says_there_are_too_many() {
        let mut ways = Vec::new();
        let same = alloc::vec![way(1, None); MOST_WAYS];
        assert_eq!(gather(&mut ways, same.clone()), Ok(()));
        assert_eq!(gather(&mut ways, same), Ok(()), "twice as many, all alike");
        assert_eq!(ways, [way(1, None)]);
        let many: Vec<Way> = (0..=MOST_WAYS).map(|end| way(end, None)).collect();
        assert_eq!(gather(&mut Vec::new(), many), Err(TooComplex));
        let most: Vec<Way> = (0..MOST_WAYS).map(|end| way(end, None)).collect();
        assert_eq!(
            gather(&mut Vec::new(), most),
            Ok(()),
            "as many as the bound"
        );
    }

    #[test]
    fn a_run_of_one_character_lists_its_lengths_within_its_counts() {
        let run = |pattern: &str, text: &str, either_case: bool| {
            let parsed = parse(pattern, Syntax::Extended).unwrap();
            let Node::Repeat(inside, least, most) = &parsed.tree else {
                panic!("a repeat");
            };
            let text = Text::new(text.as_bytes());
            let search = Search {
                text: &text,
                either_case,
                left: BUDGET,
                depth: 0,
            };
            search
                .run(inside, (*least, *most), 0, &[None; SLOTS])
                .map(|ways| ways.into_iter().map(|(end, _)| end).collect::<Vec<_>>())
        };
        assert_eq!(run("a*", "aab", false), Ok(Vec::from([2, 1, 0])));
        assert_eq!(run("a+", "aab", false), Ok(Vec::from([2, 1])));
        assert_eq!(run("a{1,2}", "aaaa", false), Ok(Vec::from([2, 1])));
        assert_eq!(run("a{3}", "aa", false), Ok(Vec::new()), "too few");
        assert_eq!(run("[ab]*", "abc", false), Ok(Vec::from([2, 1, 0])));
        assert_eq!(run(".*", "äö", false), Ok(Vec::from([2, 1, 0])));
        assert_eq!(run("A*", "aA", true), Ok(Vec::from([2, 1, 0])));
        assert_eq!(run("A*", "aA", false), Ok(Vec::from([0])));
        let long = "a".repeat(MOST_WAYS);
        assert_eq!(
            run("a*", &long, false),
            Err(TooComplex),
            "one more length than the bound"
        );
        assert!(run("a+", &long, false).is_ok(), "as many as the bound");
    }

    #[test]
    fn a_subexpression_past_the_ninth_takes_no_part_a_search_says() {
        let pattern = "\\(a\\)".repeat(10) + "\\1";
        let parsed = parse(&pattern, Syntax::Basic).unwrap();
        let eleven = "a".repeat(11);
        let text = Text::new(eleven.as_bytes());
        let spans = find(&parsed.tree, parsed.groups, &text, false, 0)
            .unwrap()
            .unwrap();
        assert_eq!(spans.len(), 11);
        assert_eq!(spans[0], Some((0, 11)));
        assert_eq!(spans[9], Some((8, 9)));
        assert_eq!(spans[10], None);
    }

    #[test]
    fn places_go_into_ways_and_out_unchanged() {
        assert_eq!(place(held(0)), 0);
        assert_eq!(place(held(123_456)), 123_456);
        assert_eq!(
            held(usize::MAX),
            u32::MAX,
            "past what a way holds, the most it can"
        );
    }
}
