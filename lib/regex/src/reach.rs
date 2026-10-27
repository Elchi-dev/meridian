//! Which places in a text the parts of an expression reach: from a set
//! of places, every place a part can end at, and back from a set of
//! places, every place it can start at. Inside a match the VM found, the
//! subexpressions are worked out with these as POSIX defines them: from
//! left to right, each part the longest it can be while the rest still
//! reaches the end of the match.

use alloc::vec::Vec;

use crate::parse::Node;
use crate::same;

/// A set of places in a text, each from 0 to its length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places {
    words: Vec<u64>,
}

impl Places {
    /// No place in a text of `len` characters.
    #[must_use]
    pub fn none(len: usize) -> Places {
        Places {
            words: alloc::vec![0; (len + 1).div_ceil(64)],
        }
    }

    /// Only the place `at` in a text of `len` characters.
    #[must_use]
    pub fn one(len: usize, at: usize) -> Places {
        let mut places = Places::none(len);
        places.put(at);
        places
    }

    #[must_use]
    pub fn holds(&self, at: usize) -> bool {
        self.words
            .get(at / 64)
            .is_some_and(|word| word >> (at % 64) & 1 == 1)
    }

    pub fn put(&mut self, at: usize) {
        self.words[at / 64] |= 1 << (at % 64);
    }

    /// Puts in every place of `other`, which is of a text as long, and
    /// says whether one of them was new.
    pub fn take(&mut self, other: &Places) -> bool {
        let mut grew = false;
        for (word, more) in self.words.iter_mut().zip(&other.words) {
            grew |= more & !*word != 0;
            *word |= more;
        }
        grew
    }

    /// The places of `self` that are not in `other`.
    #[must_use]
    pub fn without(&self, other: &Places) -> Places {
        Places {
            words: self
                .words
                .iter()
                .zip(&other.words)
                .map(|(word, less)| word & !less)
                .collect(),
        }
    }

    /// The places, from the first.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(i, &word)| {
            let mut rest = word;
            core::iter::from_fn(move || {
                let bit = rest.trailing_zeros() as usize;
                // Clears the lowest bit set.
                rest &= rest.wrapping_sub(1);
                (bit < 64).then_some(i * 64 + bit)
            })
        })
    }

    /// The last place, none when it holds none.
    #[must_use]
    pub fn last(&self) -> Option<usize> {
        let (i, word) = self
            .words
            .iter()
            .enumerate()
            .rev()
            .find(|&(_, &word)| word != 0)?;
        Some(i * 64 + 63 - word.leading_zeros() as usize)
    }
}

/// A text as its characters, none for a byte that is no UTF-8, and
/// whether characters match in either case.
#[derive(Clone, Copy, Debug)]
pub struct Chars<'t> {
    pub chars: &'t [Option<char>],
    pub either_case: bool,
}

impl Chars<'_> {
    fn none(&self) -> Places {
        Places::none(self.chars.len())
    }

    /// The place after each of `from` whose character `takes`.
    fn ahead(&self, from: &Places, takes: impl Fn(Option<char>) -> bool) -> Places {
        let mut to = self.none();
        for at in from.iter() {
            if self.chars.get(at).is_some_and(|&c| takes(c)) {
                to.put(at + 1);
            }
        }
        to
    }

    /// The place before each of `to` whose character `takes`.
    fn behind(&self, to: &Places, takes: impl Fn(Option<char>) -> bool) -> Places {
        let mut from = self.none();
        for at in to.iter().filter(|&at| at > 0) {
            if self.chars.get(at - 1).is_some_and(|&c| takes(c)) {
                from.put(at - 1);
            }
        }
        from
    }

    /// `places` where they hold `at`, else none.
    fn only(&self, places: &Places, at: usize) -> Places {
        if places.holds(at) {
            Places::one(self.chars.len(), at)
        } else {
            self.none()
        }
    }
}

/// Every place `node` can end at when it starts at one of `from`. A
/// back-reference reaches none here; an expression with one is matched by
/// a search of its own.
#[must_use]
pub fn forward(node: &Node, from: &Places, text: &Chars) -> Places {
    let either_case = text.either_case;
    match node {
        Node::Empty => from.clone(),
        Node::Char(want) => text.ahead(from, |c| same(c, *want, either_case)),
        Node::Any => text.ahead(from, |_| true),
        Node::Set(set) => text.ahead(from, |c| set.takes(c, either_case)),
        Node::Start => text.only(from, 0),
        Node::End => text.only(from, text.chars.len()),
        Node::Group(_, inside) => forward(inside, from, text),
        Node::Concat(items) => items
            .iter()
            .fold(from.clone(), |places, item| forward(item, &places, text)),
        Node::Alt(branches) => {
            let mut to = text.none();
            for branch in branches {
                to.take(&forward(branch, from, text));
            }
            to
        }
        Node::Repeat(inside, least, most) => {
            repeat(|places| forward(inside, places, text), from, *least, *most)
        }
        Node::Back(_) => text.none(),
    }
}

/// Every place `node` can start at to end at one of `to`.
#[must_use]
pub fn backward(node: &Node, to: &Places, text: &Chars) -> Places {
    let either_case = text.either_case;
    match node {
        Node::Empty => to.clone(),
        Node::Char(want) => text.behind(to, |c| same(c, *want, either_case)),
        Node::Any => text.behind(to, |_| true),
        Node::Set(set) => text.behind(to, |c| set.takes(c, either_case)),
        Node::Start => text.only(to, 0),
        Node::End => text.only(to, text.chars.len()),
        Node::Group(_, inside) => backward(inside, to, text),
        Node::Concat(items) => items
            .iter()
            .rev()
            .fold(to.clone(), |places, item| backward(item, &places, text)),
        Node::Alt(branches) => {
            let mut from = text.none();
            for branch in branches {
                from.take(&backward(branch, to, text));
            }
            from
        }
        Node::Repeat(inside, least, most) => {
            repeat(|places| backward(inside, places, text), to, *least, *most)
        }
        Node::Back(_) => text.none(),
    }
}

/// The places `step`, taken from `from` at least `least` times and at
/// most `most`, reaches.
fn repeat(
    step: impl Fn(&Places) -> Places,
    from: &Places,
    least: u32,
    most: Option<u32>,
) -> Places {
    let mut frontier = from.clone();
    for _ in 0..least {
        frontier = step(&frontier);
    }
    let mut all = frontier.clone();
    match most {
        // Each place found a second time leads only where it led before,
        // so only the new ones go on.
        None => loop {
            frontier = step(&frontier).without(&all);
            if !all.take(&frontier) {
                break all;
            }
        },
        // Once a count reaches no new place, no later count does.
        Some(most) => {
            for _ in least..most {
                frontier = step(&frontier);
                if !all.take(&frontier) {
                    break;
                }
            }
            all
        }
    }
}

/// Where each subexpression of `tree`, of `count` in all, matched in
/// the match from `start` to `end`: the match itself first, then each by
/// its number, none for one that took no part. Of a repeated one, the
/// last time it matched counts.
#[must_use]
pub fn groups(
    tree: &Node,
    count: usize,
    text: &Chars,
    (start, end): (usize, usize),
) -> Vec<Option<(usize, usize)>> {
    let mut spans = alloc::vec![None; count + 1];
    spans[0] = Some((start, end));
    assign(tree, (start, end), text, &mut spans);
    spans
}

fn holds_group(node: &Node) -> bool {
    match node {
        Node::Group(..) => true,
        Node::Concat(items) | Node::Alt(items) => items.iter().any(holds_group),
        Node::Repeat(inside, ..) => holds_group(inside),
        _ => false,
    }
}

/// Puts in `spans` where the subexpressions in `node` matched, for
/// `node` matching from `start` to `end`.
fn assign(
    node: &Node,
    (start, end): (usize, usize),
    text: &Chars,
    spans: &mut [Option<(usize, usize)>],
) {
    if !holds_group(node) {
        return;
    }
    let len = text.chars.len();
    match node {
        Node::Group(number, inside) => {
            spans[*number] = Some((start, end));
            assign(inside, (start, end), text, spans);
        }
        Node::Concat(items) => {
            // Where the items from each on can start and still end at the
            // end: the last of these for the items after all of them.
            let mut rests = Vec::from([Places::one(len, end)]);
            for item in items.iter().rev() {
                let rest = backward(item, &rests[rests.len() - 1], text);
                rests.push(rest);
            }
            rests.reverse();
            let mut at = start;
            for (item, rest) in items.iter().zip(&rests[1..]) {
                let ends = forward(item, &Places::one(len, at), text);
                let Some(next) = ends.iter().filter(|&place| rest.holds(place)).last() else {
                    return;
                };
                assign(item, (at, next), text, spans);
                at = next;
            }
        }
        Node::Alt(branches) => {
            let from = Places::one(len, start);
            if let Some(branch) = branches
                .iter()
                .find(|branch| forward(branch, &from, text).holds(end))
            {
                assign(branch, (start, end), text, spans);
            }
        }
        Node::Repeat(inside, least, most) => {
            // Each round takes the longest it can and still leave the rest
            // a way to the end; at the end only rounds the least count
            // still needs may take nothing, so the last round finds none.
            let mut at = start;
            let mut done = 0;
            loop {
                let after = done + 1;
                let rest = repeat(
                    |places| backward(inside, places, text),
                    &Places::one(len, end),
                    least.saturating_sub(after),
                    most.map(|most| most.saturating_sub(after)),
                );
                let ends = forward(inside, &Places::one(len, at), text);
                let next = ends
                    .iter()
                    .filter(|&place| (place > at || done < *least) && rest.holds(place))
                    .last();
                let Some(next) = next else {
                    return;
                };
                assign(inside, (at, next), text, spans);
                at = next;
                done = after;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Syntax;
    use crate::dice::{Dice, pattern, text};
    use crate::parse::parse;
    use crate::vm::compile;

    fn chars(text: &str) -> Vec<Option<char>> {
        text.chars().map(Some).collect()
    }

    fn tree(pattern: &str) -> Node {
        parse(pattern, Syntax::Extended).unwrap().tree
    }

    /// The places `pattern` reaches from `start` in `text`.
    fn ends(pattern: &str, text: &str, start: usize) -> Vec<usize> {
        let chars = chars(text);
        let text = Chars {
            chars: &chars,
            either_case: false,
        };
        forward(&tree(pattern), &Places::one(chars.len(), start), &text)
            .iter()
            .collect()
    }

    /// Where each subexpression of `pattern` matched in its first match
    /// in `text`, which the VM finds.
    fn spans(pattern: &str, text: &str) -> Vec<Option<(usize, usize)>> {
        let parsed = parse(pattern, Syntax::Extended).unwrap();
        let chars = chars(text);
        let found = compile(&parsed.tree, false)
            .unwrap()
            .find(&chars, 0)
            .unwrap();
        let text = Chars {
            chars: &chars,
            either_case: false,
        };
        groups(&parsed.tree, parsed.groups, &text, found)
    }

    #[test]
    fn places_hold_what_was_put_in_across_words() {
        let mut places = Places::none(200);
        assert_eq!(places.iter().next(), None);
        assert_eq!(places.last(), None);
        for at in [0, 63, 64, 65, 127, 128, 200] {
            places.put(at);
        }
        assert_eq!(
            places.iter().collect::<Vec<_>>(),
            [0, 63, 64, 65, 127, 128, 200]
        );
        assert!(places.holds(64));
        assert!(!places.holds(62));
        assert!(!places.holds(201));
        assert!(!places.holds(5000), "far past the text");
        assert_eq!(places.last(), Some(200));
        assert_eq!(Places::one(200, 70).last(), Some(70));
        assert_eq!(Places::one(5, 0).last(), Some(0));
    }

    #[test]
    fn places_are_taken_in_and_left_out() {
        let mut some = Places::one(100, 3);
        assert!(some.take(&Places::one(100, 90)), "90 is new");
        assert!(!some.take(&Places::one(100, 3)), "3 is not");
        assert_eq!(some.iter().collect::<Vec<_>>(), [3, 90]);
        let left = some.without(&Places::one(100, 90));
        assert_eq!(left.iter().collect::<Vec<_>>(), [3]);
        assert_eq!(some.without(&some).iter().next(), None);
        assert_eq!(some.without(&Places::none(100)), some);
    }

    #[test]
    fn a_part_reaches_every_place_it_can_end_at() {
        assert_eq!(ends("a*", "aab", 0), [0, 1, 2]);
        assert_eq!(ends("a*", "aab", 2), [2]);
        assert_eq!(ends("a|ab", "abc", 0), [1, 2]);
        assert_eq!(ends("a{2,3}", "aaaa", 0), [2, 3]);
        assert_eq!(ends("a{2}", "aaaa", 1), [3]);
        assert_eq!(ends("(ab)+", "ababx", 0), [2, 4]);
        assert_eq!(ends("x", "ab", 0), Vec::<usize>::new());
        assert_eq!(ends("^a", "aa", 1), Vec::<usize>::new());
        assert_eq!(ends("^a", "aa", 0), [1]);
        assert_eq!(ends("a$", "aa", 1), [2]);
        assert_eq!(ends("a$", "aa", 0), Vec::<usize>::new());
        assert_eq!(ends("[^a].", "abc", 1), [3]);
        assert_eq!(ends("()", "ab", 1), [1]);
        assert_eq!(ends("(a*)*", "aa", 0), [0, 1, 2]);
        assert_eq!(ends("(a|b){0,2}", "abab", 0), [0, 1, 2]);
        assert_eq!(ends("a{0}", "a", 0), [0]);
    }

    #[test]
    fn a_part_reaches_back_to_every_place_it_can_start_at() {
        let starts = |pattern, text: &str, end| -> Vec<usize> {
            let chars = chars(text);
            let text = Chars {
                chars: &chars,
                either_case: true,
            };
            backward(&tree(pattern), &Places::one(chars.len(), end), &text)
                .iter()
                .collect()
        };
        assert_eq!(starts("a*", "aab", 2), [0, 1, 2]);
        assert_eq!(starts("a+b", "aab", 3), [0, 1]);
        assert_eq!(starts("a|ab", "abc", 2), [0]);
        assert_eq!(starts("^a", "aa", 1), [0]);
        assert_eq!(starts("^a", "aa", 2), Vec::<usize>::new());
        assert_eq!(starts("a$", "aa", 2), [1]);
        assert_eq!(starts("a$", "aa", 1), Vec::<usize>::new());
        assert_eq!(starts("B", "abc", 2), [1], "in either case");
        assert_eq!(starts(".", "abc", 0), Vec::<usize>::new());
        assert_eq!(starts("[bc]{2}", "abc", 3), [1]);
    }

    #[test]
    fn each_subexpression_takes_the_longest_it_can_from_the_left() {
        assert_eq!(
            spans("(a*)(b*)", "aabb"),
            [Some((0, 4)), Some((0, 2)), Some((2, 4))]
        );
        assert_eq!(
            spans("(a|ab)(c|bcd)(d*)", "abcd"),
            [Some((0, 4)), Some((0, 2)), Some((2, 3)), Some((3, 4))],
            "not a, bcd and nothing, as a first match would be"
        );
        assert_eq!(
            spans("(.*)x(.*)", "axbxc"),
            [Some((0, 5)), Some((0, 3)), Some((4, 5))]
        );
        assert_eq!(
            spans("(a{2,3})(a*)", "aaaa"),
            [Some((0, 4)), Some((0, 3)), Some((3, 4))]
        );
        assert_eq!(
            spans("(a)(b)?(c)", "ac"),
            [Some((0, 2)), Some((0, 1)), None, Some((1, 2))],
            "one that took no part"
        );
        assert_eq!(spans("^(a)|b", "b"), [Some((0, 1)), None]);
        assert_eq!(spans("x(y)?", "xy"), [Some((0, 2)), Some((1, 2))]);
    }

    #[test]
    fn a_repeated_subexpression_is_where_it_matched_last() {
        assert_eq!(spans("(a|b)*", "abab"), [Some((0, 4)), Some((3, 4))]);
        assert_eq!(spans("(a*)*", "aa"), [Some((0, 2)), Some((0, 2))]);
        assert_eq!(
            spans("(a*)+", "b"),
            [Some((0, 0)), Some((0, 0))],
            "once at least, if empty"
        );
        assert_eq!(spans("(a*)*", "b"), [Some((0, 0)), None]);
        assert_eq!(
            spans("(a*)+", "aa"),
            [Some((0, 2)), Some((0, 2))],
            "no empty round after the one the least count needs"
        );
        assert_eq!(
            spans("((a)|b)+", "ab"),
            [Some((0, 2)), Some((1, 2)), Some((0, 1))],
            "an inner one keeps where it matched in an earlier round"
        );
        assert_eq!(
            spans("(ab|a)(bc|c){2}", "abcc"),
            [Some((0, 4)), Some((0, 2)), Some((3, 4))]
        );
        assert_eq!(
            spans("(a){2}(a)", "aaa"),
            [Some((0, 3)), Some((1, 2)), Some((2, 3))]
        );
        assert_eq!(spans("(a){0}b", "b"), [Some((0, 1)), None]);
        assert_eq!(spans("(a|ab)*c", "abac"), [Some((0, 4)), Some((2, 3))]);
    }

    #[test]
    fn the_vm_finds_the_match_the_places_define() {
        let mut dice = Dice(0x9e37_79b9_7f4a_7c15);
        for _ in 0..600 {
            let pattern = pattern(&mut dice, 4);
            let tree = tree(&pattern);
            let program = compile(&tree, false).unwrap();
            for _ in 0..12 {
                let words = text(&mut dice);
                let chars = chars(&words);
                let text = Chars {
                    chars: &chars,
                    either_case: false,
                };
                for from in [0, 1] {
                    let defined = (from..=chars.len()).find_map(|start| {
                        forward(&tree, &Places::one(chars.len(), start), &text)
                            .last()
                            .map(|end| (start, end))
                    });
                    assert_eq!(
                        program.find(&chars, from),
                        defined,
                        "{pattern} in {words:?} from {from}"
                    );
                }
            }
        }
    }

    #[test]
    fn forward_and_backward_agree_on_every_span() {
        let mut dice = Dice(0x2545_f491_4f6c_dd1d);
        for _ in 0..300 {
            let pattern = pattern(&mut dice, 3);
            let tree = tree(&pattern);
            let words = text(&mut dice);
            let chars = chars(&words);
            let text = Chars {
                chars: &chars,
                either_case: false,
            };
            let len = chars.len();
            for start in 0..=len {
                let ends = forward(&tree, &Places::one(len, start), &text);
                for end in 0..=len {
                    let starts = backward(&tree, &Places::one(len, end), &text);
                    assert_eq!(
                        ends.holds(end),
                        starts.holds(start),
                        "{pattern} in {words:?} from {start} to {end}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_subexpression_lies_inside_its_match_and_matches_there() {
        let mut dice = Dice(0x8f1b_bcdc_bfa5_3e0b);
        for _ in 0..300 {
            let pattern = pattern(&mut dice, 4);
            let parsed = parse(&pattern, Syntax::Extended).unwrap();
            let program = compile(&parsed.tree, false).unwrap();
            let words = text(&mut dice);
            let chars = chars(&words);
            let Some(found) = program.find(&chars, 0) else {
                continue;
            };
            let text = Chars {
                chars: &chars,
                either_case: false,
            };
            let spans = groups(&parsed.tree, parsed.groups, &text, found);
            assert_eq!(spans[0], Some(found));
            for span in spans.iter().flatten() {
                assert!(
                    found.0 <= span.0 && span.0 <= span.1 && span.1 <= found.1,
                    "{pattern} in {words:?}: {span:?} outside {found:?}"
                );
            }
        }
    }

    /// Where each subexpression of `pattern`, read as `syntax`, matched in
    /// its first match in `chars`.
    fn spans_in(
        pattern: &str,
        syntax: Syntax,
        either_case: bool,
        chars: &[Option<char>],
    ) -> Vec<Option<(usize, usize)>> {
        let parsed = parse(pattern, syntax).unwrap();
        let found = compile(&parsed.tree, either_case)
            .unwrap()
            .find(chars, 0)
            .unwrap();
        let text = Chars { chars, either_case };
        groups(&parsed.tree, parsed.groups, &text, found)
    }

    #[test]
    fn basic_groups_split_a_line_as_sed_would() {
        let line = chars("key = some value");
        assert_eq!(
            spans_in("^\\([a-z]*\\) *= *\\(.*\\)$", Syntax::Basic, false, &line),
            [Some((0, 16)), Some((0, 3)), Some((6, 16))]
        );
        let date = chars("on 2026-10-27, late");
        assert_eq!(
            spans_in(
                "\\([0-9]\\{4\\}\\)-\\([0-9]*\\)-\\([0-9]*\\)",
                Syntax::Basic,
                false,
                &date
            ),
            [Some((3, 13)), Some((3, 7)), Some((8, 10)), Some((11, 13))]
        );
        let path = chars("/usr/local/bin");
        assert_eq!(
            spans_in("\\(.*\\)/\\(.*\\)", Syntax::Basic, false, &path),
            [Some((0, 14)), Some((0, 10)), Some((11, 14))],
            "the first takes up to the last /"
        );
    }

    #[test]
    fn groups_match_in_either_case_and_over_bytes_that_are_no_utf8() {
        assert_eq!(
            spans_in("(A+)(b)", Syntax::Extended, true, &chars("xaaB")),
            [Some((1, 4)), Some((1, 3)), Some((3, 4))]
        );
        let bytes = [Some('a'), None, None, Some('b')];
        assert_eq!(
            spans_in("a(.*)b", Syntax::Extended, false, &bytes),
            [Some((0, 4)), Some((1, 3))]
        );
        assert_eq!(
            spans_in("([^a]+)", Syntax::Extended, false, &bytes),
            [Some((1, 4)), Some((1, 4))]
        );
    }

    #[test]
    fn groups_inside_groups_and_counted_rounds() {
        assert_eq!(
            spans("((a)(b)?)+", "aba"),
            [Some((0, 3)), Some((2, 3)), Some((2, 3)), Some((1, 2))]
        );
        assert_eq!(
            spans("((ab){2})c", "ababc"),
            [Some((0, 5)), Some((0, 4)), Some((2, 4))]
        );
        assert_eq!(
            spans("(a(b(c)))", "abc"),
            [Some((0, 3)), Some((0, 3)), Some((1, 3)), Some((2, 3))]
        );
        assert_eq!(
            spans("(a|(b))(c|(d))", "bd"),
            [
                Some((0, 2)),
                Some((0, 1)),
                Some((0, 1)),
                Some((1, 2)),
                Some((1, 2))
            ]
        );
        assert_eq!(
            spans("(a){1,3}(a){2}", "aaaa"),
            [Some((0, 4)), Some((1, 2)), Some((3, 4))],
            "the first rounds as many as leave the second its two"
        );
        assert_eq!(spans("(^a)?(b$)?", "b"), [Some((0, 1)), None, Some((0, 1))]);
    }

    #[test]
    fn groups_of_a_long_line_are_found_in_little_time() {
        let long = "a".repeat(3000) + &"b".repeat(3000);
        assert_eq!(
            spans("(a*)(b*)", &long),
            [Some((0, 6000)), Some((0, 3000)), Some((3000, 6000))]
        );
        assert_eq!(
            spans("(.*)(b)", &long),
            [Some((0, 6000)), Some((0, 5999)), Some((5999, 6000))]
        );
    }

    #[test]
    fn places_mark_each_word_on_its_own() {
        let len = 130;
        let mut places = Places::one(len, 64);
        assert!(!places.take(&Places::one(len, 64)));
        assert!(places.take(&Places::one(len, 128)), "a later word");
        assert!(places.take(&Places::one(len, 0)), "an earlier word");
        assert_eq!(places.iter().collect::<Vec<_>>(), [0, 64, 128]);
        assert_eq!(
            places
                .without(&Places::one(len, 64))
                .iter()
                .collect::<Vec<_>>(),
            [0, 128]
        );
        assert_eq!(Places::one(63, 63).last(), Some(63));
        assert_eq!(
            Places::one(64, 64).last(),
            Some(64),
            "one place past a word"
        );
        assert_eq!(Places::none(0).iter().count(), 0);
        assert_eq!(Places::one(0, 0).iter().collect::<Vec<_>>(), [0]);
    }
}
