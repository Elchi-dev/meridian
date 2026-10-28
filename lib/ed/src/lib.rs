//! POSIX's ed: a buffer of lines edited by commands read line by line,
//! each with the addresses before it, as the editor ed has them. What ed
//! reads and writes besides its buffer, its input, its output, files and
//! commands, it reaches through a [`World`].

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use regex::{Regex, Syntax, Text};

/// What ed reads from and writes to besides its buffer.
pub trait World {
    /// The next line of the input, without its line feed; none at its end.
    fn line(&mut self) -> Option<Vec<u8>>;

    /// Writes `bytes` to the standard output.
    fn write(&mut self, bytes: &[u8]);

    /// What the file at `path` holds.
    ///
    /// # Errors
    ///
    /// What went wrong, in words, when it cannot be read.
    fn read(&mut self, path: &[u8]) -> Result<Vec<u8>, String>;

    /// Writes `bytes` into the file at `path`, made anew, or after what it
    /// holds when `append`.
    ///
    /// # Errors
    ///
    /// What went wrong, in words, when it cannot be written.
    fn write_file(&mut self, path: &[u8], bytes: &[u8], append: bool) -> Result<(), String>;

    /// Runs `command` with `input` as its standard input, or none, and
    /// gives what it writes on its standard output.
    ///
    /// # Errors
    ///
    /// What went wrong, in words, when it cannot be run.
    fn run(&mut self, command: &[u8], input: &[u8]) -> Result<Vec<u8>, String>;

    /// Runs `command` on ed's own input and output, as `!` does.
    fn shell(&mut self, command: &[u8]);

    /// Whether the input is a person at a console, who may go on after a
    /// mistake, rather than a script, which stops at one.
    fn interactive(&mut self) -> bool;
}

/// A line of the buffer, and a number of its own, which marks keep.
#[derive(Clone, Debug)]
struct Line {
    id: u64,
    text: Vec<u8>,
}

/// What a command may change, and `u` puts back.
#[derive(Clone, Debug)]
struct State {
    lines: Vec<Line>,
    /// The current line, 0 when the buffer is empty.
    current: usize,
    /// The line each mark is on, by its number.
    marks: [Option<u64>; 26],
    modified: bool,
}

/// Why a command could not be done: what ed says after `?` in help mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault(pub &'static str);

/// How a command ended.
enum Flow {
    Go,
    Quit,
}

/// The bytes of a command line, read on from `at`.
struct Cursor {
    bytes: Vec<u8>,
    at: usize,
}

impl Cursor {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek();
        self.at += usize::from(byte.is_some());
        byte
    }

    fn blanks(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.at += 1;
        }
    }

    fn number(&mut self) -> Option<usize> {
        let from = self.at;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.at += 1;
        }
        let digits = core::str::from_utf8(&self.bytes[from..self.at]).ok()?;
        if digits.is_empty() {
            return None;
        }
        Some(digits.parse().unwrap_or(usize::MAX))
    }

    /// The rest of the line, blanks before it taken off.
    fn rest(&mut self) -> Vec<u8> {
        self.blanks();
        let rest = self.bytes[self.at..].to_vec();
        self.at = self.bytes.len();
        rest
    }

    /// The text up to the next `delimiter` not escaped, which an escape
    /// keeps out of it, and whether the delimiter came: the end of the
    /// line may stand for the last one.
    fn delimited(&mut self, delimiter: u8) -> (Vec<u8>, bool) {
        let mut text = Vec::new();
        while let Some(byte) = self.next() {
            if byte == delimiter {
                return (text, true);
            }
            if byte == b'\\' {
                match self.next() {
                    Some(next) if next == delimiter => text.push(next),
                    Some(next) => text.extend_from_slice(&[b'\\', next]),
                    None => text.push(b'\\'),
                }
                continue;
            }
            text.push(byte);
        }
        (text, false)
    }
}

/// The addresses a command was given: none, one or two, as lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Addressed {
    first: Option<usize>,
    second: Option<usize>,
}

impl Addressed {
    fn count(self) -> usize {
        usize::from(self.first.is_some()) + usize::from(self.second.is_some())
    }

    /// The range the command works on: the addresses given, or else the
    /// default range from `first` to `second`.
    fn range(self, first: usize, second: usize) -> (usize, usize) {
        match (self.first, self.second) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, a),
            _ => (first, second),
        }
    }

    /// The one line the command works on: the last address given, or
    /// else `default`.
    fn line(self, default: usize) -> usize {
        self.second.or(self.first).unwrap_or(default)
    }
}

/// How ed talks.
#[derive(Clone, Copy, Debug)]
struct Modes {
    /// Whether a mistake is said in words after its `?`.
    help: bool,
    prompting: bool,
    /// Whether byte counts and `!` go unwritten, as `-s` asks.
    quiet: bool,
}

/// A run of ed.
pub struct Ed {
    state: State,
    undo: Option<State>,
    next_id: u64,
    /// The file the buffer is named for, empty for none.
    file: Vec<u8>,
    regex: Option<Regex>,
    replacement: Option<Vec<u8>>,
    shell: Option<Vec<u8>>,
    error: Option<Fault>,
    prompt: Vec<u8>,
    modes: Modes,
    /// Whether `q` or `e` already warned of a buffer changed and not
    /// written, so that a second one does what it says.
    warned: bool,
    failed: bool,
    /// Lines a global command's list still holds, for the text of `a`,
    /// `i` and `c` in it; none outside a list.
    pending: Option<VecDeque<Vec<u8>>>,
    /// Whether a global command runs.
    global: bool,
    /// The command `&` repeats in `G` and `V`.
    interactive_command: Option<Vec<u8>>,
}

/// How many characters `l` writes on a line before it folds.
const FOLD: usize = 72;

impl Ed {
    /// A run of ed: quiet as `-s` asks, prompting with `prompt` when it
    /// is one `-p` gave.
    #[must_use]
    pub fn new(quiet: bool, prompt: Option<&[u8]>) -> Ed {
        Ed {
            state: State {
                lines: Vec::new(),
                current: 0,
                marks: [None; 26],
                modified: false,
            },
            undo: None,
            next_id: 0,
            file: Vec::new(),
            regex: None,
            replacement: None,
            shell: None,
            error: None,
            prompt: Vec::from(prompt.unwrap_or(b"*")),
            modes: Modes {
                help: false,
                prompting: prompt.is_some(),
                quiet,
            },
            warned: false,
            failed: false,
            pending: None,
            global: false,
            interactive_command: None,
        }
    }

    /// Edits the file at `path` first, as ed does with the file it is
    /// given, and then runs the commands of the input: ed's status, 0
    /// when nothing went wrong.
    pub fn run(&mut self, path: Option<&[u8]>, world: &mut impl World) -> i32 {
        if let Some(path) = path {
            // A file that cannot be read is said, and its name kept for
            // the buffer; a command's is not, so that w gives it nothing.
            if !path.starts_with(b"!") {
                self.file = path.to_vec();
            }
            if let Ok(bytes) = self.read_from(path, world) {
                let lines: Vec<Line> = Self::split(&bytes)
                    .into_iter()
                    .map(|t| self.new_line(t))
                    .collect();
                self.state.current = lines.len();
                self.state.lines = lines;
                self.count(bytes.len(), world);
            }
        }
        loop {
            if self.modes.prompting {
                world.write(&self.prompt);
            }
            let Some(line) = world.line() else {
                break;
            };
            match self.command(line, world) {
                Ok(Flow::Go) => {}
                Ok(Flow::Quit) => break,
                Err(fault) => {
                    self.fail(fault, world);
                    // A script stops at its first mistake.
                    if !world.interactive() {
                        break;
                    }
                }
            }
        }
        i32::from(self.failed)
    }

    fn fail(&mut self, fault: Fault, world: &mut impl World) {
        self.error = Some(fault);
        self.failed = true;
        world.write(b"?\n");
        if self.modes.help {
            world.write(format!("{}\n", fault.0).as_bytes());
        }
    }

    fn last(&self) -> usize {
        self.state.lines.len()
    }

    fn new_line(&mut self, text: Vec<u8>) -> Line {
        self.next_id += 1;
        Line {
            id: self.next_id,
            text,
        }
    }

    /// Keeps the state as it is for `u`, before a command changes it.
    fn keep(&mut self) {
        // A global command is undone whole.
        if !self.global {
            self.undo = Some(self.state.clone());
        }
    }

    /// Makes the regular expression `pattern` the last one, unless it is
    /// empty, which stands for the last one.
    fn set_regex(&mut self, pattern: &[u8]) -> Result<(), Fault> {
        if !pattern.is_empty() {
            let text = String::from_utf8_lossy(pattern);
            let regex =
                Regex::new(&text, Syntax::Basic, false).map_err(|e| Fault(e.code.says()))?;
            self.regex = Some(regex);
        }
        if self.regex.is_none() {
            return Err(Fault("no regular expression used before"));
        }
        Ok(())
    }

    fn matches(regex: &Regex, text: &[u8]) -> Result<bool, Fault> {
        regex
            .find(&Text::new(text), 0)
            .map(|found| found.is_some())
            .map_err(|e| Fault(e.says()))
    }

    /// The line the search from `cursor` finds: the next that matches
    /// after the current line, going round past the end, or before it
    /// going back when `back`.
    fn search(&mut self, cursor: &mut Cursor, back: bool) -> Result<usize, Fault> {
        let delimiter = if back { b'?' } else { b'/' };
        let (pattern, _) = cursor.delimited(delimiter);
        self.set_regex(&pattern)?;
        let Some(regex) = &self.regex else {
            return Err(Fault("no regular expression used before"));
        };
        let last = self.last();
        let current = self.state.current;
        for step in 1..=last {
            let at = if back {
                (current + last - step % last - 1) % last + 1
            } else {
                (current + step - 1) % last + 1
            };
            if Self::matches(regex, &self.state.lines[at - 1].text)? {
                return Ok(at);
            }
        }
        Err(Fault("no line matches"))
    }

    /// The line a mark is on.
    fn mark(&self, letter: Option<u8>) -> Result<usize, Fault> {
        let index = letter
            .filter(u8::is_ascii_lowercase)
            .ok_or(Fault("a mark is a small letter"))?;
        let id =
            self.state.marks[usize::from(index - b'a')].ok_or(Fault("no line has that mark"))?;
        self.state
            .lines
            .iter()
            .position(|line| line.id == id)
            .map(|at| at + 1)
            .ok_or(Fault("no line has that mark"))
    }

    /// One address at the cursor, with the offsets after it; none when
    /// none is there.
    fn address(&mut self, cursor: &mut Cursor) -> Result<Option<usize>, Fault> {
        cursor.blanks();
        let base = match cursor.peek() {
            Some(b'.') => {
                cursor.at += 1;
                self.state.current
            }
            Some(b'$') => {
                cursor.at += 1;
                self.last()
            }
            Some(b'0'..=b'9') => cursor.number().unwrap_or(usize::MAX),
            Some(b'\'') => {
                cursor.at += 1;
                let letter = cursor.next();
                self.mark(letter)?
            }
            Some(b'/') => {
                cursor.at += 1;
                self.search(cursor, false)?
            }
            Some(b'?') => {
                cursor.at += 1;
                self.search(cursor, true)?
            }
            // An offset alone goes from the current line.
            Some(b'+' | b'-' | b'^') => self.state.current,
            _ => return Ok(None),
        };
        let mut at = i64::try_from(base).unwrap_or(i64::MAX);
        loop {
            cursor.blanks();
            let sign = match cursor.peek() {
                Some(b'+') => 1,
                Some(b'-' | b'^') => -1,
                // A number after an address goes on from it.
                Some(b'0'..=b'9') => {
                    let n = cursor.number().unwrap_or(usize::MAX);
                    at = at.saturating_add(i64::try_from(n).unwrap_or(i64::MAX));
                    continue;
                }
                _ => break,
            };
            cursor.at += 1;
            let n = cursor.number().unwrap_or(1);
            at = at.saturating_add(sign * i64::try_from(n).unwrap_or(i64::MAX));
        }
        usize::try_from(at)
            .ok()
            .filter(|&at| at <= self.last())
            .map(Some)
            .ok_or(Fault("an address past the buffer"))
    }

    /// The addresses at the cursor, parted by `,` and `;`, of which the
    /// last two count.
    fn addresses(&mut self, cursor: &mut Cursor) -> Result<Addressed, Fault> {
        let mut given: Vec<usize> = Vec::new();
        let mut first = self.address(cursor)?;
        loop {
            cursor.blanks();
            let Some(separator @ (b',' | b';')) = cursor.peek() else {
                break;
            };
            cursor.at += 1;
            let left = first.unwrap_or(if separator == b',' {
                1.min(self.last())
            } else {
                self.state.current
            });
            if separator == b';' {
                self.state.current = left;
            }
            let right = self.address(cursor)?;
            given.push(left);
            first = Some(match right {
                Some(right) => right,
                // , and ; alone stand for the whole buffer or from the
                // current line on; after an address, for that one again.
                None if given.len() == 1 && first.is_none() => self.last(),
                None => left,
            });
        }
        if let Some(address) = first {
            given.push(address);
        }
        Ok(match given.as_slice() {
            [] => Addressed {
                first: None,
                second: None,
            },
            [one] => Addressed {
                first: Some(*one),
                second: None,
            },
            [.., a, b] => Addressed {
                first: Some(*a),
                second: Some(*b),
            },
        })
    }

    /// The command `letter` takes no address.
    fn no_address(addressed: Addressed) -> Result<(), Fault> {
        if addressed.count() > 0 {
            return Err(Fault("no address goes with this command"));
        }
        Ok(())
    }

    /// The range `addressed` names, or the default, as real lines.
    fn lines(
        &self,
        addressed: Addressed,
        first: usize,
        second: usize,
    ) -> Result<(usize, usize), Fault> {
        let (a, b) = addressed.range(first, second);
        if a == 0 || a > b || b > self.last() {
            return Err(Fault("an address past the buffer"));
        }
        Ok((a, b))
    }

    /// Nothing but blanks is left of the command.
    fn ended(cursor: &mut Cursor) -> Result<(), Fault> {
        cursor.blanks();
        if cursor.peek().is_some() {
            return Err(Fault("something after the command"));
        }
        Ok(())
    }

    /// The `l`, `n` and `p` after a command, which print the current line
    /// once it is done, and nothing else after them.
    fn suffix(cursor: &mut Cursor) -> Result<Option<u8>, Fault> {
        cursor.blanks();
        let mut shown = None;
        while let Some(byte @ (b'l' | b'n' | b'p')) = cursor.peek() {
            cursor.at += 1;
            shown = Some(match (shown, byte) {
                (Some(b'l'), _) | (_, b'l') => b'l',
                (Some(b'n'), _) | (_, b'n') => b'n',
                _ => b'p',
            });
        }
        Self::ended(cursor)?;
        Ok(shown)
    }

    /// Prints the lines from `first` to `second` as `how`, `l`, `n` or `p`,
    /// says.
    fn print(&mut self, first: usize, second: usize, how: u8, world: &mut impl World) {
        for at in first..=second {
            let text = &self.state.lines[at - 1].text;
            match how {
                b'l' => world.write(&listed(text)),
                b'n' => {
                    world.write(format!("{at}\t").as_bytes());
                    world.write(text);
                    world.write(b"\n");
                }
                _ => {
                    world.write(text);
                    world.write(b"\n");
                }
            }
        }
        self.state.current = second;
    }

    fn show(&mut self, shown: Option<u8>, world: &mut impl World) {
        if let Some(how) = shown
            && self.state.current > 0
        {
            let current = self.state.current;
            self.print(current, current, how, world);
        }
    }

    /// The lines of text `a`, `i` and `c` take, up to a line of `.` alone:
    /// from a global command's list when one runs, else from the input.
    fn text(&mut self, world: &mut impl World) -> Vec<Vec<u8>> {
        let mut text = Vec::new();
        loop {
            let line = match &mut self.pending {
                Some(pending) => pending.pop_front(),
                None => world.line(),
            };
            match line {
                Some(line) if line != b"." => text.push(line),
                _ => return text,
            }
        }
    }

    /// Puts `text` in after the line `after`, making the last of them the
    /// current line.
    fn insert(&mut self, after: usize, text: Vec<Vec<u8>>) {
        let count = text.len();
        let lines: Vec<Line> = text.into_iter().map(|t| self.new_line(t)).collect();
        self.state.lines.splice(after..after, lines);
        if count > 0 {
            self.state.current = after + count;
            self.state.modified = true;
        }
    }

    /// Takes out the lines from `first` to `second`, with their marks.
    fn delete(&mut self, first: usize, second: usize) {
        let gone: Vec<Line> = self.state.lines.drain(first - 1..second).collect();
        for mark in &mut self.state.marks {
            if mark.is_some_and(|id| gone.iter().any(|line| line.id == id)) {
                *mark = None;
            }
        }
        self.state.modified = true;
        self.state.current = first.min(self.last());
    }

    /// The buffer's lines as a file holds them, each with its line feed.
    fn bytes(&self, first: usize, second: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for line in &self.state.lines[first - 1..second] {
            bytes.extend_from_slice(&line.text);
            bytes.push(b'\n');
        }
        bytes
    }

    /// The lines of a file's bytes; the last needs no line feed.
    fn split(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut lines: Vec<Vec<u8>> = bytes.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
        if bytes.is_empty() || bytes.ends_with(b"\n") {
            lines.pop();
        }
        lines
    }

    /// The file a command names, or the one the buffer is named for; the
    /// first named becomes the buffer's name when it has none.
    fn named(&mut self, given: Vec<u8>) -> Result<Vec<u8>, Fault> {
        if given.is_empty() {
            if self.file.is_empty() {
                return Err(Fault("no file named"));
            }
            return Ok(self.file.clone());
        }
        if self.file.is_empty() && !given.starts_with(b"!") {
            self.file.clone_from(&given);
        }
        Ok(given)
    }

    /// What the file `path`, or with `!` the command, gives.
    fn read_from(&mut self, path: &[u8], world: &mut impl World) -> Result<Vec<u8>, Fault> {
        let read = match path.strip_prefix(b"!") {
            Some(command) => {
                let command = self.shell_command(command)?;
                world.run(&command, b"")
            }
            None => world.read(path),
        };
        read.map_err(|why| {
            world.write(format!("{}: {why}\n", String::from_utf8_lossy(path)).as_bytes());
            Fault("the file could not be read")
        })
    }

    fn count(&self, bytes: usize, world: &mut impl World) {
        if !self.modes.quiet {
            world.write(format!("{bytes}\n").as_bytes());
        }
    }

    /// `command` with `%` the buffer's file and `!` alone the command run
    /// last, kept as the last.
    fn shell_command(&mut self, command: &[u8]) -> Result<Vec<u8>, Fault> {
        let mut made = Vec::new();
        let mut at = 0;
        if command.first() == Some(&b'!') {
            made.extend_from_slice(self.shell.as_ref().ok_or(Fault("no command run before"))?);
            at = 1;
        }
        while at < command.len() {
            match (command[at], command.get(at + 1)) {
                (b'\\', Some(&next @ b'%')) => {
                    made.push(next);
                    at += 2;
                }
                (b'%', _) => {
                    if self.file.is_empty() {
                        return Err(Fault("no file named"));
                    }
                    made.extend_from_slice(&self.file);
                    at += 1;
                }
                (byte, _) => {
                    made.push(byte);
                    at += 1;
                }
            }
        }
        self.shell = Some(made.clone());
        Ok(made)
    }

    /// Does the command `line` says, with what follows it in the input
    /// when it takes text.
    fn command(&mut self, line: Vec<u8>, world: &mut impl World) -> Result<Flow, Fault> {
        let mut cursor = Cursor { bytes: line, at: 0 };
        let addressed = self.addresses(&mut cursor)?;
        cursor.blanks();
        let letter = cursor.next();
        let warned = core::mem::replace(&mut self.warned, false);
        match letter {
            None => {
                let at = addressed.line(self.state.current + 1);
                if at == 0 || at > self.last() {
                    return Err(Fault("an address past the buffer"));
                }
                self.print(at, at, b'p', world);
            }
            Some(letter @ (b'a' | b'i' | b'c' | b'd' | b'j' | b'k' | b'm' | b't')) => {
                self.change(letter, addressed, &mut cursor, world)?;
            }
            Some(letter @ (b'e' | b'E' | b'f' | b'r' | b'w' | b'W')) => {
                self.file_command(letter, addressed, &mut cursor, warned, world)?;
            }
            Some(global @ (b'g' | b'v' | b'G' | b'V')) => {
                let (first, second) = self.lines(addressed, 1, self.last())?;
                return self.global(global, first, second, &mut cursor, world);
            }
            Some(b's') => self.substitute(addressed, &mut cursor, world)?,
            Some(letter) => return self.other(letter, addressed, &mut cursor, warned, world),
        }
        Ok(Flow::Go)
    }

    /// The commands that change the lines: `a`, `i`, `c`, `d`, `j`, `k`,
    /// `m` and `t`.
    fn change(
        &mut self,
        letter: u8,
        addressed: Addressed,
        cursor: &mut Cursor,
        world: &mut impl World,
    ) -> Result<(), Fault> {
        let current = self.state.current;
        match letter {
            b'a' | b'i' => {
                let at = addressed.line(current);
                let shown = Self::suffix(cursor)?;
                let text = self.text(world);
                self.keep();
                let after = if letter == b'i' {
                    at.saturating_sub(1)
                } else {
                    at
                };
                if text.is_empty() {
                    self.state.current = at;
                }
                self.insert(after, text);
                self.show(shown, world);
            }
            b'c' | b'd' => {
                let (first, second) = self.lines(addressed, current, current)?;
                let shown = Self::suffix(cursor)?;
                let text = if letter == b'c' {
                    self.text(world)
                } else {
                    Vec::new()
                };
                self.keep();
                self.delete(first, second);
                self.insert(first - 1, text);
                self.show(shown, world);
            }
            b'j' => {
                let (first, second) = match addressed.count() {
                    0 => (current, current + 1),
                    _ => addressed.range(current, current),
                };
                if first == 0 || first > second || second > self.last() {
                    return Err(Fault("an address past the buffer"));
                }
                let shown = Self::suffix(cursor)?;
                if second > first {
                    self.keep();
                    let joined: Vec<u8> = self.state.lines[first - 1..second]
                        .iter()
                        .flat_map(|line| line.text.iter().copied())
                        .collect();
                    self.delete(first, second);
                    self.insert(first - 1, alloc::vec![joined]);
                } else {
                    // One line joins with nothing, and is the line joined.
                    self.state.current = first;
                }
                self.show(shown, world);
            }
            b'k' => {
                let at = addressed.line(current);
                if at == 0 || at > self.last() {
                    return Err(Fault("an address past the buffer"));
                }
                let letter = cursor
                    .next()
                    .filter(u8::is_ascii_lowercase)
                    .ok_or(Fault("a mark is a small letter"))?;
                let shown = Self::suffix(cursor)?;
                self.state.marks[usize::from(letter - b'a')] = Some(self.state.lines[at - 1].id);
                self.show(shown, world);
            }
            _ => {
                let (first, second) = self.lines(addressed, current, current)?;
                let destination = self.address(cursor)?.ok_or(Fault("no destination"))?;
                let shown = Self::suffix(cursor)?;
                if letter == b'm' && destination >= first && destination < second {
                    return Err(Fault("a destination within the lines moved"));
                }
                self.keep();
                if letter == b'm' {
                    let moved: Vec<Line> = self.state.lines.drain(first - 1..second).collect();
                    let after = if destination >= second {
                        destination - moved.len()
                    } else {
                        destination
                    };
                    let count = moved.len();
                    self.state.lines.splice(after..after, moved);
                    self.state.current = after + count;
                    self.state.modified = true;
                } else {
                    let texts: Vec<Vec<u8>> = self.state.lines[first - 1..second]
                        .iter()
                        .map(|l| l.text.clone())
                        .collect();
                    self.insert(destination, texts);
                }
                self.show(shown, world);
            }
        }
        Ok(())
    }

    /// The commands of files: `e`, `E`, `f`, `r`, `w` and `W`.
    fn file_command(
        &mut self,
        letter: u8,
        addressed: Addressed,
        cursor: &mut Cursor,
        warned: bool,
        world: &mut impl World,
    ) -> Result<(), Fault> {
        let last = self.last();
        match letter {
            b'e' | b'E' => {
                Self::no_address(addressed)?;
                let path = cursor.rest();
                if letter == b'e' && self.state.modified && !warned {
                    self.warned = true;
                    return Err(Fault("the buffer is changed and not written"));
                }
                let path = if path.is_empty() {
                    self.named(path)?
                } else {
                    if !path.starts_with(b"!") {
                        self.file.clone_from(&path);
                    }
                    path
                };
                // A file that cannot be read leaves the buffer empty and
                // named for it, as ed starts with a file not there.
                let (bytes, read) = match self.read_from(&path, world) {
                    Ok(bytes) => (bytes, Ok(())),
                    Err(fault) => (Vec::new(), Err(fault)),
                };
                let lines: Vec<Line> = Self::split(&bytes)
                    .into_iter()
                    .map(|t| self.new_line(t))
                    .collect();
                self.state = State {
                    current: lines.len(),
                    lines,
                    marks: [None; 26],
                    modified: false,
                };
                self.undo = None;
                read?;
                self.count(bytes.len(), world);
            }
            b'f' => {
                Self::no_address(addressed)?;
                let path = cursor.rest();
                if !path.is_empty() {
                    self.file = path;
                }
                if self.file.is_empty() {
                    return Err(Fault("no file named"));
                }
                world.write(&self.file);
                world.write(b"\n");
            }
            b'r' => {
                let at = addressed.line(last);
                let path = cursor.rest();
                let path = self.named(path)?;
                let bytes = self.read_from(&path, world)?;
                self.keep();
                // With no line read the line addressed is current.
                self.state.current = at;
                self.insert(at, Self::split(&bytes));
                self.count(bytes.len(), world);
            }
            _ => {
                let (first, second) = if last == 0 && addressed.count() == 0 {
                    (1, 0)
                } else {
                    self.lines(addressed, 1, last)?
                };
                let path = cursor.rest();
                let path = self.named(path)?;
                let bytes = if second >= first {
                    self.bytes(first, second)
                } else {
                    Vec::new()
                };
                let written = match path.strip_prefix(b"!") {
                    Some(command) => {
                        let command = self.shell_command(command)?;
                        world.run(&command, &bytes).map(|out| world.write(&out))
                    }
                    None => world.write_file(&path, &bytes, letter == b'W'),
                };
                written.map_err(|why| {
                    world.write(format!("{}: {why}\n", String::from_utf8_lossy(&path)).as_bytes());
                    Fault("the file could not be written")
                })?;
                if first == 1 && second == last && !path.starts_with(b"!") {
                    self.state.modified = false;
                }
                self.count(bytes.len(), world);
            }
        }
        Ok(())
    }

    /// The rest: printing, `=`, `h`, `H`, `P`, `q`, `Q`, `u` and `!`.
    fn other(
        &mut self,
        letter: u8,
        addressed: Addressed,
        cursor: &mut Cursor,
        warned: bool,
        world: &mut impl World,
    ) -> Result<Flow, Fault> {
        let current = self.state.current;
        match letter {
            b'l' | b'n' | b'p' => {
                let (first, second) = self.lines(addressed, current, current)?;
                let shown = Self::suffix(cursor)?;
                let how = match (letter, shown) {
                    (_, Some(b'l')) | (b'l', _) => b'l',
                    (_, Some(b'n')) | (b'n', _) => b'n',
                    _ => b'p',
                };
                self.print(first, second, how, world);
            }
            b'=' => {
                let at = addressed.line(self.last());
                let shown = Self::suffix(cursor)?;
                world.write(format!("{at}\n").as_bytes());
                self.show(shown, world);
            }
            b'h' | b'H' | b'P' => {
                Self::no_address(addressed)?;
                let shown = Self::suffix(cursor)?;
                match letter {
                    b'P' => self.modes.prompting = !self.modes.prompting,
                    b'H' => self.modes.help = !self.modes.help,
                    _ => {}
                }
                if (letter == b'h' || (letter == b'H' && self.modes.help))
                    && let Some(Fault(why)) = self.error
                {
                    world.write(format!("{why}\n").as_bytes());
                }
                self.show(shown, world);
            }
            b'q' | b'Q' => {
                Self::no_address(addressed)?;
                Self::ended(cursor)?;
                if letter == b'q' && self.state.modified && !warned {
                    self.warned = true;
                    return Err(Fault("the buffer is changed and not written"));
                }
                return Ok(Flow::Quit);
            }
            b'u' => {
                Self::no_address(addressed)?;
                let shown = Self::suffix(cursor)?;
                let before = self.undo.take().ok_or(Fault("nothing to undo"))?;
                self.undo = Some(core::mem::replace(&mut self.state, before));
                // A w between the change and its undoing leaves the file
                // as the change made it, so the buffer undone is changed.
                self.state.modified = true;
                self.show(shown, world);
            }
            b'!' => {
                Self::no_address(addressed)?;
                let command = cursor.bytes[cursor.at..].to_vec();
                let command = self.shell_command(&command)?;
                world.shell(&command);
                if !self.modes.quiet {
                    world.write(b"!\n");
                }
            }
            _ => return Err(Fault("no such command")),
        }
        Ok(Flow::Go)
    }

    /// `g`, `v`, `G` and `V`: the command list after the expression run
    /// on each line it matches, or does not, from `first` to `second`.
    fn global(
        &mut self,
        kind: u8,
        first: usize,
        second: usize,
        cursor: &mut Cursor,
        world: &mut impl World,
    ) -> Result<Flow, Fault> {
        if self.global {
            return Err(Fault("a global command inside another"));
        }
        let delimiter = cursor
            .next()
            .filter(|&b| b != b' ')
            .ok_or(Fault("no delimiter"))?;
        let (pattern, _) = cursor.delimited(delimiter);
        self.set_regex(&pattern)?;
        let Some(regex) = &self.regex else {
            return Err(Fault("no regular expression used before"));
        };
        let wanted = matches!(kind, b'g' | b'G');
        let mut marked = Vec::new();
        for at in first..=second {
            let line = &self.state.lines[at - 1];
            if Self::matches(regex, &line.text)? == wanted {
                marked.push(line.id);
            }
        }
        // The list goes on over lines that end with a backslash.
        let mut list = cursor.bytes[cursor.at..].to_vec();
        while continued(&list) {
            list.pop();
            list.push(b'\n');
            let Some(more) = world.line() else {
                list.pop();
                break;
            };
            list.extend_from_slice(&more);
        }
        self.keep();
        self.global = true;
        let ran = self.each_marked(kind, &marked, &list, world);
        self.global = false;
        self.pending = None;
        ran
    }

    fn each_marked(
        &mut self,
        kind: u8,
        marked: &[u64],
        list: &[u8],
        world: &mut impl World,
    ) -> Result<Flow, Fault> {
        let interactive = matches!(kind, b'G' | b'V');
        for id in marked {
            let Some(at) = self.state.lines.iter().position(|line| line.id == *id) else {
                continue;
            };
            self.state.current = at + 1;
            let commands: Vec<Vec<u8>> = if interactive {
                self.print(at + 1, at + 1, b'p', world);
                let Some(asked) = world.line() else {
                    return Ok(Flow::Go);
                };
                match asked.as_slice() {
                    b"" => continue,
                    b"&" => alloc::vec![
                        self.interactive_command
                            .clone()
                            .ok_or(Fault("no command run before"))?
                    ],
                    _ => {
                        self.interactive_command = Some(asked.clone());
                        alloc::vec![asked]
                    }
                }
            } else if list.iter().all(|b| *b == b' ' || *b == b'\t') {
                alloc::vec![b"p".to_vec()]
            } else {
                list.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect()
            };
            let mut pending: VecDeque<Vec<u8>> = commands.into_iter().collect();
            while let Some(command) = pending.pop_front() {
                // Text for a command of a list comes from the list; for
                // one asked at the console, from the input.
                self.pending = (!interactive).then_some(pending);
                let done = self.command(command, world);
                pending = self.pending.take().unwrap_or_default();
                if let Flow::Quit = done? {
                    return Ok(Flow::Quit);
                }
            }
        }
        Ok(Flow::Go)
    }

    /// `s/re/replacement/flags` on the lines `addressed` names.
    fn substitute(
        &mut self,
        addressed: Addressed,
        cursor: &mut Cursor,
        world: &mut impl World,
    ) -> Result<(), Fault> {
        let current = self.state.current;
        let (first, second) = self.lines(addressed, current, current)?;
        let delimiter = cursor
            .next()
            .filter(|&b| b != b' ' && b != b'\\')
            .ok_or(Fault("no delimiter"))?;
        let (pattern, _) = cursor.delimited(delimiter);
        let (mut replacement, closed) = cursor.delimited(delimiter);
        // A replacement that ends with a backslash goes on to the next
        // line, the line feed part of it.
        let mut replacement_ended = closed;
        while !replacement_ended && continued(&replacement) {
            replacement.pop();
            replacement.push(b'\n');
            // No command of a global command's list gets here, as each
            // ends where a backslash carried the list on and was taken
            // off: POSIX lets no line be parted in one.
            let Some(next) = world.line() else {
                break;
            };
            let mut more = Cursor { bytes: next, at: 0 };
            let (rest, closed) = more.delimited(delimiter);
            replacement.extend_from_slice(&rest);
            replacement_ended = closed;
            if closed {
                cursor.bytes.truncate(cursor.at);
                cursor.bytes.extend_from_slice(&more.bytes[more.at..]);
            }
        }
        if replacement == b"%" {
            replacement = self
                .replacement
                .clone()
                .ok_or(Fault("no replacement used before"))?;
        }
        self.replacement = Some(replacement.clone());
        let mut all = false;
        let mut nth = 1;
        let mut shown = if replacement_ended { None } else { Some(b'p') };
        loop {
            match cursor.peek() {
                Some(b'g') => {
                    all = true;
                    cursor.at += 1;
                }
                Some(b'0'..=b'9') => nth = cursor.number().unwrap_or(1).max(1),
                Some(how @ (b'l' | b'n' | b'p')) => {
                    cursor.at += 1;
                    shown = Some(how);
                }
                _ => break,
            }
        }
        cursor.blanks();
        if cursor.peek().is_some() {
            return Err(Fault("something after the command"));
        }
        self.set_regex(&pattern)?;
        let regex = self
            .regex
            .take()
            .ok_or(Fault("no regular expression used before"))?;
        let done = self.replace_lines(&regex, first, second, &replacement, all, nth);
        self.regex = Some(regex);
        if done? {
            self.show(shown, world);
        } else if !self.global {
            // Inside a global command a line it misses is passed over.
            return Err(Fault("no line matches"));
        }
        Ok(())
    }

    /// Replaces as `s` does on the lines from `first` to `second`; whether
    /// any matched.
    fn replace_lines(
        &mut self,
        regex: &Regex,
        first: usize,
        second: usize,
        replacement: &[u8],
        all: bool,
        nth: usize,
    ) -> Result<bool, Fault> {
        let mut changed_any = false;
        let mut at = first;
        let mut end = second;
        let mut kept = false;
        while at <= end {
            let text = self.state.lines[at - 1].text.clone();
            let Some(made) = replaced(regex, &text, replacement, all, nth)? else {
                at += 1;
                continue;
            };
            if !kept {
                self.keep();
                kept = true;
            }
            let pieces: Vec<Vec<u8>> = made.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
            let count = pieces.len();
            self.state.lines[at - 1].text.clone_from(&pieces[0]);
            let more: Vec<Line> = pieces[1..]
                .iter()
                .map(|t| self.new_line(t.clone()))
                .collect();
            self.state.lines.splice(at..at, more);
            self.state.modified = true;
            self.state.current = at + count - 1;
            changed_any = true;
            at += count;
            end += count - 1;
        }
        Ok(changed_any)
    }
}

/// `text` with the `nth` match of `regex`, or each from it on when `all`,
/// replaced as `replacement` says; none when nothing was.
fn replaced(
    regex: &Regex,
    text: &[u8],
    replacement: &[u8],
    all: bool,
    nth: usize,
) -> Result<Option<Vec<u8>>, Fault> {
    let searched = Text::new(text);
    let mut made = Vec::new();
    let mut copied = 0;
    let mut at = 0;
    let mut found = 0;
    let mut changed = false;
    let mut last_end = None;
    while at <= text.len() {
        let Some(spans) = regex.captures(&searched, at).map_err(|e| Fault(e.says()))? else {
            break;
        };
        let Some(whole) = spans[0] else {
            break;
        };
        let (start, end) = (whole.start, whole.end);
        // An empty match right after the one before is none.
        if start == end && last_end == Some(start) {
            at = start + char_len(&text[start..]);
            continue;
        }
        found += 1;
        if found >= nth {
            made.extend_from_slice(&text[copied..start]);
            expand(replacement, text, &spans, &mut made);
            copied = end;
            changed = true;
            if !all {
                break;
            }
        }
        last_end = Some(end);
        at = if start == end {
            start + char_len(&text[start..])
        } else {
            end
        };
    }
    if !changed {
        return Ok(None);
    }
    made.extend_from_slice(&text[copied..]);
    Ok(Some(made))
}

/// Whether `bytes` ends with a backslash no backslash before it escapes,
/// which carries the line on to the next.
fn continued(bytes: &[u8]) -> bool {
    bytes.iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// How many bytes the character at the start of `bytes` takes.
fn char_len(bytes: &[u8]) -> usize {
    bytes.utf8_chunks().next().map_or(1, |chunk| {
        chunk.valid().chars().next().map_or(1, char::len_utf8)
    })
}

/// Puts what `replacement` makes of a match onto `made`: `&` is the
/// match, `\1` to `\9` its subexpressions, and a backslash before any
/// other character that character.
fn expand(replacement: &[u8], text: &[u8], spans: &[Option<regex::Span>], made: &mut Vec<u8>) {
    let mut at = 0;
    while at < replacement.len() {
        match (replacement[at], replacement.get(at + 1)) {
            (b'&', _) => {
                if let Some(Some(whole)) = spans.first() {
                    made.extend_from_slice(&text[whole.start..whole.end]);
                }
                at += 1;
            }
            (b'\\', Some(&digit @ b'1'..=b'9')) => {
                if let Some(Some(group)) = spans.get(usize::from(digit - b'0')) {
                    made.extend_from_slice(&text[group.start..group.end]);
                }
                at += 2;
            }
            (b'\\', Some(&next)) => {
                made.push(next);
                at += 2;
            }
            (byte, _) => {
                made.push(byte);
                at += 1;
            }
        }
    }
}

/// `text` as `l` writes it: what does not print as an escape, long lines
/// folded with a backslash, and a `$` at the end.
#[must_use]
pub fn listed(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut width = 0;
    for &byte in text {
        let shown: Vec<u8> = match byte {
            b'\\' => b"\\\\".to_vec(),
            0x07 => b"\\a".to_vec(),
            0x08 => b"\\b".to_vec(),
            0x0c => b"\\f".to_vec(),
            b'\r' => b"\\r".to_vec(),
            b'\t' => b"\\t".to_vec(),
            0x0b => b"\\v".to_vec(),
            b' '..=b'~' => alloc::vec![byte],
            _ => format!("\\{byte:03o}").into_bytes(),
        };
        if width + shown.len() > FOLD - 1 {
            out.extend_from_slice(b"\\\n");
            width = 0;
        }
        width += shown.len();
        out.extend_from_slice(&shown);
    }
    out.extend_from_slice(b"$\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in ed reaches the host's own files.
    fn confined(root: &std::path::Path, path: &[u8]) -> std::path::PathBuf {
        let path = String::from_utf8_lossy(path);
        let mut depth = 0usize;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .unwrap_or_else(|| panic!("{path} climbs out of the test's directory"));
                }
                _ => depth += 1,
            }
        }
        root.join(path.trim_start_matches('/'))
    }

    /// ed's world on the host: a script of input lines, the output it
    /// writes, files in a directory of their own, gone once dropped, and
    /// commands run by the host's sh in it.
    struct Host {
        root: std::path::PathBuf,
        input: VecDeque<Vec<u8>>,
        out: Vec<u8>,
        typed: bool,
    }

    impl Host {
        fn new(name: &str) -> Host {
            static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("meridian-ed-{name}-{}-{made}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                input: VecDeque::new(),
                out: Vec::new(),
                typed: false,
            }
        }

        fn put(&self, name: &str, text: &str) {
            std::fs::write(self.root.join(name), text).unwrap();
        }

        fn holds(&self, name: &str) -> Option<String> {
            std::fs::read_to_string(self.root.join(name)).ok()
        }

        fn sh(&self, command: &[u8]) -> std::process::Command {
            let mut sh = std::process::Command::new("sh");
            sh.arg("-c")
                .arg(String::from_utf8_lossy(command).as_ref())
                .current_dir(&self.root);
            sh
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl World for Host {
        fn line(&mut self) -> Option<Vec<u8>> {
            self.input.pop_front()
        }

        fn write(&mut self, bytes: &[u8]) {
            self.out.extend_from_slice(bytes);
        }

        fn read(&mut self, path: &[u8]) -> Result<Vec<u8>, String> {
            std::fs::read(confined(&self.root, path)).map_err(|_| String::from("no such file"))
        }

        fn write_file(&mut self, path: &[u8], bytes: &[u8], append: bool) -> Result<(), String> {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(append)
                .truncate(!append)
                .open(confined(&self.root, path))
                .map_err(|_| String::from("cannot be written"))?;
            file.write_all(bytes)
                .map_err(|_| String::from("cannot be written"))
        }

        fn run(&mut self, command: &[u8], input: &[u8]) -> Result<Vec<u8>, String> {
            use std::io::Write as _;
            let mut child = self
                .sh(command)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .map_err(|_| String::from("cannot be run"))?;
            child.stdin.take().unwrap().write_all(input).unwrap();
            Ok(child.wait_with_output().unwrap().stdout)
        }

        fn shell(&mut self, command: &[u8]) {
            let done = self.sh(command).output().unwrap();
            self.out.extend_from_slice(&done.stdout);
        }

        fn interactive(&mut self) -> bool {
            self.typed
        }
    }

    /// How ed with `file`, quiet as `quiet` says, ran the script `lines`
    /// on `host`: its status and what it wrote.
    fn edit(host: &mut Host, file: Option<&str>, quiet: bool, lines: &str) -> (i32, String) {
        host.input = lines.split('\n').map(|l| l.as_bytes().to_vec()).collect();
        if lines.ends_with('\n') {
            host.input.pop_back();
        }
        host.out.clear();
        let mut ed = Ed::new(quiet, None);
        let status = ed.run(file.map(str::as_bytes), host);
        (status, String::from_utf8_lossy(&host.out).into_owned())
    }

    /// What the script `lines` writes run on a buffer of `buffer`'s lines,
    /// quiet, with its mistakes said.
    fn ed(buffer: &str, lines: &str) -> String {
        let mut host = Host::new("plain");
        host.put("f", buffer);
        let (_, out) = edit(&mut host, Some("f"), true, &format!("H\n{lines}"));
        out
    }

    #[test]
    fn text_goes_in_after_a_and_before_i() {
        assert_eq!(ed("", "a\none\ntwo\n.\n,p"), "one\ntwo\n");
        assert_eq!(ed("b\n", "1i\na\n.\n$a\nc\n.\n,p"), "a\nb\nc\n");
        assert_eq!(
            ed("x\n", "0a\nfirst\n.\n,p"),
            "first\nx\n",
            "after line 0 is before line 1"
        );
        assert_eq!(ed("x\n", "0i\nfirst\n.\n,p"), "first\nx\n");
        assert_eq!(
            ed("a\nb\n", "1a\n.\n.p"),
            "a\n",
            "with no text the line addressed is current"
        );
        assert_eq!(ed("a\nb\n", "2i\n.\n.p"), "b\n");
        assert_eq!(
            ed("a\n", "a\nb\nc\n.\n.p"),
            "c\n",
            "the last line put in is current"
        );
    }

    #[test]
    fn addresses_are_lines_marks_searches_and_offsets() {
        let buffer = "one\ntwo\nthree\nfour\nfive\n";
        for (script, out) in [
            ("1p", "one\n"),
            ("$p", "five\n"),
            ("2;+2p", "two\nthree\nfour\n"),
            ("3\n-p", "three\ntwo\n"),
            ("3\n--p", "three\none\n"),
            ("3\n^p", "three\ntwo\n"),
            ("2\n+2p", "two\nfour\n"),
            ("2 2p", "four\n"),
            ("/th/p", "three\n"),
            ("/o/p\n//p", "one\ntwo\n"),
            ("$\n?t?p", "five\nthree\n"),
            ("1\n?f?p", "one\nfive\n"),
            ("3kx\n1\n'xp", "one\nthree\n"),
            (",p", buffer),
            ("2,p", "two\n"),
            (",2p", "one\ntwo\n"),
            ("4\n;p", "four\nfour\nfive\n"),
            ("1,2,4p", "two\nthree\nfour\n"),
            ("/one/,/three/p", "one\ntwo\nthree\n"),
            ("$-1p", "four\n"),
            ("/f/+1p", "five\n"),
        ] {
            assert_eq!(ed(buffer, script), out, "{script}");
        }
        for (script, why) in [
            ("0p", "an address past the buffer"),
            ("6p", "an address past the buffer"),
            ("3,2p", "an address past the buffer"),
            ("1\n--p", "an address past the buffer"),
            ("/nothing/p", "no line matches"),
            ("'qp", "no line has that mark"),
            ("'Qp", "a mark is a small letter"),
            ("//p", "no regular expression used before"),
            ("1,2e", "no address goes with this command"),
        ] {
            assert!(
                ed(buffer, script).ends_with(&format!("?\n{why}\n")),
                "{script}: {}",
                ed(buffer, script)
            );
        }
    }

    #[test]
    fn lines_are_changed_deleted_joined_moved_and_copied() {
        let buffer = "a\nb\nc\nd\n";
        for (script, out) in [
            ("2d\n,p", "a\nc\nd\n"),
            ("2,3d\n.p", "d\n"),
            ("$d\n.p", "c\n"),
            ("2c\nB\nBB\n.\n,p", "a\nB\nBB\nc\nd\n"),
            ("2c\n.\n.p", "c\n"),
            ("1,2j\n,p", "ab\nc\nd\n"),
            ("2\nj\n,p", "b\na\nbc\nd\n"),
            ("2j\n,p", "a\nb\nc\nd\n"),
            ("1,2m$\n,p", "c\nd\na\nb\n"),
            ("3m0\n,p", "c\na\nb\nd\n"),
            ("1m2\n.p", "a\n"),
            ("1,2t$\n,p", "a\nb\nc\nd\na\nb\n"),
            ("4t0\n.p", "d\n"),
            ("2dp", "c\n"),
            ("1,3j\np", "abc\n"),
        ] {
            assert_eq!(ed(buffer, script), out, "{script}");
        }
        assert!(ed(buffer, "1,3m2").ends_with("?\na destination within the lines moved\n"));
        assert!(ed(buffer, "2m").ends_with("?\nno destination\n"));
        assert!(ed(buffer, "2dx").ends_with("?\nsomething after the command\n"));
    }

    #[test]
    fn marks_follow_their_lines_and_go_with_them() {
        assert_eq!(ed("a\nb\nc\n", "3kx\n1d\n'xp\n'xn"), "c\n2\tc\n");
        assert!(ed("a\nb\nc\n", "2kx\n2d\n'xp").ends_with("?\nno line has that mark\n"));
        assert!(ed("a\n", "1k1").ends_with("?\na mark is a small letter\n"));
    }

    #[test]
    fn u_takes_back_the_last_change_and_its_own() {
        assert_eq!(ed("a\nb\n", "1d\nu\n,p"), "a\nb\n");
        assert_eq!(ed("a\nb\n", "1d\nu\nu\n,p"), "b\n", "u undoes u");
        assert_eq!(
            ed("a\nb\n", "g/./s/$/!/\nu\n,p"),
            "a\nb\n",
            "a global command is undone whole"
        );
        assert_eq!(ed("a\n", "s/a/b/\n.p\nu\n.p"), "b\na\n");
        assert!(ed("a\n", "u").ends_with("?\nnothing to undo\n"));
        assert_eq!(
            ed("a\nb\n", "2d\n1p\nu\n,p"),
            "a\na\nb\n",
            "printing changes nothing to undo"
        );
    }

    #[test]
    fn s_replaces_with_flags() {
        let buffer = "aaa\nbab\nccc\n";
        for (script, out) in [
            ("1s/a/x/\np", "xaa\n"),
            ("1s/a/x/g\np", "xxx\n"),
            ("1s/a/x/2\np", "axa\n"),
            ("1s/a/x/2g\np", "axx\n"),
            (",s/a/[&]/\n,p", "[a]aa\nb[a]b\nccc\n"),
            ("2s/\\(b\\)a\\(b\\)/\\2-\\1/\np", "b-b\n"),
            ("1s/a/\\&/\np", "&aa\n"),
            ("1s/a/x/\n2s/a/%/\n2p", "bxb\n"),
            ("1s/a/x", "xaa\n"),
            ("1s/a/x/p", "xaa\n"),
            ("1s/a/x/n", "1\txaa\n"),
            ("1s/a/\\t/l", "taa$\n"),
            ("1s|a|/|\np", "/aa\n"),
            ("1s/a\\/*/x/\np", "xaa\n"),
            ("2s/a/\\\nZ/\n,p", "aaa\nb\nZb\nccc\n"),
            ("2s/a/\\\n/\n.=", "3\n"),
            (",s/c/C/g\n.n", "3\tCCC\n"),
            ("s/c/C/\np", "Ccc\n"),
            ("1s/x*/-/g\np", "-a-a-a-\n"),
        ] {
            assert_eq!(ed(buffer, script), out, "{script}");
        }
        assert!(ed(buffer, "1s/z/x/").ends_with("?\nno line matches\n"));
        assert!(ed(buffer, "1s/a/%/").ends_with("?\nno replacement used before\n"));
        assert!(ed(buffer, "1s").ends_with("?\nno delimiter\n"));
        assert!(ed(buffer, "1s/a/b/q").ends_with("?\nsomething after the command\n"));
        assert!(ed(buffer, "1s/\\(/b/").ends_with("?\na parenthesis without its other half\n"));
    }

    #[test]
    fn global_commands_run_their_list_on_each_line_marked() {
        let buffer = "a1\nb2\na3\nb4\n";
        for (script, out) in [
            ("g/a/p", "a1\na3\n"),
            ("v/a/p", "b2\nb4\n"),
            ("g/a/", "a1\na3\n"),
            ("g/a/d\n,p", "b2\nb4\n"),
            ("g/b/s/[0-9]/#/\n,p", "a1\nb#\na3\nb#\n"),
            ("g/a/m0\n,p", "a3\na1\nb2\nb4\n"),
            ("g/1/a\\\nnew\n,p", "a1\nnew\nb2\na3\nb4\n"),
            ("g/[0-9]/s/./X/\\\np", "X1\nX2\nX3\nX4\n"),
            ("2,3g/./n", "2\tb2\n3\ta3\n"),
            ("g/a/.=", "1\n3\n"),
        ] {
            assert_eq!(ed(buffer, script), out, "{script}");
        }
        assert!(ed(buffer, "g/a/g/1/p").ends_with("?\na global command inside another\n"));
        assert!(ed(buffer, "g").ends_with("?\nno delimiter\n"));
    }

    #[test]
    fn interactive_globals_ask_for_each_line() {
        assert_eq!(
            ed("a1\nb2\na3\n", "G/a/\ns/a/x/\n&\n,p"),
            "a1\na3\nx1\nb2\nx3\n"
        );
        assert_eq!(
            ed("a1\nb2\na3\n", "G/a/\n\nd\n,p"),
            "a1\na3\na1\nb2\n",
            "an empty line leaves the line"
        );
        assert_eq!(ed("a1\nb2\n", "V/a/\ns/$/!/\n,p"), "b2\na1\nb2!\n");
        assert!(ed("a1\n", "G/a/\n&").ends_with("?\nno command run before\n"));
    }

    #[test]
    fn l_and_n_write_lines_as_they_are_and_numbered() {
        assert_eq!(listed(b"a\tb\\c"), b"a\\tb\\\\c$\n");
        assert_eq!(
            listed(b"\x07\x08\x0c\r\x0b\x01\xff"),
            b"\\a\\b\\f\\r\\v\\001\\377$\n"
        );
        assert_eq!(listed(b""), b"$\n");
        let long = [b'x'; 100];
        let shown = listed(&long);
        let shown = String::from_utf8_lossy(&shown);
        let lines: Vec<&str> = shown.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], format!("{}\\", "x".repeat(71)));
        assert_eq!(lines[1], format!("{}$", "x".repeat(29)));
        assert_eq!(ed("a\nb\n", ",n"), "1\ta\n2\tb\n");
        assert_eq!(ed("a\tb\n", "l"), "a\\tb$\n");
        assert_eq!(
            ed("a\nb\n", "1pn"),
            "1\ta\n",
            "the last suffix of n and p wins"
        );
        assert_eq!(ed("a\nb\n", "2=\n="), "2\n2\n");
        assert_eq!(ed("", "="), "0\n");
    }

    #[test]
    fn files_are_read_and_written_with_their_bytes_counted() {
        let mut host = Host::new("files");
        host.put("f", "one\ntwo\n");
        host.put("g", "more\n");
        let (status, out) = edit(&mut host, Some("f"), false, "r g\nw\nq");
        assert_eq!((status, out.as_str()), (0, "8\n5\n13\n"));
        assert_eq!(host.holds("f").as_deref(), Some("one\ntwo\nmore\n"));
        let (_, out) = edit(&mut host, Some("f"), false, "2,3w part\nf\n1W part\nq");
        assert_eq!(
            out, "13\n9\nf\n4\n",
            "the file named first stays the buffer's"
        );
        assert_eq!(host.holds("part").as_deref(), Some("two\nmore\none\n"));
        let (_, out) = edit(&mut host, None, false, "e g\nf\nE f\n,p\nq");
        assert_eq!(out, "5\ng\n13\none\ntwo\nmore\n");
        let (_, out) = edit(&mut host, None, true, "r g\nw new\nf\nq");
        assert_eq!(out, "g\n", "r names the buffer for the file it read");
        assert_eq!(host.holds("new").as_deref(), Some("more\n"));
        host.put("partial", "no line feed");
        let (_, out) = edit(&mut host, Some("partial"), false, "p\nw\nq");
        assert_eq!(
            out, "12\nno line feed\n13\n",
            "a last line without a line feed gets one"
        );
    }

    #[test]
    fn a_buffer_changed_and_not_written_is_warned_of_once() {
        let mut host = Host::new("warn");
        host.put("f", "a\n");
        let (status, out) = edit(&mut host, Some("f"), true, "H\nd\nq\nq");
        assert_eq!(
            (status, out.as_str()),
            (1, "?\nthe buffer is changed and not written\n")
        );
        let (status, out) = edit(&mut host, Some("f"), true, "d\nQ");
        assert_eq!((status, out.as_str()), (0, ""), "Q quits all the same");
        // Past a mistake only someone at the console goes on.
        host.typed = true;
        let (_, out) = edit(&mut host, Some("f"), true, "H\nd\ne f\ne f\n,p\nq");
        assert_eq!(out, "?\nthe buffer is changed and not written\na\n");
        let (status, _) = edit(&mut host, Some("f"), true, "d\nw\nq");
        assert_eq!(status, 0, "once written there is nothing to warn of");
        host.put("f", "a\n");
        let (status, out) = edit(&mut host, Some("f"), true, "H\nd\nq\np\nq");
        assert_eq!(
            (status, out.as_str()),
            (
                1,
                "?\nthe buffer is changed and not written\n?\nan address past the buffer\n?\nthe buffer is changed and not written\n"
            ),
            "another command between the two q asks again"
        );
    }

    #[test]
    fn commands_run_by_the_shell() {
        let mut host = Host::new("shell");
        host.put("f", "b\na\n");
        let (_, out) = edit(
            &mut host,
            Some("f"),
            false,
            "!echo hi %\nr !echo more\n,p\nw !sort\nQ",
        );
        assert_eq!(out, "4\nhi f\n!\n5\nb\na\nmore\na\nb\nmore\n9\n");
        let (_, out) = edit(&mut host, Some("f"), true, "!echo one\n!!\n!echo \\%");
        assert_eq!(out, "one\none\n%\n");
        let (_, out) = edit(&mut host, None, true, "H\n!echo %");
        assert_eq!(out, "?\nno file named\n");
        let (_, out) = edit(&mut host, None, true, "H\n!!");
        assert_eq!(out, "?\nno command run before\n");
        let (_, out) = edit(&mut host, None, true, "e !printf 'x\\ny\\n'\n,p");
        assert_eq!(out, "x\ny\n");
    }

    #[test]
    fn mistakes_are_a_question_mark_and_said_in_help_mode() {
        let mut host = Host::new("help");
        host.typed = true;
        let (status, out) = edit(&mut host, None, true, "zz\nh\nH\np\nH\np");
        assert_eq!(
            (status, out.as_str()),
            (
                1,
                "?\nno such command\nno such command\n?\nan address past the buffer\n?\n"
            )
        );
        host.typed = false;
        let (status, out) = edit(&mut host, None, true, "a\nx\n.\nzz\np");
        assert_eq!(
            (status, out.as_str()),
            (1, "?\n"),
            "a script stops at its first mistake"
        );
        let (status, out) = edit(&mut host, Some("nope"), false, "f\nq");
        assert_eq!(
            (status, out.as_str()),
            (0, "nope: no such file\nnope\n"),
            "a file not there is said, and named"
        );
    }

    #[test]
    fn p_turns_the_prompt_on_and_off() {
        let mut host = Host::new("prompt");
        host.input = ["P", "a", "x", ".", "P", "p"]
            .iter()
            .map(|l| l.as_bytes().to_vec())
            .collect();
        let mut ed = Ed::new(true, None);
        ed.run(None, &mut host);
        assert_eq!(String::from_utf8_lossy(&host.out), "**x\n");
        host.out.clear();
        host.typed = true;
        host.input = ["p", "q"].iter().map(|l| l.as_bytes().to_vec()).collect();
        let mut ed = Ed::new(true, Some(b"> "));
        ed.run(None, &mut host);
        assert_eq!(
            String::from_utf8_lossy(&host.out),
            "> ?\n> ",
            "-p gives the prompt and turns it on"
        );
    }

    const FIVE: &str = "one\ntwo\nthree\nfour\nfive\n";

    #[test]
    fn addresses_chain_their_offsets() {
        assert_eq!(ed(FIVE, "$-2,$p"), "three\nfour\nfive\n");
        assert_eq!(ed(FIVE, "3\n.-1,.+1p"), "three\ntwo\nthree\nfour\n");
        assert_eq!(ed(FIVE, "1+1+1p"), "three\n");
        assert_eq!(ed(FIVE, "$--p"), "three\n");
        assert_eq!(ed(FIVE, "2-1+3p"), "four\n");
        assert_eq!(
            ed(FIVE, "/one/2p"),
            "three\n",
            "a number after a search goes on from it"
        );
        assert_eq!(ed(FIVE, "1;p"), "one\n");
        assert_eq!(ed(FIVE, "1,$-3p"), "one\ntwo\n");
        assert_eq!(ed(FIVE, " 2 , 3 p"), "two\nthree\n", "blanks may part them");
        assert!(
            ed(FIVE, "+p").ends_with("?\nan address past the buffer\n"),
            "past the last line"
        );
        assert!(ed(FIVE, "2,1p").ends_with("?\nan address past the buffer\n"));
        assert!(
            ed(FIVE, "1-1p").ends_with("?\nan address past the buffer\n"),
            "line 0 prints nothing"
        );
    }

    #[test]
    fn searches_go_round_and_semicolons_move_the_current_line() {
        assert_eq!(
            ed(FIVE, "/t/;/f/p"),
            "two\nthree\nfour\n",
            "the second search goes on from the first"
        );
        assert_eq!(
            ed(FIVE, "/t/,/f/p"),
            "two\nthree\nfour\n",
            "both search from the current line, the last"
        );
        assert_eq!(ed(FIVE, "?o?,$p"), "four\nfive\n");
        assert_eq!(
            ed(FIVE, "1\n?e?p"),
            "one\nfive\n",
            "back past the first goes round to the last"
        );
        assert_eq!(ed(FIVE, "/e$/p\n//p\n//p"), "one\nthree\nfive\n");
        assert_eq!(
            ed(FIVE, "/thr/\n?"),
            "three\nthree\n",
            "an empty search takes the last expression, back too"
        );
    }

    #[test]
    fn printing_moves_the_current_line_to_the_last_printed() {
        assert_eq!(ed(FIVE, "2,3p\n.p"), "two\nthree\nthree\n");
        assert_eq!(ed(FIVE, "2,3n\n.n"), "2\ttwo\n3\tthree\n3\tthree\n");
        assert_eq!(ed(FIVE, "1l\n.="), "one$\n1\n");
        assert_eq!(
            ed(FIVE, "4\n\n"),
            "four\nfive\n",
            "a line alone prints the next"
        );
        assert_eq!(ed(FIVE, "2\n\n\n"), "two\nthree\nfour\n");
        assert!(
            ed(FIVE, "\n").ends_with("?\nan address past the buffer\n"),
            "past the last there is none"
        );
        assert_eq!(
            ed(FIVE, "2,4\n"),
            "four\n",
            "a range alone prints its last line"
        );
        assert_eq!(ed(FIVE, "2pl"), "two$\n");
        assert_eq!(ed(FIVE, "2nl"), "two$\n", "l wins over n");
        assert!(ed(FIVE, "2px").ends_with("?\nsomething after the command\n"));
        assert!(
            ed("", "p").ends_with("?\nan address past the buffer\n"),
            "an empty buffer has no line to print"
        );
    }

    #[test]
    fn text_input_takes_lines_until_a_point_alone() {
        assert_eq!(ed("", "a\n.x\n x\n..\n.\n,p"), ".x\n x\n..\n");
        assert_eq!(
            ed("", "a\n\n\n.\n,n"),
            "1\t\n2\t\n",
            "empty lines are lines"
        );
        assert_eq!(ed(FIVE, "2c\nTWO\n.\n,p"), "one\nTWO\nthree\nfour\nfive\n");
        assert_eq!(ed(FIVE, "2,4c\nX\n.\n,p"), "one\nX\nfive\n");
        assert_eq!(ed(FIVE, "1,5c\nall\n.\n,p"), "all\n");
        assert_eq!(
            ed(FIVE, "$c\n.\n.p"),
            "four\n",
            "changed to nothing, the last is the one before"
        );
        assert_eq!(ed(FIVE, "1c\nA\nB\n.\n.p"), "B\n");
        assert_eq!(
            ed(FIVE, "3ap\nx\n.\n"),
            "x\n",
            "a suffix prints once the text is in"
        );
        assert!(
            ed("", "c\nx\n.").ends_with("?\nan address past the buffer\n"),
            "nothing to change"
        );
    }

    #[test]
    fn deleting_leaves_the_line_after_current() {
        assert_eq!(
            ed(FIVE, "1,5d\n.="),
            "0\n",
            "an empty buffer's current line is 0"
        );
        assert_eq!(ed(FIVE, "2d\n.p"), "three\n");
        assert_eq!(ed(FIVE, "4,5d\n.p"), "three\n");
        assert_eq!(ed(FIVE, "1d\n1d\n,p"), "three\nfour\nfive\n");
        assert_eq!(ed(FIVE, ",d\na\nnew\n.\n,p"), "new\n");
        assert!(ed(FIVE, "0d").ends_with("?\nan address past the buffer\n"));
    }

    #[test]
    fn joining_takes_the_range_whole() {
        assert_eq!(ed(FIVE, ",j\n,p"), "onetwothreefourfive\n");
        assert_eq!(ed(FIVE, "4,5j\n.p\n$="), "fourfive\n4\n");
        assert_eq!(
            ed(FIVE, "5\nj"),
            "five\n?\nan address past the buffer\n",
            "the line after the last is none"
        );
        assert_eq!(ed(FIVE, "1j\n.p"), "one\n", "one line joins with nothing");
    }

    #[test]
    fn moving_and_copying_land_after_their_destination() {
        assert_eq!(ed(FIVE, "1m1\n,p"), FIVE, "onto itself it stays");
        assert_eq!(ed(FIVE, "5m$\n,p"), FIVE);
        assert_eq!(ed(FIVE, "2,3m5\n,p"), "one\nfour\nfive\ntwo\nthree\n");
        assert_eq!(
            ed(FIVE, "4,5m1\n.=\n,p"),
            "3\none\nfour\nfive\ntwo\nthree\n"
        );
        assert_eq!(
            ed(FIVE, "3t.\n,p"),
            "one\ntwo\nthree\nfour\nfive\nthree\n",
            "the destination's . is the current line, not the one copied"
        );
        assert_eq!(
            ed(FIVE, "3\nt.\n,p"),
            "three\none\ntwo\nthree\nthree\nfour\nfive\n"
        );
        assert_eq!(ed(FIVE, ",t$\n$="), "10\n");
        assert_eq!(ed(FIVE, "1,2t3\n.p"), "two\n");
        assert_eq!(
            ed(FIVE, "2,4m2\n,p"),
            "?\na destination within the lines moved\n"
        );
    }

    #[test]
    fn substitution_matches_from_left_to_right() {
        assert_eq!(
            ed("baaac\n", "s/a*/X/g\np"),
            "XbXcX\n",
            "an empty match right after another is none"
        );
        assert_eq!(ed("ab\n", "s/./(&)/g\np"), "(a)(b)\n");
        assert_eq!(ed("ab\n", "s/\\(.\\)\\(.\\)/\\2\\1/\np"), "ba\n");
        assert_eq!(ed("abc\n", "s/^/>/\np"), ">abc\n");
        assert_eq!(ed("abc\n", "s/$/</\np"), "abc<\n");
        assert_eq!(ed("a.b\n", "s/\\./,/\np"), "a,b\n");
        assert_eq!(ed("aaa\n", "s/a/b/3\np"), "aab\n");
        assert_eq!(
            ed("aaa\n", "s/a/b/4"),
            "?\nno line matches\n",
            "there is no fourth"
        );
        assert_eq!(
            ed("xyz\n", "s,y,Y,\np"),
            "xYz\n",
            "any character parts them"
        );
        assert_eq!(ed("xyz\n", "s/y/[\\/]/\np"), "x[/]z\n");
        assert_eq!(
            ed("a\n", "s/a/\\\\/\np"),
            "\\\n",
            "a backslash for a backslash"
        );
        assert_eq!(ed("äb\n", "s/./X/\np"), "Xb\n", "a character, not a byte");
        assert_eq!(ed("a b\n", "s/ /\\\n/\n,p"), "a\nb\n");
        assert_eq!(
            ed("ab\n", "s/a/\\\n\\\n/\n,n"),
            "1\t\n2\t\n3\tb\n",
            "each escaped line feed is a line"
        );
    }

    #[test]
    fn substitution_on_a_range_changes_the_lines_that_match() {
        assert_eq!(ed("aaa\nbab\nccc\n", ",s/b/B/\n.n"), "2\tBab\n");
        assert_eq!(ed("aaa\nbab\nccc\n", ",s/b/B/g\n,p"), "aaa\nBaB\nccc\n");
        assert_eq!(
            ed("a\na\na\n", ",s/a/b/\n.="),
            "3\n",
            "the last line changed is current"
        );
        assert_eq!(ed("a\nx\n", ",s/a/b/\n.="), "1\n");
        assert_eq!(
            ed("ab\ncd\n", ",s/b/\\\n/\n.=\n,p"),
            "2\na\n\ncd\n",
            "the last piece of a line split is current"
        );
    }

    #[test]
    fn substitution_keeps_its_expression_and_replacement() {
        assert_eq!(
            ed("ab\nab\n", "1s/a/x/\n2s//y/\n,p"),
            "xb\nyb\n",
            "an empty expression is the last"
        );
        assert_eq!(
            ed("ab\nab\n", "1s/a/x/\n2s/b/%/\n,p"),
            "xb\nax\n",
            "% alone is the last replacement"
        );
        assert_eq!(
            ed("ab\nab\n", "/b/\ns//X/\n.p"),
            "ab\naX\n",
            "a search sets the last expression too"
        );
        assert_eq!(
            ed("a%\n", "s/%/!/\np"),
            "a!\n",
            "% in an expression is itself"
        );
        assert_eq!(
            ed("ab\n", "s/a/x%y/\np"),
            "x%yb\n",
            "% with more around is itself"
        );
    }

    #[test]
    fn global_lists_run_on_each_marked_line_in_turn() {
        assert_eq!(ed(FIVE, "g/o/s//0/g\n,p"), "0ne\ntw0\nthree\nf0ur\nfive\n");
        assert_eq!(ed(FIVE, "v/e/d\n,p"), "one\nthree\nfive\n");
        assert_eq!(
            ed(FIVE, "g/./m0\n,p"),
            "five\nfour\nthree\ntwo\none\n",
            "each moved to the top in turn"
        );
        assert_eq!(
            ed(FIVE, "g/t/t$\n,p"),
            "one\ntwo\nthree\nfour\nfive\ntwo\nthree\n",
            "copies are not marked"
        );
        assert_eq!(ed(FIVE, "g/x/p"), "", "no line marked is no mistake");
        assert_eq!(ed(FIVE, "g/e/.="), "1\n3\n5\n");
        assert_eq!(ed(FIVE, "g/o/\n.="), "one\ntwo\nfour\n4\n");
        assert_eq!(ed(FIVE, "2,4g/./d\n,p"), "one\nfive\n");
        assert_eq!(
            ed(FIVE, "g/e/a\\\n--\n,p"),
            "one\n--\ntwo\nthree\n--\nfour\nfive\n--\n"
        );
        assert_eq!(
            ed(FIVE, "g/two/i\\\nX\\\nY\n,p"),
            "one\nX\nY\ntwo\nthree\nfour\nfive\n"
        );
        assert_eq!(
            ed(FIVE, "g/two/s/t/T/\\\ns/w/W/\n,p"),
            "one\nTWo\nthree\nfour\nfive\n"
        );
        assert_eq!(ed(FIVE, "g/o/d\nu\n,p"), FIVE);
        assert_eq!(
            ed(FIVE, "g/f/d\n,p"),
            "one\ntwo\nthree\n",
            "a line deleted by an earlier one is passed over"
        );
    }

    #[test]
    fn u_puts_back_each_kind_of_change() {
        assert_eq!(
            ed(FIVE, "1d\n2d\nu\n,p"),
            "two\nthree\nfour\nfive\n",
            "only the last"
        );
        assert_eq!(
            ed(FIVE, "a\nx\n.\nu\n.="),
            "5\n",
            "and the current line with it"
        );
        assert_eq!(ed(FIVE, "1s/one/ONE/\nu\nu\n1p"), "ONE\n");
        assert_eq!(ed(FIVE, "1m$\nu\n,p"), FIVE);
        assert_eq!(ed(FIVE, "1,2j\nu\n,p"), FIVE);
        assert_eq!(ed(FIVE, "1t$\nu\n$="), "5\n");
        assert_eq!(ed(FIVE, "2kx\n2d\nu\n'xp"), "two\n", "marks come back too");
        assert_eq!(ed(FIVE, "2c\nX\n.\nu\n2p"), "two\n");
        assert_eq!(
            ed(FIVE, "1d\nw\ne f\nu"),
            "?\nnothing to undo\n",
            "editing a file anew forgets what came before"
        );
    }

    #[test]
    fn marks_go_with_lines_that_move() {
        assert_eq!(ed(FIVE, "1kx\n1m$\n'x="), "5\n");
        assert_eq!(ed(FIVE, "5ka\n1,4d\n'ap"), "five\n");
        assert_eq!(
            ed(FIVE, "2ka\n2s/two/TWO/\n'ap"),
            "TWO\n",
            "a line changed keeps its mark"
        );
        assert_eq!(ed(FIVE, "1ka\n1t1\n'a="), "1\n", "a copy has no mark");
        assert_eq!(
            ed(FIVE, "2kb\n1,2j\n'bp"),
            "?\nno line has that mark\n",
            "lines joined are new"
        );
        assert_eq!(ed("", "ka"), "?\nan address past the buffer\n");
    }

    #[test]
    fn files_are_named_read_and_written_as_asked() {
        let mut host = Host::new("named");
        let (status, out) = edit(&mut host, None, false, "H\na\nx\n.\nw");
        assert_eq!((status, out.as_str()), (1, "?\nno file named\n"));
        let (_, out) = edit(&mut host, None, false, "H\nr nope");
        assert_eq!(out, "nope: no such file\n?\nthe file could not be read\n");
        let (_, out) = edit(&mut host, None, false, "H\ne");
        assert_eq!(out, "?\nno file named\n");
        let (_, out) = edit(&mut host, None, false, "H\nf");
        assert_eq!(out, "?\nno file named\n");
        host.put("e", "");
        let (_, out) = edit(&mut host, None, false, "r e\nf\n=\nq");
        assert_eq!(out, "0\ne\n0\n", "an empty file reads none");
        host.put("g", "1\n2\n");
        let (_, out) = edit(&mut host, Some("g"), false, "0r g\n,p\nf other\nw\nq");
        assert_eq!(out, "4\n4\n1\n2\n1\n2\nother\n8\n");
        assert_eq!(host.holds("other").as_deref(), Some("1\n2\n1\n2\n"));
        assert_eq!(
            host.holds("g").as_deref(),
            Some("1\n2\n"),
            "f names the buffer for another"
        );
        let (status, out) = edit(&mut host, Some("g"), false, "H\n1w part\nq");
        assert_eq!(
            (status, out.as_str()),
            (0, "4\n2\n"),
            "a buffer not changed needs no writing"
        );
        let (status, out) = edit(&mut host, Some("g"), false, "H\n1t$\n1w part\nq");
        assert_eq!(
            (status, out.as_str()),
            (1, "4\n2\n?\nthe buffer is changed and not written\n"),
            "written in part, it is changed still"
        );
        let (_, out) = edit(&mut host, Some("g"), false, "e\n=\nq");
        assert_eq!(out, "4\n4\n2\n", "e alone edits the buffer's file again");
    }

    #[test]
    fn commands_read_and_write_through_the_shell() {
        let mut host = Host::new("through");
        host.put("f", "b\na\n");
        let (_, out) = edit(&mut host, Some("f"), true, "!echo quiet");
        assert_eq!(out, "quiet\n", "-s writes no !");
        let (_, out) = edit(&mut host, Some("f"), false, "1r !echo x\n,p\nQ");
        assert_eq!(out, "4\n2\nb\nx\na\n");
        let (_, out) = edit(&mut host, Some("f"), false, "1w !tr a-z A-Z\nq");
        assert_eq!(out, "4\nB\n2\n");
        let (_, out) = edit(&mut host, Some("f"), false, "e !echo y\nf\nQ");
        assert_eq!(out, "4\n2\nf\n", "e ! keeps the buffer's name");
    }

    fn cursor(text: &str) -> Cursor {
        Cursor {
            bytes: text.as_bytes().to_vec(),
            at: 0,
        }
    }

    #[test]
    fn a_cursor_reads_numbers_blanks_and_delimited_text() {
        let mut at = cursor("  12ab");
        at.blanks();
        assert_eq!(at.at, 2);
        assert_eq!(at.number(), Some(12));
        assert_eq!(at.peek(), Some(b'a'));
        assert_eq!(at.next(), Some(b'a'));
        assert_eq!(at.rest(), b"b");
        assert_eq!(at.next(), None, "nothing after the end");
        assert_eq!(cursor("x").number(), None);
        assert_eq!(cursor("99999999999999999999999").number(), Some(usize::MAX));
        let mut at = cursor("a\\/b/c");
        assert_eq!(at.delimited(b'/'), (b"a/b".to_vec(), true));
        assert_eq!(at.rest(), b"c");
        assert_eq!(cursor("a\\.b").delimited(b'/'), (b"a\\.b".to_vec(), false));
        assert_eq!(cursor("a\\").delimited(b'/'), (b"a\\".to_vec(), false));
        assert_eq!(cursor("|x|").delimited(b'|'), (Vec::new(), true));
        assert_eq!(cursor("   ").rest(), b"");
    }

    #[test]
    fn addresses_given_choose_the_range() {
        let none = Addressed {
            first: None,
            second: None,
        };
        assert_eq!(
            (none.count(), none.range(3, 4), none.line(7)),
            (0, (3, 4), 7)
        );
        let one = Addressed {
            first: Some(2),
            second: None,
        };
        assert_eq!((one.count(), one.range(3, 4), one.line(7)), (1, (2, 2), 2));
        let two = Addressed {
            first: Some(2),
            second: Some(5),
        };
        assert_eq!((two.count(), two.range(3, 4), two.line(7)), (2, (2, 5), 5));
    }

    #[test]
    fn a_file_s_bytes_are_its_lines() {
        assert_eq!(Ed::split(b""), Vec::<Vec<u8>>::new());
        assert_eq!(Ed::split(b"a"), [b"a".to_vec()]);
        assert_eq!(Ed::split(b"a\n"), [b"a".to_vec()]);
        assert_eq!(Ed::split(b"a\n\n"), [b"a".to_vec(), Vec::new()]);
        assert_eq!(Ed::split(b"\n"), [Vec::<u8>::new()]);
        assert_eq!(Ed::split(b"a\nb"), [b"a".to_vec(), b"b".to_vec()]);
    }

    fn basic(pattern: &str) -> Regex {
        Regex::new(pattern, Syntax::Basic, false).unwrap()
    }

    #[test]
    fn replacing_takes_the_match_its_groups_and_escapes() {
        assert_eq!(
            replaced(&basic("a"), b"aXa", b"b", true, 1),
            Ok(Some(b"bXb".to_vec()))
        );
        assert_eq!(
            replaced(&basic("a"), b"aXa", b"b", false, 2),
            Ok(Some(b"aXb".to_vec()))
        );
        assert_eq!(
            replaced(&basic("a"), b"aXaXa", b"b", true, 2),
            Ok(Some(b"aXbXb".to_vec()))
        );
        assert_eq!(replaced(&basic("z"), b"aXa", b"b", true, 1), Ok(None));
        assert_eq!(
            replaced(&basic("x*"), b"a", b"-", true, 1),
            Ok(Some(b"-a-".to_vec()))
        );
        assert_eq!(
            replaced(&basic("\\(a\\)\\(b\\)"), b"ab", b"\\2\\1&", false, 1),
            Ok(Some(b"baab".to_vec()))
        );
        assert_eq!(
            replaced(&basic("a"), b"a", b"\\&\\x\\9", false, 1),
            Ok(Some(b"&x".to_vec())),
            "a group not there is nothing"
        );
        assert_eq!(
            replaced(&basic("$"), b"ab", b"!", true, 1),
            Ok(Some(b"ab!".to_vec()))
        );
    }

    #[test]
    fn l_folds_long_lines_before_they_pass_72() {
        let fits = listed(&[b'x'; 71]);
        assert_eq!(
            fits,
            [&[b'x'; 71][..], b"$\n"].concat(),
            "71 fit with the $ after"
        );
        let folds = listed(&[b'x'; 72]);
        assert_eq!(folds, [&[b'x'; 71][..], b"\\\nx$\n"].concat());
        let escaped = listed(&[&[b'x'; 70][..], b"\t"].concat());
        assert_eq!(
            escaped,
            [&[b'x'; 70][..], b"\\\n\\t$\n"].concat(),
            "an escape is not cut in two"
        );
        assert_eq!(listed(b"\\"), b"\\\\$\n");
        assert_eq!(listed(b"~ "), b"~ $\n");
        assert_eq!(listed(b"\x7f"), b"\\177$\n");
    }

    #[test]
    fn shell_commands_take_the_file_s_name_and_the_last_command() {
        let mut ed = Ed::new(true, None);
        assert_eq!(ed.shell_command(b"ls %"), Err(Fault("no file named")));
        assert_eq!(ed.shell_command(b"!"), Err(Fault("no command run before")));
        ed.file = b"notes".to_vec();
        assert_eq!(ed.shell_command(b"wc %"), Ok(b"wc notes".to_vec()));
        assert_eq!(ed.shell_command(b"!"), Ok(b"wc notes".to_vec()));
        assert_eq!(ed.shell_command(b"! -l"), Ok(b"wc notes -l".to_vec()));
        assert_eq!(
            ed.shell_command(b"echo \\% %%"),
            Ok(b"echo % notesnotes".to_vec())
        );
        assert_eq!(ed.shell_command(b""), Ok(Vec::new()));
        assert_eq!(ed.named(Vec::new()), Ok(b"notes".to_vec()));
        assert_eq!(ed.named(b"other".to_vec()), Ok(b"other".to_vec()));
        assert_eq!(ed.file, b"notes", "a buffer named keeps its name");
        let mut fresh = Ed::new(true, None);
        assert_eq!(fresh.named(b"!cmd".to_vec()), Ok(b"!cmd".to_vec()));
        assert_eq!(fresh.file, b"", "a command names no buffer");
    }

    #[test]
    fn commands_that_take_no_address_refuse_one() {
        for script in [
            "1e f", "1E f", "1f", "1h", "1H", "1P", "1q", "1Q", "1u", "1!ls", "1,2e f",
        ] {
            assert_eq!(
                ed(FIVE, script),
                "?\nno address goes with this command\n",
                "{script}"
            );
        }
        for script in ["1k", "1kA", "1k1"] {
            assert_eq!(
                ed(FIVE, script),
                "?\na mark is a small letter\n",
                "{script}"
            );
        }
        for script in ["qx", "Qx", "ux", "hx", "Hx", "Px", "=x", "1kax"] {
            assert_eq!(
                ed(FIVE, script),
                "?\nsomething after the command\n",
                "{script}"
            );
        }
        assert_eq!(ed(FIVE, "x"), "?\nno such command\n");
        assert_eq!(ed(FIVE, "1z"), "?\nno such command\n");
    }

    #[test]
    fn a_suffix_prints_the_current_line_after_the_command() {
        assert_eq!(ed(FIVE, "1dp"), "two\n");
        assert_eq!(ed(FIVE, "1,2jp"), "onetwo\n");
        assert_eq!(ed(FIVE, "2m0p"), "two\n");
        assert_eq!(ed(FIVE, "1t0n"), "1\tone\n");
        assert_eq!(ed(FIVE, "2s/two/2/p"), "2\n");
        assert_eq!(
            ed(FIVE, "1d\nup"),
            "five\n",
            "u puts back the current line as it was"
        );
        assert_eq!(ed(FIVE, "1al\nx\n."), "x$\n");
        assert_eq!(ed(FIVE, "2cn\nX\n."), "2\tX\n");
        assert_eq!(ed(FIVE, "1,5dp"), "", "with no line left nothing prints");
    }

    /// What the file `f`, holding `before`, holds after the script
    /// `lines`, which ends by writing it; and what ed wrote meanwhile.
    fn session(before: &str, lines: &str) -> (String, String) {
        let mut host = Host::new("session");
        host.put("f", before);
        let (status, out) = edit(&mut host, Some("f"), true, &format!("H\n{lines}\nw\nq"));
        assert_eq!(status, 0, "{lines}: {out}");
        (host.holds("f").unwrap(), out)
    }

    #[test]
    fn sessions_edit_files_as_scripts_do() {
        let config = "name=x\nport=80\n# end\n";
        assert_eq!(
            session(config, "/^port=/s/=.*/=8080/").0,
            "name=x\nport=8080\n# end\n"
        );
        assert_eq!(
            session(config, "/^name=/a\nhost=h\n.").0,
            "name=x\nhost=h\nport=80\n# end\n"
        );
        assert_eq!(session(config, "/^#/d").0, "name=x\nport=80\n");
        assert_eq!(
            session(config, "0a\n# start\n.").0,
            "# start\nname=x\nport=80\n# end\n"
        );
        assert_eq!(
            session(config, "$a\nlast\n.").0,
            "name=x\nport=80\n# end\nlast\n"
        );
        assert_eq!(
            session(config, ",s/^/  /").0,
            "  name=x\n  port=80\n  # end\n"
        );
        assert_eq!(
            session(config, "g/=/s/=/: /").0,
            "name: x\nport: 80\n# end\n"
        );
        assert_eq!(
            session("a\n\nb\n\n\nc\n", "g/^$/d").0,
            "a\nb\nc\n",
            "empty lines go"
        );
        assert_eq!(
            session("a,\nb,\nc\nd\n", "g/,$/j").0,
            "a,b,\nc\nd\n",
            "a line joined is passed over"
        );
        assert_eq!(session("1\n2\n3\n", "g/^/m0").0, "3\n2\n1\n");
        assert_eq!(session("1\n2\n", "g/^/t.").0, "1\n1\n2\n2\n");
        assert_eq!(
            session("keep\ndrop\nkeep too\n", "v/keep/d").0,
            "keep\nkeep too\n"
        );
        assert_eq!(session("x\n", "s/x/a\\\nb/").0, "a\nb\n");
        assert_eq!(session("a\nb\nc\nd\n", "/b/,/c/m0").0, "b\nc\na\nd\n");
        assert_eq!(
            session("a\nb\nc\n", "1ka\n3kb\n'a,'bs/$/!/").0,
            "a!\nb!\nc!\n"
        );
        assert_eq!(
            session("one\n", "s/one/two/\ns/two/three/\nu").0,
            "two\n",
            "u takes back only the last"
        );
    }

    #[test]
    fn sessions_print_as_they_go() {
        assert_eq!(session("a\nb\nc\n", ",n").1, "1\ta\n2\tb\n3\tc\n");
        assert_eq!(session("a\nb\nc\n", "2\n=\n.=\n$=").1, "b\n3\n2\n3\n");
        assert_eq!(session("x\ty\n", "l\ns/\t/ /p\nl").1, "x\\ty$\nx y\nx y$\n");
        assert_eq!(session("a\nb\n", "g/./p\ng/./n").1, "a\nb\n1\ta\n2\tb\n");
        assert_eq!(session("abc\n", "s/b/[&]/gp").1, "a[b]c\n");
    }

    #[test]
    fn a_search_finds_its_own_line_when_no_other_matches() {
        assert_eq!(
            ed(FIVE, "4\n/four/p"),
            "four\nfour\n",
            "round past the end back to itself"
        );
        assert_eq!(ed(FIVE, "4\n?four?p"), "four\nfour\n");
        assert_eq!(
            ed(FIVE, "?t?-1p"),
            "two\n",
            "the search back finds three, one before it is two"
        );
        assert_eq!(
            ed(FIVE, "/o/;//p"),
            "one\ntwo\n",
            "after ; the second goes on from the first, which starts the range"
        );
        assert_eq!(
            ed(FIVE, "/f/,/f/p"),
            "four\n",
            "both from the current line find the same"
        );
    }

    #[test]
    fn an_empty_buffer_has_no_lines_but_its_zeroth() {
        assert_eq!(ed("", "$="), "0\n");
        assert_eq!(ed("", ".="), "0\n");
        assert_eq!(ed("", ",p"), "?\nan address past the buffer\n");
        assert_eq!(ed("", "a\nx\n.\n$="), "1\n");
        assert_eq!(ed("", "i\nx\n.\n,p"), "x\n", "before line 0 is as after it");
        assert_eq!(
            ed("", "g/x/d"),
            "?\nan address past the buffer\n",
            "a global command needs a line to go over"
        );
        let mut host = Host::new("empty");
        let (_, out) = edit(&mut host, None, false, "w new\nq");
        assert_eq!(out, "0\n");
        assert_eq!(host.holds("new").as_deref(), Some(""));
    }

    #[test]
    fn a_file_s_bytes_are_kept_as_they_are() {
        let mut host = Host::new("bytes");
        std::fs::write(host.root.join("bin"), b"a\0b\n\xff\xfe\n").unwrap();
        let (_, out) = edit(&mut host, Some("bin"), false, "l\n1l\nw copy\nq");
        assert_eq!(out, "7\n\\377\\376$\na\\000b$\n7\n");
        assert_eq!(
            std::fs::read(host.root.join("copy")).unwrap(),
            b"a\0b\n\xff\xfe\n"
        );
    }

    #[test]
    fn interactive_mistakes_go_on_and_end_with_1() {
        let mut host = Host::new("goes on");
        host.typed = true;
        let (status, out) = edit(&mut host, None, true, "p\na\nx\n.\np\nq");
        assert_eq!(
            (status, out.as_str()),
            (1, "?\nx\n?\n"),
            "and q warns of the change"
        );
        let (status, out) = edit(&mut host, None, true, "a\nx\n.\nQ");
        assert_eq!(
            (status, out.as_str()),
            (0, ""),
            "with no mistake it ends with 0"
        );
        host.input.clear();
        let mut ed = Ed::new(true, None);
        assert_eq!(ed.run(None, &mut host), 0, "the end of the input ends it");
    }

    #[test]
    fn what_cannot_be_written_is_said() {
        let mut host = Host::new("unwritable");
        host.put("f", "x\n");
        std::fs::create_dir(host.root.join("dir")).unwrap();
        let (status, out) = edit(&mut host, Some("f"), true, "H\nw dir");
        assert_eq!(
            (status, out.as_str()),
            (
                1,
                "dir: cannot be written\n?\nthe file could not be written\n"
            )
        );
        let (status, out) = edit(&mut host, Some("f"), true, "H\nW dir");
        assert_eq!(
            (status, out.as_str()),
            (
                1,
                "dir: cannot be written\n?\nthe file could not be written\n"
            )
        );
        assert_eq!(
            host.holds("f").as_deref(),
            Some("x\n"),
            "the buffer's own file stays"
        );
    }

    #[test]
    fn reading_makes_the_last_line_read_current() {
        let mut host = Host::new("read");
        host.put("f", "1\n2\n3\n");
        host.put("two", "a\nb\n");
        host.put("none", "");
        let (_, out) = edit(
            &mut host,
            Some("f"),
            true,
            "1r two\n.=\n2r none\n.=\n0r two\n.=\n,p\nQ",
        );
        assert_eq!(out, "3\n2\n2\na\nb\n1\na\nb\n2\n3\n");
    }

    #[test]
    fn substitution_flags_print_and_choose() {
        assert_eq!(ed("a-a-a\n", "s/a/b/gp"), "b-b-b\n");
        assert_eq!(ed("a-a-a\n", "s/a/b/2p"), "a-b-a\n");
        assert_eq!(ed("a-a-a\n", "s/a/b/3n"), "1\ta-a-b\n");
        assert_eq!(ed("a\tb\n", "s/a/c/l"), "c\\tb$\n");
        assert_eq!(
            ed("a-a\n", "s/a/b/g2p"),
            "a-b\n",
            "g and a count go in either order"
        );
        assert_eq!(ed("ab\n", "s/b/c/ p"), "?\nsomething after the command\n");
    }

    #[test]
    fn an_empty_expression_in_a_global_command_is_the_last() {
        assert_eq!(ed(FIVE, "/^t/\ng//d\n,p"), "two\none\nfour\nfive\n");
        assert_eq!(ed(FIVE, "g/o/p\nv//p"), "one\ntwo\nfour\nthree\nfive\n");
        assert_eq!(ed(FIVE, "g//p"), "?\nno regular expression used before\n");
    }

    #[test]
    fn h_says_the_last_mistake_and_nothing_before_one() {
        let mut host = Host::new("h");
        host.typed = true;
        let (_, out) = edit(&mut host, None, true, "h\nzz\nh\nh\nq");
        assert_eq!(out, "?\nno such command\nno such command\n");
        let (_, out) = edit(&mut host, None, true, "zz\n,p\nh\nq");
        assert_eq!(out, "?\n?\nan address past the buffer\n", "the last one");
    }

    #[test]
    fn a_number_too_great_is_past_the_buffer() {
        assert_eq!(
            ed(FIVE, "99999999999999999999999999p"),
            "?\nan address past the buffer\n"
        );
        assert_eq!(
            ed(FIVE, "1+99999999999999999999999p"),
            "?\nan address past the buffer\n"
        );
        assert_eq!(
            ed(FIVE, "$-99999999999999999999999p"),
            "?\nan address past the buffer\n"
        );
    }

    #[test]
    fn q_and_e_warn_only_of_changes_not_written() {
        let mut host = Host::new("quits");
        host.put("f", "a\n");
        host.typed = true;
        let (status, out) = edit(&mut host, Some("f"), true, "q\np");
        assert_eq!(
            (status, out.as_str()),
            (0, ""),
            "nothing changed, q quits at once"
        );
        let (status, out) = edit(&mut host, Some("f"), true, "s/a/b/\nw\ne f\np\nq");
        assert_eq!(
            (status, out.as_str()),
            (0, "b\n"),
            "once written, e reads the file again at once"
        );
        let (status, out) = edit(&mut host, Some("f"), true, "s/b/c/\nE f\np\nq");
        assert_eq!((status, out.as_str()), (0, "b\n"), "E forgets the change");
        let (status, _) = edit(&mut host, None, true, "a\nx\n.\nw new\nq");
        assert_eq!(status, 0, "a buffer written whole to a file is written");
        assert_eq!(host.holds("new").as_deref(), Some("x\n"));
    }

    #[test]
    fn a_blank_or_a_backslash_parts_nothing() {
        assert_eq!(ed(FIVE, "g o p"), "?\nno delimiter\n");
        assert_eq!(ed(FIVE, "v o p"), "?\nno delimiter\n");
        assert_eq!(ed(FIVE, "s o x "), "?\nno delimiter\n");
        assert_eq!(ed(FIVE, "s\\o\\x\\"), "?\nno delimiter\n");
        assert_eq!(ed(FIVE, "s"), "?\nno delimiter\n");
    }

    #[test]
    fn joining_lines_past_the_buffer_is_refused() {
        assert_eq!(ed(FIVE, "0,1j"), "?\nan address past the buffer\n");
        assert_eq!(ed(FIVE, "2,1j"), "?\nan address past the buffer\n");
        assert_eq!(ed("", "j"), "?\nan address past the buffer\n");
    }

    #[test]
    fn a_global_list_of_blanks_prints() {
        assert_eq!(ed(FIVE, "g/o/ "), "one\ntwo\nfour\n");
        assert_eq!(ed(FIVE, "g/o/\t"), "one\ntwo\nfour\n");
    }

    #[test]
    fn every_change_leaves_the_buffer_changed() {
        let mut host = Host::new("changed");
        host.put("f", "a\nb\n");
        for change in [
            "1m$", "1t$", "s/b/c/", "1,2j", "1d", "2c\nx\n.", "a\nx\n.", "1r f", "u",
        ] {
            let script = if change == "u" {
                "1d\nw\nu\nq"
            } else {
                &format!("{change}\nq")
            };
            let (status, out) = edit(&mut host, Some("f"), true, script);
            assert_eq!((status, out.as_str()), (1, "?\n"), "{change}");
            host.put("f", "a\nb\n");
        }
        let (status, out) = edit(&mut host, Some("f"), true, "1j\nq");
        assert_eq!(
            (status, out.as_str()),
            (0, ""),
            "one line joined with nothing is no change"
        );
        let (status, out) = edit(&mut host, Some("f"), true, "1kx\n1j\n'xp\nq");
        assert_eq!((status, out.as_str()), (0, "a\n"), "and keeps its mark");
    }

    #[test]
    fn a_second_q_quits() {
        let mut host = Host::new("second");
        host.put("f", "a\nb\n");
        host.typed = true;
        let (status, out) = edit(&mut host, Some("f"), true, "1d\nq\nq\n,p");
        assert_eq!((status, out.as_str()), (1, "?\n"));
        let (status, out) = edit(&mut host, Some("f"), true, "1d\ne f\ne f\n,p\nq");
        assert_eq!((status, out.as_str()), (1, "?\na\nb\n"));
    }

    #[test]
    fn a_substitution_over_lines_is_undone_whole() {
        assert_eq!(ed("a\na\na\n", ",s/a/b/\nu\n,p"), "a\na\na\n");
        assert_eq!(ed("a\nx\na\n", ",s/a/b/\n,p\nu\n,p"), "b\nx\nb\na\nx\na\n");
    }

    #[test]
    fn searches_and_substitutions_match_case_as_it_is() {
        assert_eq!(ed(FIVE, "/ONE/p"), "?\nno line matches\n");
        assert_eq!(ed(FIVE, "1s/ONE/1/"), "?\nno line matches\n");
        assert_eq!(ed("One\none\n", "g/one/p"), "one\n");
    }

    #[test]
    fn u_puts_back_the_current_line_too() {
        assert_eq!(ed(FIVE, "3\n1,2j\nu\n.p"), "three\nthree\n");
        assert_eq!(ed(FIVE, "2\n4d\nu\n.p"), "two\ntwo\n");
        assert_eq!(ed(FIVE, "2\n4m0\nu\n.p"), "two\ntwo\n");
        assert_eq!(ed(FIVE, "2\n5s/five/5/\nu\n.p"), "two\ntwo\n");
        assert_eq!(ed(FIVE, "2\n1r f\nu\n.p"), "two\ntwo\n");
        let undone = ed(FIVE, "2\ng/f/s/f/F/\nu\n.p\n,p");
        assert_eq!(undone, format!("two\ntwo\n{FIVE}"));
    }

    #[test]
    fn q_in_a_global_list_quits_ed() {
        assert_eq!(ed(FIVE, "g/two/q\n,p"), "");
        assert_eq!(ed(FIVE, "1d\ng/two/Q\n,p"), "");
        let mut host = Host::new("global-q");
        host.put("f", FIVE);
        let (status, out) = edit(&mut host, Some("f"), true, "H\n1d\ng/two/q\n,p");
        assert_eq!(
            (status, out.as_str()),
            (1, "?\nthe buffer is changed and not written\n"),
            "and warns as q does anywhere"
        );
        assert_eq!(
            ed("a1\na2\n", "G/a/\nq\n,p"),
            "a1\n",
            "asked for at the console too"
        );
    }

    #[test]
    fn a_global_substitution_passes_over_lines_it_misses() {
        assert_eq!(ed(FIVE, "g/./s/o/0/\n,p"), "0ne\ntw0\nthree\nf0ur\nfive\n");
        assert_eq!(
            ed(FIVE, "g/./s/o/0/p"),
            "0ne\ntw0\nf0ur\n",
            "a line missed is not printed"
        );
        assert_eq!(
            ed(FIVE, "g/./s/x/y/\n.="),
            "5\n",
            "missing every line is no mistake either"
        );
        assert_eq!(
            ed(FIVE, "s/x/y/"),
            "?\nno line matches\n",
            "outside one it is"
        );
    }

    #[test]
    fn suffixes_print_after_marks_numbers_and_help() {
        assert_eq!(ed(FIVE, "2kap"), "five\n", "k leaves the current line");
        assert_eq!(ed(FIVE, "2\n4kan\n'ap"), "two\n2\ttwo\nfour\n");
        assert_eq!(ed(FIVE, "2\n4=p"), "two\n4\ntwo\n");
        assert_eq!(ed(FIVE, "hp"), "five\n");
        assert_eq!(ed(FIVE, "Hl"), "five$\n");
        assert_eq!(ed(FIVE, "Pp"), "five\n*", "the prompt comes after");
        assert_eq!(ed("", "=p"), "0\n", "with no lines nothing is printed");
        assert_eq!(ed(FIVE, "qp"), "?\nsomething after the command\n");
        assert_eq!(ed(FIVE, "Qn"), "?\nsomething after the command\n");
        assert_eq!(ed(FIVE, "q "), "", "blanks are nothing");
    }

    #[test]
    fn an_odd_run_of_backslashes_carries_a_line_on() {
        assert!(continued(b"a\\"));
        assert!(!continued(b"a\\\\"));
        assert!(continued(b"a\\\\\\"));
        assert!(continued(b"\\"));
        assert!(!continued(b"a"));
        assert!(!continued(b""));
        assert_eq!(
            ed("a\n", "s/a/\\\\\\\nb/\n,p"),
            "\\\nb\n",
            "a backslash, and the line goes on"
        );
        assert_eq!(
            ed("a\n", "s/a/\\\\\n,p"),
            "\\\n\\\n",
            "a backslash, and the replacement ends with the line"
        );
        assert_eq!(
            ed(FIVE, "g/two/s/w/\\\\\\\ns/t/T/\n,p"),
            "t\\o\none\nT\\o\nthree\nfour\nfive\n",
            "the same in a global command's list"
        );
    }

    #[test]
    fn e_of_a_file_not_there_starts_it_empty() {
        let mut host = Host::new("e-new");
        host.put("f", "a\nb\n");
        host.typed = true;
        let (status, out) = edit(
            &mut host,
            Some("f"),
            true,
            "H\ne nope\nf\n.=\na\nnew\n.\nw\nq",
        );
        assert_eq!(
            (status, out.as_str()),
            (
                1,
                "nope: no such file\n?\nthe file could not be read\nnope\n0\n"
            )
        );
        assert_eq!(host.holds("nope").as_deref(), Some("new\n"));
        assert_eq!(host.holds("f").as_deref(), Some("a\nb\n"));
    }

    #[test]
    fn an_empty_buffer_has_nothing_to_search() {
        assert_eq!(ed("", "/x/p"), "?\nno line matches\n");
        assert_eq!(ed("", "?x?p"), "?\nno line matches\n");
        assert_eq!(ed("", "g/x/p"), "?\nan address past the buffer\n");
    }

    #[test]
    fn an_empty_buffer_writes_an_empty_file() {
        let mut host = Host::new("w-empty");
        host.put("f", "a\n");
        let (status, out) = edit(&mut host, Some("f"), false, "d\nw\nq");
        assert_eq!((status, out.as_str()), (0, "2\n0\n"));
        assert_eq!(host.holds("f").as_deref(), Some(""));
        let (_, out) = edit(&mut host, Some("f"), true, "H\n1w g");
        assert_eq!(out, "?\nan address past the buffer\n", "it has no line 1");
        assert_eq!(host.holds("g"), None);
    }

    #[test]
    fn writing_part_of_the_buffer_leaves_it_changed() {
        let mut host = Host::new("w-part");
        host.put("f", FIVE);
        let (status, out) = edit(&mut host, Some("f"), true, "1d\n1,2w g\nq");
        assert_eq!((status, out.as_str()), (1, "?\n"));
        assert_eq!(host.holds("g").as_deref(), Some("two\nthree\n"));
        let (status, out) = edit(&mut host, Some("f"), true, "1d\nw g\nq");
        assert_eq!(
            (status, out.as_str()),
            (0, ""),
            "the whole buffer written is no change, whichever the file"
        );
        assert_eq!(host.holds("g").as_deref(), Some("two\nthree\nfour\nfive\n"));
        let (status, out) = edit(&mut host, Some("f"), true, "1d\nw !cat\nq");
        assert_eq!(
            (status, out.as_str()),
            (1, "two\nthree\nfour\nfive\n?\n"),
            "nor is a command given it"
        );
        assert_eq!(host.holds("f").as_deref(), Some(FIVE));
    }

    #[test]
    fn more_addresses_than_a_command_takes_leave_the_last() {
        assert_eq!(
            ed(FIVE, "1;2;4p"),
            "two\nthree\nfour\n",
            "each ; moves the current line"
        );
        assert_eq!(ed(FIVE, "2\n;4p"), "two\ntwo\nthree\nfour\n");
        assert_eq!(ed(FIVE, "1,2,3d\n,p"), "one\nfour\nfive\n");
        assert_eq!(ed(FIVE, "2,3="), "3\n");
        assert_eq!(ed(FIVE, "1,2,3,4j\n,p"), "one\ntwo\nthreefour\nfive\n");
    }

    #[test]
    fn changing_in_a_global_list_takes_the_text_from_it() {
        assert_eq!(
            ed(FIVE, "g/two/c\\\nTWO\n,p"),
            "one\nTWO\nthree\nfour\nfive\n"
        );
        assert_eq!(
            ed(FIVE, "g/o/c\\\nX\n,p"),
            "X\nX\nthree\nX\nfive\n",
            "each line marked has the list's text anew"
        );
        assert_eq!(
            ed("a1\nb2\n", "G/a/\na\nnew\n.\n,p"),
            "a1\na1\nnew\nb2\n",
            "asked for at the console, from the input"
        );
    }

    #[test]
    fn an_interactive_global_ends_with_its_input_or_a_mistake() {
        assert_eq!(ed("a1\na2\n", "G/a/"), "a1\n");
        assert_eq!(
            ed("a1\na2\n", "G/a/\nzz\n,p"),
            "a1\n?\nno such command\n",
            "a script stops there"
        );
    }

    #[test]
    fn lines_moved_or_copied_to_the_top_end_current() {
        assert_eq!(ed(FIVE, "5m0\n.=\n,p"), "1\nfive\none\ntwo\nthree\nfour\n");
        assert_eq!(
            ed(FIVE, "2,3t0\n.=\n,p"),
            "2\ntwo\nthree\none\ntwo\nthree\nfour\nfive\n"
        );
    }

    #[test]
    fn l_folds_before_a_line_would_pass_its_width() {
        let x = |n: usize| "x".repeat(n);
        let shown = |bytes: &[u8]| String::from_utf8(listed(bytes)).unwrap();
        assert_eq!(shown(x(71).as_bytes()), format!("{}$\n", x(71)));
        assert_eq!(shown(x(72).as_bytes()), format!("{}\\\nx$\n", x(71)));
        assert_eq!(
            shown(format!("{}\t", x(69)).as_bytes()),
            format!("{}\\t$\n", x(69))
        );
        assert_eq!(
            shown(format!("{}\t", x(70)).as_bytes()),
            format!("{}\\\n\\t$\n", x(70)),
            "an escape is not parted"
        );
        assert_eq!(
            shown("ä".as_bytes()),
            "\\303\\244$\n",
            "bytes past ASCII in octal"
        );
    }

    #[test]
    fn a_global_list_ended_by_the_input_runs_what_it_has() {
        assert_eq!(ed(FIVE, "g/two/p\\"), "two\n");
        assert_eq!(ed(FIVE, "g/two/s/t/T/\\\np\\"), "Two\n");
    }

    #[test]
    fn commands_read_and_written_name_no_file() {
        let mut host = Host::new("unnamed");
        let (_, out) = edit(&mut host, Some("!echo x"), true, "H\n,p\nf");
        assert_eq!(out, "x\n?\nno file named\n", "ed started on one");
        let (_, out) = edit(&mut host, None, true, "H\ne !echo x\nf");
        assert_eq!(out, "?\nno file named\n");
        let (_, out) = edit(&mut host, None, true, "H\nr !echo x\nf");
        assert_eq!(out, "?\nno file named\n");
        let (_, out) = edit(&mut host, None, true, "H\na\nx\n.\nw !cat\nf");
        assert_eq!(out, "x\n?\nno file named\n");
        let (_, out) = edit(&mut host, Some("!echo x"), true, "H\nw");
        assert_eq!(out, "?\nno file named\n", "so w gives it nothing");
    }

    #[test]
    fn e_alone_reads_the_buffer_s_file_again() {
        assert_eq!(ed(FIVE, "1d\nw\ne\n,p"), "two\nthree\nfour\nfive\n");
        assert_eq!(
            ed(FIVE, "1d\ne\n,p"),
            "?\nthe buffer is changed and not written\n",
            "and warns first of a change"
        );
    }

    #[test]
    fn a_bad_expression_is_said_wherever_it_is() {
        let why = Regex::new("\\(", Syntax::Basic, false)
            .err()
            .map(|e| e.code.says());
        assert_eq!(why, Some("a parenthesis without its other half"));
        for script in ["/\\(/p", "?\\(?p", "g/\\(/p", "v/\\(/p", "s/\\(/x/"] {
            assert_eq!(
                ed(FIVE, script),
                "?\na parenthesis without its other half\n",
                "{script}"
            );
        }
    }

    #[test]
    fn diff_e_scripts_apply_from_the_bottom_up() {
        assert_eq!(
            session("a\nb\nc\nd\ne\n", "5d\n3c\nC\n.\n1a\nnew\n."),
            ("a\nnew\nb\nC\nd\n".into(), String::new())
        );
        assert_eq!(
            session("x\n", "1a\n..\n.\ns/.//").0,
            "x\n.\n",
            "a line of a point alone goes in doubled, then is halved"
        );
        assert_eq!(
            session("a\nb\nc\n", "2,3c\nB\n.\n0a\nstart\n.").0,
            "start\na\nB\n"
        );
    }

    #[test]
    fn global_commands_tidy_files() {
        assert_eq!(
            session("a\n\nb\n\n", "v/./d").0,
            "a\nb\n",
            "empty lines out"
        );
        assert_eq!(
            session("a  \nb\t\nc\n", ",s/[ \t]*$//").0,
            "a\nb\nc\n",
            "blanks at the ends out"
        );
        assert_eq!(
            session("a\nb\n", "g/^/t.").0,
            "a\na\nb\nb\n",
            "each line doubled"
        );
        assert_eq!(
            session("a\nb\nc\nd\n", "g/^/j").0,
            "ab\ncd\n",
            "lines joined in pairs"
        );
        assert_eq!(session("a\nb\n", ",s/^/> /").0, "> a\n> b\n");
        assert_eq!(
            session("# a\nx\n# b\ny\n", "g/^#/d").0,
            "x\ny\n",
            "comments out"
        );
    }

    #[test]
    fn marks_hold_a_block_to_move() {
        assert_eq!(
            session("x\nstart\ny\nend\nz\n", "/start/ka\n/end/kb\n'a,'bm0").0,
            "start\ny\nend\nx\nz\n"
        );
        assert_eq!(
            session("x\nstart\ny\nend\nz\n", "/start/ka\n/end/kb\n'a,'bt$").0,
            "x\nstart\ny\nend\nz\nstart\ny\nend\n"
        );
        assert_eq!(
            session("x\nstart\ny\nend\nz\n", "/start/ka\n/end/kb\n'a+1,'b-1d").0,
            "x\nstart\nend\nz\n",
            "with offsets, the lines between"
        );
    }

    #[test]
    fn a_mistake_in_a_global_list_stops_it_and_u_takes_back_what_it_did() {
        let mut host = Host::new("global-stop");
        host.put("f", FIVE);
        host.typed = true;
        let (status, out) = edit(
            &mut host,
            Some("f"),
            true,
            "H\ng/o/s/o/0/\\\nzz\n,p\nu\n,p\nQ",
        );
        assert_eq!(status, 1);
        assert_eq!(
            out,
            format!("?\nno such command\n0ne\ntwo\nthree\nfour\nfive\n{FIVE}")
        );
    }

    #[test]
    fn global_lists_print_as_their_commands_say() {
        assert_eq!(ed(FIVE, "g/o/l"), "one$\ntwo$\nfour$\n");
        assert_eq!(ed(FIVE, "v/o/n"), "3\tthree\n5\tfive\n");
        assert_eq!(ed(FIVE, "g/o/-1p"), "?\nan address past the buffer\n");
        assert_eq!(ed(FIVE, "g/t/+1p"), "three\nfour\n");
    }

    #[test]
    fn searches_take_their_delimiter_escaped_and_may_leave_it_off() {
        assert_eq!(ed("a/b\n", "/a\\/b/p"), "a/b\n");
        assert_eq!(ed("a?b\n", "?a\\?b?p"), "a?b\n");
        assert_eq!(ed(FIVE, "/two"), "two\n", "the end of the line closes it");
        assert_eq!(ed(FIVE, "?two"), "two\n");
        assert_eq!(ed(FIVE, "2;+1p"), "two\nthree\n");
    }

    #[test]
    fn destinations_are_addresses_of_any_kind() {
        assert_eq!(ed(FIVE, "1t$-1\n,p"), "one\ntwo\nthree\nfour\none\nfive\n");
        assert_eq!(ed(FIVE, "1m/four/\n,p"), "two\nthree\nfour\none\nfive\n");
        assert_eq!(ed(FIVE, "5ka\n1m'a\n,p"), "two\nthree\nfour\nfive\none\n");
        assert_eq!(
            ed(FIVE, "1t9"),
            "?\nan address past the buffer\n",
            "and must be in the buffer"
        );
        assert_eq!(ed(FIVE, "1m'z"), "?\nno line has that mark\n");
    }

    #[test]
    fn every_command_that_changes_lines_takes_a_suffix() {
        assert_eq!(ed(FIVE, "1,2jp"), "onetwo\n");
        assert_eq!(ed(FIVE, "2cp\nX\n."), "X\n");
        assert_eq!(ed(FIVE, "1m$p"), "one\n");
        assert_eq!(ed(FIVE, "1t0n"), "1\tone\n");
        assert_eq!(ed(FIVE, "2ip\nX\n."), "X\n");
        assert_eq!(
            ed(FIVE, "1d\nup"),
            "five\n",
            "u prints the line it makes current"
        );
        assert_eq!(ed(FIVE, "1,2jx"), "?\nsomething after the command\n");
        assert_eq!(ed(FIVE, "1m$x"), "?\nsomething after the command\n");
    }

    #[test]
    fn w_appending_makes_a_file_not_there() {
        let mut host = Host::new("append");
        let (status, out) = edit(&mut host, None, false, "a\nx\n.\nW g\nW g\nf\nq");
        assert_eq!((status, out.as_str()), (0, "2\n2\ng\n"));
        assert_eq!(host.holds("g").as_deref(), Some("x\nx\n"));
        let (status, out) = edit(&mut host, Some("g"), true, "1W g\nq");
        assert_eq!(
            (status, out.as_str()),
            (0, ""),
            "the buffer written whole, if after more"
        );
        assert_eq!(host.holds("g").as_deref(), Some("x\nx\nx\n"));
    }

    #[test]
    fn a_file_not_read_leaves_the_buffer_as_it_was() {
        let mut host = Host::new("r-nope");
        host.put("f", FIVE);
        host.typed = true;
        let (status, out) = edit(&mut host, Some("f"), true, "H\nr nope\n.=\nf\nq");
        assert_eq!(
            (status, out.as_str()),
            (
                1,
                "nope: no such file\n?\nthe file could not be read\n5\nf\n"
            ),
            "unchanged, so q needs no second"
        );
        let (_, out) = edit(&mut host, Some("f"), false, "r !printf 'a\\nb'\n$=\nQ");
        assert_eq!(
            out, "24\n3\n7\n",
            "a command's last line needs no line feed either"
        );
    }

    #[test]
    fn u_after_u_after_a_global_command_does_it_again() {
        assert_eq!(ed(FIVE, "g/o/d\nu\nu\n,p"), "three\nfive\n");
        assert_eq!(ed(FIVE, "g/o/d\nu\nu\nu\n,p"), FIVE);
    }

    #[test]
    fn text_changes_an_empty_buffer_and_the_whole() {
        assert_eq!(ed(FIVE, ",c\n.\n.=\n$="), "0\n0\n");
        assert_eq!(ed(FIVE, ",c\nall\n.\n,n"), "1\tall\n");
        assert_eq!(ed("", "i\nx\n.\n,p"), "x\n", "before the line 0 there is");
        assert_eq!(ed("", "a\nx\n.\n,p"), "x\n");
        assert_eq!(ed("", "0r f\n.="), "0\n", "an empty file reads as no lines");
    }

    #[test]
    fn ed_given_no_file_starts_empty_and_unnamed() {
        let mut host = Host::new("unnamed-start");
        let (status, out) = edit(&mut host, None, false, "H\n=\n$=\nf");
        assert_eq!((status, out.as_str()), (1, "0\n0\n?\nno file named\n"));
        let (status, out) = edit(&mut host, None, false, "a\nx\n.\nw\nq");
        assert_eq!((status, out.as_str()), (1, "?\n"), "nor writes to none");
        assert_eq!(std::fs::read_dir(&host.root).unwrap().count(), 0);
    }
}
