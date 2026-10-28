//! tsort: reads pairs of items, each saying its first comes before its
//! second, from a file or its standard input, and writes every item once
//! in an order that keeps all of them, as POSIX's tsort does; a pair of
//! one item twice names it alone. Of items free to come next, the one
//! named first comes first. A loop is said with its items and broken at
//! the first of them, and tsort then ends with 1.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Handle, text};

program::main!(tsort);

/// The items and what each must come before.
#[derive(Default)]
struct Graph {
    names: Vec<String>,
    index: BTreeMap<String, usize>,
    /// The items each comes before, and how many come before each.
    after: Vec<Vec<usize>>,
    before: Vec<usize>,
}

impl Graph {
    /// The number of the item `name`, a new one the first time.
    fn item(&mut self, name: &str) -> usize {
        if let Some(&at) = self.index.get(name) {
            return at;
        }
        let at = self.names.len();
        self.names.push(String::from(name));
        self.index.insert(String::from(name), at);
        self.after.push(Vec::new());
        self.before.push(0);
        at
    }

    /// Takes the pair of `first` and `second`.
    fn pair(&mut self, first: &str, second: &str) {
        let a = self.item(first);
        let b = self.item(second);
        if a != b {
            self.after[a].push(b);
            self.before[b] += 1;
        }
    }

    /// The items in order, and the loops that had to be broken for it,
    /// each its items around it from where it was broken.
    fn sorted(mut self) -> (Vec<String>, Vec<Vec<String>>) {
        let n = self.names.len();
        let mut done = alloc::vec![false; n];
        let mut free: VecDeque<usize> = (0..n).filter(|&i| self.before[i] == 0).collect();
        let mut order = Vec::new();
        let mut loops = Vec::new();
        while order.len() < n {
            let Some(next) = free.pop_front() else {
                // Every item left waits for another one left, so walking
                // back from one to an item it waits for comes to an item
                // again, and what lies between is a loop.
                let start = (0..n).find(|&i| !done[i]).unwrap_or(0);
                let mut seen = Vec::new();
                let mut at = start;
                while !seen.contains(&at) {
                    seen.push(at);
                    at = (0..n)
                        .find(|&p| !done[p] && self.after[p].contains(&at))
                        .unwrap_or(start);
                }
                let first = seen.iter().position(|&i| i == at).unwrap_or(0);
                let mut items = seen.split_off(first);
                items.reverse();
                // It is said, and broken, from its item named first.
                let named_first = (0..items.len()).min_by_key(|&k| items[k]).unwrap_or(0);
                items.rotate_left(named_first);
                loops.push(items.iter().map(|&i| self.names[i].clone()).collect());
                self.before[items[0]] = 0;
                free.push_back(items[0]);
                continue;
            };
            if done[next] {
                continue;
            }
            done[next] = true;
            order.push(self.names[next].clone());
            for &b in &self.after[next] {
                self.before[b] = self.before[b].saturating_sub(1);
                if self.before[b] == 0 && !done[b] {
                    free.push_back(b);
                }
            }
        }
        (order, loops)
    }
}

/// The graph the pairs of `text` make, or why they make none.
fn graph(text: &str) -> Result<Graph, &'static str> {
    let items: Vec<&str> = text.split_ascii_whitespace().collect();
    if items.len() % 2 == 1 {
        return Err("an item without the other of its pair");
    }
    let mut graph = Graph::default();
    for pair in items.chunks(2) {
        graph.pair(pair[0], pair[1]);
    }
    Ok(graph)
}

#[cfg_attr(test, allow(dead_code))]
fn tsort(_: Handle) -> u64 {
    let (path, name) = match program::arguments() {
        [] => (None, "-"),
        [path] => (Some(path.as_str()).filter(|&p| p != "-"), path.as_str()),
        _ => {
            let _ = program::write_error("tsort: sorts one file\n");
            return 1;
        }
    };
    let mut bytes = Vec::new();
    if let Err(e) = text::pieces(path, |piece| {
        bytes.extend_from_slice(piece);
        true
    }) {
        let _ = program::write_error(&text::trouble("tsort", name, e));
        return 1;
    }
    let graph = match graph(&String::from_utf8_lossy(&bytes)) {
        Ok(graph) => graph,
        Err(why) => {
            let _ = program::write_error(&format!("tsort: {name}: {why}\n"));
            return 1;
        }
    };
    let (order, loops) = graph.sorted();
    for items in &loops {
        let _ = program::write_error(&format!("tsort: {name}: a loop of {}\n", items.join(" ")));
    }
    let mut out = text::standard_output();
    for item in &order {
        out.put(item.as_bytes());
        out.put(b"\n");
    }
    u64::from(!out.finish() || !loops.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(text: &str) -> (String, Vec<String>) {
        let (order, loops) = graph(text).unwrap().sorted();
        let order = order.iter().fold(String::new(), |all, i| all + i + " ");
        (
            order.trim_end().into(),
            loops.iter().map(|l| l.join(" ")).collect(),
        )
    }

    #[test]
    fn items_free_to_come_next_come_in_the_order_they_were_named() {
        assert_eq!(sorted("a b\nc d\n"), ("a c b d".into(), Vec::new()));
        assert_eq!(sorted("b c\na b\n"), ("a b c".into(), Vec::new()));
        assert_eq!(
            sorted("a a\nb b\n"),
            ("a b".into(), Vec::new()),
            "an item alone"
        );
        assert_eq!(sorted(""), (String::new(), Vec::new()));
        assert_eq!(
            sorted("x y x y"),
            ("x y".into(), Vec::new()),
            "a pair twice counts once in the order"
        );
    }

    #[test]
    fn a_loop_is_said_and_broken_at_its_first_item() {
        assert_eq!(
            sorted("a b\nb c\nc a\nd e\n"),
            ("d e a b c".into(), Vec::from(["a b c".into()]))
        );
        assert_eq!(sorted("a b b a"), ("a b".into(), Vec::from(["a b".into()])));
        assert_eq!(
            sorted("x a a b b a b y"),
            ("x a b y".into(), Vec::from(["a b".into()])),
            "what follows a loop still comes after it"
        );
        assert_eq!(
            sorted("p q q p r s s r"),
            ("p q r s".into(), Vec::from(["p q".into(), "r s".into()])),
            "two loops"
        );
    }

    #[test]
    fn an_item_without_its_pair_is_refused() {
        assert_eq!(
            graph("a b c").err(),
            Some("an item without the other of its pair")
        );
        assert!(graph("a\tb\n\n c  d").is_ok(), "any blanks part the items");
    }

    #[test]
    fn random_pairs_come_out_in_an_order_that_keeps_them() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            // Pairs that go from a lower to a higher number make no loop.
            let pairs: Vec<(u64, u64)> = (0..seed % 20)
                .map(|n| {
                    let a = (seed >> (n % 50)) % 9;
                    (a, a + 1 + (seed >> (n % 40)) % 3)
                })
                .collect();
            let text = pairs.iter().fold(String::new(), |all, (a, b)| {
                all + &alloc::format!("i{a} i{b}\n")
            });
            let (order, loops) = graph(&text).unwrap().sorted();
            assert_eq!(loops, Vec::<Vec<String>>::new());
            let place = |item: u64| {
                order
                    .iter()
                    .position(|o| *o == alloc::format!("i{item}"))
                    .unwrap()
            };
            for (a, b) in &pairs {
                assert!(place(*a) < place(*b), "{a} before {b} in {order:?}");
            }
            let mut unique = order.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), order.len(), "each item once");
        }
    }

    #[test]
    fn every_loop_said_is_one_and_every_item_comes_once() {
        let mut seed = 0x6c07_8965_5d0c_2bb3_u64;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let pairs: Vec<(u64, u64)> = (0..seed % 15)
                .map(|n| ((seed >> (n % 50)) % 6, (seed >> (n % 45 + 3)) % 6))
                .collect();
            let text = pairs.iter().fold(String::new(), |all, (a, b)| {
                all + &alloc::format!("i{a} i{b} ")
            });
            let (order, loops) = graph(&text).unwrap().sorted();
            let named: alloc::collections::BTreeSet<String> = pairs
                .iter()
                .flat_map(|(a, b)| [alloc::format!("i{a}"), alloc::format!("i{b}")])
                .collect();
            let mut each = order.clone();
            each.sort_unstable();
            assert_eq!(
                each,
                named.into_iter().collect::<Vec<_>>(),
                "each item once"
            );
            // A pair neither of whose items lies on a loop keeps its order:
            // only an item of a loop is let go before what it waits for.
            let looped: alloc::collections::BTreeSet<&String> = loops.iter().flatten().collect();
            let place = |item: &String| order.iter().position(|o| o == item).unwrap();
            for (a, b) in &pairs {
                let (a, b) = (alloc::format!("i{a}"), alloc::format!("i{b}"));
                if a != b && !looped.contains(&a) && !looped.contains(&b) {
                    assert!(
                        place(&a) < place(&b),
                        "{a} before {b} in {order:?} of {text:?}"
                    );
                }
            }
            let pair = |a: &String, b: &String| {
                pairs
                    .iter()
                    .any(|(x, y)| alloc::format!("i{x}") == *a && alloc::format!("i{y}") == *b)
            };
            for items in &loops {
                assert!(items.len() >= 2, "{items:?}");
                for (at, item) in items.iter().enumerate() {
                    let next = &items[(at + 1) % items.len()];
                    assert!(pair(item, next), "{item} before {next} in {text:?}");
                }
            }
        }
    }

    #[test]
    fn items_come_back_as_they_were_named() {
        assert_eq!(
            sorted("\u{e9}t\u{e9} /a/b\n/a/b a-b.c\n"),
            ("\u{e9}t\u{e9} /a/b a-b.c".into(), Vec::new())
        );
        let long = "x".repeat(300);
        assert_eq!(
            sorted(&alloc::format!("{long} y")).0,
            alloc::format!("{long} y")
        );
    }
}
