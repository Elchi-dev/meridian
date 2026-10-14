use crate::reader::{Event, Reader, Value};
use crate::text::Str;
use crate::{Code, Error, Pos};

/// A document that has been read once and found valid, for looking up
/// settings by their path: `boot.timeout` is the entry `timeout` in the
/// unlabelled block `boot` of the root, and `timeout` alone an entry of the
/// root. When unlabelled blocks with the same name repeat, the first entry
/// on the path counts. Labelled blocks are not on any path; [`Reader`]
/// reads those.
///
/// Each lookup reads the document again from its start, which costs a
/// settings file little and needs no memory.
#[derive(Clone, Copy, Debug)]
pub struct Document<'a> {
    src: &'a [u8],
}

/// What a path leads to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item<'a> {
    /// An entry's value, with the value's position.
    Entry { value: Value<'a>, at: Pos },
    /// A block that has the name the path ends in, at its name.
    Block { at: Pos },
}

impl<'a> Document<'a> {
    /// Reads the whole document to check it.
    ///
    /// # Errors
    ///
    /// The first problem in the document.
    pub fn parse(src: &'a [u8]) -> Result<Document<'a>, Error> {
        for event in Reader::new(src) {
            event?;
        }
        Ok(Document { src })
    }

    /// The first item the path leads to, if there is one.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<Item<'a>> {
        let (blocks, key) = path.rsplit_once('.').unwrap_or(("", path));
        let want = if blocks.is_empty() {
            0
        } else {
            blocks.split('.').count()
        };
        // Open blocks, and how many of the outermost of them follow the path.
        let (mut depth, mut matched) = (0, 0);
        for event in Reader::new(self.src).map_while(Result::ok) {
            let on_path = depth == matched;
            match event {
                Event::Entry {
                    key: k,
                    value,
                    value_at,
                    ..
                } if on_path && depth == want && k == key => {
                    return Some(Item::Entry {
                        value,
                        at: value_at,
                    });
                }
                Event::Block { name, at, .. } if on_path && depth == want && name == key => {
                    return Some(Item::Block { at });
                }
                Event::Block { name, label, .. } => {
                    if on_path && label.is_none() && blocks.split('.').nth(depth) == Some(name) {
                        matched += 1;
                    }
                    depth += 1;
                }
                Event::End => {
                    if on_path {
                        matched -= 1;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        None
    }

    /// The value at the path, of whatever type.
    ///
    /// # Errors
    ///
    /// [`Code::WrongType`] at the block when the path leads to blocks.
    pub fn value(&self, path: &str) -> Result<Option<(Value<'a>, Pos)>, Error> {
        match self.get(path) {
            None => Ok(None),
            Some(Item::Entry { value, at }) => Ok(Some((value, at))),
            Some(Item::Block { at }) => Err(Error::new(Code::WrongType, at)),
        }
    }

    /// Looks a value up and converts it, pointing errors at the value.
    fn typed<T>(
        &self,
        path: &str,
        convert: impl Fn(Value<'a>) -> Result<T, Code>,
    ) -> Result<Option<T>, Error> {
        let Some((value, at)) = self.value(path)? else {
            return Ok(None);
        };
        convert(value)
            .map(Some)
            .map_err(|code| Error::new(code, at))
    }

    /// # Errors
    ///
    /// [`Code::WrongType`] when the value is not `true` or `false`.
    pub fn bool(&self, path: &str) -> Result<Option<bool>, Error> {
        self.typed(path, |value| match value {
            Value::Bool(b) => Ok(b),
            _ => Err(Code::WrongType),
        })
    }

    /// # Errors
    ///
    /// [`Code::WrongType`] when the value is not an integer.
    pub fn int(&self, path: &str) -> Result<Option<i64>, Error> {
        self.typed(path, |value| match value {
            Value::Int(n) => Ok(n),
            _ => Err(Code::WrongType),
        })
    }

    /// A float, or an integer as the closest float.
    ///
    /// # Errors
    ///
    /// [`Code::WrongType`] when the value is not a number without a unit.
    pub fn float(&self, path: &str) -> Result<Option<f64>, Error> {
        self.typed(path, |value| match value {
            Value::Float(f) => Ok(f.value),
            // Section 13 accepts integers where floats are expected, and
            // the closest double is what such a setting means.
            #[allow(clippy::cast_precision_loss)]
            Value::Int(n) => Ok(n as f64),
            _ => Err(Code::WrongType),
        })
    }

    /// A bare word, such as `kvm`.
    ///
    /// # Errors
    ///
    /// [`Code::WrongType`] when the value is not a word.
    pub fn word(&self, path: &str) -> Result<Option<&'a str>, Error> {
        self.typed(path, |value| match value {
            Value::Word(w) => Ok(w),
            _ => Err(Code::WrongType),
        })
    }

    /// Text: a string, or a word as the string of its letters.
    ///
    /// # Errors
    ///
    /// [`Code::WrongType`] when the value is neither.
    pub fn text(&self, path: &str) -> Result<Option<Str<'a>>, Error> {
        self.typed(path, |value| match value {
            Value::Str(s) => Ok(s),
            Value::Word(w) => Ok(Str::quoted(w)),
            _ => Err(Code::WrongType),
        })
    }

    /// A size in bytes, as [`crate::Quantity::size`] works it out.
    ///
    /// # Errors
    ///
    /// [`Code::WrongType`] when the value is not a quantity with a size
    /// unit, and [`Code::OutOfRange`] when the size is not a whole number
    /// of bytes, negative, or too large.
    pub fn size(&self, path: &str) -> Result<Option<u64>, Error> {
        self.typed(path, |value| match value {
            Value::Quantity(q) => q.size(),
            _ => Err(Code::WrongType),
        })
    }

    /// A duration in nanoseconds, as [`crate::Quantity::duration`] works it
    /// out.
    ///
    /// # Errors
    ///
    /// [`Code::WrongType`] when the value is not a quantity with a duration
    /// unit, and [`Code::OutOfRange`] when the duration is not a whole
    /// number of nanoseconds or does not fit an `i64`.
    pub fn duration(&self, path: &str) -> Result<Option<i64>, Error> {
        self.typed(path, |value| match value {
            Value::Quantity(q) => q.duration(),
            _ => Err(Code::WrongType),
        })
    }
}
