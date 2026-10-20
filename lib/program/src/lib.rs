//! What a Meridian program written in Rust is built on: the system calls
//! as functions, the start the kernel jumps to, a heap for `alloc`, a log
//! line built with `format_args!`, and what a panic does. A program names
//! its main function with [`main!`]; main gets the handle the kernel
//! started it with, and what it returns is the program's exit code.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::alloc::Layout;
use core::fmt;

pub use abi;
pub use abi::Error;
use abi::{Call, LOG_BYTES, answer};
pub use security;
use security::{Restriction, Token};

#[cfg(feature = "qemu-test")]
pub mod checks;
#[allow(unsafe_code)]
mod raw;

pub use raw::Allocator;

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

/// Asks for `pages` fresh pages, zeroed and writable, and returns where
/// they start. The heap takes its memory from here.
///
/// # Errors
///
/// For 0 pages or more than one call gives, and when no memory or no room
/// in the program's heap region is left.
pub fn map(pages: u64) -> Result<u64, Error> {
    answer(raw::call(Call::Map(pages)).0)
}

/// Gives back the `pages` pages at `at`, which [`map`] gave.
///
/// # Errors
///
/// When they are not all pages [`map`] gave and still there.
pub fn unmap(at: u64, pages: u64) -> Result<(), Error> {
    done(raw::call(Call::Unmap { at, pages }).0)
}

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// It exited with this code.
    Exited(u64),
    /// The exception with this vector ended it.
    Faulted(u64),
}

/// Starts the program whose file is called `name`, such as
/// `hello.mexec`, as a process of its own, handing it `carry` if there is
/// one: the program no longer holds that handle, and the new one gets it
/// as its argument. Returns a handle to the process.
///
/// # Errors
///
/// When no file has the name, it is no program the kernel runs, `carry` is
/// not the program's or may not be handed over, or there is no room for
/// the process.
pub fn spawn(name: &str, carry: Option<Handle>) -> Result<Handle, Error> {
    answer(raw::call(spawn_call(name, carry, 0)).0).map(Handle)
}

/// Starts the program as [`spawn`] does, with no more than `restriction`
/// leaves of the program's own security token.
///
/// # Errors
///
/// As for [`spawn`].
pub fn spawn_restricted(
    name: &str,
    carry: Option<Handle>,
    restriction: Restriction,
) -> Result<Handle, Error> {
    answer(raw::call(spawn_call(name, carry, restriction.word())).0).map(Handle)
}

/// What INFO says about a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub size: u64,
    pub directory: bool,
}

/// Opens the file at `path` for the rights `wanted`, those of
/// [`abi::file`], and returns a handle to it.
///
/// # Errors
///
/// When the path is none or too long, no file has it, the program's token
/// may not open it so, or the disk cannot be read.
pub fn open(path: &str, wanted: u32) -> Result<Handle, Error> {
    answer(raw::call(open_call(path, wanted)).0).map(Handle)
}

/// Reads the file `file` from its byte `offset` on into `room`, as much as
/// one call reads, and returns how many bytes it read: 0 at its end.
///
/// # Errors
///
/// When the handle is not the program's, may not read, or stands for a
/// directory, or the disk cannot be read.
pub fn read(file: Handle, offset: u64, room: &mut [u8]) -> Result<usize, Error> {
    let len = answer(raw::call(read_call(file, offset, room)).0)?;
    Ok(usize::try_from(len).unwrap_or(usize::MAX))
}

/// How big the file `file` is, and whether it is a directory.
///
/// # Errors
///
/// When the handle is not the program's or stands for no file.
pub fn info(file: Handle) -> Result<Info, Error> {
    let (rax, rdx) = raw::call(Call::Info(file.0));
    info_from(rax, rdx)
}

fn open_call(path: &str, wanted: u32) -> Call {
    Call::Open {
        len: path.len() as u64,
        at: path.as_ptr() as u64,
        wanted: u64::from(wanted),
    }
}

fn read_call(file: Handle, offset: u64, room: &mut [u8]) -> Call {
    Call::Read {
        handle: file.0,
        at: room.as_mut_ptr() as u64,
        room: room.len() as u64,
        offset,
    }
}

fn info_from(rax: u64, rdx: u64) -> Result<Info, Error> {
    Ok(Info {
        size: answer(rax)?,
        directory: rdx != 0,
    })
}

/// The program's security token; none only when the kernel and this
/// runtime disagree on how one is written.
#[must_use]
pub fn token() -> Option<Token> {
    let mut room = [0u8; Token::MAX_SIZE];
    let rax = raw::call(token_call(&mut room)).0;
    token_from(rax, &room)
}

fn token_call(room: &mut [u8]) -> Call {
    Call::Token {
        room: room.len() as u64,
        at: room.as_mut_ptr() as u64,
    }
}

/// The token TOKEN answered with `rax` wrote into `room`.
fn token_from(rax: u64, room: &[u8]) -> Option<Token> {
    let len = usize::try_from(answer(rax).ok()?).ok()?;
    Token::read(room.get(..len)?)
}

/// Waits until the process `process` ended, and says how.
///
/// # Errors
///
/// When the handle is not the program's or stands for something else.
pub fn wait(process: Handle) -> Result<Ended, Error> {
    let (rax, rdx) = raw::call(Call::Wait(process.0));
    ended(rax, rdx)
}

fn spawn_call(name: &str, carry: Option<Handle>, restriction: u64) -> Call {
    Call::Spawn {
        len: name.len() as u64,
        at: name.as_ptr() as u64,
        carry: carry.map_or(0, |h| h.0),
        restriction,
    }
}

fn ended(rax: u64, rdx: u64) -> Result<Ended, Error> {
    let value = answer(rax)?;
    Ok(if rdx == 0 {
        Ended::Exited(value)
    } else {
        Ended::Faulted(value)
    })
}

/// The largest alignment the heap gives: that of the pages it lies on.
pub const ALIGN_MAX: usize = 4096;

/// The heap's bookkeeping over memory at `base`: which of its blocks are
/// taken, in bits kept elsewhere, and where each allocation lies.
pub struct Pool<'a> {
    base: usize,
    heap: heap::Heap<'a>,
}

impl<'a> Pool<'a> {
    /// A pool over the `size` bytes at `base`, all free, its bits kept in
    /// `bits`, which need [`heap::Heap::words_for`] words.
    pub fn new(base: usize, size: usize, bits: &'a mut [u64]) -> Pool<'a> {
        Pool {
            base,
            heap: heap::Heap::new(bits, size),
        }
    }

    /// Where room for `layout` lies, now taken, or none when there is no
    /// room or it asks for more than [`ALIGN_MAX`].
    pub fn alloc(&mut self, layout: Layout) -> Option<usize> {
        if layout.align() > ALIGN_MAX {
            return None;
        }
        let offset = self.heap.alloc(layout.size(), layout.align())?;
        Some(self.base + offset)
    }

    /// Gives back the room for `layout` at `at`, which [`Pool::alloc`]
    /// handed out. Anything else is left alone.
    pub fn free(&mut self, at: usize, layout: Layout) {
        if let Some(offset) = at.checked_sub(self.base) {
            let _ = self.heap.free(offset, layout.size());
        }
    }
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
    fn a_program_is_spawned_by_its_name() {
        let name = "hello.mexec";
        assert_eq!(
            spawn_call(name, None, 0),
            Call::Spawn {
                len: 11,
                at: name.as_ptr() as u64,
                carry: 0,
                restriction: 0
            }
        );
        let low = Restriction {
            integrity: security::Integrity::Low,
            keep: security::Privileges::NONE,
        };
        assert_eq!(
            spawn_call(&name[..5], Some(Handle(0x2_0003)), low.word()),
            Call::Spawn {
                len: 5,
                at: name.as_ptr() as u64,
                carry: 0x2_0003,
                restriction: 0x8000_0001_0000_0000
            }
        );
    }

    #[test]
    fn a_file_is_opened_by_its_path_and_read_where_asked() {
        let path = "/System/Boot/meridian/boot.meri";
        assert_eq!(
            open_call(path, abi::file::READ | abi::file::RUN),
            Call::Open {
                len: 31,
                at: path.as_ptr() as u64,
                wanted: 0x21
            }
        );
        let mut room = [0u8; 100];
        assert_eq!(
            read_call(Handle(0x3_0001), 4096, &mut room[10..]),
            Call::Read {
                handle: 0x3_0001,
                at: room[10..].as_ptr() as u64,
                room: 90,
                offset: 4096
            }
        );
        assert_eq!(
            info_from(64, 0),
            Ok(Info {
                size: 64,
                directory: false
            })
        );
        assert_eq!(
            info_from(0, 1),
            Ok(Info {
                size: 0,
                directory: true
            })
        );
        assert_eq!(info_from(abi::BAD_HANDLE, 1), Err(Error::BadHandle));
    }

    #[test]
    fn the_token_is_read_from_the_room_it_was_written_to() {
        let mut room = [0u8; Token::MAX_SIZE];
        assert_eq!(
            token_call(&mut room),
            Call::Token {
                room: Token::MAX_SIZE as u64,
                at: room.as_ptr() as u64
            }
        );
        let system = Token::system();
        let len = system.write(&mut room).unwrap();
        assert_eq!(token_from(len as u64, &room), Some(system));
        assert_eq!(token_from(len as u64 - 1, &room), None, "one byte short");
        assert_eq!(
            token_from(Token::MAX_SIZE as u64 + 1, &room),
            None,
            "more than there is room"
        );
        assert_eq!(token_from(abi::TOO_SMALL, &room), None);
        assert_eq!(token_from(abi::BAD_ADDRESS, &room), None);
    }

    #[test]
    fn how_a_process_ended_reads_back() {
        assert_eq!(ended(0, 0), Ok(Ended::Exited(0)));
        assert_eq!(ended(42, 0), Ok(Ended::Exited(42)));
        assert_eq!(ended(14, 1), Ok(Ended::Faulted(14)));
        assert_eq!(ended(abi::BAD_HANDLE, 0), Err(Error::BadHandle));
        assert_eq!(
            ended(abi::DENIED, 1),
            Err(Error::Denied),
            "an error wins over rdx"
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

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn the_heap_hands_out_addresses_in_its_memory() {
        let mut bits = [0u64; 4];
        let base = 0x10_0000_0000;
        let mut pool = Pool::new(base, 4096, &mut bits);
        let first = pool.alloc(layout(24, 8)).unwrap();
        let second = pool.alloc(layout(8, 8)).unwrap();
        assert_eq!(first, base, "the first room is at the start");
        assert_eq!(second, base + 32, "24 bytes take two blocks of 16");
        let page = pool.alloc(layout(16, 1024)).unwrap();
        assert_eq!(page % 1024, 0);
        assert!((base..base + 4096).contains(&page));
    }

    #[test]
    fn room_given_back_is_handed_out_again() {
        let mut bits = [0u64; 4];
        let base = 0x20_0000;
        let mut pool = Pool::new(base, 4096, &mut bits);
        let all = pool.alloc(layout(4096, 16)).unwrap();
        assert_eq!(pool.alloc(layout(1, 1)), None, "the memory is used up");
        pool.free(all, layout(4096, 16));
        assert_eq!(pool.alloc(layout(1, 1)), Some(base));
        pool.free(base - 16, layout(16, 16));
        pool.free(base + 8192, layout(16, 16));
        assert_eq!(
            pool.alloc(layout(16, 16)),
            Some(base + 16),
            "foreign frees change nothing"
        );
    }

    #[test]
    fn the_heap_aligns_no_further_than_its_pages() {
        let mut bits = [0u64; 32];
        let mut pool = Pool::new(0x40_0000, 32 * 1024, &mut bits);
        assert!(
            pool.alloc(layout(8, ALIGN_MAX))
                .is_some_and(|a| a % ALIGN_MAX == 0)
        );
        assert_eq!(pool.alloc(layout(8, ALIGN_MAX * 2)), None);
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
