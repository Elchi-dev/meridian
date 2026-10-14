use crate::number::{self, Float, Quantity};
use crate::text::{self, Str};
use crate::{Code, Error, Pos};

/// How deep blocks and lists may nest. The root is level 0, so the deepest
/// block or list is at level 64.
pub const MAX_DEPTH: usize = 64;

/// One step through a document. Events come in document order, and every
/// block and list that starts also ends, unless reading stops at an error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event<'a> {
    /// A block starts, with the position of its name. Its items follow, up
    /// to the matching [`Event::End`].
    Block {
        name: &'a str,
        label: Option<Str<'a>>,
        at: Pos,
    },
    /// The innermost open block ends.
    End,
    /// An entry, with the positions of its key and of its value. When the
    /// value is [`Value::List`], the list's elements follow, up to the
    /// matching [`Event::ListEnd`].
    Entry {
        key: &'a str,
        at: Pos,
        value: Value<'a>,
        value_at: Pos,
    },
    /// A value in the innermost open list. When it is a list itself, its
    /// own elements follow, up to the matching [`Event::ListEnd`].
    Element { value: Value<'a>, at: Pos },
    /// The innermost open list ends.
    ListEnd,
}

/// A value of an entry or of a list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Bool(bool),
    /// A bare word other than `true` and `false`, such as `kvm`.
    Word(&'a str),
    Int(i64),
    Float(Float<'a>),
    Quantity(Quantity<'a>),
    /// A quoted string or a text.
    Str(Str<'a>),
    /// A list starts. Its elements are the next events.
    List,
}

/// Where the reader was: what a block needs to be read again from the
/// start of its items.
#[derive(Clone, Copy)]
struct Mark {
    i: usize,
    line: usize,
    line_start: usize,
}

/// An open block or list.
#[derive(Clone, Copy)]
struct Frame {
    /// The block's name or the list's `[`, where errors about the whole
    /// of it point.
    at: Pos,
    list: bool,
    items: Mark,
}

/// Reads a document as a sequence of [`Event`]s, checking it on the way.
/// After an error, or after the last event, the iterator ends.
///
/// Every name is checked against the earlier items of its block by
/// reading the block again from its start, since there is no memory to
/// remember them in. That makes a block with many items quadratic, which
/// settings files never notice.
pub struct Reader<'a> {
    src: &'a [u8],
    /// The longest prefix of `src` that is valid UTF-8. The reader never
    /// steps past it: the byte after it is reported as invalid.
    text: &'a str,
    i: usize,
    line: usize,
    line_start: usize,
    /// The last offset a column was worked out for, and that column, so
    /// long lines are not counted again for each position on them.
    col_cache: (usize, usize),
    frames: [Frame; MAX_DEPTH + 1],
    depth: usize,
    /// An item of the current block, or a value of the current list, has
    /// just ended. Every event sets it.
    after: bool,
    /// Whether names are checked against the earlier items of their block.
    /// The reader that reads a block again for that check does not check.
    check: bool,
    /// Where reading ends early: the item a block is read again for.
    stop: usize,
    done: bool,
}

impl<'a> Reader<'a> {
    /// Starts reading a document. A byte order mark at its start is
    /// skipped; invalid UTF-8 is an error once the reader gets to it.
    #[must_use]
    pub fn new(src: &'a [u8]) -> Reader<'a> {
        let text = src.utf8_chunks().next().map_or("", |chunk| chunk.valid());
        let start = if text.starts_with('\u{feff}') { 3 } else { 0 };
        Reader::starting_at(
            src,
            text,
            Mark {
                i: start,
                line: 1,
                line_start: start,
            },
        )
    }

    fn starting_at(src: &'a [u8], text: &'a str, start: Mark) -> Reader<'a> {
        let root = Frame {
            at: Pos { line: 1, col: 1 },
            list: false,
            items: start,
        };
        Reader {
            src,
            text,
            i: start.i,
            line: start.line,
            line_start: start.line_start,
            col_cache: (start.line_start, 1),
            frames: [root; MAX_DEPTH + 1],
            depth: 0,
            after: false,
            check: true,
            stop: usize::MAX,
            done: false,
        }
    }

    fn step(&mut self) -> Result<Option<Event<'a>>, Error> {
        let frame = self.frames[self.depth];
        if frame.list {
            self.in_list(frame.at)
        } else {
            self.in_block(frame.at)
        }
    }

    fn in_block(&mut self, at: Pos) -> Result<Option<Event<'a>>, Error> {
        if self.after {
            self.end_of_item()?;
        }
        loop {
            self.skip_spaces_and_comment()?;
            match self.peek() {
                None if self.depth == 0 => return Ok(None),
                None => {
                    return Err(Error::new(Code::UnclosedBlock, at));
                }
                Some(b'\n' | b'\r') => self.newline()?,
                Some(b'}') if self.depth == 0 => return Err(self.error(Code::UnexpectedClose)),
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    self.after = true;
                    return Ok(Some(Event::End));
                }
                Some(c) if is_key_start(c) => {
                    if self.i >= self.stop {
                        return Ok(None);
                    }
                    return self.item().map(Some);
                }
                Some(_) => return Err(self.unexpected(Code::UnexpectedCharacter)),
            }
        }
    }

    /// After an item only a comment may follow on its line, unless a `}`
    /// closes the block.
    fn end_of_item(&mut self) -> Result<(), Error> {
        self.skip_spaces_and_comment()?;
        match self.peek() {
            None | Some(b'}') => Ok(()),
            Some(_) if self.at_line_end() => Ok(()),
            Some(_) => Err(self.unexpected(Code::ExpectedNewline)),
        }
    }

    fn item(&mut self) -> Result<Event<'a>, Error> {
        let start = self.i;
        let at = self.pos();
        let name = self.word();
        self.skip_spaces();
        if self.peek() == Some(b'=') {
            if self.check {
                self.check_entry(name, start, at)?;
            }
            self.i += 1;
            self.skip_spaces();
            let value_at = self.pos();
            let value = self.value(false)?;
            return Ok(Event::Entry {
                key: name,
                at,
                value,
                value_at,
            });
        }
        let label = match self.peek() {
            Some(b'"') => Some(self.quoted()?),
            Some(c) if is_key_start(c) => Some(Str::quoted(self.word())),
            _ => None,
        };
        if label.is_some() {
            self.skip_spaces();
        }
        if self.peek() != Some(b'{') {
            let code = match label {
                Some(_) => Code::ExpectedBrace,
                None => Code::ExpectedEquals,
            };
            return Err(self.unexpected(code));
        }
        if self.depth == MAX_DEPTH {
            return Err(Error::new(Code::TooDeep, at));
        }
        if self.check {
            self.check_block(name, label, start, at)?;
        }
        self.i += 1;
        self.push(at, false);
        Ok(Event::Block { name, label, at })
    }

    /// Reads the current block again from the start of its items up to
    /// `stop`, to compare earlier items with the one at `stop`.
    fn reread(&self, stop: usize) -> Reader<'a> {
        let mut earlier = Reader::starting_at(self.src, self.text, self.frames[self.depth].items);
        earlier.check = false;
        earlier.stop = stop;
        earlier
    }

    /// The next item on the level the reader started on, with everything
    /// nested in the items before it skipped.
    fn next_item(&mut self) -> Option<Event<'a>> {
        loop {
            let level = self.depth;
            let event = self.step().ok()??;
            if level == 0 {
                return Some(event);
            }
        }
    }

    fn check_entry(&self, key: &str, start: usize, at: Pos) -> Result<(), Error> {
        let mut earlier = self.reread(start);
        while let Some(event) = earlier.next_item() {
            let code = match event {
                Event::Entry { key: other, .. } if other == key => Code::DuplicateKey,
                Event::Block { name, .. } if name == key => Code::NameConflict,
                _ => continue,
            };
            return Err(Error::new(code, at));
        }
        Ok(())
    }

    fn check_block(
        &self,
        name: &str,
        label: Option<Str<'a>>,
        start: usize,
        at: Pos,
    ) -> Result<(), Error> {
        let mut earlier = self.reread(start);
        while let Some(event) = earlier.next_item() {
            let code = match event {
                Event::Entry { key, .. } if key == name => Code::NameConflict,
                Event::Block {
                    name: other,
                    label: other_label,
                    ..
                } if other == name && label.is_some() && other_label == label => {
                    Code::DuplicateLabel
                }
                _ => continue,
            };
            return Err(Error::new(code, at));
        }
        Ok(())
    }

    fn value(&mut self, in_list: bool) -> Result<Value<'a>, Error> {
        let Some(c) = self.peek() else {
            return Err(self.error(Code::ExpectedValue));
        };
        let value = match c {
            b'"' if self.src[self.i..].starts_with(b"\"\"\"") => Value::Str(self.text()?),
            b'"' => Value::Str(self.quoted()?),
            b'[' => {
                let at = self.pos();
                if self.depth == MAX_DEPTH {
                    return Err(Error::new(Code::TooDeep, at));
                }
                self.i += 1;
                self.push(at, true);
                return Ok(Value::List);
            }
            b'-' | b'0'..=b'9' => {
                let at = self.pos();
                let (value, end) = number::read(self.src, self.text, self.i)
                    .map_err(|code| Error::new(code, at))?;
                self.i = end;
                value
            }
            c if is_key_start(c) => match self.word() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                word => Value::Word(word),
            },
            b',' if !in_list => return Err(self.unexpected(Code::UnexpectedCharacter)),
            b'\n' | b'\r' | b'#' | b'}' | b',' => return Err(self.unexpected(Code::ExpectedValue)),
            _ => return Err(self.unexpected(Code::UnexpectedCharacter)),
        };
        self.after = true;
        Ok(value)
    }

    fn in_list(&mut self, at: Pos) -> Result<Option<Event<'a>>, Error> {
        let unterminated = Error::new(Code::UnterminatedList, at);
        self.skip_list_space()?;
        if self.after {
            match self.peek() {
                None => return Err(unterminated),
                Some(b',') => {
                    self.i += 1;
                    self.skip_list_space()?;
                }
                Some(b']') => return Ok(Some(self.close_list())),
                Some(_) => return Err(self.unexpected(Code::ExpectedComma)),
            }
        }
        match self.peek() {
            None => Err(unterminated),
            Some(b']') => Ok(Some(self.close_list())),
            Some(_) => {
                let at = self.pos();
                let value = self.value(true)?;
                Ok(Some(Event::Element { value, at }))
            }
        }
    }

    fn close_list(&mut self) -> Event<'a> {
        self.i += 1;
        self.depth -= 1;
        self.after = true;
        Event::ListEnd
    }

    fn push(&mut self, at: Pos, list: bool) {
        let items = Mark {
            i: self.i,
            line: self.line,
            line_start: self.line_start,
        };
        self.depth += 1;
        self.frames[self.depth] = Frame { at, list, items };
        self.after = false;
    }

    fn quoted(&mut self) -> Result<Str<'a>, Error> {
        let unterminated = Error::new(Code::UnterminatedString, self.pos());
        self.i += 1;
        let start = self.i;
        loop {
            match self.peek() {
                Some(b'"') => {
                    let raw = &self.text[start..self.i];
                    self.i += 1;
                    return Ok(Str::quoted(raw));
                }
                Some(b'\\') => self.escape()?,
                None => return Err(unterminated),
                Some(_) if self.at_line_end() => return Err(unterminated),
                Some(_) => self.char()?,
            }
        }
    }

    fn escape(&mut self) -> Result<(), Error> {
        let at = self.pos();
        self.i += 1;
        match self.peek() {
            Some(b'\\' | b'"' | b'n' | b'r' | b't' | b'0') => {
                self.i += 1;
                Ok(())
            }
            Some(b'u') => self.unicode_escape(at),
            _ => Err(Error::new(Code::InvalidEscape, at)),
        }
    }

    fn unicode_escape(&mut self, at: Pos) -> Result<(), Error> {
        let bad = Error::new(Code::InvalidUnicodeEscape, at);
        self.i += 1;
        if self.peek() != Some(b'{') {
            return Err(bad);
        }
        self.i += 1;
        let (mut value, mut digits) = (0, 0);
        loop {
            let c = self.peek().ok_or(bad)?;
            self.i += 1;
            if c == b'}' {
                break;
            }
            if digits == 6 {
                return Err(bad);
            }
            value = value * 16 + char::from(c).to_digit(16).ok_or(bad)?;
            digits += 1;
        }
        match char::from_u32(value) {
            Some(_) if digits > 0 => Ok(()),
            _ => Err(bad),
        }
    }

    /// Reads a text, from its opening `"""` to the end of its closing
    /// line's `"""`.
    fn text(&mut self) -> Result<Str<'a>, Error> {
        let unterminated = Error::new(Code::UnterminatedText, self.pos());
        self.i += 3;
        self.skip_spaces();
        match self.peek() {
            None => return Err(unterminated),
            Some(b'\n' | b'\r') => self.newline()?,
            Some(_) => return Err(self.unexpected(Code::TextOpening)),
        }
        let first_line = self.line;
        let start = self.i;
        let mut end = start;
        loop {
            if self.peek().is_none() {
                return Err(unterminated);
            }
            let line_start = self.i;
            self.skip_spaces();
            if self.src[self.i..].starts_with(b"\"\"\"") {
                let indent = &self.text[line_start..self.i];
                self.i += 3;
                let body = &self.text[start..end];
                return text::check_indent(body, indent, first_line)
                    .map(|()| Str::text(body, indent));
            }
            self.rest_of_line()?;
            end = self.i;
            if self.peek().is_none() {
                return Err(unterminated);
            }
            self.newline()?;
        }
    }

    fn word(&mut self) -> &'a str {
        let start = self.i;
        self.i += 1;
        while self.peek().is_some_and(is_key_char) {
            self.i += 1;
        }
        &self.text[start..self.i]
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.i).copied()
    }

    /// Whether the reader is at a line feed, or at a carriage return that
    /// a line feed follows.
    fn at_line_end(&self) -> bool {
        match self.peek() {
            Some(b'\n') => true,
            Some(b'\r') => self.src.get(self.i + 1) == Some(&b'\n'),
            _ => false,
        }
    }

    /// Steps over the line ending at the reader.
    fn newline(&mut self) -> Result<(), Error> {
        if !self.at_line_end() {
            return Err(self.error(Code::LoneCarriageReturn));
        }
        if self.peek() == Some(b'\r') {
            self.i += 1;
        }
        self.i += 1;
        self.line += 1;
        self.line_start = self.i;
        Ok(())
    }

    fn skip_spaces(&mut self) {
        while let Some(b' ' | b'\t') = self.peek() {
            self.i += 1;
        }
    }

    fn skip_spaces_and_comment(&mut self) -> Result<(), Error> {
        self.skip_spaces();
        if self.peek() == Some(b'#') {
            self.rest_of_line()?;
        }
        Ok(())
    }

    /// Skips everything that may come between the parts of a list, line
    /// endings included.
    fn skip_list_space(&mut self) -> Result<(), Error> {
        loop {
            self.skip_spaces_and_comment()?;
            match self.peek() {
                Some(b'\n' | b'\r') => self.newline()?,
                _ => return Ok(()),
            }
        }
    }

    /// Steps over the characters of a comment, a string or a text up to
    /// the end of the line.
    fn rest_of_line(&mut self) -> Result<(), Error> {
        while self.peek().is_some() && !self.at_line_end() {
            self.char()?;
        }
        Ok(())
    }

    /// Steps over one byte inside a comment, a string or a text, unless it
    /// is allowed nowhere.
    fn char(&mut self) -> Result<(), Error> {
        if let Some(err) = self.bad_char() {
            return Err(err);
        }
        self.i += 1;
        Ok(())
    }

    /// The error for the byte at the reader when it is allowed nowhere: a
    /// control character, a carriage return without a line feed, or the
    /// start of invalid UTF-8.
    fn bad_char(&mut self) -> Option<Error> {
        let code = match self.peek()? {
            b'\r' if !self.at_line_end() => Code::LoneCarriageReturn,
            0x00..=0x08 | 0x0b..=0x0c | 0x0e..=0x1f | 0x7f => Code::ControlCharacter,
            0x80.. if self.i >= self.text.len() => Code::InvalidUtf8,
            _ => return None,
        };
        Some(self.error(code))
    }

    /// The error for something unexpected at the reader. A byte that is
    /// allowed nowhere is reported as what it is instead.
    fn unexpected(&mut self, code: Code) -> Error {
        match self.bad_char() {
            Some(err) => err,
            None => self.error(code),
        }
    }

    fn error(&mut self, code: Code) -> Error {
        Error::new(code, self.pos())
    }

    fn pos(&mut self) -> Pos {
        let (mut from, mut col) = (self.line_start, 1);
        if (self.line_start..=self.i).contains(&self.col_cache.0) {
            (from, col) = self.col_cache;
        }
        col += self.text[from..self.i].chars().count();
        self.col_cache = (self.i, col);
        Pos {
            line: self.line,
            col,
        }
    }
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Event<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let step = self.step();
        self.done = !matches!(step, Ok(Some(_)));
        step.transpose()
    }
}

fn is_key_start(c: u8) -> bool {
    matches!(c, b'a'..=b'z' | b'A'..=b'Z' | b'_')
}

fn is_key_char(c: u8) -> bool {
    matches!(c, b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9' | b'-')
}
