//! What a program's parent sends over its start channel before the
//! program's own code runs (decision record 0017): its standard streams
//! and init's line, each a message of one kind byte that brings a handle,
//! and a message of kind 0 at the end. Text on a stream goes in messages
//! of up to [`abi::MESSAGE_BYTES`], and an empty one ends it.

use crate::Handle;

/// What one start message hands over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No more messages, and no handle.
    End,
    /// Standard input, a channel end to read from.
    Input,
    /// Standard output, a channel end to write to.
    Output,
    /// Standard error, a channel end to write to.
    Error,
    /// Standard output, a file opened to write.
    OutputFile,
    /// Standard error, a file opened to write.
    ErrorFile,
    /// The line to ask init about services over.
    Line,
}

impl Kind {
    const ALL: [Kind; 7] = [
        Kind::End,
        Kind::Input,
        Kind::Output,
        Kind::Error,
        Kind::OutputFile,
        Kind::ErrorFile,
        Kind::Line,
    ];

    /// The byte a message of this kind starts with.
    #[must_use]
    pub fn byte(self) -> u8 {
        match self {
            Kind::End => 0,
            Kind::Input => 1,
            Kind::Output => 2,
            Kind::Error => 3,
            Kind::OutputFile => 4,
            Kind::ErrorFile => 5,
            Kind::Line => 6,
        }
    }

    /// The kind `byte` stands for.
    #[must_use]
    pub fn of(byte: u8) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.byte() == byte)
    }
}

/// Where a standard output or error goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sink {
    Channel(Handle),
    File(Handle),
}

/// What a program was given at its start; what it was not given stays
/// the console's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Given {
    pub input: Option<Handle>,
    pub output: Option<Sink>,
    pub error: Option<Sink>,
    pub line: Option<Handle>,
}

/// Why a start message was not taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// It is not one kind byte.
    Size,
    /// Its kind byte is none of the kinds.
    Kind(u8),
    /// It brings a handle where its kind brings none, or none where its
    /// kind brings one.
    Handle,
    /// Its kind was handed over already.
    Twice,
}

impl Given {
    /// Takes in the start message `bytes`, which brought `handle`, and
    /// says whether more are to come.
    ///
    /// # Errors
    ///
    /// When it is not one byte of a kind there is, its handle does not go
    /// with its kind, or what it hands over was handed over already.
    pub fn take(&mut self, bytes: &[u8], handle: Option<Handle>) -> Result<bool, Refused> {
        let [byte] = bytes else {
            return Err(Refused::Size);
        };
        let kind = Kind::of(*byte).ok_or(Refused::Kind(*byte))?;
        let Some(handle) = handle else {
            return if kind == Kind::End {
                Ok(false)
            } else {
                Err(Refused::Handle)
            };
        };
        let (slot, value) = match kind {
            Kind::End => return Err(Refused::Handle),
            Kind::Input => return fill(&mut self.input, handle),
            Kind::Line => return fill(&mut self.line, handle),
            Kind::Output => (&mut self.output, Sink::Channel(handle)),
            Kind::OutputFile => (&mut self.output, Sink::File(handle)),
            Kind::Error => (&mut self.error, Sink::Channel(handle)),
            Kind::ErrorFile => (&mut self.error, Sink::File(handle)),
        };
        fill(slot, value)
    }
}

/// Puts `value` in `slot` when that is empty, and says more are to come.
fn fill<T>(slot: &mut Option<T>, value: T) -> Result<bool, Refused> {
    if slot.is_some() {
        return Err(Refused::Twice);
    }
    *slot = Some(value);
    Ok(true)
}

/// The messages `bytes` goes over a stream as: pieces of up to
/// [`abi::MESSAGE_BYTES`], none for nothing, as an empty message would end
/// the stream.
pub fn pieces(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.chunks(abi::MESSAGE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_has_the_byte_the_record_gives_it() {
        let bytes: Vec<u8> = Kind::ALL.iter().map(|k| k.byte()).collect();
        assert_eq!(bytes, [0, 1, 2, 3, 4, 5, 6]);
        for kind in Kind::ALL {
            assert_eq!(Kind::of(kind.byte()), Some(kind));
        }
        for byte in 7..=u8::MAX {
            assert_eq!(Kind::of(byte), None, "{byte}");
        }
    }

    #[test]
    fn what_the_parent_sends_is_kept_until_the_end() {
        let mut given = Given::default();
        let sent = [
            (Kind::Line, Some(Handle(0x1_0001))),
            (Kind::Input, Some(Handle(0x1_0002))),
            (Kind::Output, Some(Handle(0x1_0003))),
            (Kind::ErrorFile, Some(Handle(0x1_0004))),
        ];
        for (kind, handle) in sent {
            assert_eq!(given.take(&[kind.byte()], handle), Ok(true), "{kind:?}");
        }
        assert_eq!(given.take(&[0], None), Ok(false), "the end");
        assert_eq!(
            given,
            Given {
                input: Some(Handle(0x1_0002)),
                output: Some(Sink::Channel(Handle(0x1_0003))),
                error: Some(Sink::File(Handle(0x1_0004))),
                line: Some(Handle(0x1_0001)),
            }
        );
        let mut files = Given::default();
        assert_eq!(
            files.take(&[Kind::OutputFile.byte()], Some(Handle(7))),
            Ok(true)
        );
        assert_eq!(files.take(&[Kind::Error.byte()], Some(Handle(8))), Ok(true));
        assert_eq!(
            (files.output, files.error),
            (Some(Sink::File(Handle(7))), Some(Sink::Channel(Handle(8))))
        );
        assert_eq!(
            Given::default().take(&[0], None),
            Ok(false),
            "nothing but the end"
        );
    }

    #[test]
    fn a_message_that_does_not_fit_is_refused() {
        let mut given = Given::default();
        assert_eq!(given.take(&[], None), Err(Refused::Size));
        assert_eq!(given.take(&[1, 2], Some(Handle(1))), Err(Refused::Size));
        assert_eq!(given.take(&[7], Some(Handle(1))), Err(Refused::Kind(7)));
        assert_eq!(given.take(&[255], None), Err(Refused::Kind(255)));
        assert_eq!(
            given.take(&[0], Some(Handle(1))),
            Err(Refused::Handle),
            "the end brings none"
        );
        for kind in &Kind::ALL[1..] {
            assert_eq!(
                given.take(&[kind.byte()], None),
                Err(Refused::Handle),
                "{kind:?}"
            );
        }
        assert_eq!(given, Given::default(), "nothing refused was kept");
    }

    #[test]
    fn nothing_is_handed_over_twice() {
        let twice = |first: Kind, second: Kind| {
            let mut given = Given::default();
            given.take(&[first.byte()], Some(Handle(1))).unwrap();
            given.take(&[second.byte()], Some(Handle(2)))
        };
        assert_eq!(twice(Kind::Input, Kind::Input), Err(Refused::Twice));
        assert_eq!(twice(Kind::Line, Kind::Line), Err(Refused::Twice));
        assert_eq!(
            twice(Kind::Output, Kind::OutputFile),
            Err(Refused::Twice),
            "one output"
        );
        assert_eq!(
            twice(Kind::ErrorFile, Kind::Error),
            Err(Refused::Twice),
            "one error"
        );
        assert_eq!(twice(Kind::Output, Kind::Error), Ok(true));
        assert_eq!(twice(Kind::Input, Kind::Output), Ok(true));
        let mut kept = Given::default();
        kept.take(&[Kind::Input.byte()], Some(Handle(1))).unwrap();
        let _ = kept.take(&[Kind::Input.byte()], Some(Handle(2)));
        assert_eq!(kept.input, Some(Handle(1)), "the first stays");
    }

    #[test]
    fn text_goes_in_pieces_a_message_holds() {
        let text: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
        let sizes: Vec<usize> = pieces(&text).map(<[u8]>::len).collect();
        assert_eq!(sizes, [128, 128, 44]);
        assert_eq!(pieces(&text).flatten().copied().collect::<Vec<u8>>(), text);
        assert_eq!(pieces(&text[..128]).count(), 1);
        assert_eq!(pieces(&text[..129]).count(), 2);
        assert_eq!(
            pieces(&[]).count(),
            0,
            "nothing sends nothing, never an empty message"
        );
    }

    #[test]
    fn each_kind_lands_where_it_belongs() {
        let handle = Handle(0x2_0005);
        let landed = |kind: Kind| {
            let mut given = Given::default();
            assert_eq!(
                given.take(&[kind.byte()], Some(handle)),
                Ok(true),
                "{kind:?}"
            );
            given
        };
        let only = |given: Given| (given.input, given.output, given.error, given.line);
        let (c, f) = (Sink::Channel(handle), Sink::File(handle));
        assert_eq!(only(landed(Kind::Input)), (Some(handle), None, None, None));
        assert_eq!(only(landed(Kind::Output)), (None, Some(c), None, None));
        assert_eq!(only(landed(Kind::OutputFile)), (None, Some(f), None, None));
        assert_eq!(only(landed(Kind::Error)), (None, None, Some(c), None));
        assert_eq!(only(landed(Kind::ErrorFile)), (None, None, Some(f), None));
        assert_eq!(only(landed(Kind::Line)), (None, None, None, Some(handle)));
    }
}
