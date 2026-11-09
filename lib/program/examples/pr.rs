//! pr: writes files cut into pages, as POSIX's pr does. A page is 66
//! lines, or as many as `-l` says: a header of five lines naming the
//! page, the file and when it last changed, the text, and five blank
//! lines, or with `-F` and `-f` a form feed, to end it; `-t`, or a page of
//! ten lines or fewer, leaves the header and the end out. A file goes in
//! one column, or in as many as `-column` says, filled down with the last
//! page's columns made even, or across with `-a`; `-m` puts the files side
//! by side, a column each. Columns are cut to fit a line of 72 columns,
//! 512 with `-s`, or `-w`'s, and are parted by blanks or by `-s`'s
//! character. `+page` starts at a page, `-d` puts a blank line after each,
//! `-e` turns tabs into blanks in what is read and `-i` blanks into tabs in
//! what is written, both taken for columns, `-n` numbers the lines, `-o`
//! puts blanks before them, `-h` names the file in the header otherwise,
//! `-p` waits for Enter before each page written to the console and `-f`
//! before the first, and `-r` says nothing of a file that cannot be read.
//! Times are in UTC, as Meridian knows no zones yet. It ends with 0, or 1
//! when a file could not be read or a word cannot be taken.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::date::{self, Moment};
use program::text::{self, LineReader, Pieces, Source};
use program::{Error, Handle};

program::main!(pr);

/// How the lines of a page are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// Down each column in turn.
    Down,
    /// Across the columns, a line each, as `-a` asks.
    Across,
    /// Each file in a column of its own, as `-m` asks.
    Merged,
}

/// When pr waits for Enter at the console.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pause {
    #[default]
    Never,
    /// Before the first page, as `-f` asks.
    First,
    /// Before each page, as `-p` asks.
    Every,
}

/// How pages are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Paging {
    /// The lines of a page.
    length: usize,
    /// Whether a page has neither header nor end.
    bare: bool,
    /// Whether a page ends in a form feed rather than blank lines.
    feed: bool,
    pause: Pause,
    /// The first page written, counted from 1.
    first: u64,
}

/// A character and a number, as `-e`, `-i` and `-n` take them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mark {
    mark: char,
    width: usize,
}

/// What pr's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    columns: usize,
    layout: Layout,
    /// Whether a blank line follows each line.
    double: bool,
    /// Tabs in what is read, turned into blanks.
    expand: Option<Mark>,
    /// Blanks in what is written, turned into tabs.
    squeeze: Option<Mark>,
    numbers: Option<Mark>,
    /// What the header names in place of the file.
    header: Option<&'w str>,
    offset: usize,
    separator: Option<char>,
    /// The columns of a line, for text in columns.
    width: usize,
    paging: Paging,
    /// Whether a file that cannot be read is left unsaid.
    quiet: bool,
    files: &'w [String],
}

/// Tabs every eight columns, as `-e` and `-i` have them unless told.
const TABS: Mark = Mark {
    mark: '\t',
    width: 8,
};

/// A number above 0 that `text` is.
fn positive(text: &str) -> Option<usize> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    text.parse().ok().filter(|&n| digits && n > 0)
}

/// The mark and number `-e`, `-i` or `-n` takes from the rest of its word,
/// `text`: a first character that is no digit is the mark, and the digits
/// after it the number, `default` when there are none.
fn marked(letter: char, text: &str, default: usize) -> Result<Mark, String> {
    let wrong = || format!("pr: -{letter}{text}: takes a character and a number\n");
    let (mark, digits) = match text.chars().next() {
        Some(c) if !c.is_ascii_digit() => (c, &text[c.len_utf8()..]),
        _ => ('\t', text),
    };
    if letter != 'n' && !mark.is_ascii() {
        return Err(wrong());
    }
    let width = match digits {
        "" => default,
        _ if !digits.bytes().all(|b| b.is_ascii_digit()) => return Err(wrong()),
        // A gap of 0 is the gap of 8, as POSIX has it.
        _ if letter != 'n' && digits.bytes().all(|b| b == b'0') => default,
        _ => positive(digits).ok_or_else(wrong)?,
    };
    Ok(Mark { mark, width })
}

/// Takes what the option `letter` is given: `after`, the rest of its
/// word, or for `-h`, `-l`, `-o` and `-w` with nothing after the word
/// `next` gives.
fn took<'w>(
    asked: &mut Asked<'w>,
    letter: char,
    after: &'w str,
    width: &mut Option<usize>,
    next: impl FnOnce() -> Option<&'w str>,
) -> Result<(), String> {
    match letter {
        'e' => asked.expand = Some(marked(letter, after, 8)?),
        'i' => asked.squeeze = Some(marked(letter, after, 8)?),
        'n' => asked.numbers = Some(marked(letter, after, 5)?),
        's' => {
            let mut chars = after.chars();
            asked.separator = Some(match (chars.next(), chars.next()) {
                (None, _) => '\t',
                (Some(c), None) => c,
                _ => return Err(format!("pr: -s{after}: takes one character\n")),
            });
        }
        _ => {
            let value = match after {
                "" => next().ok_or_else(|| format!("pr: -{letter} wants a value after it\n"))?,
                after => after,
            };
            match letter {
                'h' => asked.header = Some(value),
                'o' => {
                    asked.offset = value
                        .parse()
                        .ok()
                        .filter(|_| value.bytes().all(|b| b.is_ascii_digit()))
                        .ok_or_else(|| format!("pr: -o {value}: blanks are a number\n"))?;
                }
                _ => {
                    let n = positive(value)
                        .ok_or_else(|| format!("pr: -{letter} {value}: a number above 0\n"))?;
                    if letter == 'l' {
                        asked.paging.length = n;
                    } else {
                        *width = Some(n);
                    }
                }
            }
        }
    }
    Ok(())
}

/// What pr's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        columns: 1,
        layout: Layout::Down,
        double: false,
        expand: None,
        squeeze: None,
        numbers: None,
        header: None,
        offset: 0,
        separator: None,
        width: 0,
        paging: Paging {
            length: 66,
            bare: false,
            feed: false,
            pause: Pause::Never,
            first: 1,
        },
        quiet: false,
        files: &[],
    };
    let (mut across, mut merged, mut width) = (false, false, None);
    let mut at = 0;
    while let Some(word) = arguments.get(at) {
        if word == "--" {
            at += 1;
            break;
        }
        if let Some(page) = word.strip_prefix('+') {
            asked.paging.first = positive(page)
                .ok_or_else(|| format!("pr: +{page}: a page is a number above 0\n"))?
                as u64;
            at += 1;
            continue;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        at += 1;
        let mut i = 0;
        while let Some(letter) = letters[i..].chars().next() {
            let after = &letters[i + letter.len_utf8()..];
            // Most letters stand alone; the rest take what follows them.
            i += letter.len_utf8();
            match letter {
                '0'..='9' => {
                    // Digits among the letters say how many columns.
                    let start = i - 1;
                    i = letters[start..]
                        .find(|c: char| !c.is_ascii_digit())
                        .map_or(letters.len(), |n| start + n);
                    let digits = &letters[start..i];
                    asked.columns = positive(digits)
                        .ok_or_else(|| format!("pr: -{digits}: columns are a number above 0\n"))?;
                }
                'a' => across = true,
                'd' => asked.double = true,
                'F' => asked.paging.feed = true,
                'f' => {
                    asked.paging.feed = true;
                    if asked.paging.pause == Pause::Never {
                        asked.paging.pause = Pause::First;
                    }
                }
                'm' => merged = true,
                'p' => asked.paging.pause = Pause::Every,
                'r' => asked.quiet = true,
                't' => asked.paging.bare = true,
                'e' | 'i' | 'n' | 's' | 'h' | 'l' | 'o' | 'w' => {
                    i = letters.len();
                    took(&mut asked, letter, after, &mut width, || {
                        let next = arguments.get(at).map(String::as_str);
                        at += usize::from(next.is_some());
                        next
                    })?;
                }
                _ => return Err(format!("pr: -{letter} is no option\n")),
            }
        }
    }
    asked.files = &arguments[at..];
    settled(asked, merged, across, width)
}

/// `asked` with what its options together say worked out: the layout,
/// whether a page has room for a header, the tabs columns take, and the
/// width; or why they cannot go together.
fn settled(
    mut asked: Asked<'_>,
    merged: bool,
    across: bool,
    width: Option<usize>,
) -> Result<Asked<'_>, String> {
    if merged && asked.columns > 1 {
        return Err(String::from(
            "pr: -m puts each file in a column, so it takes no -column\n",
        ));
    }
    asked.layout = match (merged, across) {
        (true, _) => Layout::Merged,
        (false, true) => Layout::Across,
        (false, false) => Layout::Down,
    };
    // A page with no room for both is written as -t has it.
    asked.paging.bare |= asked.paging.length <= 10;
    if asked.in_columns() {
        asked.expand = asked.expand.or(Some(TABS));
        asked.squeeze = asked.squeeze.or(Some(TABS));
    }
    asked.width = width.unwrap_or(if asked.separator.is_some() { 512 } else { 72 });
    if asked.in_columns() && asked.column_width() == 0 {
        return Err(format!(
            "pr: a line of {} columns leaves no room for {} columns of text\n",
            asked.width,
            asked.across()
        ));
    }
    Ok(asked)
}

impl Asked<'_> {
    /// How many columns a page's text goes in.
    fn across(&self) -> usize {
        match self.layout {
            Layout::Merged => self.files.len().max(1),
            _ => self.columns,
        }
    }

    /// Whether text goes in columns that are cut to fit.
    fn in_columns(&self) -> bool {
        self.layout == Layout::Merged || self.columns > 1
    }

    /// The columns before each line of `-m`'s that its number takes, with
    /// the character after it.
    fn lead(&self) -> usize {
        match (self.layout, self.numbers) {
            (Layout::Merged, Some(n)) if n.mark == '\t' => (n.width / 8 + 1) * 8,
            (Layout::Merged, Some(n)) => n.width + 1,
            _ => 0,
        }
    }

    /// How wide each column of text is: the line's width less what parts
    /// the columns, shared out.
    fn column_width(&self) -> usize {
        let parts = self.across() - 1;
        self.width.saturating_sub(self.lead() + parts) / self.across()
    }

    /// The lines of a page past its header and end.
    fn body(&self) -> usize {
        if self.paging.bare {
            self.paging.length
        } else {
            self.paging.length - 10
        }
    }

    /// The lines of text a page holds before `-d`'s blanks go after
    /// them: half its lines with them, and one at least.
    fn rows(&self) -> usize {
        if self.double {
            (self.body() / 2).max(1)
        } else {
            self.body()
        }
    }
}

/// Whether `b` goes on a character that started before it.
fn continues(b: u8) -> bool {
    b & 0xc0 == 0x80
}

/// The columns `text` takes, a character each.
fn width_of(text: &[u8]) -> usize {
    text.iter().filter(|&&b| !continues(b)).count()
}

/// `text` cut to its first `columns` characters.
fn cut(text: &mut Vec<u8>, columns: usize) {
    let mut seen = 0;
    let end = text.iter().position(|&b| {
        if continues(b) {
            return false;
        }
        seen += 1;
        seen > columns
    });
    if let Some(end) = end {
        text.truncate(end);
    }
}

/// `text` with each of `tabs`' mark turned into blanks up to the next
/// multiple of its width, counted from the start of `text`.
fn expanded(text: &[u8], tabs: Mark) -> Vec<u8> {
    let mut out = Vec::new();
    let mut column = 0;
    for &b in text {
        if char::from(b) == tabs.mark {
            let blanks = tabs.width - column % tabs.width;
            out.resize(out.len() + blanks, b' ');
            column += blanks;
        } else {
            out.push(b);
            column += usize::from(!continues(b));
        }
    }
    out
}

/// The column `text` ends at when it starts at `column`: a tab goes on to
/// the next multiple of eight, as a terminal has it.
fn reached(text: &[u8], mut column: usize) -> usize {
    for &b in text {
        if b == b'\t' {
            column = (column / 8 + 1) * 8;
        } else if !continues(b) {
            column += 1;
        }
    }
    column
}

/// `line`, starting at `column`, with each run of two blanks or more
/// written as `tabs`' mark at each multiple of its width the run reaches,
/// the rest of it as blanks; a blank alone stays.
fn squeezed(line: &[u8], tabs: Mark, mut column: usize) -> Vec<u8> {
    let mut mark = [0u8; 4];
    let mark = tabs.mark.encode_utf8(&mut mark).as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&b) = line.get(at) {
        if b == b' ' {
            let run = line[at..].iter().take_while(|&&c| c == b' ').count();
            let end = column + run;
            if run >= 2 {
                loop {
                    let stop = (column / tabs.width + 1) * tabs.width;
                    if stop > end {
                        break;
                    }
                    out.extend_from_slice(mark);
                    column = stop;
                }
            }
            out.resize(out.len() + end - column, b' ');
            column = end;
            at += run;
            continue;
        }
        out.push(b);
        column = reached(&[b], column);
        at += 1;
    }
    out
}

/// `count` in `width` columns as `-n` writes it, its last digits when it
/// takes more, and the mark after it.
fn numbered(count: u64, numbers: Mark) -> Vec<u8> {
    let digits = format!("{count:>width$}", width = numbers.width);
    let shown = &digits[digits.len() - numbers.width..];
    format!("{shown}{}", numbers.mark).into_bytes()
}

/// The columns of text a page holds, laid out as `asked` says into the
/// lines they make: `cells` down the columns, the last page's made as
/// even as they go with the first ones taking a line more, or across.
fn laid_out(asked: &Asked, cells: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let across = asked.columns;
    if across == 1 {
        return cells.to_vec();
    }
    let rows = cells.len().div_ceil(across);
    let mut lines = Vec::new();
    for row in 0..rows {
        let picked: Vec<Option<&[u8]>> = (0..across)
            .map(|column| {
                let at = if asked.layout == Layout::Across {
                    Some(row * across + column)
                } else {
                    let (each, more) = (cells.len() / across, cells.len() % across);
                    let height = each + usize::from(column < more);
                    let start = column * each + column.min(more);
                    (row < height).then_some(start + row)
                };
                at.and_then(|at| cells.get(at)).map(Vec::as_slice)
            })
            .collect();
        lines.push(joined(asked, &picked, asked.offset));
    }
    lines
}

/// The columns of one line put side by side from `column` on, each but
/// the last filled out to its width and parted from the next, or parted
/// by `-s`'s character alone; none after the last that holds text. Each
/// column has its blanks made tabs on its own, as GNU's pr has it.
fn joined(asked: &Asked, cells: &[Option<&[u8]>], mut column: usize) -> Vec<u8> {
    let mut line = Vec::new();
    let Some(last) = cells.iter().rposition(Option::is_some) else {
        return line;
    };
    let width = asked.column_width();
    for (at, cell) in cells[..=last].iter().enumerate() {
        let mut field = Vec::from(cell.unwrap_or(&[]));
        if at < last {
            if let Some(c) = asked.separator {
                let mut room = [0u8; 4];
                field.extend_from_slice(c.encode_utf8(&mut room).as_bytes());
            } else {
                let blanks = width.saturating_sub(width_of(&field)) + 1;
                field.resize(field.len() + blanks, b' ');
            }
        }
        match asked.squeeze {
            Some(tabs) => line.extend(squeezed(&field, tabs, column)),
            None => line.extend_from_slice(&field),
        }
        column = reached(&field, column);
    }
    line
}

/// What pr does with files and its output.
trait Files {
    /// What a file is read from.
    type Input: Pieces;

    /// The file at `path`, or the standard input for `-`.
    fn open(&mut self, path: &str) -> Result<Self::Input, Error>;

    /// Closes what `open` gave.
    fn close(&mut self, input: Self::Input);

    /// When the file at `path` last changed, as seconds since 1970 began;
    /// none for the standard input and what keeps no time.
    fn changed(&mut self, path: &str) -> Option<u64>;

    /// The time of day, as seconds since 1970 began; none when not known.
    fn now(&mut self) -> Option<u64>;

    /// Writes `bytes` on the standard output.
    fn out(&mut self, bytes: &[u8]);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);

    /// Rings and waits for Enter, when the standard output is the console.
    fn pause(&mut self);
}

/// Pages being written: what makes them, and how far they have come.
struct Printer<'a, F> {
    files: &'a mut F,
    asked: &'a Asked<'a>,
    /// Whether a page was written yet.
    begun: bool,
}

impl<F: Files> Printer<'_, F> {
    /// What the header says before the page's number: when, and of what.
    fn title(&mut self, path: Option<&str>) -> String {
        let at = path
            .filter(|&p| p != "-")
            .and_then(|p| self.files.changed(p))
            .or_else(|| self.files.now())
            .unwrap_or(0);
        let name = match (self.asked.header, path) {
            (Some(header), _) => header,
            (None, Some(path)) if path != "-" => path,
            _ => "",
        };
        format!(
            "{} {name}",
            date::formatted("%b %e %H:%M %Y", &Moment::at(at))
        )
    }

    /// Writes `line` as an output line: after the offset, with blanks
    /// made tabs as `-i` says, which columns had already.
    fn line(&mut self, line: &[u8]) {
        let mut out = Vec::new();
        if !line.is_empty() {
            out.resize(self.asked.offset, b' ');
        }
        out.extend_from_slice(line);
        if let Some(tabs) = self.asked.squeeze.filter(|_| !self.asked.in_columns()) {
            out = squeezed(&out, tabs, 0);
        }
        out.push(b'\n');
        self.files.out(&out);
    }

    /// Writes page `number` of `title` holding `lines`, unless it comes
    /// before the first page asked for.
    /// With `-d` each line has a blank one after it, the last left out
    /// when the page has no room for it, as GNU's pr has it.
    fn page(&mut self, title: &str, number: u64, lines: &[Vec<u8>]) {
        if number < self.asked.paging.first {
            return;
        }
        let mut doubled = Vec::new();
        let lines = if self.asked.double {
            for line in lines {
                doubled.extend([line.clone(), Vec::new()]);
            }
            doubled.truncate(self.asked.body());
            &doubled
        } else {
            lines
        };
        let pause = match self.asked.paging.pause {
            Pause::Never => false,
            Pause::First => !self.begun,
            Pause::Every => true,
        };
        if pause {
            self.files.pause();
        }
        self.begun = true;
        let bare = self.asked.paging.bare;
        if !bare {
            self.files.out(b"\n\n");
            self.line(format!("{title} Page {number}").as_bytes());
            self.files.out(b"\n\n");
        }
        for line in lines {
            self.line(line);
        }
        if bare {
            return;
        }
        if self.asked.paging.feed {
            self.files.out(b"\x0c");
        } else {
            let blanks = self.asked.body() - lines.len() + 5;
            self.files.out(&alloc::vec![b'\n'; blanks]);
        }
    }

    /// Writes the file at `path` read from `input` cut into pages; whether
    /// it could all be read.
    fn file(&mut self, path: &str, input: F::Input) -> bool {
        let title = self.title(Some(path));
        let mut reader = LineReader::new(input);
        let asked = self.asked;
        let room = asked.rows() * asked.columns;
        let mut cells = Vec::new();
        let (mut count, mut number) = (0, 0);
        let mut went = true;
        loop {
            match reader.line() {
                Ok(Some(text)) => {
                    let mut cell = Vec::new();
                    if let Some(numbers) = asked.numbers {
                        count += 1;
                        cell = numbered(count, numbers);
                    }
                    cell.extend_from_slice(&text);
                    if let Some(tabs) = asked.expand {
                        cell = expanded(&cell, tabs);
                    }
                    if asked.in_columns() {
                        cut(&mut cell, asked.column_width());
                    }
                    cells.push(cell);
                }
                Ok(None) => break,
                Err(e) => {
                    self.files.say(&text::trouble("pr", path, e));
                    went = false;
                    break;
                }
            }
            while cells.len() >= room {
                let page: Vec<Vec<u8>> = cells.drain(..room).collect();
                number += 1;
                let lines = laid_out(asked, &page);
                self.page(&title, number, &lines);
            }
        }
        if !cells.is_empty() {
            let lines = laid_out(asked, &cells);
            self.page(&title, number + 1, &lines);
        }
        self.files.close(reader.into_pieces());
        went
    }

    /// Writes the files `asked` names side by side, a column each, cut into
    /// pages; whether they could all be read.
    fn merged(&mut self) -> bool {
        let asked = self.asked;
        let standard = [String::from("-")];
        let paths = if asked.files.is_empty() {
            &standard[..]
        } else {
            asked.files
        };
        let mut went = true;
        let mut readers: Vec<Option<LineReader<F::Input>>> = Vec::new();
        for path in paths {
            match self.files.open(path) {
                Ok(input) => readers.push(Some(LineReader::new(input))),
                Err(e) => {
                    if !asked.quiet {
                        self.files.say(&text::trouble("pr", path, e));
                    }
                    went = false;
                    readers.push(None);
                }
            }
        }
        let title = self.title(None);
        let width = asked.column_width();
        let (mut lines, mut count, mut number) = (Vec::new(), 0, 0);
        loop {
            let mut cells: Vec<Option<Vec<u8>>> = Vec::new();
            for (path, reader) in paths.iter().zip(&mut readers) {
                let Some(open) = reader else {
                    cells.push(None);
                    continue;
                };
                match open.line() {
                    Ok(Some(text)) => {
                        let mut cell = expanded(&text, asked.expand.unwrap_or(TABS));
                        cut(&mut cell, width);
                        cells.push(Some(cell));
                    }
                    Ok(None) => {
                        cells.push(None);
                        if let Some(done) = reader.take() {
                            self.files.close(done.into_pieces());
                        }
                    }
                    Err(e) => {
                        self.files.say(&text::trouble("pr", path, e));
                        went = false;
                        cells.push(None);
                        if let Some(done) = reader.take() {
                            self.files.close(done.into_pieces());
                        }
                    }
                }
            }
            if cells.iter().all(Option::is_none) {
                break;
            }
            let picked: Vec<Option<&[u8]>> = cells.iter().map(Option::as_deref).collect();
            let mut line = Vec::new();
            if let Some(numbers) = asked.numbers {
                count += 1;
                line = expanded(&numbered(count, numbers), TABS);
            }
            let start = asked.offset + width_of(&line);
            if let Some(tabs) = asked.squeeze {
                line = squeezed(&line, tabs, asked.offset);
            }
            line.extend(joined(asked, &picked, start));
            lines.push(line);
            while lines.len() >= asked.rows() {
                let page: Vec<Vec<u8>> = lines.drain(..asked.rows()).collect();
                number += 1;
                self.page(&title, number, &page);
            }
        }
        if !lines.is_empty() {
            self.page(&title, number + 1, &lines);
        }
        went
    }
}

/// Writes what `asked` names cut into pages; whether everything could be
/// read.
fn print(files: &mut impl Files, asked: &Asked) -> bool {
    let mut printer = Printer {
        files,
        asked,
        begun: false,
    };
    if asked.layout == Layout::Merged {
        return printer.merged();
    }
    let standard = [String::from("-")];
    let paths = if asked.files.is_empty() {
        &standard[..]
    } else {
        asked.files
    };
    let mut went = true;
    for path in paths {
        match printer.files.open(path) {
            Ok(input) => went &= printer.file(path, input),
            Err(e) => {
                if !asked.quiet {
                    printer.files.say(&text::trouble("pr", path, e));
                }
                went = false;
            }
        }
    }
    went
}

/// Meridian's files and the program's streams.
struct System {
    out: text::Out<text::Write>,
}

impl Files for System {
    type Input = Source;

    fn open(&mut self, path: &str) -> Result<Source, Error> {
        Source::open(path)
    }

    fn close(&mut self, input: Source) {
        input.close();
    }

    fn changed(&mut self, path: &str) -> Option<u64> {
        let file = program::open(path, program::abi::file::READ).ok()?;
        let changed = program::changed(file);
        let _ = program::close(file);
        changed.ok().flatten()
    }

    fn now(&mut self) -> Option<u64> {
        program::now().map(|nanoseconds| nanoseconds / 1_000_000_000)
    }

    fn out(&mut self, bytes: &[u8]) {
        self.out.put(bytes);
    }

    fn say(&mut self, text: &str) {
        self.out.flush();
        let _ = program::write_error(text);
    }

    fn pause(&mut self) {
        if program::output_is_console() {
            self.out.flush();
            let _ = program::write_error("\x07");
            let _ = program::typed_line();
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn pr(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => {
            let mut system = System {
                out: text::standard_output(),
            };
            let went = print(&mut system, &asked);
            let written = system.out.finish();
            u64::from(!(went && written))
        }
        Err(why) => {
            let _ = program::write_error(&why);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::path::PathBuf;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// What pr reads on the host: a file, a piece at a time, or the
    /// standard input's bytes.
    enum Input {
        File(std::fs::File),
        Bytes(Option<Vec<u8>>),
    }

    impl Pieces for Input {
        fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
            match self {
                Input::File(file) => {
                    let mut room = [0u8; 100];
                    let len = file.read(&mut room).map_err(|e| error(&e))?;
                    held.extend_from_slice(&room[..len]);
                    Ok(len > 0)
                }
                Input::Bytes(bytes) => Ok(bytes.take().map(|b| held.extend(b)).is_some()),
            }
        }
    }

    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            std::io::ErrorKind::IsADirectory => Error::NotAFile,
            _ => Error::Unreadable,
        }
    }

    /// Files among the host's temporary ones, gone once dropped; the
    /// standard input, the time of day pr is told, and what it wrote and
    /// said, and where in what it wrote it waited for Enter.
    struct Host {
        root: PathBuf,
        input: Vec<u8>,
        now: Option<u64>,
        out: Vec<u8>,
        said: String,
        pauses: Vec<usize>,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-pr-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                input: Vec::new(),
                now: Some(NOON),
                out: Vec::new(),
                said: String::new(),
                pauses: Vec::new(),
            }
        }

        /// A file at `path` holding `text`, changed at `seconds`.
        fn put(&self, path: &str, text: &str, seconds: u64) {
            let at = self.root.join(path);
            std::fs::write(&at, text).unwrap();
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
            std::fs::File::open(&at)
                .unwrap()
                .set_modified(when)
                .unwrap();
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Files for Host {
        type Input = Input;

        fn open(&mut self, path: &str) -> Result<Input, Error> {
            if path == "-" {
                return Ok(Input::Bytes(Some(core::mem::take(&mut self.input))));
            }
            assert!(
                !path.starts_with('/') && !path.contains(".."),
                "{path} stays in the test's directory"
            );
            let file = std::fs::File::open(self.root.join(path)).map_err(|e| error(&e))?;
            Ok(Input::File(file))
        }

        fn close(&mut self, _: Input) {}

        fn changed(&mut self, path: &str) -> Option<u64> {
            let found = std::fs::metadata(self.root.join(path)).ok()?;
            let since = found.modified().ok()?.duration_since(std::time::UNIX_EPOCH);
            Some(since.ok()?.as_secs())
        }

        fn now(&mut self) -> Option<u64> {
            self.now
        }

        fn out(&mut self, bytes: &[u8]) {
            self.out.extend_from_slice(bytes);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }

        fn pause(&mut self) {
            self.pauses.push(self.out.len());
        }
    }

    /// 12:00 UTC on 9 November 2026, a Monday.
    const NOON: u64 = 1_794_225_600;

    /// How pr with the words `line` ran on `host`: whether all could be
    /// read, what it wrote and what it said.
    fn ran(host: &mut Host, line: &str) -> (bool, String, String) {
        let given = words(line);
        let taken = asked(&given).unwrap();
        let went = print(host, &taken);
        let out = String::from_utf8(core::mem::take(&mut host.out)).unwrap();
        (went, out, core::mem::take(&mut host.said))
    }

    fn wrote(text: &str) -> (bool, String, String) {
        (true, text.into(), String::new())
    }

    /// The page `number` of `name` holding `body`, with `blanks` blank
    /// lines after it before the five of the end.
    fn page(name: &str, number: u64, body: &str, blanks: usize) -> String {
        format!(
            "\n\nNov  9 12:00 2026 {name} Page {number}\n\n\n{body}{}",
            "\n".repeat(blanks + 5)
        )
    }

    #[test]
    fn its_words_are_options_then_files() {
        let given = words("a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                columns: 1,
                layout: Layout::Down,
                double: false,
                expand: None,
                squeeze: None,
                numbers: None,
                header: None,
                offset: 0,
                separator: None,
                width: 72,
                paging: Paging {
                    length: 66,
                    bare: false,
                    feed: false,
                    pause: Pause::Never,
                    first: 1,
                },
                quiet: false,
                files: &given[..],
            })
        );
        let given = words("+3 -3adFrt -htitle -l 20 -o4 -w 40 -n:3 -e:4 -i -s; x");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                columns: 3,
                layout: Layout::Across,
                double: true,
                expand: Some(Mark {
                    mark: ':',
                    width: 4
                }),
                squeeze: Some(TABS),
                numbers: Some(Mark {
                    mark: ':',
                    width: 3
                }),
                header: Some("title"),
                offset: 4,
                separator: Some(';'),
                width: 40,
                paging: Paging {
                    length: 20,
                    bare: true,
                    feed: true,
                    pause: Pause::Never,
                    first: 3,
                },
                quiet: true,
                files: &given[given.len() - 1..],
            })
        );
    }

    /// What pr takes the words `line` for.
    fn taken(line: &str) -> Asked<'static> {
        asked(Vec::leak(words(line))).unwrap()
    }

    #[test]
    fn options_take_marks_numbers_and_characters() {
        assert_eq!(taken("-p -f").paging.pause, Pause::Every);
        assert_eq!(taken("-f -p").paging.pause, Pause::Every);
        assert_eq!(
            (taken("-f").paging.pause, taken("-f").paging.feed),
            (Pause::First, true)
        );
        assert_eq!(taken("-12").columns, 12);
        assert_eq!(
            (taken("-2a3").columns, taken("-2a3").layout),
            (3, Layout::Across),
            "the last digits count"
        );
    }

    #[test]
    fn marks_and_numbers_come_from_the_rest_of_the_word() {
        for (line, mark) in [
            ("-e", TABS),
            ("-e0", TABS),
            (
                "-e4",
                Mark {
                    mark: '\t',
                    width: 4,
                },
            ),
            (
                "-ex",
                Mark {
                    mark: 'x',
                    width: 8,
                },
            ),
            (
                "-e:12",
                Mark {
                    mark: ':',
                    width: 12,
                },
            ),
        ] {
            assert_eq!(taken(line).expand, Some(mark), "{line}");
        }
        assert_eq!(
            taken("-i:").squeeze,
            Some(Mark {
                mark: ':',
                width: 8
            })
        );
        for (line, mark) in [
            (
                "-n",
                Mark {
                    mark: '\t',
                    width: 5,
                },
            ),
            (
                "-n3",
                Mark {
                    mark: '\t',
                    width: 3,
                },
            ),
            (
                "-nx12",
                Mark {
                    mark: 'x',
                    width: 12,
                },
            ),
            (
                "-n\u{e9}2",
                Mark {
                    mark: '\u{e9}',
                    width: 2,
                },
            ),
        ] {
            assert_eq!(taken(line).numbers, Some(mark), "{line}");
        }
        assert_eq!(
            (taken("-s").separator, taken("-s").width),
            (Some('\t'), 512)
        );
        assert_eq!(
            (taken("-s:").separator, taken("-w 30 -s:").width),
            (Some(':'), 30)
        );
    }

    #[test]
    fn columns_and_merged_files_take_tabs_and_a_width() {
        let columns = taken("-2 -e4");
        assert_eq!(
            (columns.expand, columns.squeeze),
            (
                Some(Mark {
                    mark: '\t',
                    width: 4
                }),
                Some(TABS)
            ),
            "columns take -e and -i as they are given, or as tabs every eight"
        );
        let merged = taken("-m a b");
        assert_eq!(
            (
                merged.layout,
                merged.expand,
                merged.squeeze,
                merged.across()
            ),
            (Layout::Merged, Some(TABS), Some(TABS), 2)
        );
        assert_eq!(taken("-m").across(), 1, "the standard input alone");
        assert!(taken("-l 10").paging.bare);
        assert!(!taken("-l 11").paging.bare);
        assert_eq!(taken("-- -t").files, ["-t"]);
        assert_eq!(taken("- a").files, ["-", "a"]);
        assert_eq!(taken("-36").column_width(), 1);
    }

    #[test]
    fn words_it_cannot_take_are_said() {
        for (line, why) in [
            ("-x", "pr: -x is no option\n"),
            ("-h", "pr: -h wants a value after it\n"),
            ("-l", "pr: -l wants a value after it\n"),
            ("-o", "pr: -o wants a value after it\n"),
            ("-w", "pr: -w wants a value after it\n"),
            ("-l 0", "pr: -l 0: a number above 0\n"),
            ("-w x", "pr: -w x: a number above 0\n"),
            ("-o -1", "pr: -o -1: blanks are a number\n"),
            ("+0", "pr: +0: a page is a number above 0\n"),
            ("+", "pr: +: a page is a number above 0\n"),
            ("+x", "pr: +x: a page is a number above 0\n"),
            ("-0", "pr: -0: columns are a number above 0\n"),
            ("-e:x", "pr: -e:x: takes a character and a number\n"),
            ("-e\u{e9}", "pr: -e\u{e9}: takes a character and a number\n"),
            ("-n0", "pr: -n0: takes a character and a number\n"),
            ("-s::", "pr: -s::: takes one character\n"),
            (
                "-m -2 a",
                "pr: -m puts each file in a column, so it takes no -column\n",
            ),
            (
                "-37",
                "pr: a line of 72 columns leaves no room for 37 columns of text\n",
            ),
            (
                "-m -n -w 9 a b",
                "pr: a line of 9 columns leaves no room for 2 columns of text\n",
            ),
        ] {
            assert_eq!(asked(&words(line)).err().as_deref(), Some(why), "{line}");
        }
    }

    #[test]
    fn characters_are_counted_cut_and_their_tabs_and_blanks_turned() {
        assert_eq!(width_of("a\u{e9}\u{20ac}".as_bytes()), 3);
        let mut text = Vec::from("a\u{e9}\u{20ac}b".as_bytes());
        cut(&mut text, 2);
        assert_eq!(
            text,
            "a\u{e9}".as_bytes(),
            "a character goes whole or not at all"
        );
        let mut short = Vec::from(&b"ab"[..]);
        cut(&mut short, 5);
        assert_eq!(short, b"ab");
        cut(&mut short, 0);
        assert_eq!(short, b"");
        for (text, tabs, out) in [
            ("a\tb", TABS, "a       b"),
            (
                "\t",
                Mark {
                    mark: '\t',
                    width: 4,
                },
                "    ",
            ),
            ("12345678\tx", TABS, "12345678        x"),
            ("\u{e9}\tx", TABS, "\u{e9}       x"),
            (
                "a:b",
                Mark {
                    mark: ':',
                    width: 3,
                },
                "a  b",
            ),
            (
                "a\tb",
                Mark {
                    mark: ':',
                    width: 3,
                },
                "a\tb",
            ),
        ] {
            assert_eq!(expanded(text.as_bytes(), tabs), out.as_bytes(), "{text:?}");
        }
    }

    #[test]
    fn blanks_that_reach_a_tab_stop_become_tabs() {
        for (line, tabs, out) in [
            (
                "a       b",
                Mark {
                    mark: '\t',
                    width: 4,
                },
                "a\t\tb",
            ),
            ("1234567 x", TABS, "1234567 x"),
            ("fa0   0  x", TABS, "fa0   0\t x"),
            ("123456      z", TABS, "123456\t    z"),
            ("x\t  y", TABS, "x\t  y"),
            ("        a", TABS, "\ta"),
            ("a  ", TABS, "a  "),
            (
                "    x",
                Mark {
                    mark: ':',
                    width: 4,
                },
                ":x",
            ),
            ("\u{e9}       x", TABS, "\u{e9}\tx"),
        ] {
            assert_eq!(
                squeezed(line.as_bytes(), tabs, 0),
                out.as_bytes(),
                "{line:?}"
            );
        }
    }

    #[test]
    fn numbers_take_their_width_and_their_mark() {
        assert_eq!(
            numbered(
                7,
                Mark {
                    mark: '\t',
                    width: 5
                }
            ),
            b"    7\t"
        );
        assert_eq!(
            numbered(
                100,
                Mark {
                    mark: ':',
                    width: 2
                }
            ),
            b"00:",
            "the last digits"
        );
        assert_eq!(
            numbered(
                123,
                Mark {
                    mark: 'x',
                    width: 30
                }
            ),
            format!("{}123x", " ".repeat(27)).as_bytes(),
            "a width past what powers of ten reach keeps every digit"
        );
    }

    #[test]
    fn columns_are_filled_down_even_or_across() {
        let cells: Vec<Vec<u8>> = (1..=7).map(|n| format!("{n}").into_bytes()).collect();
        let lines = |rows: &[&str]| -> Vec<Vec<u8>> {
            rows.iter().map(|r| r.as_bytes().to_vec()).collect()
        };
        let down = asked(Vec::leak(words("-3"))).unwrap();
        assert_eq!(
            laid_out(&down, &cells),
            lines(&["1\t\t\t4\t\t\t6", "2\t\t\t5\t\t\t7", "3"]),
            "seven lines in three columns of 23 go three, two and two, as GNU's pr has them"
        );
        let across = asked(Vec::leak(words("-3 -a"))).unwrap();
        assert_eq!(
            laid_out(&across, &cells),
            lines(&["1\t\t\t2\t\t\t3", "4\t\t\t5\t\t\t6", "7"])
        );
        let two = asked(Vec::leak(words("-2 -s:"))).unwrap();
        assert_eq!(laid_out(&two, &cells[..4]), lines(&["1:3", "2:4"]));
        let one = asked(Vec::leak(words(""))).unwrap();
        assert_eq!(laid_out(&one, &cells[..2]), cells[..2]);
        let parted = asked(Vec::leak(words("-3 -s:"))).unwrap();
        assert_eq!(
            joined(&parted, &[Some(b"a"), None, Some(b"c"), None], 0),
            b"a::c",
            "an empty column between, and none after the last"
        );
        assert_eq!(
            joined(&down, &[Some(b"a"), None, Some(b"c")], 0),
            b"a\t\t\t\t\t\tc",
            "an empty column is all blanks, made tabs"
        );
        assert_eq!(
            joined(&down, &[Some(b"a"), Some(b"b")], 4),
            b"a\t\t\t    b",
            "tabs go where the stops are from where the line starts"
        );
        let halves = asked(Vec::leak(words("-2"))).unwrap();
        let long = "x".repeat(33);
        assert_eq!(
            joined(&halves, &[Some(long.as_bytes()), Some(b"    b")], 0),
            format!("{long}   \tb").into_bytes(),
            "each column of 35 and the blank after it has its blanks made tabs on its own"
        );
        assert_eq!(joined(&down, &[None, None], 0), b"");
        assert_eq!(reached(b"a\tb", 0), 9);
        assert_eq!(reached("\u{e9}\t".as_bytes(), 6), 8);
    }

    #[test]
    fn pages_have_a_header_their_text_and_an_end() {
        let mut host = Host::new("pages");
        host.put("three", "1\n2\n3\n", NOON);
        host.put("five", "one\ntwo\nthree\nfour\nfive\n", NOON);
        assert_eq!(
            ran(&mut host, "three"),
            wrote(&page("three", 1, "1\n2\n3\n", 53))
        );
        assert_eq!(
            ran(&mut host, "-l 12 three"),
            wrote(&(page("three", 1, "1\n2\n", 0) + &page("three", 2, "3\n", 1)))
        );
        assert_eq!(
            ran(&mut host, "-l 12 -F three"),
            wrote(
                "\n\nNov  9 12:00 2026 three Page 1\n\n\n1\n2\n\x0c\
                 \n\nNov  9 12:00 2026 three Page 2\n\n\n3\n\x0c"
            )
        );
        assert_eq!(
            ran(&mut host, "-t three three"),
            wrote("1\n2\n3\n1\n2\n3\n")
        );
        assert_eq!(
            ran(&mut host, "-l 10 three"),
            wrote("1\n2\n3\n"),
            "no room for a header"
        );
        assert_eq!(
            ran(&mut host, "+2 -l 12 three"),
            wrote(&page("three", 2, "3\n", 1))
        );
        assert_eq!(
            ran(&mut host, "+3 -l 12 three"),
            wrote(""),
            "no page that far"
        );
        assert_eq!(
            ran(&mut host, "-h head -l 12 three"),
            wrote(&(page("head", 1, "1\n2\n", 0) + &page("head", 2, "3\n", 1)))
        );
        assert_eq!(
            ran(&mut host, "-l 13 three five"),
            wrote(
                &(page("three", 1, "1\n2\n3\n", 0)
                    + &page("five", 1, "one\ntwo\nthree\n", 0)
                    + &page("five", 2, "four\nfive\n", 1))
            ),
            "each file counts its pages from 1"
        );
        host.put("empty", "", NOON);
        assert_eq!(ran(&mut host, "empty"), wrote(""));
        assert_eq!(ran(&mut host, "-m empty empty"), wrote(""));
    }

    #[test]
    fn the_standard_input_is_dated_now_and_named_by_nothing() {
        let mut host = Host::new("input");
        host.input = b"x\n".to_vec();
        host.now = Some(NOON + 300);
        let header = "\n\nNov  9 12:05 2026  Page 1\n\n\nx\n\n\n\n\n\n";
        assert_eq!(ran(&mut host, "-l 11"), wrote(header));
        host.input = b"x\n".to_vec();
        assert_eq!(ran(&mut host, "-l 11 -"), wrote(header));
        host.input = b"x".to_vec();
        host.now = None;
        assert_eq!(
            ran(&mut host, "-l 11"),
            wrote("\n\nJan  1 00:00 1970  Page 1\n\n\nx\n\n\n\n\n\n"),
            "with no time of day known, the start of 1970"
        );
    }

    #[test]
    fn lines_are_doubled_moved_numbered_and_their_tabs_turned() {
        let mut host = Host::new("lines");
        host.put("three", "1\n2\n3\n", NOON);
        host.put("e", "a\n\nb\n", NOON);
        host.put("tab", "a\tb\n", NOON);
        host.put("sp", "a       b\n", NOON);
        assert_eq!(ran(&mut host, "-d -t three"), wrote("1\n\n2\n\n3\n\n"));
        assert_eq!(
            ran(&mut host, "-d -l 15 three"),
            wrote(&(page("three", 1, "1\n\n2\n\n", 1) + &page("three", 2, "3\n\n", 3))),
            "a line goes with the blank after it, and the page is filled out all the same"
        );
        assert_eq!(
            ran(&mut host, "-d -l 12 three"),
            wrote(
                &(page("three", 1, "1\n\n", 0)
                    + &page("three", 2, "2\n\n", 0)
                    + &page("three", 3, "3\n\n", 0))
            )
        );
        assert_eq!(
            ran(&mut host, "-o 3 -l 12 three"),
            wrote(
                "\n\n   Nov  9 12:00 2026 three Page 1\n\n\n   1\n   2\n\n\n\n\n\n\
                 \n\n   Nov  9 12:00 2026 three Page 2\n\n\n   3\n\n\n\n\n\n\n"
            )
        );
        assert_eq!(
            ran(&mut host, "-o 2 -t e"),
            wrote("  a\n\n  b\n"),
            "no blanks before an empty line"
        );
        assert_eq!(
            ran(&mut host, "-n -t three"),
            wrote("    1\t1\n    2\t2\n    3\t3\n")
        );
        assert_eq!(
            ran(&mut host, "-n:2 -d -t three"),
            wrote(" 1:1\n\n 2:2\n\n 3:3\n\n")
        );
        assert_eq!(ran(&mut host, "-e4 -t tab"), wrote("a   b\n"));
        assert_eq!(
            ran(&mut host, "-t tab"),
            wrote("a\tb\n"),
            "one column keeps its tabs"
        );
        assert_eq!(ran(&mut host, "-i4 -t sp"), wrote("a\t\tb\n"));
        assert_eq!(ran(&mut host, "-t sp"), wrote("a       b\n"));
    }

    #[test]
    fn columns_and_merged_files_go_side_by_side() {
        let mut host = Host::new("columns");
        host.put("five", "one\ntwo\nthree\nfour\nfive\n", NOON);
        host.put("g", "x\nyy\n", NOON);
        host.put(
            "t",
            "a\tb\nlonger line here that goes past the column\nc\n",
            NOON,
        );
        let tabs = "\t\t\t\t    ";
        assert_eq!(
            ran(&mut host, "-2 -t five"),
            wrote(&format!("one{tabs}four\ntwo{tabs}five\nthree\n"))
        );
        assert_eq!(
            ran(&mut host, "-2 -d -t five"),
            wrote(&format!("one{tabs}four\n\ntwo{tabs}five\n\nthree\n\n")),
            "a blank after each line written, not in the columns"
        );
        assert_eq!(
            ran(&mut host, "-2 -a -t five"),
            wrote(&format!("one{tabs}two\nthree{tabs}four\nfive\n"))
        );
        assert_eq!(
            ran(&mut host, "-2 -s -t five"),
            wrote("one\tfour\ntwo\tfive\nthree\n")
        );
        assert_eq!(ran(&mut host, "-3 -t -w 30 t"), wrote("a\tb longer li c\n"));
        assert_eq!(
            ran(&mut host, "-m -t five g"),
            wrote(&format!("one{tabs}x\ntwo{tabs}yy\nthree\nfour\nfive\n"))
        );
        assert_eq!(
            ran(&mut host, "-m -l 11 g g"),
            wrote(&format!(
                "\n\nNov  9 12:00 2026  Page 1\n\n\nx{tabs}x\n\n\n\n\n\n\
                 \n\nNov  9 12:00 2026  Page 2\n\n\nyy{tabs}yy\n\n\n\n\n\n"
            )),
            "merged files are dated now and named by nothing"
        );
        assert_eq!(
            ran(&mut host, "-m -n -t -w 30 five g"),
            wrote("    1\tone\t   x\n    2\ttwo\t   yy\n    3\tthree\n    4\tfour\n    5\tfive\n"),
            "the number goes before the columns, as GNU's pr has it"
        );
        assert_eq!(
            ran(&mut host, "-m -n:3 -t -w 30 five g"),
            wrote("  1:one\t\t x\n  2:two\t\t yy\n  3:three\n  4:four\n  5:five\n"),
            "a number with a mark of its own takes its width and one more"
        );
        assert_eq!(
            ran(&mut host, "-m -h both -l 11 -d g"),
            wrote(&(page("both", 1, "x\n", 0) + &page("both", 2, "yy\n", 0))),
            "a page of one line has no room for the blank after it, as GNU's pr has it"
        );
    }

    #[test]
    fn p_and_f_wait_before_pages() {
        let mut host = Host::new("pause");
        host.put("three", "1\n2\n3\n", NOON);
        let first = page("three", 1, "1\n2\n", 0).len();
        ran(&mut host, "-p -l 12 three");
        assert_eq!(core::mem::take(&mut host.pauses), [0, first]);
        ran(&mut host, "-f -l 12 three three");
        assert_eq!(
            core::mem::take(&mut host.pauses),
            [0],
            "-f waits before the first page alone"
        );
        ran(&mut host, "-p +2 -l 12 three");
        assert_eq!(
            core::mem::take(&mut host.pauses),
            [0],
            "pages passed over are not waited for"
        );
        ran(&mut host, "-l 12 three");
        assert_eq!(host.pauses, Vec::<usize>::new());
    }

    #[test]
    fn what_cannot_be_read_is_said_unless_r_and_the_rest_written() {
        let mut host = Host::new("unread");
        host.put("three", "1\n2\n3\n", NOON);
        std::fs::create_dir(host.root.join("dir")).unwrap();
        assert_eq!(
            ran(&mut host, "-t nope three"),
            (false, "1\n2\n3\n".into(), "pr: nope: no such file\n".into())
        );
        assert_eq!(
            ran(&mut host, "-r -t nope three"),
            (false, "1\n2\n3\n".into(), String::new())
        );
        assert_eq!(
            ran(&mut host, "-t three dir"),
            (
                false,
                "1\n2\n3\n".into(),
                "pr: dir: a directory, not a file\n".into()
            ),
            "a file that opens and then cannot be read"
        );
        assert_eq!(
            ran(&mut host, "-r -t dir"),
            (
                false,
                String::new(),
                "pr: dir: a directory, not a file\n".into()
            ),
            "-r is quiet only of what does not open"
        );
        assert_eq!(
            ran(&mut host, "-m -t three dir"),
            (
                false,
                "1\n2\n3\n".into(),
                "pr: dir: a directory, not a file\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "-m -t nope three"),
            (
                false,
                "\t\t\t\t    1\n\t\t\t\t    2\n\t\t\t\t    3\n".into(),
                "pr: nope: no such file\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "-m -r -t nope three"),
            (
                false,
                "\t\t\t\t    1\n\t\t\t\t    2\n\t\t\t\t    3\n".into(),
                String::new()
            )
        );
    }

    /// `text` as a terminal shows it, its tabs as the blanks to the next
    /// multiple of eight, so that which blanks two programs write as tabs
    /// counts for nothing, and with the blanks that end its lines taken
    /// off, which GNU's pr leaves where Meridian's writes none, as it does
    /// -s's `:` after a file of -m's that ended, which no text holds; with
    /// `posix` its header lines written as POSIX has them, the date, the
    /// name and the page parted by one blank, and with `masked` the date
    /// left out.
    fn tidied(text: &str, posix: bool, masked: bool) -> String {
        let lines: Vec<String> = text
            .split('\n')
            .map(|line| {
                let shown = String::from_utf8(expanded(line.as_bytes(), TABS)).unwrap();
                let line = shown.trim_end_matches([' ', ':']);
                header(line, posix, masked).unwrap_or_else(|| String::from(line))
            })
            .collect();
        lines.join("\n")
    }

    /// The header line `line`, as POSIX writes it when `posix`, its date
    /// left out when `masked`; none for a line that is no header.
    fn header(line: &str, posix: bool, masked: bool) -> Option<String> {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[indent..];
        let date = rest.get(..17)?;
        if !MONTHS.contains(&date.get(..3)?) {
            return None;
        }
        let (name, page) = rest[17..].rsplit_once(" Page ")?;
        if page.is_empty() || !page.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let name = if posix {
            name.trim()
        } else {
            name.strip_prefix(' ')?
        };
        let date = if masked { "DATE" } else { date };
        Some(format!("{}{date} {name} Page {page}", " ".repeat(indent)))
    }

    /// What GNU's pr writes for the words `line` in `dir`, in the C locale
    /// and as POSIX asks, tidied as [`tidied`] does.
    fn gnu(dir: &std::path::Path, line: &str) -> String {
        let done = std::process::Command::new("pr")
            .args(words(line))
            .current_dir(dir)
            .env("LC_ALL", "C")
            .env("POSIXLY_CORRECT", "1")
            .env("TZ", "UTC")
            .output()
            .unwrap();
        tidied(
            &String::from_utf8(done.stdout).unwrap(),
            true,
            line.contains("-m"),
        )
    }

    /// A text of random lines, some longer than a column, with blanks
    /// and tabs among their characters.
    fn random_text(seed: &mut u64) -> String {
        let mut next = |below: u64| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            usize::try_from(*seed % below).unwrap()
        };
        let characters = b"abcdefgh xyz0123  \t";
        let mut text = String::new();
        for _ in 0..next(140) {
            for _ in 0..next(90) {
                text.push(char::from(characters[next(characters.len() as u64)]));
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn pages_are_what_gnu_s_pr_writes_but_for_posix_s_header() {
        let mut host = Host::new("gnu");
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for round in 0..6 {
            for name in ["a", "b", "c"] {
                host.put(name, &random_text(&mut seed), NOON + round);
            }
            // GNU's -i, which columns take too, counts a tab that is there
            // as eight columns wherever it stands, where a terminal goes on
            // to the next multiple of eight, as Meridian's does; n has no
            // tabs. With -s it miscounts where the later columns stand, and
            // writes tabs that move the text; o and p have no two blanks
            // in a row, which make no tabs.
            host.put("n", &random_text(&mut seed).replace('\t', " "), NOON);
            for name in ["o", "p"] {
                let text = random_text(&mut seed).replace('\t', " ");
                let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
                host.put(name, &words.join(" ").replace(" \n", "\n"), NOON);
            }
            for line in [
                "a",
                "-t a",
                "-l 20 a b",
                "-l 16 -d a",
                "-n a",
                "-n:3 -t a",
                "-e4 -t a",
                "-i -t n",
                "-o 3 a",
                "-o 2 -l 14 -t a",
                "-F -l 13 a",
                "+2 -l 14 a",
                "-h title -l 16 a",
                "-2 a",
                "-2 -t a",
                "-3 -a -t a",
                "-4 -l 25 a",
                "-2 -s. -t o",
                "-3 -s: -t o",
                "-2 -s -t o",
                "-2 -w 40 -t a",
                "-3 -a -w 50 a",
                "-m a b c",
                "-m -t a b",
                "-m -s: -t o p",
                "-m -w 50 -t a b c",
                "-m -n -t a b",
                "-2 -d -l 30 a",
                "-2 -l 20 a",
                "-5 -t a",
            ] {
                host.now = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                );
                let (went, ours, said) = ran(&mut host, line);
                assert_eq!((went, said.as_str()), (true, ""), "{line}");
                let theirs = gnu(&host.root, line);
                let ours = tidied(&ours, false, line.contains("-m"));
                let (mine, gnus): (Vec<&str>, Vec<&str>) =
                    (ours.split('\n').collect(), theirs.split('\n').collect());
                // Where the two first part, as the lines are long.
                let at = mine.iter().zip(&gnus).position(|(a, b)| a != b);
                let at = at.unwrap_or(mine.len().min(gnus.len()));
                assert_eq!(
                    mine.get(at.saturating_sub(1)..=at),
                    gnus.get(at.saturating_sub(1)..=at),
                    "{line}, round {round}, line {at}"
                );
                assert_eq!(mine.len(), gnus.len(), "{line}, round {round}");
            }
        }
    }
}
