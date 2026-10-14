use core::fmt::{self, Write};

use crate::{Code, Error, Pos};

/// A string value or a block label, as written in the document. Escapes
/// are decoded, and a text loses its indentation, only as the characters
/// are read, so nothing has to be copied: [`Str::chars`] walks them, and
/// [`Str::decode`] writes them into a buffer.
///
/// Two `Str`s are equal when their characters are, however they were
/// written: the label `main` equals the label `"m\u{61}in"`.
#[derive(Clone, Copy, Debug)]
pub struct Str<'a> {
    raw: &'a str,
    /// The indentation of a text, which every line loses. `None` for a
    /// quoted string or a word.
    indent: Option<&'a str>,
}

impl<'a> Str<'a> {
    /// A quoted string without its quotes, or a word.
    pub(crate) fn quoted(raw: &'a str) -> Str<'a> {
        Str { raw, indent: None }
    }

    /// A text: the lines between its opening and closing lines, and the
    /// indentation of the closing line.
    pub(crate) fn text(body: &'a str, indent: &'a str) -> Str<'a> {
        Str {
            raw: body,
            indent: Some(indent),
        }
    }

    #[must_use]
    pub fn chars(&self) -> Chars<'a> {
        Chars {
            rest: self.raw.chars(),
            indent: self.indent,
            line_start: true,
        }
    }

    /// The string as it is in the document, when it needs no decoding: a
    /// word, or a quoted string without escapes.
    #[must_use]
    pub fn as_str(&self) -> Option<&'a str> {
        match self.indent {
            None if !self.raw.contains('\\') => Some(self.raw),
            _ => None,
        }
    }

    /// Writes the characters into `buf` and returns them, or `None` when
    /// they do not fit.
    #[must_use]
    pub fn decode<'b>(&self, buf: &'b mut [u8]) -> Option<&'b str> {
        let mut len = 0;
        for c in self.chars() {
            let end = len + c.len_utf8();
            c.encode_utf8(buf.get_mut(len..end)?);
            len = end;
        }
        core::str::from_utf8(&buf[..len]).ok()
    }
}

impl PartialEq for Str<'_> {
    fn eq(&self, other: &Str<'_>) -> bool {
        self.chars().eq(other.chars())
    }
}

impl PartialEq<str> for Str<'_> {
    fn eq(&self, other: &str) -> bool {
        self.chars().eq(other.chars())
    }
}

impl PartialEq<&str> for Str<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.chars().eq(other.chars())
    }
}

impl fmt::Display for Str<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.chars().try_for_each(|c| f.write_char(c))
    }
}

/// The characters of a [`Str`].
#[derive(Clone, Debug)]
pub struct Chars<'a> {
    rest: core::str::Chars<'a>,
    indent: Option<&'a str>,
    line_start: bool,
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let Some(indent) = self.indent else {
            return match self.rest.next()? {
                '\\' => unescape(&mut self.rest),
                c => Some(c),
            };
        };
        if self.line_start {
            let rest = self.rest.as_str();
            let line = rest.split('\n').next().unwrap_or_default();
            let skip = if is_blank(line) {
                line.len()
            } else {
                indent.len()
            };
            self.rest = rest[skip..].chars();
        }
        let mut c = self.rest.next()?;
        if c == '\r' {
            c = self.rest.next()?;
        }
        self.line_start = c == '\n';
        Some(c)
    }
}

/// Decodes the escape after a backslash. The reader has checked it.
fn unescape(rest: &mut core::str::Chars<'_>) -> Option<char> {
    Some(match rest.next()? {
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        '0' => '\0',
        'u' => {
            let mut value = 0;
            for c in rest.by_ref().skip(1) {
                match c.to_digit(16) {
                    Some(d) => value = value * 16 + d,
                    None => break,
                }
            }
            char::from_u32(value)?
        }
        c => c,
    })
}

/// Whether a line of a text holds nothing but spaces and tabs, apart from
/// the carriage return of a CR LF ending.
fn is_blank(line: &str) -> bool {
    line.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r'))
}

/// Checks that every line of a text that is not blank starts with the
/// closing line's indentation. The lines are numbered from `first_line`.
pub(crate) fn check_indent(body: &str, indent: &str, first_line: usize) -> Result<(), Error> {
    for (n, line) in body.split('\n').enumerate() {
        if !is_blank(line) && !line.starts_with(indent) {
            let at = Pos {
                line: first_line + n,
                col: 1,
            };
            return Err(Error::new(Code::TextIndent, at));
        }
    }
    Ok(())
}
