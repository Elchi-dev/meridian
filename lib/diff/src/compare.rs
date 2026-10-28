//! The lines two texts have in common, the most there can be, and the
//! runs of lines between them that differ.

use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;

/// A run of lines that differ: those of the first text left out and
/// those of the second put in their place, either of them none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub a: Range<usize>,
    pub b: Range<usize>,
}

/// Where the shortest way through two texts, neither of them empty, may
/// be cut in two: a line of each that the way passes between.
fn middle<T: PartialEq>(a: &[T], b: &[T]) -> Option<(usize, usize)> {
    let long = isize::try_from(a.len()).ok()?;
    let wide = isize::try_from(b.len()).ok()?;
    // Neither way takes more steps than this before the two meet.
    let most = (long + wide + 1) / 2;
    // Each diagonal, the steps along a less those along b, by its place
    // in the lists.
    let place = |diagonal: isize| usize::try_from(diagonal + most).ok();
    let size = usize::try_from(2 * most + 1).ok()?;
    // How far along a each diagonal has come, from the start forwards and
    // from the end backwards; -1 where none came yet.
    let mut forward = vec![-1isize; size];
    let mut backward = vec![-1isize; size];
    forward[place(1)?] = 0;
    backward[place(1)?] = 0;
    let delta = long - wide;
    let odd = delta % 2 != 0;
    let same = |along: isize, down: isize| -> Option<bool> {
        Some(a[usize::try_from(along).ok()?] == b[usize::try_from(down).ok()?])
    };
    // How far a diagonal starts this round: on from the one above or
    // down from the one below, whichever came further. The first and the
    // last of a round have one of them only.
    let start = |seen: &[isize], diagonal: isize, edits: isize| -> Option<isize> {
        if diagonal == -edits {
            return Some(seen[place(diagonal + 1)?]);
        }
        let below = seen[place(diagonal - 1)?];
        if diagonal == edits {
            return Some(below + 1);
        }
        Some(below.max(seen[place(diagonal + 1)?] - 1) + 1)
    };
    let mut edits = 0;
    loop {
        for diagonal in (-edits..=edits).step_by(2) {
            let mut along = start(&forward, diagonal, edits)?;
            let mut down = along - diagonal;
            while along < long && down < wide && same(along, down)? {
                along += 1;
                down += 1;
            }
            forward[place(diagonal)?] = along;
            // A way may run on past the end of a text, so one that has
            // not come to a diagonal yet, at -1, meets none there.
            let back = place(delta - diagonal).and_then(|at| backward.get(at));
            if odd && back.is_some_and(|&back| back != -1 && along >= long - back) {
                return Some((usize::try_from(along).ok()?, usize::try_from(down).ok()?));
            }
        }
        for diagonal in (-edits..=edits).step_by(2) {
            let mut along = start(&backward, diagonal, edits)?;
            let mut down = along - diagonal;
            while along < long && down < wide && same(long - along - 1, wide - down - 1)? {
                along += 1;
                down += 1;
            }
            backward[place(diagonal)?] = along;
            let front = place(delta - diagonal).and_then(|at| forward.get(at));
            if let Some(&front) =
                front.filter(|&&front| !odd && front != -1 && front >= long - along)
            {
                let front_down = front - (delta - diagonal);
                return Some((
                    usize::try_from(front).ok()?,
                    usize::try_from(front_down).ok()?,
                ));
            }
        }
        edits += 1;
    }
}

/// What is left to do: compare two parts, a range of lines of each, or
/// hand out a pair of lines found in common already.
enum Task {
    Compare(usize, usize, usize, usize),
    Pair(usize, usize),
}

/// The lines `a` and `b` have in common, the most there can be, as pairs
/// of their places in each, in order.
#[must_use]
pub fn common<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    // Taken from the end: what is to come first goes on last.
    let mut tasks = vec![Task::Compare(0, a.len(), 0, b.len())];
    while let Some(task) = tasks.pop() {
        let (mut a0, mut a1, mut b0, mut b1) = match task {
            Task::Pair(x, y) => {
                pairs.push((x, y));
                continue;
            }
            Task::Compare(a0, a1, b0, b1) => (a0, a1, b0, b1),
        };
        while a0 < a1 && b0 < b1 && a[a0] == b[b0] {
            pairs.push((a0, b0));
            a0 += 1;
            b0 += 1;
        }
        // The lines both end with come after all between, so they go on
        // first, the last of them lowest.
        while a0 < a1 && b0 < b1 && a[a1 - 1] == b[b1 - 1] {
            a1 -= 1;
            b1 -= 1;
            tasks.push(Task::Pair(a1, b1));
        }
        if a0 == a1 || b0 == b1 {
            continue;
        }
        // Cut where the shortest way passes. With the lines both start
        // and end with gone, it takes two steps at least, and the cut
        // lies between them.
        if let Some((x, y)) = middle(&a[a0..a1], &b[b0..b1]) {
            tasks.push(Task::Compare(a0 + x, a1, b0 + y, b1));
            tasks.push(Task::Compare(a0, a0 + x, b0, b0 + y));
        }
    }
    pairs
}

/// The runs of lines between those `a` and `b` have in common.
#[must_use]
pub fn hunks<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Hunk> {
    let mut found = Vec::new();
    let (mut x, mut y) = (0, 0);
    for (next_x, next_y) in common(a, b).into_iter().chain([(a.len(), b.len())]) {
        if next_x > x || next_y > y {
            found.push(Hunk {
                a: x..next_x,
                b: y..next_y,
            });
        }
        x = next_x + 1;
        y = next_y + 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many lines two texts have in common at most, found the slow
    /// way, filling in a table of every pair of starts.
    fn longest(a: &[u8], b: &[u8]) -> usize {
        let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                table[i][j] = if a[i] == b[j] {
                    table[i + 1][j + 1] + 1
                } else {
                    table[i + 1][j].max(table[i][j + 1])
                };
            }
        }
        table[0][0]
    }

    /// `a` with each hunk's lines of it put back as the hunk's lines of `b`.
    fn applied(a: &[u8], b: &[u8], hunks: &[Hunk]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut at = 0;
        for hunk in hunks {
            out.extend_from_slice(&a[at..hunk.a.start]);
            out.extend_from_slice(&b[hunk.b.clone()]);
            at = hunk.a.end;
        }
        out.extend_from_slice(&a[at..]);
        out
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn what_posix_s_example_changes() {
        let a = b"abcdefg";
        let b = b"aBcdfgh";
        assert_eq!(
            hunks(a, b),
            [
                Hunk { a: 1..2, b: 1..2 },
                Hunk { a: 4..5, b: 4..4 },
                Hunk { a: 7..7, b: 6..7 },
            ]
        );
        assert_eq!(common(a, b), [(0, 0), (2, 2), (3, 3), (5, 4), (6, 5)]);
    }

    #[test]
    fn texts_alike_empty_or_with_nothing_in_common() {
        assert_eq!(hunks(b"abc", b"abc"), []);
        assert_eq!(hunks(b"", b""), []);
        assert_eq!(hunks(b"", b"ab"), [Hunk { a: 0..0, b: 0..2 }]);
        assert_eq!(hunks(b"ab", b""), [Hunk { a: 0..2, b: 0..0 }]);
        assert_eq!(hunks(b"ab", b"cd"), [Hunk { a: 0..2, b: 0..2 }]);
        assert_eq!(common(b"a", b"b"), []);
    }

    #[test]
    fn random_texts_keep_the_most_lines_in_common_and_change_into_each_other() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..2000 {
            let a: Vec<u8> = (0..shuffled(&mut seed) % 14)
                .map(|_| b'a' + (shuffled(&mut seed) % 4) as u8)
                .collect();
            let b: Vec<u8> = (0..shuffled(&mut seed) % 14)
                .map(|_| b'a' + (shuffled(&mut seed) % 4) as u8)
                .collect();
            let pairs = common(&a, &b);
            assert_eq!(pairs.len(), longest(&a, &b), "{a:?} {b:?}");
            for window in pairs.windows(2) {
                assert!(window[0].0 < window[1].0 && window[0].1 < window[1].1);
            }
            assert!(pairs.iter().all(|&(x, y)| a[x] == b[y]));
            let found = hunks(&a, &b);
            assert_eq!(applied(&a, &b, &found), b);
            assert!(found.iter().all(|h| !h.a.is_empty() || !h.b.is_empty()));
            for window in found.windows(2) {
                assert!(
                    window[0].a.end < window[1].a.start || window[0].b.end < window[1].b.start,
                    "hunks apart"
                );
            }
        }
    }

    #[test]
    fn the_middle_lies_on_a_shortest_way_through_both_texts() {
        let mut seed = 0x0123_4567_89ab_cdef_u64;
        for round in 0..3000 {
            // Lengths far apart as well as near.
            let long = 1 + shuffled(&mut seed) % if round % 2 == 0 { 4 } else { 30 };
            let wide = 1 + shuffled(&mut seed) % 30;
            let letters = 2 + shuffled(&mut seed) % 3;
            let mut text = |length: u64| -> Vec<u8> {
                (0..length)
                    .map(|_| b'a' + u8::try_from(shuffled(&mut seed) % letters).unwrap())
                    .collect()
            };
            let (a, b) = (text(long), text(wide));
            let (x, y) = middle(&a, &b).unwrap();
            assert!(x <= a.len() && y <= b.len(), "{a:?} {b:?} cut at {x} {y}");
            assert_eq!(
                longest(&a[..x], &b[..y]) + longest(&a[x..], &b[y..]),
                longest(&a, &b),
                "{a:?} {b:?} cut at {x} {y}"
            );
        }
    }

    #[test]
    fn long_texts_that_differ_a_little_and_a_lot() {
        let a: Vec<u32> = (0..5000).collect();
        let mut b = a.clone();
        b.remove(2500);
        b.insert(10, 99_999);
        assert_eq!(
            hunks(&a, &b),
            [
                Hunk {
                    a: 10..10,
                    b: 10..11
                },
                Hunk {
                    a: 2500..2501,
                    b: 2501..2501
                }
            ]
        );
        let c: Vec<u32> = (0..3000).map(|n| n * 2).collect();
        let d: Vec<u32> = (0..3000).map(|n| n * 3).collect();
        let pairs = common(&c, &d);
        assert_eq!(pairs.len(), 1000, "the multiples of six");
        assert!(pairs.iter().all(|&(x, y)| c[x] == d[y]));
    }
}
