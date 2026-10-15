//! System calls, the parts that need no processor: user code puts the
//! call's number in rax and its arguments in rdi, rsi, rdx and r10, runs
//! `int 0x2e`, and finds the answer in rax. Answers from the top of the
//! range are errors.

/// Ends the calling thread; rdi is its exit code.
pub const EXIT: u64 = 0;
/// Writes a line to the kernel's log: rdi bytes at rsi are its text.
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

/// The call does not exist.
pub const NO_SUCH_CALL: u64 = u64::MAX;
/// An address the caller gave is not its own to give.
pub const BAD_ADDRESS: u64 = u64::MAX - 1;
/// A line is longer than [`LOG_MAX`].
pub const TOO_LONG: u64 = u64::MAX - 2;
/// No handle of the caller's has this number.
pub const BAD_HANDLE: u64 = u64::MAX - 3;
/// The handle does not allow what was asked, or stands for something else.
pub const DENIED: u64 = u64::MAX - 4;
/// The channel holds as many messages as it takes.
pub const FULL: u64 = u64::MAX - 5;
/// No message waits.
pub const EMPTY: u64 = u64::MAX - 6;
/// The other end of the channel is closed.
pub const CLOSED: u64 = u64::MAX - 7;
/// The message waiting is longer than the room given for it; it stays.
pub const TOO_SMALL: u64 = u64::MAX - 8;
/// No room is left for another channel or handle.
pub const NO_ROOM: u64 = u64::MAX - 9;

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
    Unknown(u64),
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
        assert_eq!(decode(9, [1, 2, 3, 0]), Call::Unknown(9));
        assert_eq!(decode(u64::MAX, [1, 2, 3, 0]), Call::Unknown(u64::MAX));
        assert_eq!(
            [
                EXIT, LOG, YIELD, SLEEP, TICKS, CHANNEL, SEND, RECEIVE, CLOSE
            ],
            [0, 1, 2, 3, 4, 5, 6, 7, 8]
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
