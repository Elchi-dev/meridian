//! System calls, the parts that need no processor: user code puts the
//! call's number in rax and its arguments in rdi and rsi, runs `int 0x2e`,
//! and finds the answer in rax. Answers from the top of the range are
//! errors.

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

/// The call does not exist.
pub const NO_SUCH_CALL: u64 = u64::MAX;
/// An address the caller gave is not its own to give.
pub const BAD_ADDRESS: u64 = u64::MAX - 1;
/// A line is longer than [`LOG_MAX`].
pub const TOO_LONG: u64 = u64::MAX - 2;

/// The longest line the log takes, in bytes.
pub const LOG_BYTES: usize = 256;
pub const LOG_MAX: u64 = LOG_BYTES as u64;

/// Where the lower half, the one user code has, ends.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// A system call, read from the registers it came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Exit(u64),
    Log { len: u64, at: u64 },
    Yield,
    Sleep(u64),
    Ticks,
    Unknown(u64),
}

/// The call `number` asks for, with `first` and `second` as rdi and rsi.
#[must_use]
pub fn decode(number: u64, first: u64, second: u64) -> Call {
    match number {
        EXIT => Call::Exit(first),
        LOG => Call::Log {
            len: first,
            at: second,
        },
        YIELD => Call::Yield,
        SLEEP => Call::Sleep(first),
        TICKS => Call::Ticks,
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
        assert_eq!(decode(EXIT, 3, 9), Call::Exit(3));
        assert_eq!(
            decode(LOG, 12, 0x40_0100),
            Call::Log {
                len: 12,
                at: 0x40_0100
            }
        );
        assert_eq!(decode(YIELD, 1, 2), Call::Yield);
        assert_eq!(decode(SLEEP, 25, 2), Call::Sleep(25));
        assert_eq!(decode(TICKS, 1, 2), Call::Ticks);
        assert_eq!(decode(5, 1, 2), Call::Unknown(5));
        assert_eq!(decode(u64::MAX, 1, 2), Call::Unknown(u64::MAX));
        assert_eq!([EXIT, LOG, YIELD, SLEEP, TICKS], [0, 1, 2, 3, 4]);
    }

    #[test]
    fn errors_sit_at_the_top_where_no_answer_reaches() {
        assert_eq!(
            [NO_SUCH_CALL, BAD_ADDRESS, TOO_LONG],
            [u64::MAX, u64::MAX - 1, u64::MAX - 2]
        );
        assert!(
            TOO_LONG > USER_END,
            "no address or count is mistaken for an error"
        );
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
