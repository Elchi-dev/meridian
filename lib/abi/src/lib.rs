//! What programs and the kernel agree on. System calls: user code puts the
//! call's number in rax and its arguments in rdi, rsi, rdx and r10, runs
//! `int 0x2e`, and finds the answer in rax. Answers from the top of the
//! range are errors.

#![cfg_attr(not(test), no_std)]

/// Ends the calling thread; rdi is its exit code.
pub const EXIT: u64 = 0;
/// Writes a line to the kernel's log: rdi bytes at rsi are its text, and
/// any byte but printable ASCII shows as a question mark.
pub const LOG: u64 = 1;
/// Lets other threads of the same or a higher priority run first.
pub const YIELD: u64 = 2;
/// Sleeps for rdi timer ticks.
pub const SLEEP: u64 = 3;
/// Answers how many times the timer has ticked.
pub const TICKS: u64 = 4;
/// Makes a channel: the handle of one end comes back in rax, the other's
/// in rdx.
pub const CHANNEL: u64 = 5;
/// Sends rdx bytes at rsi through the channel end of handle rdi, handing
/// over handle r10 with them unless r10 is 0.
pub const SEND: u64 = 6;
/// Takes the next message for the channel end of handle rdi into the rdx
/// bytes at rsi, and answers its length; a handle it brought comes back in
/// rdx, or 0.
pub const RECEIVE: u64 = 7;
/// Closes handle rdi.
pub const CLOSE: u64 = 8;
/// Gives the program rdi fresh pages of memory, zeroed, writable and not
/// executable, and answers where they start.
pub const MAP: u64 = 9;
/// Gives back the rsi pages at rdi that [`MAP`] gave.
pub const UNMAP: u64 = 10;

/// Why a call failed. Each error is an answer counting down from the top
/// of the range, so no address, length or handle is mistaken for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The call does not exist.
    NoSuchCall,
    /// An address the caller gave is not its own to give.
    BadAddress,
    /// A line is longer than [`LOG_MAX`], or a message longer than a
    /// channel takes.
    TooLong,
    /// No handle of the caller's has this number.
    BadHandle,
    /// The handle does not allow what was asked, or stands for something
    /// else.
    Denied,
    /// The channel holds as many messages as it takes.
    Full,
    /// No message waits.
    Empty,
    /// The other end of the channel is closed.
    Closed,
    /// The message waiting is longer than the room given for it; it stays.
    TooSmall,
    /// No room is left for another channel or handle, or no memory for
    /// more pages.
    NoRoom,
    /// The count of pages is 0, or more than one call takes.
    BadSize,
}

const ERRORS: [Error; 11] = [
    Error::NoSuchCall,
    Error::BadAddress,
    Error::TooLong,
    Error::BadHandle,
    Error::Denied,
    Error::Full,
    Error::Empty,
    Error::Closed,
    Error::TooSmall,
    Error::NoRoom,
    Error::BadSize,
];

impl Error {
    /// The answer in rax that stands for this error.
    #[must_use]
    pub const fn code(self) -> u64 {
        u64::MAX - self as u64
    }
}

/// Reads the answer a call left in rax.
///
/// # Errors
///
/// The error the answer stands for, when it is one of [`Error`]'s codes.
pub fn answer(rax: u64) -> Result<u64, Error> {
    match ERRORS.into_iter().find(|e| e.code() == rax) {
        Some(error) => Err(error),
        None => Ok(rax),
    }
}

pub const NO_SUCH_CALL: u64 = Error::NoSuchCall.code();
pub const BAD_ADDRESS: u64 = Error::BadAddress.code();
pub const TOO_LONG: u64 = Error::TooLong.code();
pub const BAD_HANDLE: u64 = Error::BadHandle.code();
pub const DENIED: u64 = Error::Denied.code();
pub const FULL: u64 = Error::Full.code();
pub const EMPTY: u64 = Error::Empty.code();
pub const CLOSED: u64 = Error::Closed.code();
pub const TOO_SMALL: u64 = Error::TooSmall.code();
pub const NO_ROOM: u64 = Error::NoRoom.code();
pub const BAD_SIZE: u64 = Error::BadSize.code();

/// The longest line the log takes, in bytes.
pub const LOG_BYTES: usize = 256;
pub const LOG_MAX: u64 = LOG_BYTES as u64;

/// Where the lower half, the one user code has, ends.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// A system call, read from the registers it came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Exit(u64),
    Log {
        len: u64,
        at: u64,
    },
    Yield,
    Sleep(u64),
    Ticks,
    Channel,
    Send {
        handle: u64,
        at: u64,
        len: u64,
        carry: u64,
    },
    Receive {
        handle: u64,
        at: u64,
        room: u64,
    },
    Close(u64),
    Map(u64),
    Unmap {
        at: u64,
        pages: u64,
    },
    Unknown(u64),
}

impl Call {
    /// The number and the registers rdi, rsi, rdx and r10 that ask for
    /// this call, the way [`decode`] reads them. Registers the call does
    /// not use are 0, so a later call can give them a meaning.
    #[must_use]
    pub fn encode(self) -> (u64, [u64; 4]) {
        match self {
            Call::Exit(code) => (EXIT, [code, 0, 0, 0]),
            Call::Log { len, at } => (LOG, [len, at, 0, 0]),
            Call::Yield => (YIELD, [0; 4]),
            Call::Sleep(ticks) => (SLEEP, [ticks, 0, 0, 0]),
            Call::Ticks => (TICKS, [0; 4]),
            Call::Channel => (CHANNEL, [0; 4]),
            Call::Send {
                handle,
                at,
                len,
                carry,
            } => (SEND, [handle, at, len, carry]),
            Call::Receive { handle, at, room } => (RECEIVE, [handle, at, room, 0]),
            Call::Close(handle) => (CLOSE, [handle, 0, 0, 0]),
            Call::Map(pages) => (MAP, [pages, 0, 0, 0]),
            Call::Unmap { at, pages } => (UNMAP, [at, pages, 0, 0]),
            Call::Unknown(number) => (number, [0; 4]),
        }
    }
}

/// The call `number` asks for, with `args` as rdi, rsi, rdx and r10.
#[must_use]
pub fn decode(number: u64, args: [u64; 4]) -> Call {
    let [first, second, third, fourth] = args;
    match number {
        EXIT => Call::Exit(first),
        LOG => Call::Log {
            len: first,
            at: second,
        },
        YIELD => Call::Yield,
        SLEEP => Call::Sleep(first),
        TICKS => Call::Ticks,
        CHANNEL => Call::Channel,
        SEND => Call::Send {
            handle: first,
            at: second,
            len: third,
            carry: fourth,
        },
        RECEIVE => Call::Receive {
            handle: first,
            at: second,
            room: third,
        },
        CLOSE => Call::Close(first),
        MAP => Call::Map(first),
        UNMAP => Call::Unmap {
            at: first,
            pages: second,
        },
        other => Call::Unknown(other),
    }
}

/// Whether the `len` bytes from `at` lie in the lower half, the user's,
/// without wrapping around.
#[must_use]
pub fn in_user_half(at: u64, len: u64) -> bool {
    at.checked_add(len).is_some_and(|end| end <= USER_END)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_come_from_their_registers() {
        assert_eq!(decode(EXIT, [3, 9, 8, 0]), Call::Exit(3));
        assert_eq!(
            decode(LOG, [12, 0x40_0100, 8, 0]),
            Call::Log {
                len: 12,
                at: 0x40_0100
            }
        );
        assert_eq!(decode(YIELD, [1, 2, 3, 0]), Call::Yield);
        assert_eq!(decode(SLEEP, [25, 2, 3, 0]), Call::Sleep(25));
        assert_eq!(decode(TICKS, [1, 2, 3, 0]), Call::Ticks);
        assert_eq!(decode(CHANNEL, [1, 2, 3, 0]), Call::Channel);
        assert_eq!(
            decode(SEND, [0x1_0001, 0x7f_f000, 4, 0x2_0002]),
            Call::Send {
                handle: 0x1_0001,
                at: 0x7f_f000,
                len: 4,
                carry: 0x2_0002
            }
        );
        assert_eq!(
            decode(RECEIVE, [2, 0x7f_f100, 128, 0]),
            Call::Receive {
                handle: 2,
                at: 0x7f_f100,
                room: 128
            }
        );
        assert_eq!(decode(CLOSE, [7, 2, 3, 0]), Call::Close(7));
        assert_eq!(decode(MAP, [16, 2, 3, 0]), Call::Map(16));
        assert_eq!(
            decode(UNMAP, [0x10_0000_0000, 4, 3, 0]),
            Call::Unmap {
                at: 0x10_0000_0000,
                pages: 4
            }
        );
        assert_eq!(decode(11, [1, 2, 3, 0]), Call::Unknown(11));
        assert_eq!(decode(u64::MAX, [1, 2, 3, 0]), Call::Unknown(u64::MAX));
        assert_eq!(
            [
                EXIT, LOG, YIELD, SLEEP, TICKS, CHANNEL, SEND, RECEIVE, CLOSE, MAP, UNMAP
            ],
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
    }

    #[test]
    fn errors_sit_at_the_top_where_no_answer_reaches() {
        let errors = [
            NO_SUCH_CALL,
            BAD_ADDRESS,
            TOO_LONG,
            BAD_HANDLE,
            DENIED,
            FULL,
            EMPTY,
            CLOSED,
            TOO_SMALL,
            NO_ROOM,
            BAD_SIZE,
        ];
        for (i, &e) in errors.iter().enumerate() {
            assert_eq!(
                e,
                u64::MAX - i as u64,
                "each its own, counting down from the top"
            );
            assert!(
                e > USER_END,
                "no address, length or handle is mistaken for an error"
            );
        }
    }

    #[test]
    fn every_call_reads_back_as_it_was_written() {
        let calls = [
            Call::Exit(3),
            Call::Log {
                len: 12,
                at: 0x40_0100,
            },
            Call::Yield,
            Call::Sleep(25),
            Call::Ticks,
            Call::Channel,
            Call::Send {
                handle: 0x1_0001,
                at: 0x7f_f000,
                len: 4,
                carry: 0x2_0002,
            },
            Call::Receive {
                handle: 2,
                at: 0x7f_f100,
                room: 128,
            },
            Call::Close(7),
            Call::Map(32),
            Call::Unmap {
                at: 0x10_0000_2000,
                pages: 3,
            },
            Call::Unknown(11),
        ];
        for call in calls {
            let (number, args) = call.encode();
            assert_eq!(
                decode(number, args),
                call,
                "{call:?} as {number}, {args:x?}"
            );
        }
    }

    #[test]
    fn each_call_reads_exactly_the_registers_it_names() {
        let used = [
            (EXIT, 1),
            (LOG, 2),
            (YIELD, 0),
            (SLEEP, 1),
            (TICKS, 0),
            (CHANNEL, 0),
            (SEND, 4),
            (RECEIVE, 3),
            (CLOSE, 1),
            (MAP, 1),
            (UNMAP, 2),
        ];
        let args = [11, 22, 33, 44];
        for (number, count) in used {
            let call = decode(number, args);
            for register in 0..4 {
                let mut changed = args;
                changed[register] += 100;
                assert_eq!(
                    decode(number, changed) != call,
                    register < count,
                    "call {number}, register {register}"
                );
            }
        }
    }

    #[test]
    fn the_calls_are_numbered_without_gaps() {
        for number in 0..64 {
            let call = decode(number, [0; 4]);
            assert_eq!(
                call == Call::Unknown(number),
                number > UNMAP,
                "call {number}"
            );
            assert_eq!(call.encode(), (number, [0; 4]), "call {number}");
        }
        for error in ERRORS {
            assert_eq!(decode(error.code(), [0; 4]), Call::Unknown(error.code()));
        }
    }

    #[test]
    fn calls_leave_the_registers_they_do_not_use_at_zero() {
        assert_eq!(Call::Exit(3).encode(), (EXIT, [3, 0, 0, 0]));
        assert_eq!(Call::Log { len: 5, at: 6 }.encode(), (LOG, [5, 6, 0, 0]));
        assert_eq!(Call::Yield.encode(), (YIELD, [0; 4]));
        assert_eq!(Call::Sleep(1).encode(), (SLEEP, [1, 0, 0, 0]));
        assert_eq!(Call::Ticks.encode(), (TICKS, [0; 4]));
        assert_eq!(Call::Channel.encode(), (CHANNEL, [0; 4]));
        let receive = Call::Receive {
            handle: 1,
            at: 2,
            room: 3,
        };
        assert_eq!(receive.encode(), (RECEIVE, [1, 2, 3, 0]));
        assert_eq!(Call::Close(4).encode(), (CLOSE, [4, 0, 0, 0]));
        assert_eq!(Call::Map(5).encode(), (MAP, [5, 0, 0, 0]));
        assert_eq!(
            Call::Unmap { at: 6, pages: 7 }.encode(),
            (UNMAP, [6, 7, 0, 0])
        );
        assert_eq!(Call::Unknown(40).encode(), (40, [0; 4]));
    }

    #[test]
    fn answers_are_errors_only_at_the_top() {
        for (i, error) in ERRORS.into_iter().enumerate() {
            assert_eq!(answer(u64::MAX - i as u64), Err(error));
        }
        assert_eq!(answer(u64::MAX), Err(Error::NoSuchCall));
        assert_eq!(answer(BAD_SIZE), Err(Error::BadSize));
        assert_eq!(answer(BAD_SIZE - 1), Ok(BAD_SIZE - 1), "just below them");
        assert_eq!(answer(0), Ok(0));
        assert_eq!(answer(0x4_0003), Ok(0x4_0003), "a handle");
        assert_eq!(answer(USER_END), Ok(USER_END));
        assert_eq!(
            answer(0x10_0000_0000),
            Ok(0x10_0000_0000),
            "where MAP puts memory"
        );
        assert_eq!(
            answer(USER_END - 4096),
            Ok(USER_END - 4096),
            "the last page there is"
        );
    }

    #[test]
    fn each_error_has_the_code_the_kernel_answers_with() {
        let codes = [
            (Error::NoSuchCall, NO_SUCH_CALL),
            (Error::BadAddress, BAD_ADDRESS),
            (Error::TooLong, TOO_LONG),
            (Error::BadHandle, BAD_HANDLE),
            (Error::Denied, DENIED),
            (Error::Full, FULL),
            (Error::Empty, EMPTY),
            (Error::Closed, CLOSED),
            (Error::TooSmall, TOO_SMALL),
            (Error::NoRoom, NO_ROOM),
            (Error::BadSize, BAD_SIZE),
        ];
        for (error, code) in codes {
            assert_eq!(error.code(), code, "{error:?}");
            assert_eq!(answer(code), Err(error));
        }
    }

    #[test]
    fn only_the_lower_half_is_the_users() {
        assert!(in_user_half(0x40_0000, 4096));
        assert!(in_user_half(0, 0));
        assert!(in_user_half(USER_END - 1, 1), "the last byte of the half");
        assert!(in_user_half(USER_END, 0), "nothing at the end");
        assert!(!in_user_half(USER_END - 1, 2), "one byte past the half");
        assert!(!in_user_half(USER_END, 1));
        assert!(
            !in_user_half(0xffff_8000_0000_0000, 8),
            "the kernel's direct map"
        );
        assert!(!in_user_half(0x40_0000, u64::MAX), "wrapping around");
        assert!(!in_user_half(u64::MAX, 1));
    }
}
