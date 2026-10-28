//! Differences read back from what diff wrote, in any of its forms, as
//! patch reads them: the patches a file holds, the names each gives, and
//! their hunks.

use alloc::vec::Vec;

/// One line of a hunk, and whether diff's note that it has no line feed
/// follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line<'a> {
    pub text: &'a [u8],
    pub unended: bool,
}

/// Where a line of a hunk is: in both texts, in the old one only, or in
/// the new one only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Same,
    Gone,
    Put,
}

/// A hunk of a patch in any form but ed's: where its lines start in the
/// old text and in the new, counted from 0, and its lines in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit<'a> {
    pub old: usize,
    pub new: usize,
    pub lines: Vec<(Mark, Line<'a>)>,
}

impl<'a> Edit<'a> {
    /// The hunk that undoes this one.
    #[must_use]
    pub fn reversed(&self) -> Edit<'a> {
        let swap = |mark| match mark {
            Mark::Gone => Mark::Put,
            Mark::Put => Mark::Gone,
            Mark::Same => Mark::Same,
        };
        Edit {
            old: self.new,
            new: self.old,
            lines: self.lines.iter().map(|&(m, l)| (swap(m), l)).collect(),
        }
    }

    /// Its lines that are in the old text, or with `put` the new.
    pub fn side(&self, put: bool) -> impl Iterator<Item = Line<'a>> + '_ {
        let left = if put { Mark::Gone } else { Mark::Put };
        self.lines
            .iter()
            .filter(move |(mark, _)| *mark != left)
            .map(|&(_, line)| line)
    }
}

/// A command of an ed script, as diff writes them: after line `first`
/// with `a`, or on lines `first` to `last` with `c` and `d`, counted from
/// 1, and the lines it puts in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command<'a> {
    pub letter: u8,
    pub first: usize,
    pub last: usize,
    pub put: Vec<&'a [u8]>,
}

/// The form a patch is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Context,
    Unified,
    Ed,
}

/// The hunks of a patch: an ed script's commands, in the order they run,
/// or every other form's hunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hunks<'a> {
    Edits(Vec<Edit<'a>>),
    Ed(Vec<Command<'a>>),
}

/// One patch of a file: its form, the names its header gives the old
/// text and the new one, the name an `Index:` line gives, and its hunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch<'a> {
    pub kind: Kind,
    pub old_name: Option<&'a [u8]>,
    pub new_name: Option<&'a [u8]>,
    pub index: Option<&'a [u8]>,
    pub hunks: Hunks<'a>,
}

/// Why a patch file could not be read: the line, counted from 1, and
/// what is wrong there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wrong {
    pub line: usize,
    pub why: &'static str,
}

/// What diff says after a line that has no line feed.
const NOTE: &[u8] = b"\\ No newline at end of file";

/// The patches in `bytes`, each of the form `kind` when it is given and
/// of whatever form its first hunk is in otherwise. Lines between patches
/// that give no name are left alone, as patch leaves them.
///
/// # Errors
///
/// A hunk that is cut short or does not add up.
pub fn patches(bytes: &[u8], kind: Option<Kind>) -> Result<Vec<Patch<'_>>, Wrong> {
    let lines = lines_of(bytes);
    let mut found = Vec::new();
    let (mut old_name, mut new_name, mut index) = (None, None, None);
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        if let Some(form) = starts(line, kind) {
            let (hunks, next) = Reader { lines: &lines, at }.hunks(form)?;
            found.push(Patch {
                kind: form,
                old_name: old_name.take(),
                new_name: new_name.take(),
                index: index.take(),
                hunks,
            });
            at = next;
            continue;
        }
        let next = lines.get(at + 1).copied().unwrap_or_default();
        if let Some(name) = line.strip_prefix(b"Index: ") {
            index = Some(name_in(name));
        } else if let (Some(old), Some(new)) = (
            line.strip_prefix(b"*** ")
                .filter(|_| kind != Some(Kind::Unified)),
            next.strip_prefix(b"--- "),
        ) {
            (old_name, new_name) = (Some(name_in(old)), Some(name_in(new)));
            at += 1;
        } else if let (Some(old), Some(new)) =
            (line.strip_prefix(b"--- "), next.strip_prefix(b"+++ "))
        {
            (old_name, new_name) = (Some(name_in(old)), Some(name_in(new)));
            at += 1;
        }
        at += 1;
    }
    Ok(found)
}

/// The lines of `bytes`, without the run of blanks every one of them
/// starts with, as when a patch was indented in a mail.
fn lines_of(bytes: &[u8]) -> Vec<&[u8]> {
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let lines: Vec<&[u8]> = if bytes.is_empty() {
        Vec::new()
    } else {
        body.split(|&b| b == b'\n').collect()
    };
    let blanks = |line: &[u8]| {
        line.iter()
            .take_while(|&&b| matches!(b, b' ' | b'\t'))
            .count()
    };
    let shared = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| (blanks(line), *line))
        .reduce(|(most, first), (count, line)| {
            let alike = first.iter().zip(line).take(most.min(count));
            (alike.take_while(|(x, y)| x == y).count(), first)
        })
        .map_or(0, |(most, _)| most);
    lines
        .into_iter()
        .map(|line| line.get(shared..).unwrap_or_default())
        .collect()
}

/// A file's name as a header gives it: up to a tab, after which a time
/// may follow.
fn name_in(rest: &[u8]) -> &[u8] {
    rest.split(|&b| b == b'\t').next().unwrap_or_default()
}

/// The form of the hunk that starts at `line`, if one does, of the form
/// `kind` only when that is given.
fn starts(line: &[u8], kind: Option<Kind>) -> Option<Kind> {
    let form = if line.starts_with(b"@@ -") {
        Kind::Unified
    } else if line.len() >= 15 && line.iter().all(|&b| b == b'*') {
        Kind::Context
    } else {
        match command(line)?.new {
            None => Kind::Ed,
            Some(_) => Kind::Normal,
        }
    };
    kind.is_none_or(|kind| kind == form).then_some(form)
}

/// A range of lines as diff's own form and ed write it: one number, or
/// two parted by a comma.
fn range(text: &[u8]) -> Option<(usize, usize)> {
    let number = |digits: &[u8]| -> Option<usize> {
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        core::str::from_utf8(digits).ok()?.parse().ok()
    };
    match text.iter().position(|&b| b == b',') {
        Some(comma) => Some((number(&text[..comma])?, number(&text[comma + 1..])?)),
        None => number(text).map(|n| (n, n)),
    }
}

/// A command line of diff's own form or of an ed script: the range of
/// the old text, the letter, and in diff's own form the range of the new.
struct Header {
    lines: (usize, usize),
    letter: u8,
    new: Option<(usize, usize)>,
}

fn command(line: &[u8]) -> Option<Header> {
    let at = line.iter().position(|b| matches!(b, b'a' | b'c' | b'd'))?;
    let lines = range(&line[..at])?;
    let rest = &line[at + 1..];
    let new = if rest.is_empty() {
        None
    } else {
        Some(range(rest)?)
    };
    Some(Header {
        lines,
        letter: line[at],
        new,
    })
}

/// The lines of a patch file, read from line `at` on.
struct Reader<'l, 'a> {
    lines: &'l [&'a [u8]],
    at: usize,
}

impl<'a> Reader<'_, 'a> {
    /// What is wrong with the line read last.
    fn wrong(&self, why: &'static str) -> Wrong {
        Wrong { line: self.at, why }
    }

    fn peek(&self) -> Option<&'a [u8]> {
        self.lines.get(self.at).copied()
    }

    /// The next line, which there has to be.
    fn next(&mut self) -> Result<&'a [u8], Wrong> {
        let line = self.peek().ok_or(Wrong {
            line: self.at + 1,
            why: "the patch ends inside a hunk",
        })?;
        self.at += 1;
        Ok(line)
    }

    /// Takes diff's note when it comes next, and says whether it did.
    fn note(&mut self) -> bool {
        let noted = self.peek() == Some(NOTE);
        self.at += usize::from(noted);
        noted
    }

    /// The hunks of the form `kind` from here on, and the line after them.
    fn hunks(mut self, kind: Kind) -> Result<(Hunks<'a>, usize), Wrong> {
        if kind == Kind::Ed {
            let mut commands = Vec::new();
            while let Some(header) = self.peek().and_then(command).filter(|h| h.new.is_none()) {
                self.at += 1;
                let (first, last) = header.lines;
                commands.push(self.ed(first, last, header.letter)?);
            }
            return Ok((Hunks::Ed(commands), self.at));
        }
        let mut edits = Vec::new();
        while self
            .peek()
            .and_then(|line| starts(line, Some(kind)))
            .is_some()
        {
            let first = self.at;
            let header = self.next()?;
            let edit = match kind {
                Kind::Unified => self.unified(header)?,
                Kind::Context => self.context()?,
                _ => self.normal(header)?,
            };
            if edits.last().is_some_and(|last: &Edit| edit.old < last.old) {
                return Err(Wrong {
                    line: first + 1,
                    why: "the hunks of a patch go up the file",
                });
            }
            edits.push(edit);
        }
        Ok((Hunks::Edits(edits), self.at))
    }

    /// A hunk of the unified form, after its header.
    fn unified(&mut self, header: &[u8]) -> Result<Edit<'a>, Wrong> {
        let ranges = header[3..]
            .split(|&b| b == b' ')
            .take(2)
            .collect::<Vec<_>>();
        let side = |text: Option<&&[u8]>, sign: u8| -> Option<(usize, usize)> {
            let text = text?.strip_prefix(&[sign])?;
            let (start, count) = match text.iter().position(|&b| b == b',') {
                Some(comma) => (range(&text[..comma])?.0, range(&text[comma + 1..])?.0),
                None => (range(text)?.0, 1),
            };
            let first = if count == 0 {
                start
            } else {
                start.checked_sub(1)?
            };
            Some((first, count))
        };
        let (Some((old, mut long)), Some((new, mut wide))) =
            (side(ranges.first(), b'-'), side(ranges.get(1), b'+'))
        else {
            return Err(self.wrong("a hunk header that is not one"));
        };
        let mut lines = Vec::new();
        while long > 0 || wide > 0 {
            let line = self.next()?;
            let (mark, text) = match line.split_first() {
                Some((b' ', text)) => (Mark::Same, text),
                Some((b'-', text)) => (Mark::Gone, text),
                Some((b'+', text)) => (Mark::Put, text),
                // A line in both that lost its blank.
                None => (Mark::Same, line),
                Some(_) => return Err(self.wrong("a line in a hunk that is none")),
            };
            let (old_left, new_left) = match mark {
                Mark::Same => (long.checked_sub(1), wide.checked_sub(1)),
                Mark::Gone => (long.checked_sub(1), Some(wide)),
                Mark::Put => (Some(long), wide.checked_sub(1)),
            };
            let (Some(old_left), Some(new_left)) = (old_left, new_left) else {
                return Err(self.wrong("more lines in a hunk than its header says"));
            };
            (long, wide) = (old_left, new_left);
            let unended = self.note();
            lines.push((mark, Line { text, unended }));
        }
        Ok(Edit { old, new, lines })
    }

    /// A hunk of the context form, after its row of stars.
    fn context(&mut self) -> Result<Edit<'a>, Wrong> {
        let header = self.next()?;
        let old = header
            .strip_prefix(b"*** ")
            .and_then(|r| r.strip_suffix(b" ****"))
            .and_then(range)
            .ok_or(self.wrong("a hunk header that is not one"))?;
        let first = self.side(b"- ");
        let header = self.next()?;
        let new = header
            .strip_prefix(b"--- ")
            .and_then(|r| r.strip_suffix(b" ----"))
            .and_then(range)
            .ok_or(self.wrong("a hunk header that is not one"))?;
        let second = self.side(b"+ ");
        let lines = merged(&first, &second).ok_or(self.wrong("the two sides of a hunk differ"))?;
        let mut edit = Edit {
            old: 0,
            new: 0,
            lines,
        };
        // A range of none names the line before it, one of some its first
        // and last lines.
        let start = |(first, last): (usize, usize), count: usize| -> Option<usize> {
            if count == 0 {
                return (first == last).then_some(first);
            }
            (last.checked_sub(first)? + 1 == count).then(|| first.checked_sub(1))?
        };
        let (Some(old), Some(new)) = (
            start(old, edit.side(false).count()),
            start(new, edit.side(true).count()),
        ) else {
            return Err(self.wrong("a hunk whose lines do not fit its header"));
        };
        (edit.old, edit.new) = (old, new);
        Ok(edit)
    }

    /// The lines of one side of a context hunk, marked `alone` where the
    /// other side has none, with their marks.
    fn side(&mut self, alone: &[u8]) -> Vec<(u8, Line<'a>)> {
        let mut lines = Vec::new();
        while let Some(line) = self.peek() {
            let mark = match line.get(..2) {
                Some(b"  ") => b' ',
                Some(b"! ") => b'!',
                Some(mark) if mark == alone => alone[0],
                _ => break,
            };
            self.at += 1;
            let unended = self.note();
            lines.push((
                mark,
                Line {
                    text: &line[2..],
                    unended,
                },
            ));
        }
        lines
    }

    /// A hunk of diff's own form, after its header.
    fn normal(&mut self, header: &[u8]) -> Result<Edit<'a>, Wrong> {
        let Some(Header {
            lines: (first, last),
            letter,
            new: Some((start, end)),
        }) = command(header)
        else {
            return Err(self.wrong("a hunk header that is not one"));
        };
        let count = |first: usize, last: usize| last.checked_sub(first).map(|n| n + 1);
        let (old, gone) = if letter == b'a' {
            (Some(first), Some(0))
        } else {
            (first.checked_sub(1), count(first, last))
        };
        let (new, put) = if letter == b'd' {
            (Some(start), Some(0))
        } else {
            (start.checked_sub(1), count(start, end))
        };
        let (Some(old), Some(gone), Some(new), Some(put)) = (old, gone, new, put) else {
            return Err(self.wrong("a hunk header that is not one"));
        };
        let mut lines = Vec::new();
        for _ in 0..gone {
            lines.push((Mark::Gone, self.marked(b"< ")?));
        }
        if gone > 0 && put > 0 && self.next()? != b"---" {
            return Err(self.wrong("a change without its ---"));
        }
        for _ in 0..put {
            lines.push((Mark::Put, self.marked(b"> ")?));
        }
        Ok(Edit { old, new, lines })
    }

    /// The next line, which has to begin with `mark`.
    fn marked(&mut self, mark: &[u8]) -> Result<Line<'a>, Wrong> {
        let line = self.next()?;
        let text = line
            .strip_prefix(mark)
            .ok_or(self.wrong("fewer lines in a hunk than its header says"))?;
        Ok(Line {
            text,
            unended: self.note(),
        })
    }

    /// An ed command after its line, with the lines it puts in. A line of
    /// one dot comes as two and is put right with `s/.//`, after which
    /// `a` takes more lines.
    fn ed(&mut self, first: usize, last: usize, letter: u8) -> Result<Command<'a>, Wrong> {
        let mut put: Vec<&'a [u8]> = Vec::new();
        if letter != b'd' {
            loop {
                let line = self.next()?;
                if line == b"." {
                    if self.peek() != Some(b"s/.//") {
                        break;
                    }
                    self.at += 1;
                    let Some(last) = put.last_mut() else {
                        return Err(self.wrong("s/.// with no line before it"));
                    };
                    *last = last.get(1..).unwrap_or_default();
                    if self.peek() != Some(b"a") {
                        break;
                    }
                    self.at += 1;
                    continue;
                }
                put.push(line);
            }
        }
        Ok(Command {
            letter,
            first,
            last,
            put,
        })
    }
}

/// The lines of a context hunk in order, from its two sides: the first's
/// lines marked `-` or `!` gone, the second's `+` or `!` put, and those in
/// both once. A side left out is the other's lines in both.
fn merged<'a>(
    first: &[(u8, Line<'a>)],
    second: &[(u8, Line<'a>)],
) -> Option<Vec<(Mark, Line<'a>)>> {
    let alike = |side: &[(u8, Line<'a>)]| -> Vec<(u8, Line<'a>)> {
        side.iter().copied().filter(|(m, _)| *m == b' ').collect()
    };
    let (first, second) = match (first.is_empty(), second.is_empty()) {
        (true, _) => (alike(second), second.to_vec()),
        (_, true) => (first.to_vec(), alike(first)),
        _ => (first.to_vec(), second.to_vec()),
    };
    let mut lines = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < first.len() || j < second.len() {
        let (x, y) = (first.get(i), second.get(j));
        if let Some(&(b'-', line)) = x {
            lines.push((Mark::Gone, line));
            i += 1;
        } else if let Some(&(b'+', line)) = y {
            lines.push((Mark::Put, line));
            j += 1;
        } else if let Some(&(b'!', _)) = x {
            while let Some(&(b'!', line)) = first.get(i) {
                lines.push((Mark::Gone, line));
                i += 1;
            }
            while let Some(&(b'!', line)) = second.get(j) {
                lines.push((Mark::Put, line));
                j += 1;
            }
        } else if let (Some(&(b' ', a)), Some(&(b' ', b))) = (x, y) {
            if a.text != b.text {
                return None;
            }
            lines.push((Mark::Same, a));
            i += 1;
            j += 1;
        } else {
            return None;
        }
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::{Form, Text, keys, write};
    use alloc::string::String;
    use alloc::vec;

    /// A text as its lines, each with whether it ends without a line feed.
    type Lines = Vec<(Vec<u8>, bool)>;

    fn lines_in(text: &str) -> Lines {
        let text = Text::new(text.as_bytes());
        (0..text.lines.len())
            .map(|at| {
                (
                    text.lines[at].to_vec(),
                    !text.ended && at + 1 == text.lines.len(),
                )
            })
            .collect()
    }

    /// What diff writes for `a` and `b` in `form`, the texts named A and B.
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

    /// `edits` made on `a` each where it says, the lines it says are
    /// there checked against `a`, and where it says the new text starts
    /// against the lines before it.
    fn exactly(a: &Lines, edits: &[Edit]) -> Lines {
        let mut b: Lines = Vec::new();
        let mut at = 0;
        for edit in edits {
            assert!(at <= edit.old, "hunks in order");
            b.extend_from_slice(&a[at..edit.old]);
            assert_eq!(b.len(), edit.new, "where the hunk starts in the new text");
            let old: Lines = edit
                .side(false)
                .map(|l| (l.text.to_vec(), l.unended))
                .collect();
            assert_eq!(old[..], a[edit.old..edit.old + old.len()], "the old lines");
            b.extend(edit.side(true).map(|l| (l.text.to_vec(), l.unended)));
            at = edit.old + old.len();
        }
        b.extend_from_slice(&a[at..]);
        b
    }

    /// `commands` run on `a` one after the other, as ed runs them.
    fn run(a: &Lines, commands: &[Command]) -> Lines {
        let mut text = a.clone();
        for command in commands {
            let (start, end) = match command.letter {
                b'a' => (command.first, command.first),
                _ => (command.first - 1, command.last),
            };
            let put = command.put.iter().map(|l| (l.to_vec(), false));
            text.splice(start..end, put);
        }
        text
    }

    fn next(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    fn random_lines(seed: &mut u64, most: u64) -> Vec<&'static str> {
        const WORDS: [&str; 9] = ["a", "b", "c", "", " a", "a ", ".", "..", "-x"];
        (0..next(seed) % (most + 1))
            .map(|_| WORDS[usize::try_from(next(seed) % 9).unwrap()])
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
    fn every_form_diff_writes_reads_back_to_what_it_says() {
        let mut seed = 0x5851_f42d_4c95_7f2d_u64;
        for round in 0..1500 {
            let first = random_lines(&mut seed, 25);
            let mut second = first.clone();
            for _ in 0..next(&mut seed) % 5 {
                let at = usize::try_from(next(&mut seed)).unwrap() % (second.len() + 1);
                let new = random_lines(&mut seed, 2);
                second.splice(at..(at + 1).min(second.len()), new);
            }
            let a = joined(&first, round % 3 != 0);
            let b = joined(&second, round % 4 != 0);
            let around = usize::try_from(next(&mut seed) % 4).unwrap();
            for (form, kind) in [
                (Form::Normal, Kind::Normal),
                (Form::Context(around), Kind::Context),
                (Form::Unified(around), Kind::Unified),
                (Form::Ed, Kind::Ed),
            ] {
                let patch = written(form, &a, &b);
                let found = patches(&patch, None).unwrap();
                assert_eq!(found, patches(&patch, Some(kind)).unwrap(), "as asked");
                if patch.is_empty() {
                    assert_eq!(found, []);
                    continue;
                }
                let shown = String::from_utf8_lossy(&patch);
                let [one] = &found[..] else {
                    panic!("one patch in {shown}");
                };
                assert_eq!(one.kind, kind, "{shown}");
                let named = matches!(kind, Kind::Context | Kind::Unified);
                assert_eq!(one.old_name, named.then_some(&b"A"[..]));
                assert_eq!(one.new_name, named.then_some(&b"B"[..]));
                let made = match &one.hunks {
                    Hunks::Edits(edits) => exactly(&lines_in(&a), edits),
                    Hunks::Ed(commands) => {
                        let ended = |lines: Lines| -> Lines {
                            lines.into_iter().map(|(l, _)| (l, false)).collect()
                        };
                        assert_eq!(
                            run(&ended(lines_in(&a)), commands),
                            ended(lines_in(&b)),
                            "{shown}"
                        );
                        continue;
                    }
                };
                assert_eq!(made, lines_in(&b), "{form:?} of {a:?} and {b:?}:\n{shown}");
            }
        }
    }

    #[test]
    fn a_form_asked_for_finds_no_hunk_of_another() {
        let patch = written(Form::Unified(3), "a\nb\n", "a\nc\n");
        assert_eq!(patches(&patch, Some(Kind::Normal)).unwrap(), []);
        assert_eq!(patches(&patch, Some(Kind::Context)).unwrap(), []);
        assert_eq!(patches(&patch, Some(Kind::Ed)).unwrap(), []);
        let normal = written(Form::Normal, "a\nb\n", "a\nc\n");
        assert_eq!(patches(&normal, Some(Kind::Ed)).unwrap(), []);
        assert_eq!(patches(&normal, Some(Kind::Unified)).unwrap(), []);
        let ed = written(Form::Ed, "a\nb\n", "a\nc\n");
        assert_eq!(patches(&ed, Some(Kind::Normal)).unwrap(), []);
        assert_eq!(patches(&ed, None).unwrap()[0].kind, Kind::Ed);
    }

    #[test]
    fn several_patches_with_their_names_and_lines_around_them() {
        // As GNU's diff -ru writes them, times and all.
        let file = b"Only in a: x\n\
            diff -ru a/f b/f\n\
            --- a/f\t2026-10-28 07:39:51.973898918 +0100\n\
            +++ b/f\t2026-10-28 07:39:51.974307502 +0100\n\
            @@ -1 +1 @@\n-x\n+y\n\
            Index: c/g\n\
            *** a/g\n--- b/g\n***************\n*** 1 ****\n! 1\n--- 1 ----\n! 2\n\
            trailing words\n";
        let found = patches(file, None).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            (
                found[0].kind,
                found[0].old_name,
                found[0].new_name,
                found[0].index
            ),
            (Kind::Unified, Some(&b"a/f"[..]), Some(&b"b/f"[..]), None)
        );
        assert_eq!(
            (
                found[1].kind,
                found[1].old_name,
                found[1].new_name,
                found[1].index
            ),
            (
                Kind::Context,
                Some(&b"a/g"[..]),
                Some(&b"b/g"[..]),
                Some(&b"c/g"[..])
            )
        );
        let line = |text: &'static [u8]| Line {
            text,
            unended: false,
        };
        assert_eq!(
            found[1].hunks,
            Hunks::Edits(vec![Edit {
                old: 0,
                new: 0,
                lines: vec![(Mark::Gone, line(b"1")), (Mark::Put, line(b"2"))],
            }])
        );
        let normal = patches(b"Index: h\n1c1\n< p\n---\n> q\n", None).unwrap();
        assert_eq!(normal[0].index, Some(&b"h"[..]));
        assert_eq!((normal[0].old_name, normal[0].new_name), (None, None));
    }

    #[test]
    fn blanks_every_line_starts_with_are_left_out() {
        let indented = b"    --- a\n    +++ b\n    @@ -1 +1 @@\n    -x\n    +y\n";
        let plain = b"--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n";
        assert_eq!(patches(indented, None), patches(plain, None));
        assert_eq!(patches(indented, None).unwrap().len(), 1);
        let mixed = b"\t--- a\n  +++ b\n";
        assert_eq!(
            lines_of(mixed),
            [&b"\t--- a"[..], b"  +++ b"],
            "only blanks alike"
        );
        assert_eq!(
            lines_of(b"  a\n\n   b\n"),
            [&b"a"[..], b"", b" b"],
            "an empty line counts not"
        );
        assert_eq!(lines_of(b""), Vec::<&[u8]>::new());
        assert_eq!(lines_of(b"a"), [&b"a"[..]]);
    }

    #[test]
    fn a_line_in_both_may_have_lost_its_blank() {
        let found = patches(b"@@ -1,3 +1,3 @@\n a\n\n-b\n+c\n", None).unwrap();
        let Hunks::Edits(edits) = &found[0].hunks else {
            panic!("edits");
        };
        assert_eq!(
            edits[0].lines[1],
            (
                Mark::Same,
                Line {
                    text: b"",
                    unended: false
                }
            )
        );
    }

    #[test]
    fn a_note_marks_the_line_before_it() {
        let found = patches(b"@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n", None).unwrap();
        let Hunks::Edits(edits) = &found[0].hunks else {
            panic!("edits");
        };
        assert_eq!(
            edits[0].lines,
            vec![
                (
                    Mark::Gone,
                    Line {
                        text: b"x",
                        unended: true
                    }
                ),
                (
                    Mark::Put,
                    Line {
                        text: b"y",
                        unended: false
                    }
                ),
            ]
        );
    }

    #[test]
    fn an_ed_script_puts_a_dot_back_and_takes_more_after_a() {
        let found = patches(b"1a\n..\n.\ns/.//\na\nx\n..\n.\ns/.//\n3,4d\n", None).unwrap();
        assert_eq!(
            found[0].hunks,
            Hunks::Ed(vec![
                Command {
                    letter: b'a',
                    first: 1,
                    last: 1,
                    put: vec![&b"."[..], b"x", b"."]
                },
                Command {
                    letter: b'd',
                    first: 3,
                    last: 4,
                    put: vec![]
                },
            ])
        );
        assert_eq!(
            patches(b"2c\n.\ns/.//\n", None),
            Err(Wrong {
                line: 3,
                why: "s/.// with no line before it"
            })
        );
    }

    #[test]
    fn a_hunk_that_does_not_add_up_is_refused() {
        let wrong = |file: &[u8]| patches(file, None).err();
        let at = |line, why| Some(Wrong { line, why });
        assert_eq!(
            wrong(b"@@ -1 +1 @@\n-x\n"),
            at(3, "the patch ends inside a hunk")
        );
        assert_eq!(
            wrong(b"@@ -x +1 @@\n"),
            at(1, "a hunk header that is not one")
        );
        assert_eq!(
            wrong(b"@@ -0 +1 @@\n"),
            at(1, "a hunk header that is not one")
        );
        assert_eq!(
            wrong(b"@@ -1 +1 @@\n*x\n"),
            at(2, "a line in a hunk that is none")
        );
        assert_eq!(
            wrong(b"@@ -1 +1 @@\n x\n x\n"),
            None,
            "a line after the hunk is no part of it"
        );
        assert_eq!(
            wrong(b"@@ -1,0 +1 @@\n-x\n"),
            at(2, "more lines in a hunk than its header says")
        );
        assert_eq!(wrong(b"1c1\n< p\n> q\n"), at(3, "a change without its ---"));
        assert_eq!(
            wrong(b"1d0\n> p\n"),
            at(2, "fewer lines in a hunk than its header says")
        );
        assert_eq!(wrong(b"0d0\n"), at(1, "a hunk header that is not one"));
        assert_eq!(
            wrong(b"3d2\n< c\n1d0\n< a\n"),
            at(3, "the hunks of a patch go up the file")
        );
        assert_eq!(
            wrong(b"***************\n*** 1 ****\n  a\n--- 1 ----\n  b\n"),
            at(5, "the two sides of a hunk differ")
        );
        assert_eq!(
            wrong(b"***************\n*** 1,2 ****\n- a\n--- 1 ----\n"),
            at(4, "a hunk whose lines do not fit its header")
        );
        assert_eq!(
            wrong(b"***************\n*** x ****\n"),
            at(2, "a hunk header that is not one")
        );
        assert_eq!(
            wrong(b"***************\n*** 1 ****\n- a\n--- 0 --\n"),
            at(4, "a hunk header that is not one")
        );
    }

    #[test]
    fn two_hunks_may_start_at_one_line() {
        let found = patches(b"@@ -0,0 +1 @@\n+x\n@@ -1 +2 @@\n-a\n+b\n", None).unwrap();
        let Hunks::Edits(edits) = &found[0].hunks else {
            panic!("edits");
        };
        assert_eq!(
            edits.iter().map(|e| (e.old, e.new)).collect::<Vec<_>>(),
            [(0, 0), (0, 1)]
        );
    }

    #[test]
    fn a_hunk_undone_and_its_sides() {
        let line = |text: &'static [u8]| Line {
            text,
            unended: false,
        };
        let edit = Edit {
            old: 2,
            new: 3,
            lines: vec![
                (Mark::Same, line(b"s")),
                (Mark::Gone, line(b"g")),
                (Mark::Put, line(b"p")),
            ],
        };
        let undone = edit.reversed();
        assert_eq!((undone.old, undone.new), (3, 2));
        assert_eq!(
            undone.lines.iter().map(|(m, _)| *m).collect::<Vec<_>>(),
            [Mark::Same, Mark::Put, Mark::Gone]
        );
        assert_eq!(undone.reversed(), edit);
        let texts = |put| edit.side(put).map(|l| l.text).collect::<Vec<_>>();
        assert_eq!(texts(false), [&b"s"[..], b"g"]);
        assert_eq!(texts(true), [&b"s"[..], b"p"]);
    }

    #[test]
    fn ranges_and_command_lines() {
        assert_eq!(range(b"12"), Some((12, 12)));
        assert_eq!(range(b"3,40"), Some((3, 40)));
        assert_eq!(range(b""), None);
        assert_eq!(range(b"3,"), None);
        assert_eq!(range(b"+3"), None);
        assert_eq!(range(b"99999999999999999999999"), None);
        assert!(
            command(b"2a3,4")
                .is_some_and(|h| h.lines == (2, 2) && h.letter == b'a' && h.new == Some((3, 4)))
        );
        assert!(
            command(b"2,3d")
                .is_some_and(|h| h.lines == (2, 3) && h.letter == b'd' && h.new.is_none())
        );
        assert!(command(b"a3").is_none());
        assert!(command(b"2x3").is_none());
        assert!(command(b"2c3x").is_none());
        assert_eq!(starts(b"***************", None), Some(Kind::Context));
        assert_eq!(
            starts(b"**************", None),
            None,
            "fourteen stars are no row"
        );
        assert_eq!(starts(b"*** 1,2 ****", None), None);
        assert_eq!(
            starts(b"@@ -1 +1 @@", Some(Kind::Unified)),
            Some(Kind::Unified)
        );
        assert_eq!(name_in(b"a b\t2026-10-28"), b"a b");
        assert_eq!(name_in(b""), b"");
    }
}
