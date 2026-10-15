//! Channels: two ends, and a queue of messages going each way. A message
//! is up to [`MESSAGE`] bytes, and each queue holds up to [`QUEUED`] of
//! them in the order they were sent. Once one end is closed, the other can
//! still read what was sent before, and then learns the channel is closed.
//! Nothing is allocated.

#![cfg_attr(not(test), no_std)]

/// The longest message, in bytes.
pub const MESSAGE: usize = 128;
/// How many messages wait in each direction at most.
pub const QUEUED: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The message is longer than [`MESSAGE`].
    TooLong,
    /// [`QUEUED`] messages wait already.
    Full,
    /// Nothing waits.
    Empty,
    /// The message waiting is longer than the room given for it; it stays.
    TooSmall(usize),
    /// The other end is closed.
    Closed,
}

/// The two ends of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    A,
    B,
}

impl End {
    fn index(self) -> usize {
        match self {
            End::A => 0,
            End::B => 1,
        }
    }

    /// The end at the other side.
    #[must_use]
    pub fn other(self) -> End {
        match self {
            End::A => End::B,
            End::B => End::A,
        }
    }
}

#[derive(Clone, Copy)]
struct Queue {
    messages: [[u8; MESSAGE]; QUEUED],
    lengths: [usize; QUEUED],
    first: usize,
    len: usize,
}

impl Queue {
    const EMPTY: Queue = Queue {
        messages: [[0; MESSAGE]; QUEUED],
        lengths: [0; QUEUED],
        first: 0,
        len: 0,
    };

    fn push(&mut self, message: &[u8]) -> Result<(), Error> {
        if message.len() > MESSAGE {
            return Err(Error::TooLong);
        }
        if self.len == QUEUED {
            return Err(Error::Full);
        }
        let at = (self.first + self.len) % QUEUED;
        self.messages[at][..message.len()].copy_from_slice(message);
        self.lengths[at] = message.len();
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self, into: &mut [u8]) -> Result<usize, Error> {
        if self.len == 0 {
            return Err(Error::Empty);
        }
        let len = self.lengths[self.first];
        if len > into.len() {
            return Err(Error::TooSmall(len));
        }
        into[..len].copy_from_slice(&self.messages[self.first][..len]);
        self.first = (self.first + 1) % QUEUED;
        self.len -= 1;
        Ok(len)
    }
}

/// A channel: what each end sent the other, and which ends are closed.
pub struct Channel {
    /// The messages waiting for each end.
    waiting: [Queue; 2],
    closed: [bool; 2],
}

impl Default for Channel {
    fn default() -> Self {
        Self::new()
    }
}

impl Channel {
    /// A channel with both ends open and nothing sent.
    #[must_use]
    pub const fn new() -> Channel {
        Channel {
            waiting: [Queue::EMPTY; 2],
            closed: [false; 2],
        }
    }

    /// Sends `message` from `end` to the other end.
    ///
    /// # Errors
    ///
    /// When the message is too long, the other end's queue is full, or
    /// either end is closed.
    pub fn send(&mut self, end: End, message: &[u8]) -> Result<(), Error> {
        if self.closed != [false, false] {
            return Err(Error::Closed);
        }
        self.waiting[end.other().index()].push(message)
    }

    /// Takes the first message waiting for `end` into `into`, and returns
    /// its length.
    ///
    /// # Errors
    ///
    /// When `end` is closed, nothing waits, which is [`Error::Closed`] once
    /// the other end is closed, or the message does not fit, in which case
    /// it stays.
    pub fn receive(&mut self, end: End, into: &mut [u8]) -> Result<usize, Error> {
        if self.closed[end.index()] {
            return Err(Error::Closed);
        }
        match self.waiting[end.index()].pop(into) {
            Err(Error::Empty) if self.closed[end.other().index()] => Err(Error::Closed),
            other => other,
        }
    }

    /// How many messages wait for `end`.
    #[must_use]
    pub fn waiting(&self, end: End) -> usize {
        self.waiting[end.index()].len
    }

    /// Closes `end`. What waits for it is dropped.
    pub fn close(&mut self, end: End) {
        self.closed[end.index()] = true;
        self.waiting[end.index()] = Queue::EMPTY;
    }

    /// Whether both ends are closed, so the channel can go.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.closed == [true, true]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn read(c: &mut Channel, end: End) -> Result<Vec<u8>, Error> {
        let mut buf = [0u8; MESSAGE];
        c.receive(end, &mut buf).map(|n| buf[..n].to_vec())
    }

    #[test]
    fn messages_arrive_in_order_each_way() {
        let mut c = Channel::new();
        c.send(End::A, b"one").unwrap();
        c.send(End::A, b"two").unwrap();
        c.send(End::B, b"back").unwrap();
        assert_eq!((c.waiting(End::B), c.waiting(End::A)), (2, 1));
        assert_eq!(read(&mut c, End::B), Ok(b"one".to_vec()));
        assert_eq!(read(&mut c, End::A), Ok(b"back".to_vec()));
        assert_eq!(read(&mut c, End::B), Ok(b"two".to_vec()));
        assert_eq!(read(&mut c, End::B), Err(Error::Empty));
        assert_eq!(read(&mut c, End::A), Err(Error::Empty));
        assert_eq!(c.send(End::A, b""), Ok(()), "an empty message is one too");
        assert_eq!(read(&mut c, End::B), Ok(Vec::new()));
        assert_eq!(Channel::default().waiting(End::A), 0);
    }

    #[test]
    fn a_queue_holds_its_messages_and_no_more() {
        let mut c = Channel::new();
        assert_eq!(c.send(End::A, &[1; MESSAGE + 1]), Err(Error::TooLong));
        assert_eq!(
            c.send(End::A, &[7; MESSAGE]),
            Ok(()),
            "exactly as long as allowed"
        );
        for i in 1..QUEUED {
            c.send(End::A, &[u8::try_from(i).unwrap()]).unwrap();
        }
        assert_eq!(c.send(End::A, b"x"), Err(Error::Full));
        assert_eq!(
            c.send(End::B, b"the other way"),
            Ok(()),
            "each way has its own"
        );
        assert_eq!(read(&mut c, End::B), Ok(vec![7; MESSAGE]));
        assert_eq!(c.send(End::A, b"x"), Ok(()), "room again");
        for i in 1..QUEUED {
            assert_eq!(read(&mut c, End::B), Ok(vec![u8::try_from(i).unwrap()]));
        }
        assert_eq!(read(&mut c, End::B), Ok(b"x".to_vec()), "after going round");
    }

    #[test]
    fn a_message_too_long_for_its_room_stays() {
        let mut c = Channel::new();
        c.send(End::B, b"hello").unwrap();
        let mut small = [0u8; 4];
        assert_eq!(c.receive(End::A, &mut small), Err(Error::TooSmall(5)));
        assert_eq!(c.waiting(End::A), 1);
        let mut exact = [0u8; 5];
        assert_eq!(c.receive(End::A, &mut exact), Ok(5));
        assert_eq!(&exact, b"hello");
    }

    #[test]
    fn closing_lets_the_other_end_read_what_was_sent_and_then_know() {
        let mut c = Channel::new();
        c.send(End::A, b"last words").unwrap();
        c.send(End::B, b"never read").unwrap();
        c.close(End::A);
        assert_eq!(c.waiting(End::A), 0, "what waited for the closed end goes");
        assert_eq!(c.send(End::B, b"too late"), Err(Error::Closed));
        assert_eq!(read(&mut c, End::B), Ok(b"last words".to_vec()));
        assert_eq!(read(&mut c, End::B), Err(Error::Closed));
        assert!(!c.finished());
        assert_eq!(
            c.send(End::A, b"from beyond"),
            Err(Error::Closed),
            "a closed end sends nothing"
        );
        assert_eq!(
            read(&mut c, End::A),
            Err(Error::Closed),
            "and reads nothing"
        );
        c.close(End::B);
        assert!(c.finished());
        assert_eq!((End::A.other(), End::B.other()), (End::B, End::A));
    }

    #[test]
    fn messages_of_every_length_come_back_as_sent() {
        let mut c = Channel::new();
        for len in 0..=MESSAGE {
            let message: Vec<u8> = (0..len)
                .map(|i| u8::try_from((i * 7 + len) % 256).unwrap())
                .collect();
            c.send(End::B, &message).unwrap();
            assert_eq!(read(&mut c, End::A), Ok(message), "{len} bytes");
        }
    }

    #[test]
    fn a_message_too_long_for_its_room_stays_across_the_wrap() {
        let mut c = Channel::new();
        for _ in 0..QUEUED - 1 {
            c.send(End::A, b"filler").unwrap();
            read(&mut c, End::B).unwrap();
        }
        c.send(End::A, b"at the end").unwrap();
        c.send(End::A, b"at the start").unwrap();
        let mut small = [0u8; 3];
        assert_eq!(c.receive(End::B, &mut small), Err(Error::TooSmall(10)));
        assert_eq!(read(&mut c, End::B), Ok(b"at the end".to_vec()));
        assert_eq!(read(&mut c, End::B), Ok(b"at the start".to_vec()));
    }

    #[test]
    fn a_channel_closed_at_both_ends_refuses_everything() {
        let mut c = Channel::new();
        c.send(End::A, b"lost").unwrap();
        c.close(End::B);
        c.close(End::A);
        assert!(c.finished());
        for end in [End::A, End::B] {
            assert_eq!(c.send(end, b"x"), Err(Error::Closed));
            assert_eq!(read(&mut c, end), Err(Error::Closed));
            assert_eq!(c.waiting(end), 0);
        }
    }

    #[test]
    fn ten_thousand_steps_agree_with_two_queues() {
        let mut c = Channel::new();
        let mut model = [VecDeque::new(), VecDeque::new()];
        let mut closed = [false, false];
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut random = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        for step in 0..10_000u64 {
            let end = if random(2) == 0 { End::A } else { End::B };
            let (mine, theirs) = (end.index(), end.other().index());
            match random(40) {
                0 => {
                    // Once in a while an end closes, and once both have, the
                    // next channel starts.
                    c.close(end);
                    closed[mine] = true;
                    model[mine].clear();
                    if closed == [true, true] {
                        assert!(c.finished(), "step {step}");
                        c = Channel::new();
                        closed = [false, false];
                    }
                }
                1..20 => {
                    let len = usize::try_from(random(MESSAGE as u64 + 2)).unwrap();
                    let message = vec![u8::try_from(step % 251).unwrap(); len];
                    let want = if closed != [false, false] {
                        Err(Error::Closed)
                    } else if len > MESSAGE {
                        Err(Error::TooLong)
                    } else if model[theirs].len() == QUEUED {
                        Err(Error::Full)
                    } else {
                        model[theirs].push_back(message.clone());
                        Ok(())
                    };
                    assert_eq!(c.send(end, &message), want, "step {step}");
                }
                _ => {
                    let want = if closed[mine] {
                        Err(Error::Closed)
                    } else {
                        let empty = if closed[theirs] {
                            Error::Closed
                        } else {
                            Error::Empty
                        };
                        model[mine].pop_front().ok_or(empty)
                    };
                    assert_eq!(read(&mut c, end), want, "step {step}");
                }
            }
            assert_eq!(c.waiting(End::A), model[0].len(), "step {step}");
            assert_eq!(c.waiting(End::B), model[1].len(), "step {step}");
            assert_eq!(c.finished(), closed == [true, true], "step {step}");
        }
    }
}
