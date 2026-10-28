//! The forms diff writes differences in: its own, with context, unified
//! with context, and as a script for ed, backwards or forwards.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use crate::compare::Hunk;
use crate::read::{Edit, Line, Mark};

/// A text as diff reads it: its lines, and whether its last line ends
/// with a line feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text<'a> {
    pub lines: Vec<&'a [u8]>,
    pub ended: bool,
}

impl<'a> Text<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Text<'a> {
        let Some((&last, body)) = bytes.split_last() else {
            return Text {
                lines: Vec::new(),
                ended: true,
            };
        };
        let ended = last == b'\n';
        let body = if ended { body } else { bytes };
        Text {
            lines: body.split(|&b| b == b'\n').collect(),
            ended,
        }
    }

    /// Whether line `at` is the last, and has no line feed after it.
    fn unended(&self, at: usize) -> bool {
        !self.ended && at + 1 == self.lines.len()
    }
}

/// What a line is compared by: with `blanks`, each run of blanks as one
/// and none at its end, as `-b` asks. A last line without its line feed
/// is otherwise never the same as one with it.
fn key(text: &Text, at: usize, blanks: bool) -> Vec<u8> {
    let line = text.lines[at];
    let mut key = if blanks {
        let mut squeezed = Vec::new();
        let mut blank = false;
        for &byte in line {
            if matches!(byte, b' ' | b'\t') {
                blank = true;
                continue;
            }
            if blank {
                squeezed.push(b' ');
            }
            squeezed.push(byte);
            blank = false;
        }
        squeezed
    } else {
        line.to_vec()
    };
    // No line holds a line feed, so this sets the last one apart.
    if text.unended(at) && !blanks {
        key.push(b'\n');
    }
    key
}

/// The lines of `a` and `b` as numbers, the same for lines that compare
/// the same, so the comparing compares numbers.
#[must_use]
pub fn keys(a: &Text, b: &Text, blanks: bool) -> (Vec<usize>, Vec<usize>) {
    let mut seen: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    let mut number = |text: &Text, at: usize| {
        let next = seen.len();
        *seen.entry(key(text, at, blanks)).or_insert(next)
    };
    let left = (0..a.lines.len()).map(|at| number(a, at)).collect();
    let right = (0..b.lines.len()).map(|at| number(b, at)).collect();
    (left, right)
}

/// The form differences are written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Normal,
    /// With so many lines around each change.
    Context(usize),
    Unified(usize),
    /// A script for ed that makes the second text of the first, from its
    /// last change to its first.
    Ed,
    /// Its commands from the first change on, letter first, which no ed
    /// takes.
    Forward,
}

impl Form {
    /// Whether the form can say that a last line has no line feed: the
    /// ed scripts cannot.
    #[must_use]
    pub fn says_unended(self) -> bool {
        !matches!(self, Form::Ed | Form::Forward)
    }
}

/// What diff says after a last line that has no line feed.
const UNENDED: &[u8] = b"\\ No newline at end of file\n";

/// A range of lines as diff's own form and ed write it, counted from 1:
/// one number for one line, the first and the last for more.
fn range(lines: &Range<usize>, apart: &str) -> String {
    if lines.len() == 1 {
        format!("{}", lines.start + 1)
    } else {
        format!("{}{apart}{}", lines.start + 1, lines.end)
    }
}

/// Writes line `at` of `text` after `mark`, and diff's note when it is
/// the last and has no line feed.
fn put(out: &mut Vec<u8>, mark: &[u8], text: &Text, at: usize) {
    out.extend_from_slice(mark);
    out.extend_from_slice(text.lines[at]);
    out.push(b'\n');
    if text.unended(at) {
        out.extend_from_slice(UNENDED);
    }
}

/// Writes the differences `hunks` between `a` and `b`, named `names`,
/// in `form` onto `out`; nothing when there are none.
pub fn write(
    form: Form,
    (a, b): (&Text, &Text),
    hunks: &[Hunk],
    names: (&str, &str),
    out: &mut Vec<u8>,
) {
    if hunks.is_empty() {
        return;
    }
    match form {
        Form::Normal => normal(a, b, hunks, out),
        Form::Context(around) => {
            out.extend_from_slice(format!("*** {}\n--- {}\n", names.0, names.1).as_bytes());
            for group in groups(hunks, around, a.lines.len()) {
                context(a, b, &group, out);
            }
        }
        Form::Unified(around) => {
            out.extend_from_slice(format!("--- {}\n+++ {}\n", names.0, names.1).as_bytes());
            for group in groups(hunks, around, a.lines.len()) {
                unified(a, b, &group, out);
            }
        }
        Form::Ed => {
            for hunk in hunks.iter().rev() {
                let command = match (hunk.a.is_empty(), hunk.b.is_empty()) {
                    (true, _) => format!("{}a\n", hunk.a.start),
                    (_, true) => format!("{}d\n", range(&hunk.a, ",")),
                    _ => format!("{}c\n", range(&hunk.a, ",")),
                };
                out.extend_from_slice(command.as_bytes());
                if !hunk.b.is_empty() {
                    lines_for_ed(b, &hunk.b, out);
                }
            }
        }
        Form::Forward => {
            for hunk in hunks {
                let command = match (hunk.a.is_empty(), hunk.b.is_empty()) {
                    (true, _) => format!("a{}\n", hunk.a.start),
                    (_, true) => format!("d{}\n", range(&hunk.a, " ")),
                    _ => format!("c{}\n", range(&hunk.a, " ")),
                };
                out.extend_from_slice(command.as_bytes());
                if !hunk.b.is_empty() {
                    for at in hunk.b.clone() {
                        out.extend_from_slice(b.lines[at]);
                        out.push(b'\n');
                    }
                    out.extend_from_slice(b".\n");
                }
            }
        }
    }
}

/// Diff's own form: each hunk as what it does, then the lines it leaves
/// out and those it puts in.
fn normal(a: &Text, b: &Text, hunks: &[Hunk], out: &mut Vec<u8>) {
    for hunk in hunks {
        let header = match (hunk.a.is_empty(), hunk.b.is_empty()) {
            (true, _) => format!("{}a{}\n", hunk.a.start, range(&hunk.b, ",")),
            (_, true) => format!("{}d{}\n", range(&hunk.a, ","), hunk.b.start),
            _ => format!("{}c{}\n", range(&hunk.a, ","), range(&hunk.b, ",")),
        };
        out.extend_from_slice(header.as_bytes());
        for at in hunk.a.clone() {
            put(out, b"< ", a, at);
        }
        if !hunk.a.is_empty() && !hunk.b.is_empty() {
            out.extend_from_slice(b"---\n");
        }
        for at in hunk.b.clone() {
            put(out, b"> ", b, at);
        }
    }
}

/// The lines an ed script puts in. A dot alone would end what ed takes,
/// so such a line goes in as two dots, put right with a substitution
/// once the lines are in, and ed is told to take more after it.
fn lines_for_ed(text: &Text, lines: &Range<usize>, out: &mut Vec<u8>) {
    for at in lines.clone() {
        if at > lines.start && text.lines[at - 1] == b"." {
            out.extend_from_slice(b".\ns/.//\na\n");
        }
        if text.lines[at] == b"." {
            out.extend_from_slice(b"..\n");
        } else {
            out.extend_from_slice(text.lines[at]);
            out.push(b'\n');
        }
    }
    out.extend_from_slice(b".\n");
    if text.lines[lines.end - 1] == b"." {
        out.extend_from_slice(b"s/.//\n");
    }
}

/// Hunks close enough to share the lines around them, and the lines of
/// each text the group shows: so many before the first and after the
/// last, as far as the texts go.
struct Group<'h> {
    hunks: &'h [Hunk],
    a: Range<usize>,
    b: Range<usize>,
}

/// `hunks` in groups, a hunk in the group before it when no more than
/// twice `around` lines lie between them. As many lines are alike
/// before the first hunk in both texts, and after the last, as in the
/// first text, `long` lines long.
fn groups(hunks: &[Hunk], around: usize, long: usize) -> Vec<Group<'_>> {
    let mut found = Vec::new();
    let mut first = 0;
    for at in 0..hunks.len() {
        let last = at + 1 == hunks.len();
        if last || hunks[at + 1].a.start - hunks[at].a.end > around.saturating_mul(2) {
            let (head, tail) = (&hunks[first], &hunks[at]);
            let before = around.min(head.a.start);
            let after = around.min(long - tail.a.end);
            found.push(Group {
                hunks: &hunks[first..=at],
                a: head.a.start - before..tail.a.end + after,
                b: head.b.start - before..tail.b.end + after,
            });
            first = at + 1;
        }
    }
    found
}

/// A range of lines as the unified form writes it: where it starts,
/// counted from 1, and how many, left out for one. A range of none
/// starts at the line before it.
fn unified_range(lines: &Range<usize>) -> String {
    match lines.len() {
        0 => format!("{},0", lines.start),
        1 => format!("{}", lines.start + 1),
        count => format!("{},{count}", lines.start + 1),
    }
}

/// One group in the unified form: the lines around as they are, those
/// left out after a minus and those put in after a plus.
fn unified(a: &Text, b: &Text, group: &Group, out: &mut Vec<u8>) {
    let header = format!(
        "@@ -{} +{} @@\n",
        unified_range(&group.a),
        unified_range(&group.b)
    );
    out.extend_from_slice(header.as_bytes());
    let mut at = group.a.start;
    for hunk in group.hunks {
        for same in at..hunk.a.start {
            put(out, b" ", a, same);
        }
        for gone in hunk.a.clone() {
            put(out, b"-", a, gone);
        }
        for added in hunk.b.clone() {
            put(out, b"+", b, added);
        }
        at = hunk.a.end;
    }
    for same in at..group.a.end {
        put(out, b" ", a, same);
    }
}

/// A range of lines as the context form writes it: the first and the
/// last, counted from 1, one number for one line, and for none the line
/// before.
fn context_range(lines: &Range<usize>) -> String {
    match lines.len() {
        0 => format!("{}", lines.start),
        1 => format!("{}", lines.start + 1),
        _ => format!("{},{}", lines.start + 1, lines.end),
    }
}

/// One group in the context form: the first text's lines, if any were
/// left out or changed, then the second's, if any were put in or
/// changed. A changed line is marked `! `, one left out `- ` and one put
/// in `+ `.
fn context(a: &Text, b: &Text, group: &Group, out: &mut Vec<u8>) {
    let header = format!("***************\n*** {} ****\n", context_range(&group.a));
    out.extend_from_slice(header.as_bytes());
    if group.hunks.iter().any(|h| !h.a.is_empty()) {
        let sides = group.hunks.iter().map(|h| (&h.a, h.b.is_empty()));
        side(a, &group.a, sides, b"- ", out);
    }
    out.extend_from_slice(format!("--- {} ----\n", context_range(&group.b)).as_bytes());
    if group.hunks.iter().any(|h| !h.b.is_empty()) {
        let sides = group.hunks.iter().map(|h| (&h.b, h.a.is_empty()));
        side(b, &group.b, sides, b"+ ", out);
    }
}

/// The lines of one text in the context form: `lines` of `text`, and
/// each hunk's own as `hunks` gives them, marked `alone` when the other
/// text has none there and `! ` when it has some.
fn side<'h>(
    text: &Text,
    lines: &Range<usize>,
    hunks: impl Iterator<Item = (&'h Range<usize>, bool)>,
    alone: &[u8],
    out: &mut Vec<u8>,
) {
    let mut at = lines.start;
    for (own, only) in hunks {
        for same in at..own.start {
            put(out, b"  ", text, same);
        }
        let mark = if only { alone } else { b"! " };
        for changed in own.clone() {
            put(out, mark, text, changed);
        }
        at = own.end;
    }
    for same in at..lines.end {
        put(out, b"  ", text, same);
    }
}

/// Writes the hunks `edits`, which patch could not put in, as patch
/// keeps them: in the unified form when `unified`, else with context,
/// under the names `names`.
pub fn rejects(unified: bool, edits: &[Edit], names: (&str, &str), out: &mut Vec<u8>) {
    let (old, new) = if unified {
        ("---", "+++")
    } else {
        ("***", "---")
    };
    out.extend_from_slice(format!("{old} {}\n{new} {}\n", names.0, names.1).as_bytes());
    for edit in edits {
        let count = |put| edit.side(put).count();
        let olds = edit.old..edit.old + count(false);
        let news = edit.new..edit.new + count(true);
        if unified {
            let header = format!(
                "@@ -{} +{} @@\n",
                unified_range(&olds),
                unified_range(&news)
            );
            out.extend_from_slice(header.as_bytes());
            for &(mark, line) in &edit.lines {
                let sign: &[u8] = match mark {
                    Mark::Same => b" ",
                    Mark::Gone => b"-",
                    Mark::Put => b"+",
                };
                put_line(out, sign, line);
            }
            continue;
        }
        out.extend_from_slice(
            format!("***************\n*** {} ****\n", context_range(&olds)).as_bytes(),
        );
        edit_side(edit, Mark::Gone, b"- ", out);
        out.extend_from_slice(format!("--- {} ----\n", context_range(&news)).as_bytes());
        edit_side(edit, Mark::Put, b"+ ", out);
    }
}

/// Writes `line` after `mark`, and diff's note when it has no line feed.
fn put_line(out: &mut Vec<u8>, mark: &[u8], line: Line) {
    out.extend_from_slice(mark);
    out.extend_from_slice(line.text);
    out.push(b'\n');
    if line.unended {
        out.extend_from_slice(UNENDED);
    }
}

/// One side of `edit` in the context form, the lines in both and those
/// marked `own`, if it has any of those: `alone` before a line of a run
/// of changes that has none of the other side's, `! ` before the others.
fn edit_side(edit: &Edit, own: Mark, alone: &[u8], out: &mut Vec<u8>) {
    if !edit.lines.iter().any(|(mark, _)| *mark == own) {
        return;
    }
    for run in edit
        .lines
        .chunk_by(|(a, _), (b, _)| (*a == Mark::Same) == (*b == Mark::Same))
    {
        let both =
            run.iter().any(|(m, _)| *m == Mark::Gone) && run.iter().any(|(m, _)| *m == Mark::Put);
        for &(mark, line) in run {
            if mark == Mark::Same {
                put_line(out, b"  ", line);
            } else if mark == own {
                put_line(out, if both { b"! " } else { alone }, line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use core::iter::Peekable;

    fn diff(form: Form, a: &str, b: &str) -> String {
        let (mut a, mut b) = (Text::new(a.as_bytes()), Text::new(b.as_bytes()));
        if !form.says_unended() {
            (a.ended, b.ended) = (true, true);
        }
        let (left, right) = keys(&a, &b, false);
        let mut out = Vec::new();
        write(
            form,
            (&a, &b),
            &crate::hunks(&left, &right),
            ("A", "B"),
            &mut out,
        );
        String::from_utf8(out).unwrap()
    }

    const FORMS: [Form; 7] = [
        Form::Normal,
        Form::Unified(3),
        Form::Context(3),
        Form::Ed,
        Form::Forward,
        Form::Unified(0),
        Form::Context(1),
    ];

    const ELEVEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n";

    /// Texts and what GNU's diff 3.12 writes for them in each of `FORMS`,
    /// without the names of the texts at the top.
    const GNU: [(&str, &str, [&str; 7]); 9] = [
        (
            "one\ntwo\nthree\nfour\nfive\n",
            "one\nTWO\nthree\nfive\nsix\n",
            [
                "2c2\n< two\n---\n> TWO\n4d3\n< four\n5a5\n> six\n",
                "@@ -1,5 +1,5 @@\n one\n-two\n+TWO\n three\n-four\n five\n+six\n",
                "***************\n*** 1,5 ****\n  one\n! two\n  three\n- four\n  five\n\
                 --- 1,5 ----\n  one\n! TWO\n  three\n  five\n+ six\n",
                "5a\nsix\n.\n4d\n2c\nTWO\n.\n",
                "c2\nTWO\n.\nd4\na5\nsix\n.\n",
                "@@ -2 +2 @@\n-two\n+TWO\n@@ -4 +3,0 @@\n-four\n@@ -5,0 +5 @@\n+six\n",
                "***************\n*** 1,5 ****\n  one\n! two\n  three\n- four\n  five\n\
                 --- 1,5 ----\n  one\n! TWO\n  three\n  five\n+ six\n",
            ],
        ),
        (
            ELEVEN,
            "1\n2x\n3\n4\n5\n6\n7\n8x\n9\n10\n11\n",
            [
                "2c2\n< 2\n---\n> 2x\n8c8\n< 8\n---\n> 8x\n",
                "@@ -1,11 +1,11 @@\n 1\n-2\n+2x\n 3\n 4\n 5\n 6\n 7\n-8\n+8x\n 9\n 10\n 11\n",
                "***************\n*** 1,11 ****\n  1\n! 2\n  3\n  4\n  5\n  6\n  7\n! 8\n  9\n  10\n\
                 \x20 11\n--- 1,11 ----\n  1\n! 2x\n  3\n  4\n  5\n  6\n  7\n! 8x\n  9\n  10\n  11\n",
                "8c\n8x\n.\n2c\n2x\n.\n",
                "c2\n2x\n.\nc8\n8x\n.\n",
                "@@ -2 +2 @@\n-2\n+2x\n@@ -8 +8 @@\n-8\n+8x\n",
                "***************\n*** 1,3 ****\n  1\n! 2\n  3\n--- 1,3 ----\n  1\n! 2x\n  3\n\
                 ***************\n*** 7,9 ****\n  7\n! 8\n  9\n--- 7,9 ----\n  7\n! 8x\n  9\n",
            ],
        ),
        (
            ELEVEN,
            "1\n2x\n3\n4\n5\n6\n7\n8\n9\n10x\n11\n",
            [
                "2c2\n< 2\n---\n> 2x\n10c10\n< 10\n---\n> 10x\n",
                "@@ -1,5 +1,5 @@\n 1\n-2\n+2x\n 3\n 4\n 5\n@@ -7,5 +7,5 @@\n 7\n 8\n 9\n-10\n+10x\n 11\n",
                "***************\n*** 1,5 ****\n  1\n! 2\n  3\n  4\n  5\n--- 1,5 ----\n  1\n! 2x\n  3\n\
                 \x20 4\n  5\n***************\n*** 7,11 ****\n  7\n  8\n  9\n! 10\n  11\n\
                 --- 7,11 ----\n  7\n  8\n  9\n! 10x\n  11\n",
                "10c\n10x\n.\n2c\n2x\n.\n",
                "c2\n2x\n.\nc10\n10x\n.\n",
                "@@ -2 +2 @@\n-2\n+2x\n@@ -10 +10 @@\n-10\n+10x\n",
                "***************\n*** 1,3 ****\n  1\n! 2\n  3\n--- 1,3 ----\n  1\n! 2x\n  3\n\
                 ***************\n*** 9,11 ****\n  9\n! 10\n  11\n--- 9,11 ----\n  9\n! 10x\n  11\n",
            ],
        ),
        (
            ELEVEN,
            "1\n2x\n3\n4\n5\n6\n7\n8\n9x\n10\n11\n",
            [
                "2c2\n< 2\n---\n> 2x\n9c9\n< 9\n---\n> 9x\n",
                "@@ -1,11 +1,11 @@\n 1\n-2\n+2x\n 3\n 4\n 5\n 6\n 7\n 8\n-9\n+9x\n 10\n 11\n",
                "***************\n*** 1,11 ****\n  1\n! 2\n  3\n  4\n  5\n  6\n  7\n  8\n! 9\n\
                 \x20 10\n  11\n--- 1,11 ----\n  1\n! 2x\n  3\n  4\n  5\n  6\n  7\n  8\n! 9x\n\
                 \x20 10\n  11\n",
                "9c\n9x\n.\n2c\n2x\n.\n",
                "c2\n2x\n.\nc9\n9x\n.\n",
                "@@ -2 +2 @@\n-2\n+2x\n@@ -9 +9 @@\n-9\n+9x\n",
                "***************\n*** 1,3 ****\n  1\n! 2\n  3\n--- 1,3 ----\n  1\n! 2x\n  3\n\
                 ***************\n*** 8,10 ****\n  8\n! 9\n  10\n--- 8,10 ----\n  8\n! 9x\n  10\n",
            ],
        ),
        (
            "",
            "a\nb\n",
            [
                "0a1,2\n> a\n> b\n",
                "@@ -0,0 +1,2 @@\n+a\n+b\n",
                "***************\n*** 0 ****\n--- 1,2 ----\n+ a\n+ b\n",
                "0a\na\nb\n.\n",
                "a0\na\nb\n.\n",
                "@@ -0,0 +1,2 @@\n+a\n+b\n",
                "***************\n*** 0 ****\n--- 1,2 ----\n+ a\n+ b\n",
            ],
        ),
        (
            "a\nb\n",
            "",
            [
                "1,2d0\n< a\n< b\n",
                "@@ -1,2 +0,0 @@\n-a\n-b\n",
                "***************\n*** 1,2 ****\n- a\n- b\n--- 0 ----\n",
                "1,2d\n",
                "d1 2\n",
                "@@ -1,2 +0,0 @@\n-a\n-b\n",
                "***************\n*** 1,2 ****\n- a\n- b\n--- 0 ----\n",
            ],
        ),
        (
            "x\ny",
            "x\nz\n",
            [
                "2c2\n< y\n\\ No newline at end of file\n---\n> z\n",
                "@@ -1,2 +1,2 @@\n x\n-y\n\\ No newline at end of file\n+z\n",
                "***************\n*** 1,2 ****\n  x\n! y\n\\ No newline at end of file\n\
                 --- 1,2 ----\n  x\n! z\n",
                "2c\nz\n.\n",
                "c2\nz\n.\n",
                "@@ -2 +2 @@\n-y\n\\ No newline at end of file\n+z\n",
                "***************\n*** 1,2 ****\n  x\n! y\n\\ No newline at end of file\n\
                 --- 1,2 ----\n  x\n! z\n",
            ],
        ),
        (
            "x\ny",
            "x\ny\n",
            [
                "2c2\n< y\n\\ No newline at end of file\n---\n> y\n",
                "@@ -1,2 +1,2 @@\n x\n-y\n\\ No newline at end of file\n+y\n",
                "***************\n*** 1,2 ****\n  x\n! y\n\\ No newline at end of file\n\
                 --- 1,2 ----\n  x\n! y\n",
                "",
                "",
                "@@ -2 +2 @@\n-y\n\\ No newline at end of file\n+y\n",
                "***************\n*** 1,2 ****\n  x\n! y\n\\ No newline at end of file\n\
                 --- 1,2 ----\n  x\n! y\n",
            ],
        ),
        (
            "a\n",
            "b\n",
            [
                "1c1\n< a\n---\n> b\n",
                "@@ -1 +1 @@\n-a\n+b\n",
                "***************\n*** 1 ****\n! a\n--- 1 ----\n! b\n",
                "1c\nb\n.\n",
                "c1\nb\n.\n",
                "@@ -1 +1 @@\n-a\n+b\n",
                "***************\n*** 1 ****\n! a\n--- 1 ----\n! b\n",
            ],
        ),
    ];

    #[test]
    fn every_form_is_written_as_gnu_writes_it() {
        for (a, b, written) in GNU {
            for (form, gnu) in FORMS.into_iter().zip(written) {
                let names = match form {
                    Form::Unified(_) => "--- A\n+++ B\n",
                    Form::Context(_) => "*** A\n--- B\n",
                    _ => "",
                };
                let expected = if gnu.is_empty() {
                    String::new()
                } else {
                    String::from(names) + gnu
                };
                assert_eq!(diff(form, a, b), expected, "{form:?} of {a:?} and {b:?}");
            }
        }
    }

    /// The hunks of the one patch in `patch`.
    fn edits_in(patch: &[u8]) -> Vec<crate::read::Edit<'_>> {
        match crate::read::patches(patch, None)
            .unwrap()
            .pop()
            .map(|p| p.hunks)
        {
            Some(crate::read::Hunks::Edits(edits)) => edits,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejected_hunks_read_back_as_they_were() {
        let mut seed = 0x4f1b_bcdc_bfa5_3e0a_u64;
        for _ in 0..600 {
            let first = random_text(&mut seed, 20);
            let mut second = first.clone();
            for _ in 0..=next(&mut seed) % 3 {
                let at = usize::try_from(next(&mut seed)).unwrap() % (second.len() + 1);
                let new = random_text(&mut seed, 2);
                second.splice(at..(at + 1).min(second.len()), new);
            }
            let (a, b) = (
                joined(&first, true),
                joined(&second, next(&mut seed).is_multiple_of(2)),
            );
            let around = usize::try_from(next(&mut seed) % 3).unwrap();
            for form in [Form::Normal, Form::Context(around), Form::Unified(around)] {
                let patch = diff(form, &a, &b);
                if patch.is_empty() {
                    continue;
                }
                let edits = edits_in(patch.as_bytes());
                for unified in [false, true] {
                    let mut out = Vec::new();
                    rejects(unified, &edits, ("A", "B"), &mut out);
                    assert_eq!(edits_in(&out), edits, "{form:?} as {unified}:\n{patch}");
                    let found = crate::read::patches(&out, None).unwrap();
                    assert_eq!(
                        (found[0].old_name, found[0].new_name),
                        (Some(&b"A"[..]), Some(&b"B"[..]))
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_from_diff_s_own_form_are_written_with_context() {
        let patch = "3c3\n< 3\n---\n> 3x\n7d6\n< 7\n10a10\n> n\n";
        let mut out = Vec::new();
        rejects(false, &edits_in(patch.as_bytes()), ("f", "f"), &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "*** f\n--- f\n\
             ***************\n*** 3 ****\n! 3\n--- 3 ----\n! 3x\n\
             ***************\n*** 7 ****\n- 7\n--- 6 ----\n\
             ***************\n*** 10 ****\n--- 10 ----\n+ n\n"
        );
        let mut out = Vec::new();
        rejects(
            true,
            &edits_in(b"@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n"),
            ("a", "b"),
            &mut out,
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "--- a\n+++ b\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n"
        );
    }

    #[test]
    fn alike_texts_give_nothing_at_all() {
        for form in FORMS {
            assert_eq!(diff(form, ELEVEN, ELEVEN), "", "{form:?}");
            assert_eq!(diff(form, "", ""), "", "{form:?}");
        }
    }

    #[test]
    fn any_number_of_lines_around_shows_the_whole_text() {
        let b = "1\n2x\n3\n4\n5\n6\n7\n8\n9\n10x\n11\n";
        assert_eq!(
            diff(Form::Unified(usize::MAX), ELEVEN, b),
            diff(Form::Unified(11), ELEVEN, b)
        );
        assert_eq!(
            diff(Form::Context(usize::MAX), ELEVEN, b),
            diff(Form::Context(4), ELEVEN, b)
        );
        assert!(
            diff(Form::Unified(usize::MAX), ELEVEN, b)
                .starts_with("--- A\n+++ B\n@@ -1,11 +1,11 @@\n")
        );
    }

    #[test]
    fn an_ed_script_puts_a_line_of_one_dot_in_as_two() {
        let d1 = "a\nb\nc\n";
        assert_eq!(
            diff(Form::Ed, d1, "a\n.\nx\n.\nb\nc\n"),
            "1a\n..\n.\ns/.//\na\nx\n..\n.\ns/.//\n"
        );
        assert_eq!(diff(Form::Ed, d1, "a\n.\n"), "2,3c\n..\n.\ns/.//\n");
        assert_eq!(
            diff(Form::Ed, d1, "a\n..\nb\nc\n"),
            "1a\n..\n.\n",
            "only a dot alone"
        );
        assert_eq!(
            diff(Form::Forward, d1, "a\n.\nx\n.\nb\nc\n"),
            "a1\n.\nx\n.\n.\n",
            "the forward script is for reading, and leaves the dots be"
        );
    }

    #[test]
    fn a_text_is_its_lines_and_whether_the_last_ends() {
        let lines = |bytes: &'static [u8]| Text::new(bytes);
        assert_eq!(
            lines(b""),
            Text {
                lines: Vec::new(),
                ended: true
            }
        );
        assert_eq!(
            lines(b"a"),
            Text {
                lines: Vec::from([&b"a"[..]]),
                ended: false
            }
        );
        assert_eq!(
            lines(b"a\n"),
            Text {
                lines: Vec::from([&b"a"[..]]),
                ended: true
            }
        );
        assert_eq!(
            lines(b"\n"),
            Text {
                lines: Vec::from([&b""[..]]),
                ended: true
            }
        );
        assert_eq!(
            lines(b"a\n\nb"),
            Text {
                lines: Vec::from([&b"a"[..], b"", b"b"]),
                ended: false
            }
        );
        assert_eq!(
            lines(b"\n\n"),
            Text {
                lines: Vec::from([&b""[..], b""]),
                ended: true
            }
        );
        let text = lines(b"a\nb");
        assert!(!text.unended(0) && text.unended(1));
        assert!(!lines(b"a\nb\n").unended(1));
    }

    /// Whether lines `a` and `b`, each the last of its text and ended as
    /// said, compare the same.
    fn same(a: (&str, bool), b: (&str, bool), blanks: bool) -> bool {
        let a = Text {
            lines: Vec::from([a.0.as_bytes()]),
            ended: a.1,
        };
        let b = Text {
            lines: Vec::from([b.0.as_bytes()]),
            ended: b.1,
        };
        let (left, right) = keys(&a, &b, blanks);
        left == right
    }

    #[test]
    fn with_blanks_a_run_of_them_is_one_and_none_end_a_line() {
        // As GNU's diff -b compares them.
        for (a, b, alike) in [
            (" a", "a", false),
            ("a ", "a", true),
            ("a  b", "a b", true),
            ("a\tb", "a b", true),
            ("a b", "ab", false),
            (" a", "\t a", true),
            ("a \t ", "a", true),
        ] {
            assert_eq!(same((a, true), (b, true), true), alike, "{a:?} {b:?}");
            assert_eq!(same((a, true), (b, true), false), a == b, "{a:?} {b:?}");
            assert!(same((a, true), (a, true), false));
        }
        assert!(same(("a ", false), ("a", false), true));
        assert!(
            same(("a ", false), ("a", true), true),
            "a missing line feed is a blank at the end"
        );
        assert!(!same(("a", false), ("a", true), false));
        assert!(same(("a", false), ("a", false), false));
    }

    #[test]
    fn lines_alike_get_one_number() {
        let a = Text::new(b"x\ny\nx\n");
        let b = Text::new(b"y\nz\nx");
        assert_eq!(
            keys(&a, &b, false),
            (Vec::from([0, 1, 0]), Vec::from([1, 2, 3]))
        );
    }

    #[test]
    fn only_ed_scripts_cannot_say_a_line_feed_is_missing() {
        let saying: Vec<bool> = FORMS.iter().map(|f| f.says_unended()).collect();
        assert_eq!(saying, [true, true, true, false, false, true, true]);
    }

    /// A text as its lines, each with whether it is followed by diff's
    /// note that it has no line feed.
    type Lines = Vec<(String, bool)>;

    fn lines_of(text: &str) -> Lines {
        let text = Text::new(text.as_bytes());
        (0..text.lines.len())
            .map(|at| {
                let line = core::str::from_utf8(text.lines[at]).unwrap().to_string();
                (line, text.unended(at))
            })
            .collect()
    }

    /// The lines of the first text a hunk takes the place of, and the
    /// lines it puts there.
    type Edit = (Range<usize>, Lines);

    /// The second text, as `edits` make it of the first, `a`.
    fn apply(a: &Lines, mut edits: Vec<Edit>) -> Lines {
        edits.sort_by_key(|(gone, _)| gone.start);
        let mut b = Vec::new();
        let mut at = 0;
        for (gone, added) in edits {
            assert!(at <= gone.start, "hunks that overlap");
            b.extend_from_slice(&a[at..gone.start]);
            b.extend(added);
            at = gone.end;
        }
        b.extend_from_slice(&a[at..]);
        b
    }

    const NOTE: &str = "\\ No newline at end of file";

    /// The lines `mark` begins, from where `lines` is, each with whether
    /// diff's note follows it.
    fn taken<'s>(lines: &mut Peekable<impl Iterator<Item = &'s str>>, mark: &str) -> Lines {
        let mut taken = Vec::new();
        while let Some(line) = lines.next_if(|l| l.starts_with(mark)) {
            let note = lines.next_if_eq(&NOTE).is_some();
            taken.push((line[mark.len()..].to_string(), note));
        }
        taken
    }

    /// The first and last of a range that `numbers` gives, as one number
    /// or two parted by `apart`.
    fn ends(numbers: &str, apart: char) -> (usize, usize) {
        match numbers.split_once(apart) {
            Some((first, last)) => (first.parse().unwrap(), last.parse().unwrap()),
            None => (numbers.parse().unwrap(), numbers.parse().unwrap()),
        }
    }

    /// The hunks of diff's own form, each line it says is gone checked
    /// against the first text.
    fn read_normal(a: &Lines, script: &str) -> Vec<Edit> {
        let mut lines = script.split_terminator('\n').peekable();
        let mut edits = Vec::new();
        let mut shift = 0isize;
        while let Some(header) = lines.next() {
            let letter = header.find(['a', 'c', 'd']).unwrap();
            let (first, last) = ends(&header[..letter], ',');
            let (start, end) = ends(&header[letter + 1..], ',');
            let gone = if header.as_bytes()[letter] == b'a' {
                first..first
            } else {
                first - 1..last
            };
            assert_eq!(taken(&mut lines, "< "), a[gone.clone()]);
            if !gone.is_empty() {
                lines.next_if_eq(&"---");
            }
            let added = taken(&mut lines, "> ");
            let at = gone.start.checked_add_signed(shift).unwrap();
            if added.is_empty() {
                assert_eq!((start, end), (at, at), "{header}");
            } else {
                assert_eq!((start, end), (at + 1, at + added.len()), "{header}");
            }
            shift += added.len().cast_signed() - gone.len().cast_signed();
            edits.push((gone, added));
        }
        edits
    }

    /// The hunks of the unified form, each line it says is there or gone
    /// checked against the first text, and each header's counts against
    /// the lines below it.
    fn read_unified(a: &Lines, script: &str) -> Vec<Edit> {
        let mut lines = script.split_terminator('\n').peekable();
        assert_eq!(lines.next(), Some("--- A"));
        assert_eq!(lines.next(), Some("+++ B"));
        let mut edits = Vec::new();
        let mut shift = 0isize;
        let range = |numbers: &str| {
            let (start, count) = numbers.split_once(',').unwrap_or((numbers, "1"));
            let (start, count): (usize, usize) = (start.parse().unwrap(), count.parse().unwrap());
            (if count == 0 { start } else { start - 1 }, count)
        };
        while let Some(header) = lines.next() {
            let ranges = header
                .strip_prefix("@@ -")
                .unwrap()
                .strip_suffix(" @@")
                .unwrap();
            let (left, right) = ranges.split_once(" +").unwrap();
            let ((mut at, long), (start, wide)) = (range(left), range(right));
            assert_eq!(at.checked_add_signed(shift), Some(start), "{header}");
            let (mut seen_a, mut seen_b) = (0, 0);
            loop {
                let alike = taken(&mut lines, " ");
                assert_eq!(alike, a[at..at + alike.len()]);
                at += alike.len();
                let gone = taken(&mut lines, "-");
                let added = taken(&mut lines, "+");
                seen_a += alike.len() + gone.len();
                seen_b += alike.len() + added.len();
                if gone.is_empty() && added.is_empty() {
                    break;
                }
                assert_eq!(gone, a[at..at + gone.len()]);
                shift += added.len().cast_signed() - gone.len().cast_signed();
                edits.push((at..at + gone.len(), added));
                at += gone.len();
            }
            assert_eq!((seen_a, seen_b), (long, wide), "{header}");
        }
        edits
    }

    /// The lines from where `lines` is that begin with one of `marks`,
    /// marks and all, each with whether diff's note follows it.
    fn marked<'s>(lines: &mut Peekable<impl Iterator<Item = &'s str>>, marks: [&str; 3]) -> Lines {
        let mut taken = Vec::new();
        while let Some(line) = lines.next_if(|l| marks.iter().any(|m| l.starts_with(m))) {
            let note = lines.next_if_eq(&NOTE).is_some();
            taken.push((line.to_string(), note));
        }
        taken
    }

    /// The lines a range of the context form names, `count` of them.
    fn shown(numbers: &str, count: usize) -> Range<usize> {
        let (first, last) = ends(numbers, ',');
        if count == 0 {
            assert_eq!(first, last, "{numbers}");
            first..first
        } else {
            assert_eq!(last + 1 - first, count, "{numbers}");
            first - 1..last
        }
    }

    /// The groups of the context form, each as the lines of the first
    /// text it shows, checked against them, and the lines of the second
    /// that take their place.
    fn read_context(a: &Lines, script: &str) -> Vec<Edit> {
        let mut lines = script.split_terminator('\n').peekable();
        assert_eq!(lines.next(), Some("*** A"));
        assert_eq!(lines.next(), Some("--- B"));
        let unmarked = |side: &Lines| -> Lines {
            side.iter()
                .map(|(line, note)| (line[2..].to_string(), *note))
                .collect()
        };
        // A side left out is the other's lines that are in both.
        let alike = |side: &Lines| -> Lines {
            side.iter()
                .filter(|(line, _)| line.starts_with("  "))
                .cloned()
                .collect()
        };
        let mut edits = Vec::new();
        let mut shift = 0isize;
        while let Some(stars) = lines.next() {
            assert_eq!(stars, "***************");
            let header = lines.next().unwrap();
            let left = header
                .strip_prefix("*** ")
                .unwrap()
                .strip_suffix(" ****")
                .unwrap();
            let first = marked(&mut lines, ["  ", "- ", "! "]);
            let header = lines.next().unwrap();
            let right = header
                .strip_prefix("--- ")
                .unwrap()
                .strip_suffix(" ----")
                .unwrap();
            let second = marked(&mut lines, ["  ", "+ ", "! "]);
            assert!(!first.is_empty() || !second.is_empty());
            let first = if first.is_empty() {
                alike(&second)
            } else {
                first
            };
            let second = if second.is_empty() {
                alike(&first)
            } else {
                second
            };
            let (gone, put) = (shown(left, first.len()), shown(right, second.len()));
            assert_eq!(gone.start.checked_add_signed(shift), Some(put.start));
            assert_eq!(unmarked(&first), a[gone.clone()]);
            shift += put.len().cast_signed() - gone.len().cast_signed();
            edits.push((gone, unmarked(&second)));
        }
        edits
    }

    /// The second text, as `script` makes it of the first when run as
    /// ed runs it, each command where the one before left off.
    fn run_ed(a: &Lines, script: &str) -> Lines {
        let mut text: Vec<String> = a.iter().map(|(line, _)| line.clone()).collect();
        let mut current = text.len();
        let mut lines = script.split_terminator('\n');
        while let Some(command) = lines.next() {
            if command == "s/.//" {
                text[current - 1].remove(0);
                continue;
            }
            let (place, letter) = command.split_at(command.len() - 1);
            let (first, last) = if place.is_empty() {
                (current, current)
            } else {
                ends(place, ',')
            };
            let at = match letter {
                "a" => first,
                "c" | "d" => {
                    text.drain(first - 1..last);
                    first - 1
                }
                _ => panic!("{command:?} in {script:?}"),
            };
            if letter != "d" {
                let put: Vec<String> = lines
                    .by_ref()
                    .take_while(|line| *line != ".")
                    .map(ToString::to_string)
                    .collect();
                current = at + put.len();
                text.splice(at..at, put);
            }
        }
        text.into_iter().map(|line| (line, false)).collect()
    }

    /// The hunks of a forward script, which come in the order of the
    /// first text.
    fn read_forward(script: &str) -> Vec<Edit> {
        let mut lines = script.split_terminator('\n');
        let mut edits: Vec<Edit> = Vec::new();
        while let Some(command) = lines.next() {
            let (letter, place) = command.split_at(1);
            let (first, last) = ends(place, ' ');
            let gone = if letter == "a" {
                first..first
            } else {
                first - 1..last
            };
            if let Some((before, _)) = edits.last() {
                assert!(before.end <= gone.start, "{script:?}");
            }
            let put = if letter == "d" {
                Vec::new()
            } else {
                lines
                    .by_ref()
                    .take_while(|line| *line != ".")
                    .map(|line| (line.to_string(), false))
                    .collect()
            };
            edits.push((gone, put));
        }
        edits
    }

    fn next(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// A text of up to `most` lines, some of them dots, ended or not.
    fn random_text(seed: &mut u64, most: u64) -> Vec<&'static str> {
        const WORDS: [&str; 8] = ["a", "b", "c", "", " a", "a ", ".", ".."];
        let long = next(seed) % (most + 1);
        (0..long)
            .map(|_| WORDS[usize::try_from(next(seed) % 8).unwrap()])
            .collect()
    }

    fn joined(lines: &[&str], ended: bool) -> String {
        let mut text = lines.join("\n");
        if ended && !lines.is_empty() {
            text.push('\n');
        }
        text
    }

    #[test]
    fn every_form_makes_the_second_text_of_the_first() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..1500 {
            let first = random_text(&mut seed, 30);
            // Mostly the first again, with a few lines changed, so that
            // hunks lie near enough to share their lines around.
            let mut second = first.clone();
            for _ in 0..next(&mut seed) % 5 {
                let at = usize::try_from(next(&mut seed)).unwrap() % (second.len() + 1);
                let new = random_text(&mut seed, 2);
                match next(&mut seed) % 3 {
                    0 if at < second.len() => drop(second.remove(at)),
                    1 if at < second.len() => second.splice(at..=at, new).for_each(drop),
                    _ => second.splice(at..at, new).for_each(drop),
                }
            }
            if next(&mut seed).is_multiple_of(4) {
                second = random_text(&mut seed, 12);
            }
            let a = joined(&first, !next(&mut seed).is_multiple_of(3));
            let b = joined(&second, !next(&mut seed).is_multiple_of(3));
            let around = usize::try_from(next(&mut seed) % 4).unwrap();
            let (a_lines, b_lines) = (lines_of(&a), lines_of(&b));
            let ended = |lines: &Lines| -> Lines {
                lines
                    .iter()
                    .map(|(line, _)| (line.clone(), false))
                    .collect()
            };
            for form in [
                Form::Normal,
                Form::Unified(around),
                Form::Context(around),
                Form::Ed,
                Form::Forward,
            ] {
                let script = diff(form, &a, &b);
                let (made, wanted) = match form {
                    Form::Normal => (
                        apply(&a_lines, read_normal(&a_lines, &script)),
                        b_lines.clone(),
                    ),
                    _ if script.is_empty() && form.says_unended() => {
                        (a_lines.clone(), b_lines.clone())
                    }
                    Form::Unified(_) => (
                        apply(&a_lines, read_unified(&a_lines, &script)),
                        b_lines.clone(),
                    ),
                    Form::Context(_) => (
                        apply(&a_lines, read_context(&a_lines, &script)),
                        b_lines.clone(),
                    ),
                    Form::Ed => (run_ed(&a_lines, &script), ended(&b_lines)),
                    // A dot alone would end the lines put in too soon.
                    Form::Forward if second.contains(&".") => continue,
                    Form::Forward => (
                        apply(&ended(&a_lines), read_forward(&script)),
                        ended(&b_lines),
                    ),
                };
                assert_eq!(made, wanted, "{form:?} of {a:?} and {b:?}:\n{script}");
            }
        }
    }

    #[test]
    fn a_reader_finds_a_script_that_does_not_fit_the_text() {
        // So the readers above check what they say they check.
        let a = lines_of("one\ntwo\n");
        assert_eq!(
            apply(&a, read_normal(&a, "2c2\n< two\n---\n> 2\n")),
            lines_of("one\n2\n")
        );
        for (read, script) in [
            (
                read_normal as fn(&Lines, &str) -> Vec<Edit>,
                "2c2\n< one\n---\n> 2\n",
            ),
            (read_normal, "2c1\n< two\n---\n> 2\n"),
            (
                read_unified,
                "--- A\n+++ B\n@@ -1,2 +1,2 @@\n one\n-tw\n+2\n",
            ),
            (
                read_unified,
                "--- A\n+++ B\n@@ -1,2 +1,3 @@\n one\n-two\n+2\n",
            ),
            (
                read_context,
                "*** A\n--- B\n***************\n*** 1,2 ****\n  one\n! tw\n--- 1,2 ----\n  one\n! 2\n",
            ),
            (
                read_context,
                "*** A\n--- B\n***************\n*** 1,2 ****\n  one\n! two\n--- 2,3 ----\n  one\n! 2\n",
            ),
        ] {
            let caught = std::panic::catch_unwind(|| read(&a, script)).is_err();
            assert!(caught, "{script:?}");
        }
        let run = run_ed(&a, "2c\n..\n.\ns/.//\na\nx\n.\n1d\n");
        assert_eq!(run, [(".".to_string(), false), ("x".to_string(), false)]);
    }
}
