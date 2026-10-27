//! What a Meridian program written in Rust is built on: the system calls
//! as functions, the start the kernel jumps to, a heap for `alloc`, a log
//! line built with `format_args!`, and what a panic does. A program names
//! its main function with [`main!`]; main gets the handle the kernel
//! started it with, and what it returns is the program's exit code.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::boxed::Box;

use core::alloc::Layout;
use core::fmt;

pub use abi;
pub use abi::Error;
use abi::{Call, LOG_BYTES, answer};
pub use security;
use security::{Restriction, Token};

#[cfg(feature = "qemu-test")]
pub mod checks;
pub mod layout;
#[allow(unsafe_code)]
mod raw;
pub mod start;

pub use raw::{Allocator, arguments};

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

/// Ends the calling thread. Should it be its program's last, the program
/// ends with `code`.
pub fn end_thread(code: u64) -> ! {
    raw::call(Call::EndThread(code));
    // The kernel never comes back from the end of a thread.
    loop {
        core::hint::spin_loop();
    }
}

/// The pages of the stack [`thread`] gives each thread: 64 KiB.
pub const THREAD_STACK_PAGES: u64 = 16;

/// Starts another thread of this program that does `work`, on a stack of
/// [`THREAD_STACK_PAGES`] fresh pages with one left unmapped below, so a
/// thread that runs out of stack faults rather than writing over what lies
/// there. The stack stays until the program ends. The thread ends once
/// `work` returns, and the program once its last thread ended, with the
/// code that one returned; [`exit`] ends every thread at once.
///
/// # Errors
///
/// When there is no memory for the stack, no thread left, or the program
/// is ending.
pub fn thread(work: impl FnOnce() -> u64 + Send + 'static) -> Result<(), Error> {
    let guard = map(THREAD_STACK_PAGES + 1)?;
    let (stack, top) = thread_stack(guard);
    let started = unmap(guard, 1).and_then(|()| raw::start_thread(Box::new(work), top));
    started.inspect_err(|_| {
        let _ = unmap(stack, THREAD_STACK_PAGES);
    })
}

/// Where a thread's stack lies in the pages mapped from `guard` on: the
/// first stays unmapped as its guard, and the stack takes the
/// [`THREAD_STACK_PAGES`] above it. The second number is its top.
const fn thread_stack(guard: u64) -> (u64, u64) {
    let stack = guard + 4096;
    (stack, stack + THREAD_STACK_PAGES * 4096)
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

/// Closes `handle`. What it stood for closes with the last handle to it.
///
/// # Errors
///
/// When the handle is not the program's.
pub fn close(handle: Handle) -> Result<(), Error> {
    done(raw::call(Call::Close(handle.0)).0)
}

/// A second handle to what `handle` stands for, with the same rights
/// (decision record 0019).
///
/// # Errors
///
/// When the handle is not the program's, or no room is left for another.
pub fn duplicate(handle: Handle) -> Result<Handle, Error> {
    answer(raw::call(Call::Duplicate(handle.0)).0).map(Handle)
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

/// Starts the program `name` as [`spawn`] does, on a fresh start channel
/// over which it hands the program `arguments` and the end of its start
/// messages (decision record 0018), and answers its process and this
/// side of that channel, for whatever else the two say to each other.
///
/// # Errors
///
/// As for [`spawn`], and when no channel is left or the messages cannot
/// be sent.
pub fn spawn_with(name: &str, arguments: &[&str]) -> Result<(Handle, Handle), Error> {
    spawn_handing(name, arguments, None)
}

/// Starts the program `name` as [`spawn_with`] does, handing it `output`
/// as its standard output too, when that is some. The handle goes with
/// the program, and is closed when the program does not start.
///
/// # Errors
///
/// As for [`spawn_with`].
pub fn spawn_handing(
    name: &str,
    arguments: &[&str],
    output: Option<start::Sink>,
) -> Result<(Handle, Handle), Error> {
    let (mine, theirs) = channel()?;
    let child = match spawn(name, Some(theirs)) {
        Ok(child) => child,
        Err(e) => {
            let _ = close(mine);
            let _ = close(theirs);
            if let Some(start::Sink::Channel(h) | start::Sink::File(h)) = output {
                let _ = close(h);
            }
            return Err(e);
        }
    };
    if let Some(output) = output {
        let (kind, handle) = match output {
            start::Sink::Channel(h) => (start::Kind::Output, h),
            start::Sink::File(h) => (start::Kind::OutputFile, h),
        };
        send(mine, &[kind.byte()], Some(handle))?;
    }
    let end = alloc::vec::Vec::from([start::Kind::End.byte()]);
    for message in arguments
        .iter()
        .flat_map(|a| start::argument_messages(a))
        .chain([end])
    {
        send_waiting(mine, &message)?;
    }
    Ok((child, mine))
}

/// Sends `message` through `end` as [`send`] does, waiting a tick at a
/// time while the channel is full.
///
/// # Errors
///
/// As for [`send`], but for a full channel.
pub fn send_waiting(end: Handle, message: &[u8]) -> Result<(), Error> {
    loop {
        match send(end, message, None) {
            Err(Error::Full) => sleep(1),
            other => return other,
        }
    }
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

/// A key going down or coming up, by its USB HID usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub usage: u16,
    pub pressed: bool,
}

/// Writes `text` to the program's standard output: the channel or the
/// file its parent handed it (decision record 0017), at the file's end, or
/// the console when it was handed none.
///
/// # Errors
///
/// When the kernel refuses a piece.
pub fn write(text: &str) -> Result<(), Error> {
    match start::given().output {
        Some(start::Sink::Channel(end)) => {
            start::pieces(text.as_bytes()).try_for_each(|piece| send_waiting(end, piece))
        }
        Some(start::Sink::File(file)) => {
            let end = info(file)?.size;
            write_file(file, end, text.as_bytes())
        }
        None => console(text),
    }
}

/// Writes `text` to the console, in pieces as long as one call takes.
fn console(text: &str) -> Result<(), Error> {
    let mut rest = text;
    while !rest.is_empty() {
        let mut take = rest.len().min(abi::WRITE_BYTES);
        while !rest.is_char_boundary(take) {
            take -= 1;
        }
        done(raw::call(write_call(&rest[..take])).0)?;
        rest = &rest[take..];
    }
    Ok(())
}

/// Ends the process `process` stands for with the exit code `code`, at its
/// next way back to ring 3. One that ended already stays as it ended.
///
/// # Errors
///
/// When the handle stands for no process, or may not end it.
pub fn terminate(process: Handle, code: u64) -> Result<(), Error> {
    done(
        raw::call(Call::Terminate {
            handle: process.0,
            code,
        })
        .0,
    )
}

/// The next key the keyboard sent, when one waits.
#[must_use]
pub fn key() -> Option<KeyEvent> {
    key_from(raw::call(Call::Key).0)
}

fn write_call(text: &str) -> Call {
    Call::Write {
        len: text.len() as u64,
        at: text.as_ptr() as u64,
    }
}

fn key_from(rax: u64) -> Option<KeyEvent> {
    let value = answer(rax).ok()?;
    Some(KeyEvent {
        usage: u16::try_from(value & 0xffff).unwrap_or(0),
        pressed: value >> 16 & 1 == 1,
    })
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
    open_with(path, wanted, abi::file::OPEN_EXISTING)
}

/// Opens the file at `path` as [`open`] does, doing what `how` says by
/// whether it is there: one of [`abi::file::CREATE_NEW`] and the four
/// after it. Making or emptying a file takes the right to write it.
///
/// # Errors
///
/// As for [`open`], and when `how` is none of them or does nothing with
/// the file as it is, its volume is mounted read only and the file would
/// be written, or the disk is full or cannot be written.
pub fn open_with(path: &str, wanted: u32, how: u64) -> Result<Handle, Error> {
    answer(raw::call(open_call(path, wanted, how)).0).map(Handle)
}

/// Writes `bytes` into the file `file` from its byte `offset` on, which
/// may lie past its end, as many calls as it takes.
///
/// # Errors
///
/// When the handle is not the program's, may not write, or stands for a
/// directory, or the disk is full or cannot be written. What came before
/// the call that failed is written.
pub fn write_file(file: Handle, offset: u64, bytes: &[u8]) -> Result<(), Error> {
    for (i, piece) in (0u64..).zip(bytes.chunks(abi::WRITE_BYTES)) {
        let at = offset + i * abi::WRITE_BYTES as u64;
        answer(raw::call(write_file_call(file, at, piece)).0)?;
    }
    Ok(())
}

/// Removes the file or the empty directory at `path`.
///
/// # Errors
///
/// When the path is none or too long, no file has it, the program's token
/// may not remove it, its volume is mounted read only, it is open, or it
/// is a directory that holds entries.
pub fn remove(path: &str) -> Result<(), Error> {
    answer(raw::call(path_call(path, abi::REMOVE)).0).map(|_| ())
}

/// Makes a directory at `path`.
///
/// # Errors
///
/// When the path is none or too long, the directory it would be in is not
/// there, the program's token may not write there, its volume is mounted
/// read only, something has the path already, or the disk is full.
pub fn make_directory(path: &str) -> Result<(), Error> {
    answer(raw::call(path_call(path, abi::MAKE_DIRECTORY)).0).map(|_| ())
}

fn write_file_call(file: Handle, offset: u64, bytes: &[u8]) -> Call {
    Call::WriteFile {
        handle: file.0,
        at: bytes.as_ptr() as u64,
        len: bytes.len() as u64,
        offset,
    }
}

/// The call `number`, [`abi::REMOVE`] or [`abi::MAKE_DIRECTORY`], for the
/// path `path`.
fn path_call(path: &str, number: u64) -> Call {
    abi::decode(number, [path.len() as u64, path.as_ptr() as u64, 0, 0])
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

/// Writes the name of entry `index` of the directory `directory`, counted
/// from 0, into `room`, and returns its length and whether the entry is a
/// directory itself.
///
/// # Errors
///
/// When the handle is not the program's, may not read, or stands for a
/// file, the directory has no such entry, the name does not fit, or the
/// disk cannot be read.
pub fn list(directory: Handle, index: u64, room: &mut [u8]) -> Result<(usize, bool), Error> {
    let (rax, rdx) = raw::call(list_call(directory, index, room));
    listed(rax, rdx)
}

fn list_call(directory: Handle, index: u64, room: &mut [u8]) -> Call {
    Call::List {
        handle: directory.0,
        at: room.as_mut_ptr() as u64,
        room: room.len() as u64,
        index,
    }
}

fn listed(rax: u64, rdx: u64) -> Result<(usize, bool), Error> {
    let len = answer(rax)?;
    Ok((usize::try_from(len).unwrap_or(usize::MAX), rdx != 0))
}

fn open_call(path: &str, wanted: u32, how: u64) -> Call {
    Call::Open {
        len: path.len() as u64,
        at: path.as_ptr() as u64,
        wanted: u64::from(wanted),
        how,
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
    fn a_thread_stack_takes_the_pages_above_its_guard() {
        assert_eq!(thread_stack(0x10_0000), (0x10_1000, 0x11_1000));
        assert_eq!(
            thread_stack(0x7f_f000).1 - thread_stack(0x7f_f000).0,
            64 << 10,
            "the whole 64 KiB its pages promise"
        );
    }

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
    fn text_goes_to_the_console_and_keys_come_back() {
        let text = "ls /System";
        assert_eq!(
            write_call(text),
            Call::Write {
                len: 10,
                at: text.as_ptr() as u64
            }
        );
        assert_eq!(
            key_from(0x1_0004),
            Some(KeyEvent {
                usage: 0x04,
                pressed: true
            })
        );
        assert_eq!(
            key_from(0x00e1),
            Some(KeyEvent {
                usage: 0xe1,
                pressed: false
            })
        );
        assert_eq!(key_from(abi::EMPTY), None, "no key waits");
        assert_eq!(
            key_from(0x3_0052),
            Some(KeyEvent {
                usage: 0x52,
                pressed: true
            }),
            "only bit 16 says pressed"
        );
    }

    #[test]
    fn files_are_written_removed_and_made_by_their_calls() {
        let bytes = [7u8; 300];
        assert_eq!(
            write_file_call(Handle(0x3_0002), 1 << 33, &bytes[100..]),
            Call::WriteFile {
                handle: 0x3_0002,
                at: bytes[100..].as_ptr() as u64,
                len: 200,
                offset: 1 << 33
            }
        );
        let path = "/System/New";
        assert_eq!(
            path_call(path, abi::REMOVE),
            Call::Remove {
                len: 11,
                at: path.as_ptr() as u64
            }
        );
        assert_eq!(
            path_call(path, abi::MAKE_DIRECTORY),
            Call::MakeDirectory {
                len: 11,
                at: path.as_ptr() as u64
            }
        );
        assert_eq!(
            open_call(path, abi::file::WRITE, abi::file::CREATE_NEW),
            Call::Open {
                len: 11,
                at: path.as_ptr() as u64,
                wanted: abi::file::WRITE.into(),
                how: abi::file::CREATE_NEW
            }
        );
        let Call::WriteFile { handle, len, .. } = write_file_call(Handle(1), 0, &[]) else {
            panic!("a write is a WRITE_FILE");
        };
        assert_eq!(
            (handle, len),
            (1, 0),
            "nothing to write is a call of length 0"
        );
    }

    #[test]
    fn a_file_is_opened_by_its_path_and_read_where_asked() {
        let path = "/System/Boot/meridian/boot.meri";
        assert_eq!(
            open_call(
                path,
                abi::file::READ | abi::file::RUN,
                abi::file::OPEN_EXISTING
            ),
            Call::Open {
                len: 31,
                at: path.as_ptr() as u64,
                wanted: 0x21,
                how: 3
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
        let mut names = [0u8; 64];
        assert_eq!(
            list_call(Handle(0x3_0002), 5, &mut names),
            Call::List {
                handle: 0x3_0002,
                at: names.as_ptr() as u64,
                room: 64,
                index: 5
            }
        );
        assert_eq!(listed(9, 0), Ok((9, false)));
        assert_eq!(listed(8, 1), Ok((8, true)));
        assert_eq!(listed(abi::NOT_FOUND, 0), Err(Error::NotFound));
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
