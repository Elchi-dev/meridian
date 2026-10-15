//! What a Meridian program written in Rust is built on: the system calls
//! as functions, the start the kernel jumps to, a log line built with
//! `format_args!`, and what a panic does. A program names its main function
//! with [`main!`]; main gets the handle the kernel started it with, and
//! what it returns is the program's exit code.

#![cfg_attr(not(test), no_std)]

use core::fmt;

pub use abi::Error;
use abi::{Call, LOG_BYTES, answer};

#[cfg(feature = "qemu-test")]
pub mod checks;
#[allow(unsafe_code)]
mod raw;

/// A handle of the program's, standing for a kernel object such as a
/// channel end. Its number is never 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub u64);

/// The exit code of a program that panicked.
pub const PANICKED: u64 = 101;

/// Ends the program with `code`.
pub fn exit(code: u64) -> ! {
    raw::call(Call::Exit(code));
    // The kernel never comes back from an exit.
    loop {
        core::hint::spin_loop();
    }
}

/// Writes `line` to the kernel's log.
///
/// # Errors
///
/// When the line is longer than the log takes.
pub fn log(line: &str) -> Result<(), Error> {
    done(raw::call(log_call(line)).0)
}

/// Lets other threads of the same or a higher priority run first.
pub fn yield_now() {
    raw::call(Call::Yield);
}

/// Sleeps for `ticks` timer ticks.
pub fn sleep(ticks: u64) {
    raw::call(Call::Sleep(ticks));
}

/// How many times the timer has ticked.
#[must_use]
pub fn ticks() -> u64 {
    raw::call(Call::Ticks).0
}

/// Makes a channel, and returns the handles of its two ends.
///
/// # Errors
///
/// When the kernel has no room for another channel, or the program none
/// for two more handles.
pub fn channel() -> Result<(Handle, Handle), Error> {
    let (rax, rdx) = raw::call(Call::Channel);
    pair(rax, rdx)
}

/// Sends `message` through the channel end `end`, and hands over `carry`
/// with it, if there is one: the program no longer holds that handle.
///
/// # Errors
///
/// When a handle is not the program's or does not allow it, the message
/// is too long, the channel is full, or its other end is closed.
pub fn send(end: Handle, message: &[u8], carry: Option<Handle>) -> Result<(), Error> {
    done(raw::call(send_call(end, message, carry)).0)
}

/// Takes the next message for the channel end `end` into `room`, and
/// returns its length and the handle it brought, if any.
///
/// # Errors
///
/// When the handle is not the program's or does not allow it, no message
/// waits, the one waiting does not fit into `room`, or the other end is
/// closed and nothing is left.
pub fn receive(end: Handle, room: &mut [u8]) -> Result<(usize, Option<Handle>), Error> {
    let (rax, rdx) = raw::call(receive_call(end, room));
    received(rax, rdx)
}

/// Closes `handle`.
///
/// # Errors
///
/// When the handle is not the program's.
pub fn close(handle: Handle) -> Result<(), Error> {
    done(raw::call(Call::Close(handle.0)).0)
}

fn log_call(line: &str) -> Call {
    Call::Log {
        len: line.len() as u64,
        at: line.as_ptr() as u64,
    }
}

fn send_call(end: Handle, message: &[u8], carry: Option<Handle>) -> Call {
    Call::Send {
        handle: end.0,
        at: message.as_ptr() as u64,
        len: message.len() as u64,
        carry: carry.map_or(0, |h| h.0),
    }
}

fn receive_call(end: Handle, room: &mut [u8]) -> Call {
    Call::Receive {
        handle: end.0,
        at: room.as_mut_ptr() as u64,
        room: room.len() as u64,
    }
}

fn done(rax: u64) -> Result<(), Error> {
    answer(rax).map(|_| ())
}

fn pair(rax: u64, rdx: u64) -> Result<(Handle, Handle), Error> {
    answer(rax).map(|first| (Handle(first), Handle(rdx)))
}

fn received(rax: u64, rdx: u64) -> Result<(usize, Option<Handle>), Error> {
    let len = answer(rax)?;
    let carried = (rdx != 0).then_some(Handle(rdx));
    Ok((usize::try_from(len).unwrap_or(usize::MAX), carried))
}

/// A line for the log, written to with `write!`. What does not fit into
/// the [`LOG_BYTES`] the log takes is cut off after the last character
/// that does.
pub struct Line {
    bytes: [u8; LOG_BYTES],
    len: usize,
}

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}

impl Line {
    #[must_use]
    pub const fn new() -> Self {
        Line {
            bytes: [0; LOG_BYTES],
            len: 0,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut take = s.len().min(LOG_BYTES - self.len);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Writes a line to the kernel's log the way `format!` builds a string,
/// and answers as [`log`] does.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {{
        let mut line = $crate::Line::new();
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!($($arg)*));
        $crate::log(line.as_str())
    }};
}

/// Logs where and why the program panicked, and ends it with
/// [`PANICKED`]. The panic handler [`main!`] gives a program calls it.
pub fn panicked(info: &core::panic::PanicInfo) -> ! {
    let _ = if let Some(at) = info.location() {
        log!("panic at {}:{}: {}", at.file(), at.line(), info.message())
    } else {
        log!("panic: {}", info.message())
    };
    exit(PANICKED)
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use super::*;

    #[test]
    fn a_log_line_is_given_by_where_it_is_and_how_long() {
        let line = "hello";
        assert_eq!(
            log_call(line),
            Call::Log {
                len: 5,
                at: line.as_ptr() as u64
            }
        );
        assert_eq!(
            log_call(""),
            Call::Log {
                len: 0,
                at: "".as_ptr() as u64
            }
        );
    }

    #[test]
    fn a_message_goes_with_its_handle_or_with_0() {
        let message = [1u8, 2, 3];
        let at = message.as_ptr() as u64;
        assert_eq!(
            send_call(Handle(7), &message, None),
            Call::Send {
                handle: 7,
                at,
                len: 3,
                carry: 0
            }
        );
        assert_eq!(
            send_call(Handle(7), &message[1..], Some(Handle(0x2_0004))),
            Call::Send {
                handle: 7,
                at: at + 1,
                len: 2,
                carry: 0x2_0004
            }
        );
    }

    #[test]
    fn a_message_is_received_into_the_room_given() {
        let mut room = [0u8; 64];
        let at = room.as_ptr() as u64;
        assert_eq!(
            receive_call(Handle(3), &mut room),
            Call::Receive {
                handle: 3,
                at,
                room: 64
            }
        );
        assert_eq!(
            receive_call(Handle(3), &mut room[60..]),
            Call::Receive {
                handle: 3,
                at: at + 60,
                room: 4
            }
        );
    }

    #[test]
    fn answers_read_back_as_results() {
        assert_eq!(done(0), Ok(()));
        assert_eq!(done(abi::FULL), Err(Error::Full));
        assert_eq!(
            pair(0x1_0001, 0x1_0002),
            Ok((Handle(0x1_0001), Handle(0x1_0002)))
        );
        assert_eq!(pair(abi::NO_ROOM, 0x1_0002), Err(Error::NoRoom));
        assert_eq!(received(12, 0), Ok((12, None)));
        assert_eq!(received(0, 0x3_0005), Ok((0, Some(Handle(0x3_0005)))));
        assert_eq!(received(abi::EMPTY, 0x3_0005), Err(Error::Empty));
        assert_eq!(received(abi::TOO_SMALL, 0), Err(Error::TooSmall));
    }

    #[test]
    fn a_line_holds_what_it_was_given() {
        let mut line = Line::new();
        assert_eq!(line.as_str(), "");
        let (ticks, word) = (42, "late");
        write!(line, "ticks: {ticks}, {word}").unwrap();
        assert_eq!(line.as_str(), "ticks: 42, late");
        write!(line, "!").unwrap();
        assert_eq!(line.as_str(), "ticks: 42, late!");
    }

    #[test]
    fn a_long_line_is_cut_where_the_log_ends() {
        let mut line = Line::new();
        for _ in 0..LOG_BYTES + 10 {
            line.write_str("a").unwrap();
        }
        assert_eq!(line.as_str().len(), LOG_BYTES);
        assert!(line.as_str().bytes().all(|b| b == b'a'));
        let mut exact = Line::new();
        exact.write_str(&"b".repeat(LOG_BYTES)).unwrap();
        assert_eq!(exact.as_str().len(), LOG_BYTES, "a full line loses nothing");
    }

    #[test]
    fn a_line_is_never_cut_inside_a_character() {
        let mut line = Line::new();
        line.write_str(&"a".repeat(LOG_BYTES - 1)).unwrap();
        line.write_str("\u{e9}z").unwrap();
        assert_eq!(
            line.as_str().len(),
            LOG_BYTES - 1,
            "two bytes do not fit into one"
        );
        let mut fits = Line::new();
        fits.write_str(&"a".repeat(LOG_BYTES - 2)).unwrap();
        fits.write_str("\u{e9}z").unwrap();
        assert!(fits.as_str().ends_with('\u{e9}'));
        assert_eq!(fits.as_str().len(), LOG_BYTES);
    }
}
