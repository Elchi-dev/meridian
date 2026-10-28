//! Hunks put into a text, as patch puts them: each where it says, or as
//! near there as its lines are found, with a line or two of context at
//! its ends left out when that is what it takes.

use alloc::vec::Vec;

use crate::read::{Command, Edit, Line, Mark};
use crate::write::Text;

/// The lines of `text` as hunks hold theirs.
#[must_use]
pub fn lines<'a>(text: &Text<'a>) -> Vec<Line<'a>> {
    let count = text.lines.len();
    text.lines
        .iter()
        .enumerate()
        .map(|(at, &line)| Line {
            text: line,
            unended: !text.ended && at + 1 == count,
        })
        .collect()
}

/// How closely a hunk's lines have to match a text's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fit {
    /// Whether any run of blanks matches any other, as `-l` asks.
    pub loose: bool,
    /// How many lines of context each end of a hunk may leave out.
    pub fuzz: usize,
}

/// Where a hunk went: the line of the text its lines start at, counted
/// from 0, how far that is from where it said, and how many lines of
/// context it left out at each end; or nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    At {
        line: usize,
        offset: isize,
        fuzz: usize,
    },
    Rejected,
}

/// Whether `a` and `b` are the same line, with `loose` each run of
/// blanks the same as any other.
fn alike(a: &[u8], b: &[u8], loose: bool) -> bool {
    if loose {
        squeezed(a).eq(squeezed(b))
    } else {
        a == b
    }
}

/// The bytes of `line` with each run of blanks one space.
fn squeezed(line: &[u8]) -> impl Iterator<Item = u8> + '_ {
    let mut blank = false;
    line.iter().filter_map(move |&byte| {
        let was = blank;
        blank = matches!(byte, b' ' | b'\t');
        match (blank, was) {
            (true, true) => None,
            (true, false) => Some(b' '),
            _ => Some(byte),
        }
    })
}

/// A hunk to put in with `fuzz` lines of context left out at each end, as
/// many as it has there: how many it leaves out before, and the lines
/// that remain of it.
fn trimmed<'e, 'a>(edit: &'e Edit<'a>, fuzz: usize) -> (usize, &'e [(Mark, Line<'a>)]) {
    let lines = &edit.lines[..];
    let before = lines.iter().take_while(|(m, _)| *m == Mark::Same).count();
    let after = lines
        .iter()
        .rev()
        .take_while(|(m, _)| *m == Mark::Same)
        .count();
    // A hunk all context leaves out no more than it has.
    let front = fuzz.min(before);
    let back = fuzz.min(after).min(lines.len() - front);
    (front, &lines[front..lines.len() - back])
}

/// Whether the old lines of `lines` are those of `text` from `at` on.
fn matches(text: &[Line], at: usize, lines: &[(Mark, Line)], loose: bool) -> bool {
    let mut old = lines.iter().filter(|(m, _)| *m != Mark::Put);
    let count = old.clone().count();
    text.get(at..at + count).is_some_and(|here| {
        here.iter()
            .zip(&mut old)
            .all(|(t, (_, l))| alike(t.text, l.text, loose))
    })
}

/// Where in `text` the old lines of `lines` are, at `from` or later,
/// nearest to `near`: a hunk with none goes only where it says.
fn found(
    text: &[Line],
    lines: &[(Mark, Line)],
    near: usize,
    from: usize,
    loose: bool,
) -> Option<usize> {
    let count = lines.iter().filter(|(m, _)| *m != Mark::Put).count();
    let last = text.len().checked_sub(count)?;
    if count == 0 || from > last {
        return (from..=last).contains(&near).then_some(near);
    }
    let reach = near.abs_diff(from).max(near.abs_diff(last));
    (0..=reach).find_map(|distance| {
        let later = near.checked_add(distance).filter(|&at| at <= last);
        // At no distance both are the line itself, tried twice.
        let earlier = near.checked_sub(distance);
        [later, earlier]
            .into_iter()
            .flatten()
            .find(|&at| at >= from && matches(text, at, lines, loose))
    })
}

/// The place for `edit` in `text`, at `from` or later, nearest to where
/// it says moved by `offset`: the line its old lines start at, and the
/// fuzz it took.
fn place(
    text: &[Line],
    edit: &Edit,
    offset: isize,
    from: usize,
    fit: Fit,
) -> Option<(usize, usize)> {
    (0..=fit.fuzz).find_map(|fuzz| {
        let (front, lines) = trimmed(edit, fuzz);
        let near = (edit.old + front).checked_add_signed(offset)?;
        let at = found(text, lines, near, from, fit.loose)?;
        Some((at - front, fuzz))
    })
}

/// Whether `edit` has a place in `text` that `fit` lets it take.
#[must_use]
pub fn fits(text: &[Line], edit: &Edit, fit: Fit) -> bool {
    place(text, edit, 0, 0, fit).is_some()
}

/// What `text` becomes with `edits` put in, each where `fit` lets it go
/// and none before the one ahead of it, and where each went. With
/// `define`, both the old lines and the new are kept, between the lines
/// a C preprocessor reads so that the new ones count only where the name
/// is defined.
#[must_use]
pub fn apply(
    text: &[Line],
    edits: &[Edit],
    fit: Fit,
    define: Option<&[u8]>,
) -> (Vec<u8>, Vec<Placed>) {
    let mut out = Out::default();
    let mut placed = Vec::new();
    let (mut done, mut offset) = (0, 0isize);
    for edit in edits {
        let Some((line, fuzz)) = place(text, edit, offset, done, fit) else {
            placed.push(Placed::Rejected);
            continue;
        };
        let (front, lines) = trimmed(edit, fuzz);
        let start = line + front;
        for kept in &text[done..start] {
            out.line(*kept);
        }
        let mut at = start;
        let mut runs = lines.chunk_by(|(a, _), (b, _)| (*a == Mark::Same) == (*b == Mark::Same));
        for run in &mut runs {
            let gone = run.iter().filter(|(m, _)| *m == Mark::Gone).count();
            let put = run.iter().filter(|(m, _)| *m == Mark::Put).map(|&(_, l)| l);
            if run[0].0 == Mark::Same {
                // The text's own lines, which with -l may differ in blanks.
                for kept in &text[at..at + run.len()] {
                    out.line(*kept);
                }
                at += run.len();
                continue;
            }
            let old = &text[at..at + gone];
            at += gone;
            match define {
                None => put.for_each(|l| out.line(l)),
                Some(name) => out.defined(name, old, put),
            }
        }
        done = at;
        let said = isize::try_from(edit.old).unwrap_or(isize::MAX);
        offset = isize::try_from(line).unwrap_or(isize::MAX) - said;
        placed.push(Placed::At { line, offset, fuzz });
    }
    for kept in &text[done..] {
        out.line(*kept);
    }
    (out.finish(), placed)
}

/// Lines written out, each with its line feed but the last when it has
/// none.
#[derive(Default)]
struct Out {
    bytes: Vec<u8>,
    unended: bool,
}

impl Out {
    fn line(&mut self, line: Line) {
        if self.unended {
            self.bytes.push(b'\n');
        }
        self.bytes.extend_from_slice(line.text);
        self.bytes.push(b'\n');
        self.unended = line.unended;
        if line.unended {
            self.bytes.pop();
        }
    }

    fn raw(&mut self, parts: &[&[u8]]) {
        let text = parts.concat();
        self.line(Line {
            text: &text,
            unended: false,
        });
    }

    /// A change kept both ways: `old` where `name` is not defined, `put`
    /// where it is, as GNU's patch writes it.
    fn defined<'a>(&mut self, name: &[u8], old: &[Line], put: impl Iterator<Item = Line<'a>>) {
        let put: Vec<Line> = put.collect();
        if old.is_empty() {
            self.raw(&[b"#ifdef ", name]);
        } else {
            self.raw(&[b"#ifndef ", name]);
            for &line in old {
                self.line(line);
            }
            if !put.is_empty() {
                self.raw(&[b"#else"]);
            }
        }
        for line in put {
            self.line(line);
        }
        self.raw(&[b"#endif"]);
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// What `text` becomes when the ed script `commands` runs on it, each
/// command on the text the one before left, every line with its line
/// feed as ed writes them; or the command, counted from 0, whose lines
/// are not there.
///
/// # Errors
///
/// The command that names lines the text does not have.
pub fn run_ed(text: &[Line], commands: &[Command]) -> Result<Vec<u8>, usize> {
    let mut lines: Vec<&[u8]> = text.iter().map(|l| l.text).collect();
    for (at, command) in commands.iter().enumerate() {
        let span = match command.letter {
            b'a' => Some(command.first..command.first),
            // A range whose lines go backwards is no range to ed.
            _ => command
                .first
                .checked_sub(1)
                .filter(|_| command.first <= command.last)
                .map(|start| start..command.last),
        };
        let Some(span) = span.filter(|s| s.start <= s.end && s.end <= lines.len()) else {
            return Err(at);
        };
        lines.splice(span, command.put.iter().copied());
    }
    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::{Hunks, patches};
    use crate::write::{Form, keys, write};
    use alloc::format;
    use alloc::string::String;
    use alloc::vec;

    /// What diff writes for `a` and `b` in `form`.
    fn written(form: Form, a: &str, b: &str) -> Vec<u8> {
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
        out
    }

    fn edits_of(patch: &[u8]) -> Vec<Edit<'_>> {
        match patches(patch, None).unwrap().pop().map(|p| p.hunks) {
            Some(Hunks::Edits(edits)) => edits,
            Some(Hunks::Ed(_)) => panic!("an ed script"),
            None => Vec::new(),
        }
    }

    const STRICT: Fit = Fit {
        loose: false,
        fuzz: 0,
    };
    const PATCH: Fit = Fit {
        loose: false,
        fuzz: 2,
    };

    /// `patch` put into `text` as `fit` lets it, and where its hunks went.
    fn patched(text: &str, patch: &[u8], fit: Fit, define: Option<&[u8]>) -> (String, Vec<Placed>) {
        let text = Text::new(text.as_bytes());
        let (bytes, placed) = apply(&lines(&text), &edits_of(patch), fit, define);
        (String::from_utf8(bytes).unwrap(), placed)
    }

    fn next(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    fn joined(lines: &[String], ended: bool) -> String {
        let mut text = lines.join("\n");
        if ended && !lines.is_empty() {
            text.push('\n');
        }
        text
    }

    /// Two texts, the second the first with a few lines changed: of a few
    /// words when `few`, or else each line its own.
    fn texts(seed: &mut u64, few: bool) -> (Vec<String>, Vec<String>) {
        let word = |seed: &mut u64, n: u64| {
            if few {
                ["a", "b", "", " a", "."][usize::try_from(next(seed) % 5).unwrap()].into()
            } else {
                format!("l{n}")
            }
        };
        let first: Vec<String> = (0..next(seed) % 25).map(|n| word(seed, n)).collect();
        let mut second = first.clone();
        for change in 0..next(seed) % 5 {
            let at = usize::try_from(next(seed)).unwrap() % (second.len() + 1);
            let count = next(seed) % 3;
            let new: Vec<String> = (0..count)
                .map(|n| word(seed, 100 * change + n + 1000))
                .collect();
            let end = (at + usize::try_from(next(seed) % 2).unwrap()).min(second.len());
            second.splice(at..end, new);
        }
        (first, second)
    }

    #[test]
    fn every_form_puts_the_first_text_right_where_it_says() {
        let mut seed = 0x2f69_3a8c_51e0_77b1_u64;
        for round in 0..1500 {
            let (first, second) = texts(&mut seed, true);
            let a = joined(&first, round % 3 != 0);
            let b = joined(&second, round % 5 != 0);
            let around = usize::try_from(next(&mut seed) % 4).unwrap();
            for form in [Form::Normal, Form::Context(around), Form::Unified(around)] {
                let patch = written(form, &a, &b);
                let (made, placed) = patched(&a, &patch, STRICT, None);
                assert_eq!(
                    made,
                    b,
                    "{form:?} of {a:?}:\n{}",
                    String::from_utf8_lossy(&patch)
                );
                assert!(placed.iter().all(|p| matches!(
                    p,
                    Placed::At {
                        offset: 0,
                        fuzz: 0,
                        ..
                    }
                )));
            }
            let script = written(Form::Ed, &a, &b);
            let found = patches(&script, None).unwrap();
            let ed = match found.first().map(|p| &p.hunks) {
                Some(Hunks::Ed(commands)) => commands.clone(),
                _ => Vec::new(),
            };
            let made = run_ed(&lines(&Text::new(a.as_bytes())), &ed).unwrap();
            let fed: Vec<u8> = Text::new(b.as_bytes())
                .lines
                .iter()
                .flat_map(|line| line.iter().chain(b"\n"))
                .copied()
                .collect();
            assert_eq!(made, fed, "ed writes every line feed");
        }
    }

    #[test]
    fn lines_put_in_before_move_every_hunk_by_as_many() {
        let mut seed = 0x7a3b_c1d9_0e62_f845_u64;
        for _ in 0..800 {
            let (first, second) = texts(&mut seed, false);
            if first.is_empty() {
                continue;
            }
            let (a, b) = (joined(&first, true), joined(&second, true));
            let more = usize::try_from(next(&mut seed) % 6).unwrap();
            let before: String =
                (0..more).fold(String::new(), |all, n| all + "p" + &n.to_string() + "\n");
            let around = 1 + usize::try_from(next(&mut seed) % 3).unwrap();
            for form in [Form::Context(around), Form::Unified(around)] {
                let patch = written(form, &a, &b);
                let (made, placed) = patched(&(before.clone() + &a), &patch, STRICT, None);
                assert_eq!(made, before.clone() + &b, "{form:?}");
                let moved = isize::try_from(more).unwrap();
                assert!(
                    placed
                        .iter()
                        .all(|p| matches!(p, Placed::At { offset, .. } if *offset == moved)),
                    "{placed:?}"
                );
            }
        }
    }

    const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    const NEW: &str = "1\n2\n3x\n4\n5\n6\n8\n9\n10\nn\n";

    #[test]
    fn context_that_differs_is_left_out_a_line_at_a_time() {
        // As GNU's patch puts it in.
        let patch = written(Form::Unified(3), TEN, NEW);
        let first = TEN.replacen("1\n", "1changed\n", 1);
        assert_eq!(
            patched(&first, &patch, PATCH, None),
            (
                NEW.replacen("1\n", "1changed\n", 1),
                vec![Placed::At {
                    line: 0,
                    offset: 0,
                    fuzz: 1
                }]
            )
        );
        let second = TEN.replacen("2\n", "2changed\n", 1);
        assert_eq!(
            patched(&second, &patch, PATCH, None),
            (
                NEW.replacen("2\n", "2changed\n", 1),
                vec![Placed::At {
                    line: 0,
                    offset: 0,
                    fuzz: 2
                }]
            )
        );
        assert_eq!(
            patched(
                &second,
                &patch,
                Fit {
                    loose: false,
                    fuzz: 1
                },
                None
            )
            .1,
            [Placed::Rejected]
        );
        assert_eq!(
            patched(&second, &patch, STRICT, None),
            (second.clone(), vec![Placed::Rejected])
        );
        let third = TEN.replacen("3\n", "three\n", 1);
        assert_eq!(
            patched(&third, &patch, PATCH, None).1,
            [Placed::Rejected],
            "a changed line is no context"
        );
    }

    #[test]
    fn a_hunk_goes_where_its_lines_are_nearest() {
        let patch = b"@@ -3 +3 @@\n-x\n+y\n";
        assert_eq!(
            patched("x\na\nb\nc\nd\nx\n", patch, STRICT, None),
            (
                "y\na\nb\nc\nd\nx\n".into(),
                vec![Placed::At {
                    line: 0,
                    offset: -2,
                    fuzz: 0
                }]
            ),
            "two lines before is nearer than three later"
        );
        assert_eq!(
            patched("x\na\nb\nc\nx\n", patch, STRICT, None).1,
            [Placed::At {
                line: 4,
                offset: 2,
                fuzz: 0
            }],
            "as near both ways, the later"
        );
        assert_eq!(
            patched("a\nx\nb\nc\nd\nx\n", patch, STRICT, None).1,
            [Placed::At {
                line: 1,
                offset: -1,
                fuzz: 0
            }]
        );
        assert_eq!(
            patched("a\nb\nc\n", patch, STRICT, None),
            ("a\nb\nc\n".into(), vec![Placed::Rejected])
        );
        let two = b"@@ -1 +1 @@\n-x\n+y\n@@ -3 +3 @@\n-x\n+z\n";
        assert_eq!(
            patched("q\nx\nq\nx\n", two, STRICT, None),
            (
                "q\ny\nq\nz\n".into(),
                vec![
                    Placed::At {
                        line: 1,
                        offset: 1,
                        fuzz: 0
                    },
                    Placed::At {
                        line: 3,
                        offset: 1,
                        fuzz: 0
                    },
                ]
            ),
            "the second goes on from where the first went"
        );
        assert_eq!(
            patched("x\nq\n", two, STRICT, None),
            (
                "y\nq\n".into(),
                vec![
                    Placed::At {
                        line: 0,
                        offset: 0,
                        fuzz: 0
                    },
                    Placed::Rejected
                ]
            ),
            "never back before a hunk put in"
        );
    }

    #[test]
    fn a_hunk_may_start_where_the_one_before_put_lines_in() {
        let patch = b"@@ -0,0 +1 @@\n+x\n@@ -1 +2 @@\n-a\n+b\n";
        assert_eq!(
            patched("a\n", patch, STRICT, None),
            (
                "x\nb\n".into(),
                vec![
                    Placed::At {
                        line: 0,
                        offset: 0,
                        fuzz: 0
                    },
                    Placed::At {
                        line: 0,
                        offset: 0,
                        fuzz: 0
                    },
                ]
            )
        );
    }

    #[test]
    fn a_hunk_with_no_lines_to_find_goes_only_where_it_says() {
        let patch = b"2a3\n> new\n";
        assert_eq!(patched("a\nb\nc\n", patch, PATCH, None).0, "a\nb\nnew\nc\n");
        assert_eq!(patched("a\nb\n", patch, PATCH, None).0, "a\nb\nnew\n");
        assert_eq!(
            patched("a\n", patch, PATCH, None),
            ("a\n".into(), vec![Placed::Rejected])
        );
        let after_one = b"1c1\n< x\n---\n> y\n2a3\n> new\n";
        assert_eq!(
            patched("q\nx\nz\n", after_one, PATCH, None),
            (
                "q\ny\nz\nnew\n".into(),
                vec![
                    Placed::At {
                        line: 1,
                        offset: 1,
                        fuzz: 0
                    },
                    Placed::At {
                        line: 3,
                        offset: 1,
                        fuzz: 0
                    },
                ]
            ),
            "it moves as the hunk before it moved"
        );
    }

    #[test]
    fn loose_lines_match_in_any_blanks_and_keep_the_texts_own() {
        // As GNU's patch -l puts it in.
        let patch = written(Form::Unified(3), "a  b\nc d\ne\n", "a  b\nc d\nE\n");
        let text = "a b\nc  d\ne\n";
        assert_eq!(
            patched(
                text,
                &patch,
                Fit {
                    loose: true,
                    fuzz: 0
                },
                None
            )
            .0,
            "a b\nc  d\nE\n"
        );
        assert_eq!(patched(text, &patch, STRICT, None).1, [Placed::Rejected]);
        assert!(alike(b"a \t b", b"a b", true));
        assert!(alike(b"\ta", b" a", true));
        assert!(!alike(b"ab", b"a b", true), "a blank matches only blanks");
        assert!(!alike(b"a b", b"a  b", false));
        assert_eq!(squeezed(b" a  b\t").collect::<Vec<_>>(), b" a b ");
    }

    #[test]
    fn with_a_name_defined_both_versions_stay() {
        // What GNU's patch -D FOO writes.
        let gnu = "1\n2\n#ifndef FOO\n3\n#else\n3x\n#endif\n4\n5\n6\n#ifndef FOO\n7\n#endif\n\
                   8\n9\n10\n#ifdef FOO\nn\n#endif\n";
        for form in [Form::Normal, Form::Unified(3), Form::Context(1)] {
            let patch = written(form, TEN, NEW);
            assert_eq!(patched(TEN, &patch, PATCH, Some(b"FOO")).0, gnu, "{form:?}");
        }
    }

    #[test]
    fn a_missing_line_feed_comes_and_goes_with_the_last_line() {
        for (a, b) in [
            ("x\ny", "x\nz\n"),
            ("x\ny\n", "x\ny"),
            ("x", "x\ny"),
            ("x\ny", "q\nx\ny"),
            ("a", ""),
        ] {
            for form in [Form::Normal, Form::Unified(1), Form::Context(0)] {
                let patch = written(form, a, b);
                assert_eq!(
                    patched(a, &patch, STRICT, None).0,
                    b,
                    "{form:?} {a:?} {b:?}"
                );
            }
        }
        let mut out = Out::default();
        out.line(Line {
            text: b"a",
            unended: true,
        });
        out.line(Line {
            text: b"b",
            unended: false,
        });
        assert_eq!(
            out.finish(),
            b"a\nb\n",
            "a line without its feed gets one once more follow"
        );
    }

    #[test]
    fn a_hunk_that_fits_and_one_already_put_in() {
        let patch = written(Form::Unified(1), TEN, NEW);
        let edits = edits_of(&patch);
        let old = lines(&Text::new(TEN.as_bytes()));
        let new = lines(&Text::new(NEW.as_bytes()));
        assert!(fits(&old, &edits[0], STRICT));
        assert!(!fits(&new, &edits[0], STRICT), "once in, it fits no more");
        assert!(fits(&new, &edits[0].reversed(), STRICT), "undone it does");
        assert!(!fits(&old, &edits[0].reversed(), STRICT));
        let spaced = lines(&Text::new(b"1\n2  \n3\n"));
        let tight = edits_of(b"@@ -1,3 +1,3 @@\n 1\n 2 \n-3\n+4\n");
        assert!(!fits(&spaced, &tight[0], STRICT));
        assert!(fits(
            &spaced,
            &tight[0],
            Fit {
                loose: true,
                fuzz: 0
            }
        ));
    }

    #[test]
    fn an_ed_script_runs_one_command_after_another() {
        let text = lines(&Text::new(b"a\nb\nc"));
        let script = patches(b"3c\nC\n.\n1a\nx\n.\n2,3d\n0a\nfirst\n.\n", None).unwrap();
        let Hunks::Ed(commands) = &script[0].hunks else {
            panic!("ed");
        };
        assert_eq!(run_ed(&text, commands), Ok(b"first\na\nC\n".to_vec()));
        for (bad, at) in [
            (&b"4a\nx\n.\n"[..], 0),
            (b"1d\n0d\n", 1),
            (b"2,4c\nx\n.\n", 0),
            (b"3,2d\n", 0),
        ] {
            let script = patches(bad, None).unwrap();
            let Hunks::Ed(commands) = &script[0].hunks else {
                panic!("ed");
            };
            assert_eq!(
                run_ed(&text, commands),
                Err(at),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn a_text_s_lines_know_which_lacks_its_feed() {
        let line = |text: &'static [u8], unended| Line { text, unended };
        assert_eq!(
            lines(&Text::new(b"a\nb")),
            [line(b"a", false), line(b"b", true)]
        );
        assert_eq!(
            lines(&Text::new(b"a\nb\n")),
            [line(b"a", false), line(b"b", false)]
        );
        assert_eq!(lines(&Text::new(b"")), []);
    }

    #[test]
    fn fuzz_leaves_out_no_more_context_than_there_is() {
        let edit = |marks: &[Mark]| Edit {
            old: 0,
            new: 0,
            lines: marks
                .iter()
                .map(|&m| {
                    (
                        m,
                        Line {
                            text: b"x",
                            unended: false,
                        },
                    )
                })
                .collect(),
        };
        let (s, g, p) = (Mark::Same, Mark::Gone, Mark::Put);
        assert_eq!(trimmed(&edit(&[s, s, s, g, p, s]), 2).0, 2);
        assert_eq!(trimmed(&edit(&[s, s, s, g, p, s]), 2).1.len(), 3);
        assert_eq!(trimmed(&edit(&[g, s, s]), 2).0, 0);
        assert_eq!(trimmed(&edit(&[g, s, s]), 2).1.len(), 1);
        let all = edit(&[s, s, s]);
        let (front, rest) = trimmed(&all, 2);
        assert_eq!(
            (front, rest.len()),
            (2, 0),
            "a hunk of context alone keeps none"
        );
        assert_eq!(trimmed(&all, 1).1.len(), 1);
    }
}
