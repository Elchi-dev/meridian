//! Ring 3. Every program gets an address space of its own, whose kernel
//! half is the kernel's: its machine code and a stack page are mapped for
//! ring 3 in its lower half, and a thread of its own enters it. System
//! calls and faults bring the thread back into the kernel, on its kernel
//! stack; a fault in ring 3 ends only that program. Once it ends, its
//! address space and everything in it go back to the frame allocator.

use alloc::boxed::Box;
use core::arch::global_asm;
use core::fmt::Write;

use kernel::syscall::{self, Call};
use paging::Flags;

use super::interrupts::Frame;
use super::memory;
use super::thread;

/// Where a program's code goes, and the top of its stack.
pub const CODE_AT: u64 = 0x40_0000;
pub const STACK_TOP: u64 = 0x80_0000;

/// A program: its machine code, position independent and at most a page.
pub struct Program {
    code: fn() -> &'static [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No memory was left for its address space.
    NoMemory,
    /// Its thread could not start.
    Thread(thread::Error),
}

/// Starts `program` in ring 3 with `argument` in rdi, in an address space
/// and a thread of its own on `core`, and returns the thread's number.
///
/// # Errors
///
/// When no memory is left for the address space, or the thread cannot
/// start.
pub fn start(core: usize, program: &'static Program, argument: u64) -> Result<usize, Error> {
    let root = memory::new_space().ok_or(Error::NoMemory)?;
    let stack = Flags::USER | Flags::WRITABLE | Flags::NO_EXECUTE;
    let mapped = memory::map_fresh_in(root, CODE_AT, Flags::USER, (program.code)()).is_ok()
        && memory::map_fresh_in(root, STACK_TOP - 4096, stack, &[]).is_ok();
    if !mapped {
        memory::release_space(root);
        return Err(Error::NoMemory);
    }
    let argument = Box::into_raw(Box::new(argument)) as u64;
    thread::spawn_in(core, root, run, argument, thread::NORMAL).map_err(|e| {
        // SAFETY: the box made just above, which no thread took.
        drop(unsafe { Box::from_raw(argument as *mut u64) });
        memory::release_space(root);
        Error::Thread(e)
    })
}

/// The program's thread, in its own address space: enters ring 3.
extern "C" fn run(argument: u64) {
    // SAFETY: start hands each thread the box it made for it, once.
    let argument = *unsafe { Box::from_raw(argument as *mut u64) };
    // SAFETY: start mapped the code and stack pages for ring 3 in this
    // thread's address space, which the switch to it loaded, every switch
    // to it put its kernel stack in the task state, and the IDT has the
    // system call gate for ring 3.
    unsafe { x86::raw::enter_user(CODE_AT, STACK_TOP, argument) }
}

/// Ends the program: back on the kernel's address space, its own goes back
/// to the frame allocator, and its thread ends.
fn finish() -> ! {
    let root = thread::current_root();
    // SAFETY: the kernel's address space maps all the kernel does, this
    // stack included.
    unsafe { x86::raw::load_cr3(memory::kernel_root()) };
    if root != 0 {
        memory::release_space(root);
    }
    thread::exit()
}

/// A system call from ring 3, on the calling thread's kernel stack: the
/// answer goes back in rax.
pub fn system_call(frame: &mut Frame) {
    #[cfg(feature = "qemu-test")]
    seen::RING.store(frame.cs & 3, core::sync::atomic::Ordering::Release);
    frame.rax = match syscall::decode(frame.rax, frame.rdi, frame.rsi) {
        Call::Exit(code) => {
            let _ = writeln!(
                x86::raw::Com1::already_open(),
                "user: the program ended with {code}"
            );
            #[cfg(feature = "qemu-test")]
            seen::exited(code);
            finish()
        }
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
        Call::Unknown(_) => syscall::NO_SUCH_CALL,
    };
}

/// Writes the program's line to the log, once it is sure every byte is
/// the program's to give.
fn log(at: u64, len: u64) -> u64 {
    let Some(count) = usize::try_from(len)
        .ok()
        .filter(|&n| n <= syscall::LOG_BYTES)
    else {
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
    unsafe { core::ptr::copy_nonoverlapping(at as *const u8, text.as_mut_ptr(), text.len()) };
    let mut serial = x86::raw::Com1::already_open();
    let _ = serial.write_str("user: ");
    for &byte in text.iter() {
        let shown = if byte.is_ascii_graphic() || byte == b' ' {
            byte
        } else {
            b'?'
        };
        let _ = serial.write_char(char::from(shown));
    }
    let _ = serial.write_char('\n');
    #[cfg(feature = "qemu-test")]
    seen::keep_log(text);
    0
}

/// A fault in ring 3: the program ends, and the kernel goes on.
pub fn fault(frame: &Frame) -> ! {
    let _ = writeln!(
        x86::raw::Com1::already_open(),
        "user: the program ended by exception {} with error {:#x} at {:#x}",
        frame.vector,
        frame.error,
        frame.rip
    );
    #[cfg(feature = "qemu-test")]
    seen::faulted(frame.vector, frame.error);
    finish()
}

fn bytes(start: *const u8, end: *const u8) -> &'static [u8] {
    // SAFETY: the two symbols of a program in .rodata, end after start,
    // and the bytes between them never change.
    unsafe { core::slice::from_raw_parts(start, end as usize - start as usize) }
}

// The first program: it writes a line to the log and ends.
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

unsafe extern "C" {
    static meridian_hello_start: u8;
    static meridian_hello_end: u8;
}

fn hello() -> &'static [u8] {
    bytes(
        &raw const meridian_hello_start,
        &raw const meridian_hello_end,
    )
}

/// The first program Meridian runs in ring 3.
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
        // Ends with 42, by way of its stack.
        ".global meridian_stack_start",
        "meridian_stack_start:",
        "push 42",
        "pop rdi",
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
        static meridian_run_stack_start: u8;
        static meridian_probes_end: u8;
    }

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
    use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use kernel::thread::THREADS;

    /// The privilege level the last system call came from.
    pub static RING: AtomicU64 = AtomicU64::new(0);
    /// Each thread's exit code, and the vector of the fault that ended it.
    static EXIT: [AtomicU64; THREADS] = [const { AtomicU64::new(u64::MAX) }; THREADS];
    static FAULT: [AtomicU64; THREADS] = [const { AtomicU64::new(u64::MAX) }; THREADS];
    static ERROR: [AtomicU64; THREADS] = [const { AtomicU64::new(u64::MAX) }; THREADS];
    static LOG: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
    static LOG_LEN: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn exited(code: u64) {
        if let Some(e) = EXIT.get(super::thread::current()) {
            e.store(code, Ordering::Release);
        }
    }

    pub(super) fn faulted(vector: u64, error: u64) {
        let slot = super::thread::current();
        if let (Some(f), Some(e)) = (FAULT.get(slot), ERROR.get(slot)) {
            f.store(vector, Ordering::Release);
            e.store(error, Ordering::Release);
        }
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

    pub(super) fn keep_log(text: &[u8]) {
        let mut words = [0u64; 8];
        for (i, &b) in text.iter().take(64).enumerate() {
            words[i / 8] |= u64::from(b) << (i % 8 * 8);
        }
        for (slot, word) in LOG.iter().zip(words) {
            slot.store(word, Ordering::Release);
        }
        LOG_LEN.store(text.len(), Ordering::Release);
    }

    /// Whether the last line logged is `text`, up to 64 bytes of it.
    pub fn logged(text: &[u8]) -> bool {
        LOG_LEN.load(Ordering::Acquire) == text.len()
            && text.iter().take(64).enumerate().all(|(i, &b)| {
                LOG[i / 8].load(Ordering::Acquire) >> (i % 8 * 8) & 0xff == u64::from(b)
            })
    }

    /// Forgets what thread `slot` and the last program left.
    pub fn forget(slot: usize) {
        RING.store(0, Ordering::Release);
        LOG_LEN.store(0, Ordering::Release);
        if let (Some(e), Some(f)) = (EXIT.get(slot), FAULT.get(slot)) {
            e.store(u64::MAX, Ordering::Release);
            f.store(u64::MAX, Ordering::Release);
        }
    }
}
