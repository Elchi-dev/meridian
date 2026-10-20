//! Ring 3. Every program gets an address space of its own, whose kernel
//! half is the kernel's: its image and its stack are mapped for ring 3 in
//! its lower half, and a thread of its own enters it. It also
//! gets a handle table, its way to kernel objects such as channels. System
//! calls and faults bring the thread back into the kernel, on its kernel
//! stack; a fault in ring 3 ends only that program. Once it ends, its
//! handles are closed and its address space and everything in it go back
//! to the frame allocator.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
#[cfg(feature = "qemu-test")]
use core::arch::global_asm;
use core::fmt::Write;

use channel::MESSAGE;
use handles::{Handle, Rights, Table};
use kernel::object::Object;
use kernel::process::{Outcome, State};
use kernel::program::{self, Refusal, STACK_PAGES};
use kernel::syscall::{self, Call};
use kernel::thread::THREADS;
use paging::Flags;
use security::{Restriction, Token};

use super::interrupts::Frame;
use super::memory::{self, Spin};
use super::{ipc, thread};

/// The handles a program holds, at most this many.
const HANDLES: usize = 64;

/// What the kernel keeps for a program besides its address space.
struct Process {
    handles: Table<HANDLES>,
    /// How much of its heap region its MAP calls took.
    heap: program::Heap,
    /// The name its file gives it, up to [`NAME`] bytes, for the log.
    name: Vec<u8>,
    /// The slot of the process object a handle of whoever spawned it
    /// stands for, which learns how it ended.
    object: Option<usize>,
    /// Who it runs as, and what it may do.
    token: Token,
    /// How many of its threads run, and how it ends once they all left.
    threads: kernel::process::Threads,
}

/// The processes handles can stand for: those programs spawned.
static OBJECTS: Spin<kernel::process::Table<THREADS>> = Spin::new(kernel::process::Table::new());

/// The rights a program has to a process it spawned: to wait for it, to
/// end it and to hand the handle on.
const SPAWNED: Rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::TRANSFER.0);

/// The most of a program's name the kernel keeps.
const NAME: usize = 32;

impl Process {
    fn new(token: &Token) -> Box<Process> {
        Box::new(Process {
            handles: Table::new(),
            heap: program::Heap::new(),
            name: Vec::new(),
            object: None,
            token: *token,
            threads: kernel::process::Threads::new(),
        })
    }
}

/// A process, which each of its threads holds.
type Shared = Arc<Spin<Process>>;

/// Each program's process, by the number of each of its threads.
static PROCESSES: Spin<[Option<Shared>; THREADS]> = Spin::new([const { None }; THREADS]);

/// The running program's process.
fn current_process() -> Option<Shared> {
    PROCESSES.lock().get(thread::current())?.clone()
}

/// The rights a program has to the channel ends it holds.
const ENDS: Rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::TRANSFER.0);

/// What a program's thread takes with it into ring 3.
struct Start {
    entry: u64,
    stack: u64,
    argument: u64,
    process: Shared,
}

pub use kernel::program::{CODE_AT, STACK_TOP};

/// A program built into the test image: its machine code, position
/// independent and at most a page.
#[cfg(feature = "qemu-test")]
pub struct Program {
    code: fn() -> &'static [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No memory was left for its address space.
    NoMemory,
    /// Its thread could not start.
    Thread(thread::Error),
    /// Its file is no ELF file Meridian reads.
    Elf(elf::Error),
    /// Its file is one, but not a program Meridian runs.
    Refused(Refusal),
}

/// Starts `program` in ring 3 with `argument` in rdi, in an address space
/// and a thread of its own on `core`, and returns the thread's number.
///
/// # Errors
///
/// When no memory is left for the address space, or the thread cannot
/// start.
#[cfg(feature = "qemu-test")]
pub fn start(core: usize, program: &'static Program, argument: u64) -> Result<usize, Error> {
    launch(core, program, argument, Process::new(&Token::system()))
}

/// Starts the program in the ELF file `file` in ring 3 with `argument` in
/// rdi, in an address space and a thread of its own on `core`, and returns
/// the thread's number. Each page of its image goes where [`program::place`] says, as
/// [`program::page_flags`] allows. It runs as SYSTEM, as everything the
/// kernel starts itself does.
///
/// # Errors
///
/// When the file is no program Meridian runs, no memory is left for its
/// address space, or its thread cannot start.
pub fn start_file(core: usize, file: &[u8], argument: u64) -> Result<usize, Error> {
    start_file_as(core, file, argument, Process::new(&Token::system()))
}

/// Starts the program in `file` as [`start_file`] does, as `process`.
fn start_file_as(
    core: usize,
    file: &[u8],
    argument: u64,
    mut process: Box<Process>,
) -> Result<usize, Error> {
    let elf = elf::Elf::parse(file).map_err(Error::Elf)?;
    let (first, size) = elf.span().map_err(Error::Elf)?;
    let segments = elf.segments().map_err(Error::Elf)?;
    let segments = segments.as_slice();
    let about = program::metadata(elf.notes()).map_err(Error::Refused)?;
    let entry = program::place((first, size), elf.entry(), segments).map_err(Error::Refused)?;
    let mut image = vec![0u8; usize::try_from(size).map_err(|_| Error::NoMemory)?];
    elf.load(&mut image, CODE_AT).map_err(Error::Elf)?;
    let root = memory::new_space().ok_or(Error::NoMemory)?;
    for (offset, page) in (0..).step_by(4096).zip(image.chunks(4096)) {
        if let Ok(Some(flags)) = program::page_flags(segments, first + offset)
            && memory::map_fresh_in(root, CODE_AT + offset, flags, page).is_err()
        {
            memory::release_space(root);
            return Err(Error::NoMemory);
        }
    }
    process.name = about.name.iter().copied().take(NAME).collect();
    spawn(core, root, entry, argument, process)
}

/// Starts `program` as [`start`] does, holding a handle with `rights` to
/// each of the channel ends `objects` from the start; the first one's
/// handle is its argument. Only the test image hands programs handles at
/// their start yet.
///
/// # Errors
///
/// As for [`start`], and when the handles do not fit its table.
#[cfg(feature = "qemu-test")]
pub fn start_holding(
    core: usize,
    program: &'static Program,
    objects: &[u64],
    rights: Rights,
) -> Result<usize, Error> {
    let mut process = Process::new(&Token::system());
    let mut first = 0;
    for (i, &object) in objects.iter().enumerate() {
        let handle = process
            .handles
            .insert(object, rights)
            .map_err(|_| Error::NoMemory)?;
        if i == 0 {
            first = u64::from(handle.0);
        }
    }
    launch(core, program, first, process)
}

#[cfg(feature = "qemu-test")]
fn launch(
    core: usize,
    program: &'static Program,
    argument: u64,
    process: Box<Process>,
) -> Result<usize, Error> {
    let root = memory::new_space().ok_or(Error::NoMemory)?;
    if memory::map_fresh_in(root, CODE_AT, Flags::USER, (program.code)()).is_err() {
        memory::release_space(root);
        return Err(Error::NoMemory);
    }
    spawn(core, root, CODE_AT, argument, process)
}

/// Gives the program whose image is mapped in the address space at `root`
/// its stack and a thread that enters it at `entry`.
fn spawn(
    core: usize,
    root: u64,
    entry: u64,
    argument: u64,
    process: Box<Process>,
) -> Result<usize, Error> {
    let stack = Flags::USER | Flags::WRITABLE | Flags::NO_EXECUTE;
    let bottom = STACK_TOP - STACK_PAGES * 4096;
    let stacked = (bottom..STACK_TOP)
        .step_by(4096)
        .all(|page| memory::map_fresh_in(root, page, stack, &[]).is_ok());
    if !stacked {
        memory::release_space(root);
        return Err(Error::NoMemory);
    }
    let start = Box::into_raw(Box::new(Start {
        entry,
        stack: STACK_TOP,
        argument,
        process: Arc::new(Spin::new(*process)),
    })) as u64;
    thread::spawn_in(core, root, run, start, thread::NORMAL).map_err(|e| {
        // SAFETY: the box made just above, which no thread took.
        drop(unsafe { Box::from_raw(start as *mut Start) });
        memory::release_space(root);
        Error::Thread(e)
    })
}

/// A thread of a program, in the program's address space: keeps its
/// process and enters ring 3.
extern "C" fn run(start: u64) {
    // SAFETY: spawn and start_thread hand each thread the box they made
    // for it, once.
    let Start {
        entry,
        stack,
        argument,
        process,
    } = *unsafe { Box::from_raw(start as *mut Start) };
    #[cfg(feature = "qemu-test")]
    seen::starting(thread::current());
    if let Some(slot) = PROCESSES.lock().get_mut(thread::current()) {
        *slot = Some(process);
    }
    // SAFETY: the image is mapped for ring 3 in this thread's address
    // space, which the switch to it loaded, and so is the stack for the
    // first thread, which spawn mapped; another thread's stack is its
    // program's to give, and only the program faults when it is not one.
    // Every switch to the thread put its kernel stack in the task state,
    // and the IDT has the system call gate for ring 3.
    unsafe { x86::raw::enter_user(entry, stack, argument) }
}

/// Ends the whole program as `outcome`: its other threads end at their
/// next way back to ring 3, and this one leaves at once.
fn end_program(outcome: Outcome) -> ! {
    if let Some(process) = current_process() {
        process.lock().threads.end(outcome);
    }
    leave(0)
}

/// The running thread leaves its program, having ended with `code` when it
/// ended on its own. The last thread to leave closes the program's handles,
/// says how the program ended to whoever waits for it, and gives its
/// address space back; each goes back to the kernel's address space first,
/// and ends.
fn leave(code: u64) -> ! {
    let process = PROCESSES
        .lock()
        .get_mut(thread::current())
        .and_then(Option::take);
    let last = process.is_some_and(|process| {
        let mut process = process.lock();
        let on_its_own = process.threads.ending().is_none();
        let Some(outcome) = process.threads.leave(code) else {
            return false;
        };
        if on_its_own {
            log_ended(&process.name, format_args!("ended with {code}"));
            #[cfg(feature = "qemu-test")]
            seen::exited(code);
        }
        process.handles.close_all(close_object);
        if let Some(slot) = process.object {
            OBJECTS.lock().end(slot, outcome);
        }
        true
    });
    let root = thread::current_root();
    // SAFETY: the kernel's address space maps all the kernel does, this
    // stack included.
    unsafe { x86::raw::load_cr3(memory::kernel_root()) };
    if last && root != 0 {
        memory::release_space(root);
    }
    thread::exit()
}

/// Starts another thread of the running program at `entry` in ring 3, its
/// stack pointer at `stack` and `argument` in rdi, on this core to begin
/// with.
fn start_thread(entry: u64, stack: u64, argument: u64) -> Result<u64, u64> {
    if !abi::in_user_half(entry, 1) || !abi::in_user_half(stack, 0) {
        return Err(syscall::BAD_ADDRESS);
    }
    let process = current_process().ok_or(syscall::BAD_HANDLE)?;
    if !process.lock().threads.start() {
        return Err(syscall::DENIED);
    }
    let start = Box::into_raw(Box::new(Start {
        entry,
        stack,
        argument,
        process: process.clone(),
    })) as u64;
    let core = super::interrupts::this_core();
    thread::spawn_in(core, thread::current_root(), run, start, thread::NORMAL).map_or_else(
        |_| {
            // SAFETY: the box made just above, which no thread took.
            drop(unsafe { Box::from_raw(start as *mut Start) });
            process.lock().threads.not_started();
            Err(syscall::NO_ROOM)
        },
        |_| Ok(0),
    )
}

/// A system call from ring 3, on the calling thread's kernel stack: the
/// answer goes back in rax.
pub fn system_call(frame: &mut Frame) {
    #[cfg(feature = "qemu-test")]
    seen::called_from(frame.cs, frame.rax);
    frame.rax = match syscall::decode(frame.rax, [frame.rdi, frame.rsi, frame.rdx, frame.r10]) {
        Call::Exit(code) => {
            ended(format_args!("ended with {code}"));
            #[cfg(feature = "qemu-test")]
            seen::exited(code);
            end_program(Outcome::Exited(code))
        }
        Call::Thread {
            entry,
            stack,
            argument,
        } => start_thread(entry, stack, argument).unwrap_or_else(|e| e),
        Call::EndThread(code) => leave(code),
        Call::Log { len, at } => log(at, len),
        Call::Yield => {
            thread::yield_now();
            0
        }
        Call::Sleep(ticks) => {
            thread::sleep(ticks);
            0
        }
        Call::Ticks => super::interrupts::ticks(),
        Call::Channel => match make_channel() {
            Ok((a, b)) => {
                frame.rdx = b;
                a
            }
            Err(e) => e,
        },
        Call::Send {
            handle,
            at,
            len,
            carry,
        } => send(handle, at, len, carry).map_or_else(|e| e, |()| 0),
        Call::Receive { handle, at, room } => match receive(handle, at, room) {
            Ok((len, brought)) => {
                frame.rdx = brought;
                len
            }
            Err(e) => e,
        },
        Call::Close(handle) => close(handle).map_or_else(|e| e, |()| 0),
        Call::Map(pages) => map(pages).unwrap_or_else(|e| e),
        Call::Spawn {
            len,
            at,
            carry,
            restriction,
        } => spawn_program(len, at, carry, restriction).unwrap_or_else(|e| e),
        Call::Wait(handle) => match wait(handle) {
            Ok((value, faulted)) => {
                frame.rdx = faulted;
                value
            }
            Err(e) => e,
        },
        Call::Unmap { at, pages } => unmap(at, pages).map_or_else(|e| e, |()| 0),
        Call::Token { room, at } => token(room, at).unwrap_or_else(|e| e),
        Call::Open { len, at, wanted } => open(len, at, wanted).unwrap_or_else(|e| e),
        Call::Read {
            handle,
            at,
            room,
            offset,
        } => read(handle, at, room, offset).unwrap_or_else(|e| e),
        Call::List {
            handle,
            at,
            room,
            index,
        } => match list(handle, at, room, index) {
            Ok((len, directory)) => {
                frame.rdx = u64::from(directory);
                len
            }
            Err(e) => e,
        },
        Call::Write { len, at } => write(len, at).map_or_else(|e| e, |()| 0),
        Call::Terminate { handle, code } => terminate(handle, code).map_or_else(|e| e, |()| 0),
        Call::Key => super::ps2::next().map_or(syscall::EMPTY, |e| {
            u64::from(e.key.0) | u64::from(e.pressed) << 16
        }),
        Call::Info(handle) => match info(handle) {
            Ok((size, directory)) => {
                frame.rdx = u64::from(directory);
                size
            }
            Err(e) => e,
        },
        Call::Unknown(_) => syscall::NO_SUCH_CALL,
    };
}

/// Asks the process the handle `handle` stands for to end with `code`; a
/// process that ended already, or was asked before, stays as it is.
fn terminate(handle: u64, code: u64) -> Result<(), u64> {
    let Some(Object::Process { slot }) = Object::from_number(object(handle, Rights::WRITE)?) else {
        return Err(syscall::DENIED);
    };
    OBJECTS.lock().ask_to_end(slot, code);
    Ok(())
}

/// Ends the running thread when another one ended its program, and the
/// whole program when it was asked to, with the code it was asked to end
/// with: on every way back to ring 3, and while it waits.
pub fn end_if_asked() {
    let Some((ending, object)) = current_process().map(|process| {
        let process = process.lock();
        (process.threads.ending(), process.object)
    }) else {
        return;
    };
    if ending.is_some() {
        leave(0)
    }
    if let Some(code) = object.and_then(|slot| OBJECTS.lock().asked(slot)) {
        ended(format_args!("was stopped, with {code}"));
        #[cfg(feature = "qemu-test")]
        seen::exited(code);
        end_program(Outcome::Exited(code))
    }
}

/// Writes to the log how the running program ended, under its name.
fn ended(how: core::fmt::Arguments<'_>) {
    let name = with_process(|p| Ok(p.name.clone())).unwrap_or_default();
    log_ended(&name, how);
}

/// Writes to the log how the program `name` ended.
fn log_ended(name: &[u8], how: core::fmt::Arguments<'_>) {
    let serial = super::serial::already_open();
    #[cfg(not(feature = "qemu-test"))]
    let _ = kernel::log::ended(&mut { serial }, name, how);
    #[cfg(feature = "qemu-test")]
    {
        let mut through = seen::Through::new(serial);
        let _ = kernel::log::ended(&mut through, name, how);
        seen::keep_ended(&through);
    }
}

/// Closes the object `object` stands for, whatever its kind.
pub fn close_object(object: u64) {
    match Object::from_number(object) {
        Some(Object::ChannelEnd { .. }) => ipc::close(object),
        Some(Object::Process { slot }) => OBJECTS.lock().close(slot),
        Some(Object::File { slot }) => super::vfs::close(slot),
        None => {}
    }
}

/// Starts the program whose file is named by the `len` bytes at `at`, as
/// a process of its own the running program holds a handle to, handing it
/// the handle `carry` unless that is 0, and answers the handle. The name is
/// a program's the loader read, or a path from the root to a file the
/// running program's token may run. The program gets that token,
/// restricted by the word `restriction` unless that is 0.
fn spawn_program(len: u64, at: u64, carry: u64, restriction: u64) -> Result<u64, u64> {
    let mut name = [0u8; syscall::PATH_BYTES];
    let name = copy_in(at, len, &mut name, syscall::TOO_LONG)?;
    let mut token = with_process(|p| Ok(p.token))?;
    let read: Vec<u8>;
    let file = if name.first() == Some(&b'/') {
        read = read_program(name, &token)?;
        &read
    } else {
        super::module(name).ok_or(syscall::NO_SUCH_PROGRAM)?
    };
    if restriction != 0 {
        let restriction = Restriction::from_word(restriction).ok_or(syscall::BAD_ARGUMENT)?;
        token = token.restricted(restriction);
    }
    let mut child = Process::new(&token);
    let mut argument = 0;
    if carry != 0 {
        let handle = handle_of(carry)?;
        let rights = with_handles(|t| t.rights(handle).map_err(|_| syscall::BAD_HANDLE))?;
        let object = object(carry, Rights::TRANSFER)?;
        let given = child
            .handles
            .insert(object, rights)
            .map_err(|_| syscall::NO_ROOM)?;
        argument = u64::from(given.0);
    }
    let slot = OBJECTS.lock().create().ok_or(syscall::NO_ROOM)?;
    child.object = Some(slot);
    let handle = match with_handles(|t| {
        t.insert(Object::Process { slot }.number(), SPAWNED)
            .map_err(|_| syscall::NO_ROOM)
    }) {
        Ok(handle) => handle,
        Err(e) => {
            let mut objects = OBJECTS.lock();
            objects.end(slot, Outcome::Exited(0));
            objects.close(slot);
            return Err(e);
        }
    };
    let core = super::interrupts::this_core();
    if let Err(e) = start_file_as(core, file, argument, child) {
        let _ = with_handles(|t| t.close(handle).map_err(|_| syscall::BAD_HANDLE));
        let mut objects = OBJECTS.lock();
        objects.end(slot, Outcome::Exited(0));
        objects.close(slot);
        return Err(match e {
            Error::Elf(_) | Error::Refused(_) => syscall::NOT_A_PROGRAM,
            Error::NoMemory | Error::Thread(_) => syscall::NO_ROOM,
        });
    }
    if carry != 0 {
        // It went to the child, which holds it now.
        with_handles(|t| t.close(handle_of(carry)?).map_err(|_| syscall::BAD_HANDLE))?;
    }
    Ok(u64::from(handle.0))
}

/// The file at the path `path`, read whole, once `token` may run it.
fn read_program(path: &[u8], token: &Token) -> Result<Vec<u8>, u64> {
    let path = core::str::from_utf8(path).map_err(|_| syscall::BAD_ARGUMENT)?;
    let slot = super::vfs::open(path, token, syscall::file::RUN).map_err(file_answer)?;
    let read = read_whole(slot);
    super::vfs::close(slot);
    read
}

/// Every byte of the open file in `slot`.
fn read_whole(slot: usize) -> Result<Vec<u8>, u64> {
    let (size, directory) = super::vfs::info(slot).ok_or(syscall::NOT_FOUND)?;
    if directory {
        return Err(syscall::NOT_A_FILE);
    }
    let mut file = Vec::new();
    let size = usize::try_from(size).map_err(|_| syscall::NO_ROOM)?;
    file.try_reserve_exact(size).map_err(|_| syscall::NO_ROOM)?;
    file.resize(size, 0);
    let len = super::vfs::read(slot, 0, &mut file).map_err(file_answer)?;
    file.truncate(len);
    Ok(file)
}

/// Waits until the process the handle `handle` stands for ended, and
/// answers its exit code with 0, or the vector of the exception that ended
/// it with 1.
fn wait(handle: u64) -> Result<(u64, u64), u64> {
    let Some(Object::Process { slot }) = Object::from_number(object(handle, Rights::READ)?) else {
        return Err(syscall::DENIED);
    };
    loop {
        let state = OBJECTS.lock().state(slot);
        match state {
            Some(State::Ended(Outcome::Exited(code))) => return Ok((code, 0)),
            Some(State::Ended(Outcome::Faulted(vector))) => return Ok((vector, 1)),
            Some(State::Running) => {
                end_if_asked();
                thread::sleep(1);
            }
            None => return Err(syscall::BAD_HANDLE),
        }
    }
}

/// Runs `with` on the running program's process.
fn with_process<R>(with: impl FnOnce(&mut Process) -> Result<R, u64>) -> Result<R, u64> {
    let process = current_process().ok_or(syscall::BAD_HANDLE)?;
    let mut process = process.lock();
    with(&mut process)
}

/// Runs `with` on the running program's handle table.
fn with_handles<R>(with: impl FnOnce(&mut Table<HANDLES>) -> Result<R, u64>) -> Result<R, u64> {
    with_process(|p| with(&mut p.handles))
}

/// Maps `pages` fresh pages into the running program's heap region,
/// zeroed, writable and not executable, and answers where they start.
fn map(pages: u64) -> Result<u64, u64> {
    let at = with_process(|p| p.heap.take(pages).map_err(syscall::Error::code))?;
    let flags = Flags::USER | Flags::WRITABLE | Flags::NO_EXECUTE;
    memory::map_pages(thread::current_root(), at, pages, flags).map_err(|()| syscall::NO_ROOM)?;
    Ok(at)
}

/// Gives back the `pages` pages at `at`, which MAP gave the running
/// program and which are all still there.
fn unmap(at: u64, pages: u64) -> Result<(), u64> {
    let given = with_process(|p| Ok(p.heap.holds(at, pages)))?;
    if !given || !memory::user_writable(at, pages * 4096) {
        return Err(syscall::BAD_ADDRESS);
    }
    memory::unmap_pages(thread::current_root(), at, pages);
    Ok(())
}

/// The object a handle of the running program stands for, when it allows
/// `needed`.
fn object(handle: u64, needed: Rights) -> Result<u64, u64> {
    let handle = handle_of(handle)?;
    with_handles(|t| {
        t.get(handle, needed).map_err(|e| match e {
            handles::Error::Denied(_) => syscall::DENIED,
            _ => syscall::BAD_HANDLE,
        })
    })
}

/// A channel error as the answer to a system call.
fn answer(e: channel::Error) -> u64 {
    match e {
        channel::Error::TooLong => syscall::TOO_LONG,
        channel::Error::Full => syscall::FULL,
        channel::Error::Empty => syscall::EMPTY,
        channel::Error::TooSmall(_) => syscall::TOO_SMALL,
        channel::Error::Closed => syscall::CLOSED,
    }
}

/// A new channel, and the running program's handles to its two ends.
fn make_channel() -> Result<(u64, u64), u64> {
    let (a, b) = ipc::create().ok_or(syscall::NO_ROOM)?;
    let handles = with_handles(|t| {
        let first = t.insert(a, ENDS).map_err(|_| syscall::NO_ROOM)?;
        let Ok(second) = t.insert(b, ENDS) else {
            let _ = t.close(first);
            return Err(syscall::NO_ROOM);
        };
        Ok((u64::from(first.0), u64::from(second.0)))
    });
    if handles.is_err() {
        ipc::close(a);
        ipc::close(b);
    }
    handles
}

/// Sends the `len` bytes at `at` through the channel end `handle` stands
/// for, handing over the handle `carry` with them unless it is 0. A handle
/// handed over leaves the sender's table once the message went.
fn send(handle: u64, at: u64, len: u64, carry: u64) -> Result<(), u64> {
    let count = usize::try_from(len)
        .ok()
        .filter(|&n| n <= MESSAGE)
        .ok_or(syscall::TOO_LONG)?;
    if !memory::user_readable(at, len) {
        return Err(syscall::BAD_ADDRESS);
    }
    let end = object(handle, Rights::WRITE)?;
    let carried = match carry {
        0 => None,
        carry => {
            let object = object(carry, Rights::TRANSFER)?;
            let rights =
                with_handles(|t| t.rights(handle_of(carry)?).map_err(|_| syscall::BAD_HANDLE))?;
            Some(channel::Attachment {
                object,
                rights: rights.0,
            })
        }
    };
    let mut message = [0u8; MESSAGE];
    // SAFETY: user_readable found every byte mapped for ring 3 in this
    // program's address space, which only this thread runs.
    x86::raw::with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(at as *const u8, message.as_mut_ptr(), count);
    });
    ipc::send(end, &message[..count], carried).map_err(answer)?;
    if carried.is_some() {
        with_handles(|t| t.close(handle_of(carry)?).map_err(|_| syscall::BAD_HANDLE))?;
    }
    Ok(())
}

/// The handle a program named by `number`.
fn handle_of(number: u64) -> Result<Handle, u64> {
    u32::try_from(number)
        .map(Handle)
        .map_err(|_| syscall::BAD_HANDLE)
}

/// Takes the next message for the channel end `handle` stands for into
/// the `room` bytes at `at`, and answers its length and the handle to what
/// it brought along, or 0. What it brought that does not fit the
/// program's table is closed.
fn receive(handle: u64, at: u64, room: u64) -> Result<(u64, u64), u64> {
    if !memory::user_writable(at, room) {
        return Err(syscall::BAD_ADDRESS);
    }
    let end = object(handle, Rights::READ)?;
    let room = usize::try_from(room).unwrap_or(MESSAGE).min(MESSAGE);
    let mut message = [0u8; MESSAGE];
    let (len, brought) = ipc::receive(end, &mut message[..room]).map_err(answer)?;
    // SAFETY: user_writable found every byte of the room mapped writable
    // for ring 3 in this program's address space, and len fits in it.
    x86::raw::with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(message.as_ptr(), at as *mut u8, len);
    });
    let handle = match brought {
        None => 0,
        Some(a) => with_handles(|t| Ok(t.insert(a.object, Rights(a.rights)).ok()))?.map_or_else(
            || {
                close_object(a.object);
                0
            },
            |h| u64::from(h.0),
        ),
    };
    Ok((len as u64, handle))
}

/// Writes the running program's token into the `room` bytes at `at`, and
/// answers how many bytes it took.
fn token(room: u64, at: u64) -> Result<u64, u64> {
    let token = with_process(|p| Ok(p.token))?;
    let mut bytes = [0u8; Token::MAX_SIZE];
    let len = token.write(&mut bytes).ok_or(syscall::TOO_SMALL)?;
    let size = len as u64;
    if room < size {
        return Err(syscall::TOO_SMALL);
    }
    if !memory::user_writable(at, size) {
        return Err(syscall::BAD_ADDRESS);
    }
    // SAFETY: user_writable found each of the bytes mapped writable for
    // ring 3 in this program's address space, which only this thread runs.
    x86::raw::with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, len);
    });
    Ok(size)
}

/// A file error as the answer to a system call.
fn file_answer(e: super::vfs::Error) -> u64 {
    use super::vfs::Error;
    match e {
        Error::BadPath => syscall::BAD_ARGUMENT,
        Error::NotFound => syscall::NOT_FOUND,
        Error::NotAFile => syscall::NOT_A_FILE,
        Error::NotADirectory => syscall::NOT_A_DIRECTORY,
        Error::TooSmall => syscall::TOO_SMALL,
        Error::Refused(_) => syscall::DENIED,
        Error::NoRoom => syscall::NO_ROOM,
        Error::Read => syscall::UNREADABLE,
    }
}

/// Opens the file at the path of the `len` bytes at `at` for the rights
/// `wanted`, as the running program's token may, and answers a handle to
/// it that may read it when that was asked for, and be handed on.
fn open(len: u64, at: u64, wanted: u64) -> Result<u64, u64> {
    let mut bytes = [0u8; syscall::PATH_BYTES];
    let path = copy_in(at, len, &mut bytes, syscall::TOO_LONG)?;
    let path = core::str::from_utf8(path).map_err(|_| syscall::BAD_ARGUMENT)?;
    let wanted = u32::try_from(wanted).map_err(|_| syscall::BAD_ARGUMENT)?;
    let token = with_process(|p| Ok(p.token))?;
    let slot = super::vfs::open(path, &token, wanted).map_err(file_answer)?;
    let mut rights = Rights::TRANSFER;
    if wanted & syscall::file::READ != 0 {
        rights = Rights(rights.0 | Rights::READ.0);
    }
    with_handles(|t| {
        t.insert(Object::File { slot }.number(), rights)
            .map_err(|_| syscall::NO_ROOM)
    })
    .map(|handle| u64::from(handle.0))
    .inspect_err(|_| super::vfs::close(slot))
}

/// The most bytes one READ reads.
const READ_MAX: usize = 4096;

/// Reads at most `room` bytes, and no more than [`READ_MAX`], of the file
/// `handle` stands for from its byte `offset` on into the room at `at`,
/// and answers how many it read.
fn read(handle: u64, at: u64, room: u64, offset: u64) -> Result<u64, u64> {
    let Some(Object::File { slot }) = Object::from_number(object(handle, Rights::READ)?) else {
        return Err(syscall::DENIED);
    };
    if !memory::user_writable(at, room) {
        return Err(syscall::BAD_ADDRESS);
    }
    let room = usize::try_from(room).unwrap_or(READ_MAX).min(READ_MAX);
    let mut bytes = [0u8; READ_MAX];
    let len = super::vfs::read(slot, offset, &mut bytes[..room]).map_err(file_answer)?;
    // SAFETY: user_writable found every byte of the room mapped writable
    // for ring 3 in this program's address space, and len fits in it.
    x86::raw::with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, len);
    });
    Ok(len as u64)
}

/// Writes the `len` bytes at `at`, which have to be UTF-8, to the console.
fn write(len: u64, at: u64) -> Result<(), u64> {
    let mut bytes = [0u8; syscall::WRITE_BYTES];
    let text = copy_in(at, len, &mut bytes, syscall::TOO_LONG)?;
    super::console::write(core::str::from_utf8(text).map_err(|_| syscall::BAD_ARGUMENT)?);
    Ok(())
}

/// Copies the `len` bytes at `at`, the running program's, into the start
/// of `into` and returns them: `too_long` when they do not fit there, and
/// `BAD_ADDRESS` when they are not all the program's to give.
fn copy_in(at: u64, len: u64, into: &mut [u8], too_long: u64) -> Result<&[u8], u64> {
    let count = usize::try_from(len)
        .ok()
        .filter(|&n| n <= into.len())
        .ok_or(too_long)?;
    if !memory::user_readable(at, len) {
        return Err(syscall::BAD_ADDRESS);
    }
    // SAFETY: user_readable found every byte mapped for ring 3 in this
    // program's address space, which only this thread runs, so nothing
    // unmaps them while they are copied, and they fit `into`.
    x86::raw::with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(at as *const u8, into.as_mut_ptr(), count);
    });
    Ok(&into[..count])
}

/// The most bytes a name LIST hands out takes: 255 units of UTF-16, each
/// at most four bytes of UTF-8.
const NAME_MAX: usize = 1024;

/// Writes the name of entry `index` of the directory `handle` stands for
/// into the `room` bytes at `at`, and answers its length and whether that
/// entry is a directory.
fn list(handle: u64, at: u64, room: u64, index: u64) -> Result<(u64, bool), u64> {
    let Some(Object::File { slot }) = Object::from_number(object(handle, Rights::READ)?) else {
        return Err(syscall::DENIED);
    };
    if !memory::user_writable(at, room) {
        return Err(syscall::BAD_ADDRESS);
    }
    let room = usize::try_from(room).unwrap_or(NAME_MAX).min(NAME_MAX);
    let mut bytes = [0u8; NAME_MAX];
    let (len, directory) =
        super::vfs::list(slot, index, &mut bytes[..room]).map_err(file_answer)?;
    // SAFETY: user_writable found every byte of the room mapped writable
    // for ring 3 in this program's address space, and len fits in it.
    x86::raw::with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, len);
    });
    Ok((len as u64, directory))
}

/// The size of the file `handle` stands for, and whether it is a
/// directory.
fn info(handle: u64) -> Result<(u64, bool), u64> {
    let Some(Object::File { slot }) = Object::from_number(object(handle, Rights(0))?) else {
        return Err(syscall::DENIED);
    };
    super::vfs::info(slot).ok_or(syscall::BAD_HANDLE)
}

/// Closes `handle`, and the channel end it stood for.
fn close(handle: u64) -> Result<(), u64> {
    let handle = handle_of(handle)?;
    let object = with_handles(|t| t.close(handle).map_err(|_| syscall::BAD_HANDLE))?;
    close_object(object);
    Ok(())
}

/// Writes the program's line to the log, once it is sure every byte is
/// the program's to give.
fn log(at: u64, len: u64) -> u64 {
    let Some(count) = kernel::log::line_len(len) else {
        return syscall::TOO_LONG;
    };
    if !memory::user_readable(at, len) {
        return syscall::BAD_ADDRESS;
    }
    let mut text = [0u8; syscall::LOG_BYTES];
    let text = &mut text[..count];
    // SAFETY: user_readable found every byte mapped for ring 3 in the
    // address space this thread runs on, and only this thread runs the
    // program, so nothing unmaps them while they are copied.
    x86::raw::with_user_access(|| unsafe {
        core::ptr::copy_nonoverlapping(at as *const u8, text.as_mut_ptr(), text.len());
    });
    let mut serial = super::serial::already_open();
    let _ = serial.write_str("user: ");
    for &byte in text.iter() {
        let _ = serial.write_char(kernel::log::shown(byte));
    }
    let _ = serial.write_char('\n');
    #[cfg(feature = "qemu-test")]
    seen::keep_log(text);
    0
}

/// A fault in ring 3: the program ends, and the kernel goes on.
pub fn fault(frame: &Frame) -> ! {
    ended(format_args!(
        "ended by exception {} with error {:#x} at {:#x}",
        frame.vector, frame.error, frame.rip
    ));
    #[cfg(feature = "qemu-test")]
    seen::faulted(frame.vector, frame.error);
    end_program(Outcome::Faulted(frame.vector))
}

#[cfg(feature = "qemu-test")]
fn bytes(start: *const u8, end: *const u8) -> &'static [u8] {
    // SAFETY: the two symbols of a program in .rodata, end after start,
    // and the bytes between them never change.
    unsafe { core::slice::from_raw_parts(start, end as usize - start as usize) }
}

// The first program Meridian ran: it writes a line to the log and ends.
#[cfg(feature = "qemu-test")]
global_asm!(
    ".pushsection .rodata.meridian_programs, \"a\"",
    ".global meridian_hello_start",
    ".global meridian_hello_end",
    "meridian_hello_start:",
    "lea rsi, [rip + 2f]",
    "lea rdi, [rip + 3f]",
    "sub rdi, rsi",
    "mov eax, {log}",
    "int 0x2e",
    "xor edi, edi",
    "mov eax, {exit}",
    "int 0x2e",
    "ud2",
    "2: .ascii \"Hello from ring 3, through a system call\"",
    "3:",
    "meridian_hello_end:",
    ".popsection",
    log = const syscall::LOG,
    exit = const syscall::EXIT,
);

#[cfg(feature = "qemu-test")]
unsafe extern "C" {
    static meridian_hello_start: u8;
    static meridian_hello_end: u8;
}

#[cfg(feature = "qemu-test")]
fn hello() -> &'static [u8] {
    bytes(
        &raw const meridian_hello_start,
        &raw const meridian_hello_end,
    )
}

/// The first program Meridian ran in ring 3, written in machine code.
#[cfg(feature = "qemu-test")]
pub static HELLO: Program = Program { code: hello };

/// Programs that try what ring 3 must not do, or reach the edges of what
/// it may, for the test image. Each ends with the answer of its last
/// system call as its exit code, or by a fault.
#[cfg(feature = "qemu-test")]
pub mod probe {
    use core::arch::global_asm;

    use kernel::syscall;

    use super::{Program, bytes};

    global_asm!(
        ".pushsection .rodata.meridian_probes, \"a\"",
        // Reads the kernel's direct map.
        ".global meridian_read_kernel_start",
        "meridian_read_kernel_start:",
        "movabs rax, 0xffff800000000000",
        "mov rax, [rax]",
        "ud2",
        ".global meridian_halt_start",
        // Halts the processor, which only ring 0 may.
        "meridian_halt_start:",
        "hlt",
        "ud2",
        // Asks the log to read the kernel's memory.
        ".global meridian_bad_log_start",
        "meridian_bad_log_start:",
        "movabs rsi, 0xffff800000001000",
        "mov edi, 8",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Asks the log for a line longer than it takes.
        ".global meridian_long_log_start",
        "meridian_long_log_start:",
        "lea rsi, [rip]",
        "mov edi, {too_long}",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Asks for a call that does not exist.
        ".global meridian_unknown_start",
        "meridian_unknown_start:",
        "mov eax, 99",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Sleeps two ticks, then ends with the ticks it finds.
        ".global meridian_nap_start",
        "meridian_nap_start:",
        "mov edi, 2",
        "mov eax, {sleep}",
        "int 0x2e",
        "mov eax, {ticks}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Gives way once and ends with the answer.
        ".global meridian_yielder_start",
        "meridian_yielder_start:",
        "mov eax, {yield_now}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Asks the log to read a user address where nothing is mapped.
        ".global meridian_unmapped_log_start",
        "meridian_unmapped_log_start:",
        "mov esi, 0x1000",
        "mov edi, 8",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Keeps the processor for 20 ticks without giving way: only the
        // timer can take it.
        ".global meridian_spinner_start",
        "meridian_spinner_start:",
        "mov eax, {ticks}",
        "int 0x2e",
        "lea rbx, [rax + 20]",
        "2:",
        "mov eax, {ticks}",
        "int 0x2e",
        "cmp rax, rbx",
        "jb 2b",
        "xor edi, edi",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Writes over its own code.
        ".global meridian_write_code_start",
        "meridian_write_code_start:",
        "mov byte ptr [rip], 0x90",
        "ud2",
        // Maps a page, writes to it, gives it back and reads it again,
        // which has to fault at once.
        ".global meridian_stale_start",
        "meridian_stale_start:",
        "mov edi, 1",
        "mov eax, {map}",
        "int 0x2e",
        "mov rbx, rax",
        "mov qword ptr [rbx], 42",
        "mov rdi, rbx",
        "mov esi, 1",
        "mov eax, {unmap}",
        "int 0x2e",
        "mov rdi, qword ptr [rbx]",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Asks to spawn a program by the longest name there may be, from
        // memory that is not its own, and ends with the answer.
        ".global meridian_spawn_longest_start",
        "meridian_spawn_longest_start:",
        "mov edi, 512",
        "mov esi, 0x900000",
        "xor edx, edx",
        "mov eax, {spawn}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // The same with a name one byte too long.
        ".global meridian_spawn_too_long_start",
        "meridian_spawn_too_long_start:",
        "mov edi, 513",
        "mov esi, 0x900000",
        "xor edx, edx",
        "mov eax, {spawn}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Sets the AC flag and ends through a system call.
        ".global meridian_set_ac_start",
        "meridian_set_ac_start:",
        "pushfq",
        "or dword ptr [rsp], 0x40000",
        "popfq",
        "xor edi, edi",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Writes a return onto its stack and jumps there.
        ".global meridian_run_stack_start",
        "meridian_run_stack_start:",
        "mov byte ptr [rsp - 16], 0xc3",
        "lea rax, [rsp - 16]",
        "jmp rax",
        // Keeps its argument at the bottom of its stack page, sleeps three
        // ticks, and ends with what it finds there then.
        ".global meridian_keep_start",
        "meridian_keep_start:",
        "mov [0x7ff000], rdi",
        "mov edi, 3",
        "mov eax, {sleep}",
        "int 0x2e",
        "mov rdi, [0x7ff000]",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Makes a channel, sends itself "hello" through one end, takes it
        // at the other, logs it and ends with its length.
        ".global meridian_pair_start",
        "meridian_pair_start:",
        "mov eax, {channel}",
        "int 0x2e",
        "mov r12, rax",
        "mov r13, rdx",
        "mov rdi, r12",
        "lea rsi, [rip + 6f]",
        "mov edx, 5",
        "mov eax, {send}",
        "int 0x2e",
        "mov rdi, r13",
        "lea rsi, [rsp - 256]",
        "mov edx, 128",
        "mov eax, {receive}",
        "int 0x2e",
        "mov r14, rax",
        "mov rdi, r14",
        "lea rsi, [rsp - 256]",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, r14",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        "6: .ascii \"hello\"",
        // Sends "ping" through the channel end its handle in rdi stands
        // for, and ends with the answer.
        ".global meridian_sender_start",
        "meridian_sender_start:",
        "lea rsi, [rip + 7f]",
        "mov edx, 4",
        "mov eax, {send}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        "7: .ascii \"ping\"",
        // Waits for a message at the channel end its handle in rdi stands
        // for, sleeping a tick while none is there, logs it and ends with
        // its length, or with what it got instead.
        ".global meridian_receiver_start",
        "meridian_receiver_start:",
        "mov rbx, rdi",
        "8:",
        "mov rdi, rbx",
        "lea rsi, [rsp - 256]",
        "mov edx, 128",
        "mov eax, {receive}",
        "int 0x2e",
        "mov r12, rax",
        "mov rcx, {empty}",
        "cmp rax, rcx",
        "jne 9f",
        "mov edi, 1",
        "mov eax, {sleep}",
        "int 0x2e",
        "jmp 8b",
        "9:",
        "mov rdi, r12",
        "lea rsi, [rsp - 256]",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, r12",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Makes a channel, sends "hi" into one end, and hands the other
        // end over through the channel end its handle in rdi stands for.
        // The end it handed over is not its own any more: it ends with
        // what closing it answers.
        ".global meridian_passer_start",
        "meridian_passer_start:",
        "mov rbx, rdi",
        "mov eax, {channel}",
        "int 0x2e",
        "mov r12, rax",
        "mov r13, rdx",
        "mov rdi, r12",
        "lea rsi, [rip + 12f]",
        "mov edx, 2",
        "xor r10d, r10d",
        "mov eax, {send}",
        "int 0x2e",
        "mov rdi, rbx",
        "lea rsi, [rip + 13f]",
        "mov edx, 4",
        "mov r10, r13",
        "mov eax, {send}",
        "int 0x2e",
        "mov rdi, r13",
        "mov eax, {close}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        "12: .ascii \"hi\"",
        "13: .ascii \"take\"",
        // Waits for a message bringing a handle at the channel end its
        // handle in rdi stands for, then for a message at the end that
        // handle stands for, logs it and ends with its length.
        ".global meridian_taker_start",
        "meridian_taker_start:",
        "mov rbx, rdi",
        "14:",
        "mov rdi, rbx",
        "lea rsi, [rsp - 256]",
        "mov edx, 128",
        "mov eax, {receive}",
        "int 0x2e",
        "mov rcx, {empty}",
        "cmp rax, rcx",
        "jne 15f",
        "mov edi, 1",
        "mov eax, {sleep}",
        "int 0x2e",
        "jmp 14b",
        "15:",
        "mov r12, rdx",
        "16:",
        "mov rdi, r12",
        "lea rsi, [rsp - 256]",
        "mov edx, 128",
        "mov eax, {receive}",
        "int 0x2e",
        "mov rcx, {empty}",
        "cmp rax, rcx",
        "jne 17f",
        "mov edi, 1",
        "mov eax, {sleep}",
        "int 0x2e",
        "jmp 16b",
        "17:",
        "mov r13, rax",
        "mov rdi, r13",
        "lea rsi, [rsp - 256]",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, r13",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Logs nothing at all.
        ".global meridian_empty_log_start",
        "meridian_empty_log_start:",
        "xor edi, edi",
        "xor esi, esi",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Logs the last 8 bytes of its stack page, up to the very end.
        ".global meridian_edge_log_start",
        "meridian_edge_log_start:",
        "movabs rax, 0x64726f777473616c",
        "mov [0x7ffff8], rax",
        "mov esi, 0x7ffff8",
        "mov edi, 8",
        "mov eax, {log}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Sends nine messages into a channel of its own that takes eight,
        // and ends with the last answer.
        ".global meridian_fill_start",
        "meridian_fill_start:",
        "mov eax, {channel}",
        "int 0x2e",
        "mov r12, rax",
        "mov r15, 9",
        "10:",
        "mov rdi, r12",
        "lea rsi, [rip]",
        "mov edx, 1",
        "mov eax, {send}",
        "int 0x2e",
        "dec r15",
        "jnz 10b",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Sends itself 5 bytes and gives 2 of room to take them, then
        // gives its read only code as the room.
        ".global meridian_small_room_start",
        "meridian_small_room_start:",
        "mov eax, {channel}",
        "int 0x2e",
        "mov r12, rax",
        "mov r13, rdx",
        "mov rdi, r12",
        "lea rsi, [rip]",
        "mov edx, 5",
        "mov eax, {send}",
        "int 0x2e",
        "mov rdi, r13",
        "lea rsi, [rsp - 64]",
        "mov edx, 2",
        "mov eax, {receive}",
        "int 0x2e",
        "mov r14, rax",
        "mov rdi, r13",
        "lea rsi, [rip]",
        "mov edx, 8",
        "mov eax, {receive}",
        "int 0x2e",
        "mov rcx, {bad_address}",
        "cmp rax, rcx",
        "jne 11f",
        "mov rdi, r14",
        "mov eax, {exit}",
        "int 0x2e",
        "11:",
        "ud2",
        // Sends through a handle it never got.
        ".global meridian_bad_handle_start",
        "meridian_bad_handle_start:",
        "mov edi, 999",
        "lea rsi, [rip]",
        "mov edx, 1",
        "mov eax, {send}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Ends with 42, by way of its stack.
        ".global meridian_stack_start",
        "meridian_stack_start:",
        "push 42",
        "pop rdi",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Asks for a thread that would start in the kernel's half, and ends
        // with the answer.
        ".global meridian_thread_nowhere_start",
        "meridian_thread_nowhere_start:",
        "mov rdi, {user_end}",
        "mov rsi, {stack_top}",
        "xor edx, edx",
        "mov eax, {thread}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // The same with a stack above the program's half.
        ".global meridian_thread_no_stack_start",
        "meridian_thread_no_stack_start:",
        "lea rdi, [rip + meridian_thread_no_stack_start]",
        "mov rsi, {past_end}",
        "xor edx, edx",
        "mov eax, {thread}",
        "int 0x2e",
        "mov rdi, rax",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        // Starts a second thread on the lower half of its stack, which
        // sleeps and ends with the 5 it was given, and ends its first one
        // with 3 at once: the program ends with the last one's code.
        ".global meridian_two_threads_start",
        "meridian_two_threads_start:",
        "lea rdi, [rip + 2f]",
        "mov rsi, {lower_stack}",
        "mov edx, 5",
        "mov eax, {thread}",
        "int 0x2e",
        "mov edi, 3",
        "mov eax, {end_thread}",
        "int 0x2e",
        "ud2",
        "2:",
        "mov rbx, rdi",
        "mov edi, 2",
        "mov eax, {sleep}",
        "int 0x2e",
        "mov rdi, rbx",
        "mov eax, {end_thread}",
        "int 0x2e",
        "ud2",
        // Starts a second thread that spins without ever calling the
        // kernel, then exits with 9, which has to end that one too.
        ".global meridian_exit_all_start",
        "meridian_exit_all_start:",
        "lea rdi, [rip + 2f]",
        "mov rsi, {lower_stack}",
        "xor edx, edx",
        "mov eax, {thread}",
        "int 0x2e",
        "mov edi, 2",
        "mov eax, {sleep}",
        "int 0x2e",
        "mov edi, 9",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        "2:",
        "pause",
        "jmp 2b",
        // Maps a page to read and one for flags, and starts a second thread
        // that reads the first page over and over and notes the core it
        // runs on. Once that is another core than the one it finds itself
        // on, it gives the page back and says so: the second thread's next
        // read has to fault, and ends with 77 if it still finds the page.
        // 55 when the two never ran apart.
        ".global meridian_stale_threads_start",
        "meridian_stale_threads_start:",
        "mov edi, 2",
        "mov eax, {map}",
        "int 0x2e",
        "mov r12, rax",
        "lea r13, [rax + 4096]",
        "lea rdi, [rip + 3f]",
        "mov rsi, {lower_stack}",
        "mov rdx, r12",
        "mov eax, {thread}",
        "int 0x2e",
        "mov r14, {patience}",
        "2:",
        "rdtscp",
        "mov r15, rcx",
        "mov rax, [r13 + 24]",
        "test rax, rax",
        "jz 4f",
        "dec rax",
        "cmp rax, r15",
        "jne 5f",
        "4:",
        "pause",
        "dec r14",
        "jnz 2b",
        "mov edi, 55",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        "5:",
        "mov rdi, r12",
        "mov esi, 1",
        "mov eax, {unmap}",
        "int 0x2e",
        "mov qword ptr [r13 + 8], 1",
        "6:",
        "pause",
        "jmp 6b",
        "3:",
        "mov r12, rdi",
        "lea r13, [rdi + 4096]",
        "7:",
        "mov rax, [r12]",
        "rdtscp",
        "inc rcx",
        "mov [r13 + 24], rcx",
        "cmp qword ptr [r13 + 8], 0",
        "je 7b",
        "mov rax, [r12]",
        "mov edi, 77",
        "mov eax, {exit}",
        "int 0x2e",
        "ud2",
        ".global meridian_probes_end",
        "meridian_probes_end:",
        ".popsection",
        log = const syscall::LOG,
        exit = const syscall::EXIT,
        sleep = const syscall::SLEEP,
        ticks = const syscall::TICKS,
        too_long = const syscall::LOG_MAX + 1,
        yield_now = const syscall::YIELD,
        channel = const syscall::CHANNEL,
        send = const syscall::SEND,
        receive = const syscall::RECEIVE,
        empty = const syscall::EMPTY,
        bad_address = const syscall::BAD_ADDRESS,
        close = const syscall::CLOSE,
        map = const syscall::MAP,
        unmap = const syscall::UNMAP,
        spawn = const syscall::SPAWN,
        thread = const syscall::THREAD,
        end_thread = const syscall::END_THREAD,
        user_end = const syscall::USER_END,
        past_end = const syscall::USER_END + 16,
        stack_top = const super::STACK_TOP,
        lower_stack = const super::STACK_TOP - 0x8000,
        patience = const 1u64 << 25,
    );

    unsafe extern "C" {
        static meridian_read_kernel_start: u8;
        static meridian_halt_start: u8;
        static meridian_bad_log_start: u8;
        static meridian_long_log_start: u8;
        static meridian_unknown_start: u8;
        static meridian_nap_start: u8;
        static meridian_stack_start: u8;
        static meridian_yielder_start: u8;
        static meridian_unmapped_log_start: u8;
        static meridian_spinner_start: u8;
        static meridian_keep_start: u8;
        static meridian_write_code_start: u8;
        static meridian_pair_start: u8;
        static meridian_sender_start: u8;
        static meridian_receiver_start: u8;
        static meridian_bad_handle_start: u8;
        static meridian_empty_log_start: u8;
        static meridian_passer_start: u8;
        static meridian_taker_start: u8;
        static meridian_edge_log_start: u8;
        static meridian_fill_start: u8;
        static meridian_small_room_start: u8;
        static meridian_run_stack_start: u8;
        static meridian_set_ac_start: u8;
        static meridian_stale_start: u8;
        static meridian_spawn_longest_start: u8;
        static meridian_spawn_too_long_start: u8;
        static meridian_thread_nowhere_start: u8;
        static meridian_thread_no_stack_start: u8;
        static meridian_two_threads_start: u8;
        static meridian_exit_all_start: u8;
        static meridian_stale_threads_start: u8;
        static meridian_probes_end: u8;
    }

    fn thread_nowhere() -> &'static [u8] {
        bytes(
            &raw const meridian_thread_nowhere_start,
            &raw const meridian_probes_end,
        )
    }

    fn thread_no_stack() -> &'static [u8] {
        bytes(
            &raw const meridian_thread_no_stack_start,
            &raw const meridian_probes_end,
        )
    }

    fn two_threads() -> &'static [u8] {
        bytes(
            &raw const meridian_two_threads_start,
            &raw const meridian_probes_end,
        )
    }

    fn exit_all() -> &'static [u8] {
        bytes(
            &raw const meridian_exit_all_start,
            &raw const meridian_probes_end,
        )
    }

    pub static THREAD_NOWHERE: Program = Program {
        code: thread_nowhere,
    };
    pub static THREAD_NO_STACK: Program = Program {
        code: thread_no_stack,
    };
    pub static TWO_THREADS: Program = Program { code: two_threads };
    pub static EXIT_ALL: Program = Program { code: exit_all };

    fn stale_threads() -> &'static [u8] {
        bytes(
            &raw const meridian_stale_threads_start,
            &raw const meridian_probes_end,
        )
    }

    pub static STALE_THREADS: Program = Program {
        code: stale_threads,
    };

    // Each probe runs to the end of them all; what follows its own code
    // is never reached.
    fn read_kernel() -> &'static [u8] {
        bytes(
            &raw const meridian_read_kernel_start,
            &raw const meridian_probes_end,
        )
    }
    fn halt() -> &'static [u8] {
        bytes(
            &raw const meridian_halt_start,
            &raw const meridian_probes_end,
        )
    }
    fn bad_log() -> &'static [u8] {
        bytes(
            &raw const meridian_bad_log_start,
            &raw const meridian_probes_end,
        )
    }
    fn long_log() -> &'static [u8] {
        bytes(
            &raw const meridian_long_log_start,
            &raw const meridian_probes_end,
        )
    }
    fn unknown() -> &'static [u8] {
        bytes(
            &raw const meridian_unknown_start,
            &raw const meridian_probes_end,
        )
    }
    fn nap() -> &'static [u8] {
        bytes(
            &raw const meridian_nap_start,
            &raw const meridian_probes_end,
        )
    }
    fn stack() -> &'static [u8] {
        bytes(
            &raw const meridian_stack_start,
            &raw const meridian_probes_end,
        )
    }

    fn yielder() -> &'static [u8] {
        bytes(
            &raw const meridian_yielder_start,
            &raw const meridian_probes_end,
        )
    }
    fn unmapped_log() -> &'static [u8] {
        bytes(
            &raw const meridian_unmapped_log_start,
            &raw const meridian_probes_end,
        )
    }
    fn spinner() -> &'static [u8] {
        bytes(
            &raw const meridian_spinner_start,
            &raw const meridian_probes_end,
        )
    }

    pub static YIELDER: Program = Program { code: yielder };
    pub static UNMAPPED_LOG: Program = Program { code: unmapped_log };
    pub static SPINNER: Program = Program { code: spinner };
    fn keep() -> &'static [u8] {
        bytes(
            &raw const meridian_keep_start,
            &raw const meridian_probes_end,
        )
    }

    pub static KEEP: Program = Program { code: keep };

    fn write_code() -> &'static [u8] {
        bytes(
            &raw const meridian_write_code_start,
            &raw const meridian_probes_end,
        )
    }
    fn run_stack() -> &'static [u8] {
        bytes(
            &raw const meridian_run_stack_start,
            &raw const meridian_probes_end,
        )
    }

    pub static WRITE_CODE: Program = Program { code: write_code };

    fn set_ac() -> &'static [u8] {
        bytes(
            &raw const meridian_set_ac_start,
            &raw const meridian_probes_end,
        )
    }

    pub static SET_AC: Program = Program { code: set_ac };

    fn stale() -> &'static [u8] {
        bytes(
            &raw const meridian_stale_start,
            &raw const meridian_probes_end,
        )
    }

    pub static STALE: Program = Program { code: stale };

    fn spawn_longest() -> &'static [u8] {
        bytes(
            &raw const meridian_spawn_longest_start,
            &raw const meridian_probes_end,
        )
    }
    fn spawn_too_long() -> &'static [u8] {
        bytes(
            &raw const meridian_spawn_too_long_start,
            &raw const meridian_probes_end,
        )
    }

    pub static SPAWN_LONGEST: Program = Program {
        code: spawn_longest,
    };
    pub static SPAWN_TOO_LONG: Program = Program {
        code: spawn_too_long,
    };

    fn pair() -> &'static [u8] {
        bytes(
            &raw const meridian_pair_start,
            &raw const meridian_probes_end,
        )
    }
    fn sender() -> &'static [u8] {
        bytes(
            &raw const meridian_sender_start,
            &raw const meridian_probes_end,
        )
    }
    fn receiver() -> &'static [u8] {
        bytes(
            &raw const meridian_receiver_start,
            &raw const meridian_probes_end,
        )
    }
    fn bad_handle() -> &'static [u8] {
        bytes(
            &raw const meridian_bad_handle_start,
            &raw const meridian_probes_end,
        )
    }

    pub static PAIR: Program = Program { code: pair };
    pub static SENDER: Program = Program { code: sender };
    pub static RECEIVER: Program = Program { code: receiver };
    pub static BAD_HANDLE: Program = Program { code: bad_handle };

    fn empty_log() -> &'static [u8] {
        bytes(
            &raw const meridian_empty_log_start,
            &raw const meridian_probes_end,
        )
    }
    fn edge_log() -> &'static [u8] {
        bytes(
            &raw const meridian_edge_log_start,
            &raw const meridian_probes_end,
        )
    }
    fn fill() -> &'static [u8] {
        bytes(
            &raw const meridian_fill_start,
            &raw const meridian_probes_end,
        )
    }
    fn small_room() -> &'static [u8] {
        bytes(
            &raw const meridian_small_room_start,
            &raw const meridian_probes_end,
        )
    }

    pub static EMPTY_LOG: Program = Program { code: empty_log };

    fn passer() -> &'static [u8] {
        bytes(
            &raw const meridian_passer_start,
            &raw const meridian_probes_end,
        )
    }
    fn taker() -> &'static [u8] {
        bytes(
            &raw const meridian_taker_start,
            &raw const meridian_probes_end,
        )
    }

    pub static PASSER: Program = Program { code: passer };
    pub static TAKER: Program = Program { code: taker };
    pub static EDGE_LOG: Program = Program { code: edge_log };
    pub static FILL: Program = Program { code: fill };
    pub static SMALL_ROOM: Program = Program { code: small_room };
    pub static RUN_STACK: Program = Program { code: run_stack };
    pub static READ_KERNEL: Program = Program { code: read_kernel };
    pub static HALT: Program = Program { code: halt };
    pub static BAD_LOG: Program = Program { code: bad_log };
    pub static LONG_LOG: Program = Program { code: long_log };
    pub static UNKNOWN: Program = Program { code: unknown };
    pub static NAP: Program = Program { code: nap };
    pub static STACK: Program = Program { code: stack };
}

/// What the test image looks at once a program ended.
#[cfg(feature = "qemu-test")]
pub mod seen {
    use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    use kernel::thread::THREADS;

    /// The privilege level the last system call came from, the stack
    /// segment the kernel ran it on, and whether the AC flag was set then.
    pub static RING: AtomicU64 = AtomicU64::new(0);
    pub static STACK_SEGMENT: AtomicU64 = AtomicU64::new(0);
    pub static USER_ACCESS: AtomicBool = AtomicBool::new(false);
    /// Where the last fault of a program was, as CR2 had it.
    static ADDRESS: AtomicU64 = AtomicU64::new(u64::MAX);
    /// Each thread's exit code, and the vector of the fault that ended it.
    static EXIT: [AtomicU64; THREADS] = [const { AtomicU64::new(u64::MAX) }; THREADS];
    static FAULT: [AtomicU64; THREADS] = [const { AtomicU64::new(u64::MAX) }; THREADS];
    static ERROR: [AtomicU64; THREADS] = [const { AtomicU64::new(u64::MAX) }; THREADS];
    /// The last line a program logged, and the last line the kernel logged
    /// for a program's end, each up to 64 bytes of it and its length.
    static LOG: Kept = Kept::new();
    static ENDED: Kept = Kept::new();

    /// How often each call was made since the last program started.
    static CALLS: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

    pub(super) fn called_from(cs: u64, number: u64) {
        if let Some(count) = usize::try_from(number).ok().and_then(|n| CALLS.get(n)) {
            count.fetch_add(1, Ordering::AcqRel);
        }
        RING.store(cs & 3, Ordering::Release);
        let (_, ss) = x86::raw::segments();
        STACK_SEGMENT.store(u64::from(ss), Ordering::Release);
        USER_ACCESS.store(x86::raw::user_access(), Ordering::Release);
    }

    pub(super) fn exited(code: u64) {
        if let Some(e) = EXIT.get(super::thread::current()) {
            e.store(code, Ordering::Release);
        }
    }

    pub(super) fn faulted(vector: u64, error: u64) {
        ADDRESS.store(x86::raw::cr2(), Ordering::Release);
        let slot = super::thread::current();
        if let (Some(f), Some(e)) = (FAULT.get(slot), ERROR.get(slot)) {
            f.store(vector, Ordering::Release);
            e.store(error, Ordering::Release);
        }
    }

    /// Where the last fault that ended a program was.
    pub fn fault_address() -> u64 {
        ADDRESS.load(Ordering::Acquire)
    }

    /// The error code of the fault that ended thread `slot`, or `u64::MAX`.
    pub fn error_of(slot: usize) -> u64 {
        ERROR
            .get(slot)
            .map_or(u64::MAX, |e| e.load(Ordering::Acquire))
    }

    /// The exit code thread `slot` ended with, or `u64::MAX`.
    pub fn exit_of(slot: usize) -> u64 {
        EXIT.get(slot)
            .map_or(u64::MAX, |e| e.load(Ordering::Acquire))
    }

    /// The fault that ended thread `slot`, or `u64::MAX`.
    pub fn fault_of(slot: usize) -> u64 {
        FAULT
            .get(slot)
            .map_or(u64::MAX, |f| f.load(Ordering::Acquire))
    }

    struct Kept {
        words: [AtomicU64; 8],
        len: AtomicUsize,
    }

    impl Kept {
        const fn new() -> Kept {
            Kept {
                words: [const { AtomicU64::new(0) }; 8],
                len: AtomicUsize::new(0),
            }
        }

        fn keep(&self, text: &[u8], len: usize) {
            let mut words = [0u64; 8];
            for (i, &b) in text.iter().take(64).enumerate() {
                words[i / 8] |= u64::from(b) << (i % 8 * 8);
            }
            for (slot, word) in self.words.iter().zip(words) {
                slot.store(word, Ordering::Release);
            }
            self.len.store(len, Ordering::Release);
        }

        fn holds(&self, text: &[u8]) -> bool {
            self.len.load(Ordering::Acquire) == text.len()
                && text.iter().take(64).enumerate().all(|(i, &b)| {
                    self.words[i / 8].load(Ordering::Acquire) >> (i % 8 * 8) & 0xff == u64::from(b)
                })
        }
    }

    pub(super) fn keep_log(text: &[u8]) {
        LOG.keep(text, text.len());
    }

    /// Whether the last line logged is `text`, up to 64 bytes of it.
    pub fn logged(text: &[u8]) -> bool {
        LOG.holds(text)
    }

    /// A writer that passes everything on to `out` and keeps the first 64
    /// bytes, counting the rest.
    pub(super) struct Through<W> {
        out: W,
        kept: [u8; 64],
        len: usize,
    }

    impl<W: core::fmt::Write> Through<W> {
        pub(super) fn new(out: W) -> Through<W> {
            Through {
                out,
                kept: [0; 64],
                len: 0,
            }
        }
    }

    impl<W: core::fmt::Write> core::fmt::Write for Through<W> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if let Some(room) = self.kept.get_mut(self.len) {
                    *room = b;
                }
                self.len += 1;
            }
            self.out.write_str(s)
        }
    }

    pub(super) fn keep_ended<W>(line: &Through<W>) {
        ENDED.keep(&line.kept, line.len);
    }

    /// Whether the last line the kernel logged for a program's end is
    /// `line`, up to 64 bytes of it.
    pub fn ended(line: &[u8]) -> bool {
        ENDED.holds(line)
    }

    /// How often the call `number` was made since the last program
    /// started.
    pub fn calls(number: u64) -> u64 {
        usize::try_from(number)
            .ok()
            .and_then(|n| CALLS.get(n))
            .map_or(0, |c| c.load(Ordering::Acquire))
    }

    /// Forgets what the programs before left: the calls they made, the
    /// last line they logged and the last end logged. A check does this
    /// before it starts the program it looks at, which may run at once on
    /// another tick.
    pub fn forget() {
        for count in &CALLS {
            count.store(0, Ordering::Release);
        }
        RING.store(0, Ordering::Release);
        STACK_SEGMENT.store(0, Ordering::Release);
        USER_ACCESS.store(false, Ordering::Release);
        LOG.len.store(0, Ordering::Release);
        ENDED.len.store(0, Ordering::Release);
    }

    /// Forgets how the program before ended on thread `slot`, as the
    /// thread enters a new one.
    pub(super) fn starting(slot: usize) {
        if let (Some(e), Some(f)) = (EXIT.get(slot), FAULT.get(slot)) {
            e.store(u64::MAX, Ordering::Release);
            f.store(u64::MAX, Ordering::Release);
        }
    }
}
