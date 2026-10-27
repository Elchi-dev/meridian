//! What a program's parent sends over its start channel before the
//! program's own code runs (decision record 0017): its standard streams
//! and init's line, each a message of one kind byte that brings a handle,
//! and a message of kind 0 at the end. Text on a stream goes in messages
//! of up to [`abi::MESSAGE_BYTES`], and an empty one ends it.

use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use crate::{Error, Handle};

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
    /// The line to ask init about services over: the handle it brings, or
    /// without one, the start channel itself.
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
    /// Takes in the start message `bytes`, which came over the start
    /// channel `start` and brought `handle`, and says whether more are to
    /// come.
    ///
    /// # Errors
    ///
    /// When it is not one byte of a kind there is, its handle does not go
    /// with its kind, or what it hands over was handed over already.
    pub fn take(
        &mut self,
        bytes: &[u8],
        handle: Option<Handle>,
        start: Handle,
    ) -> Result<bool, Refused> {
        let [byte] = bytes else {
            return Err(Refused::Size);
        };
        let kind = Kind::of(*byte).ok_or(Refused::Kind(*byte))?;
        let Some(handle) = handle else {
            return match kind {
                Kind::End => Ok(false),
                Kind::Line => fill(&mut self.line, start),
                _ => Err(Refused::Handle),
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

/// What [`begin`] kept: standard input, output and error, and init's line,
/// each a handle or 0, the outputs with [`FILE`] set when they are files.
static GIVEN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// The bit that marks a kept output as a file, which no handle has.
const FILE: u64 = 1 << 63;

impl Given {
    /// Each part as a word, as [`GIVEN`] keeps it.
    fn words(self) -> [u64; 4] {
        let sink = |s: Option<Sink>| match s {
            None => 0,
            Some(Sink::Channel(h)) => h.0,
            Some(Sink::File(h)) => h.0 | FILE,
        };
        let handle = |h: Option<Handle>| h.map_or(0, |h| h.0);
        [
            handle(self.input),
            sink(self.output),
            sink(self.error),
            handle(self.line),
        ]
    }

    /// What [`Given::words`] made `words` of.
    fn from_words([input, output, error, line]: [u64; 4]) -> Given {
        let sink = |w: u64| match (w, w & FILE) {
            (0, _) => None,
            (w, 0) => Some(Sink::Channel(Handle(w))),
            (w, _) => Some(Sink::File(Handle(w & !FILE))),
        };
        let handle = |w: u64| (w != 0).then_some(Handle(w));
        Given {
            input: handle(input),
            output: sink(output),
            error: sink(error),
            line: handle(line),
        }
    }
}

/// What the program was given at its start.
#[must_use]
pub fn given() -> Given {
    Given::from_words(GIVEN.each_ref().map(|w| w.load(SeqCst)))
}

/// Reads the start messages from `start`, keeps what they hand over for
/// [`given`], and returns `start`: before `main`, which the runtime gives
/// it. A program started with no handle reads none. Reading stops at the
/// end, at a message no parent sends, and once the parent's end closed.
#[cfg_attr(test, allow(dead_code))]
pub fn begin(start: Handle) -> Handle {
    if start.0 == 0 {
        return start;
    }
    let mut given = Given::default();
    let mut room = [0u8; abi::MESSAGE_BYTES];
    loop {
        match crate::receive(start, &mut room) {
            Ok((len, carried)) => {
                if given.take(&room[..len], carried, start) != Ok(true) {
                    break;
                }
            }
            Err(Error::Empty) => crate::sleep(1),
            Err(_) => break,
        }
    }
    for (slot, word) in GIVEN.iter().zip(given.words()) {
        slot.store(word, SeqCst);
    }
    start
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

    /// The start channel the messages come over.
    const START: Handle = Handle(0x1_0000);

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
            assert_eq!(
                given.take(&[kind.byte()], handle, START),
                Ok(true),
                "{kind:?}"
            );
        }
        assert_eq!(given.take(&[0], None, START), Ok(false), "the end");
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
            files.take(&[Kind::OutputFile.byte()], Some(Handle(7)), START),
            Ok(true)
        );
        assert_eq!(
            files.take(&[Kind::Error.byte()], Some(Handle(8)), START),
            Ok(true)
        );
        assert_eq!(
            (files.output, files.error),
            (Some(Sink::File(Handle(7))), Some(Sink::Channel(Handle(8))))
        );
        assert_eq!(
            Given::default().take(&[0], None, START),
            Ok(false),
            "nothing but the end"
        );
    }

    #[test]
    fn a_message_that_does_not_fit_is_refused() {
        let mut given = Given::default();
        assert_eq!(given.take(&[], None, START), Err(Refused::Size));
        assert_eq!(
            given.take(&[1, 2], Some(Handle(1)), START),
            Err(Refused::Size)
        );
        assert_eq!(
            given.take(&[7], Some(Handle(1)), START),
            Err(Refused::Kind(7))
        );
        assert_eq!(given.take(&[255], None, START), Err(Refused::Kind(255)));
        assert_eq!(
            given.take(&[0], Some(Handle(1)), START),
            Err(Refused::Handle),
            "the end brings none"
        );
        for kind in &Kind::ALL[1..6] {
            assert_eq!(
                given.take(&[kind.byte()], None, START),
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
            given.take(&[first.byte()], Some(Handle(1)), START).unwrap();
            given.take(&[second.byte()], Some(Handle(2)), START)
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
        kept.take(&[Kind::Input.byte()], Some(Handle(1)), START)
            .unwrap();
        let _ = kept.take(&[Kind::Input.byte()], Some(Handle(2)), START);
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
                given.take(&[kind.byte()], Some(handle), START),
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

    #[test]
    fn a_line_without_a_handle_is_the_start_channel_itself() {
        let mut given = Given::default();
        assert_eq!(given.take(&[Kind::Line.byte()], None, START), Ok(true));
        assert_eq!(given.line, Some(START));
        assert_eq!(
            given.take(&[Kind::Line.byte()], Some(Handle(9)), START),
            Err(Refused::Twice),
            "once is all"
        );
        let mut other = Given::default();
        assert_eq!(
            other.take(&[Kind::Line.byte()], Some(Handle(9)), START),
            Ok(true)
        );
        assert_eq!(
            other.take(&[Kind::Line.byte()], None, START),
            Err(Refused::Twice)
        );
        assert_eq!(other.line, Some(Handle(9)));
    }

    #[test]
    fn what_is_given_is_kept_as_words_and_read_back() {
        let all = Given {
            input: Some(Handle(0x1_0001)),
            output: Some(Sink::File(Handle(0x1_0002))),
            error: Some(Sink::Channel(Handle(0x1_0003))),
            line: Some(Handle(0x1_0004)),
        };
        assert_eq!(all.words(), [0x1_0001, 0x1_0002 | FILE, 0x1_0003, 0x1_0004]);
        assert_eq!(Given::from_words(all.words()), all);
        assert_eq!(Given::default().words(), [0; 4]);
        assert_eq!(Given::from_words([0; 4]), Given::default());
        let channel = Given {
            output: Some(Sink::Channel(Handle(5))),
            error: Some(Sink::File(Handle(6))),
            ..Given::default()
        };
        assert_eq!(Given::from_words(channel.words()), channel);
        assert_eq!(given(), Given::default(), "nothing kept without a start");
    }

    #[test]
    fn each_part_keeps_its_own_place_as_a_word() {
        let parts = [
            (
                Given {
                    input: Some(Handle(3)),
                    ..Given::default()
                },
                [3, 0, 0, 0],
            ),
            (
                Given {
                    line: Some(Handle(4)),
                    ..Given::default()
                },
                [0, 0, 0, 4],
            ),
            (
                Given {
                    error: Some(Sink::File(Handle(5))),
                    ..Given::default()
                },
                [0, 0, 5 | FILE, 0],
            ),
            (
                Given {
                    output: Some(Sink::Channel(Handle(6))),
                    ..Given::default()
                },
                [0, 6, 0, 0],
            ),
        ];
        for (given, words) in parts {
            assert_eq!(given.words(), words, "{given:?}");
            assert_eq!(Given::from_words(words), given);
        }
    }
}
