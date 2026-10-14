//! The kernel's own descriptor tables and the way into it for the 32
//! processor exceptions. Each entry stub puts an error code and the vector
//! on the stack in the same places, saves the registers and calls
//! `exception`, which shows the fault on the panic screen. In the test
//! image, a check can ask for a fault and have the handler resume after it.

use core::arch::naked_asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use x86::tables::{self, Gate, TaskState};

/// The registers as the stubs leave them on the stack, lowest address
/// first: what the stubs saved, the vector and error code, and what the
/// processor saved.
#[repr(C)]
pub struct Frame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// The stack double faults, NMIs and machine checks switch to, so they
/// still have one when the kernel's own is used up.
const FAULT_STACK: usize = 16 * 1024;

struct Tables {
    gdt: [u64; 5],
    tss: TaskState,
    idt: [Gate; 256],
    fault_stack: [u8; FAULT_STACK],
}

struct Shared(UnsafeCell<Tables>);

// SAFETY: install writes the tables once, before anything else reads them,
// and after that only the processor reads them.
unsafe impl Sync for Shared {}

static TABLES: Shared = Shared(UnsafeCell::new(Tables {
    gdt: [0; 5],
    tss: TaskState::new(),
    idt: [Gate::MISSING; 256],
    fault_stack: [0; FAULT_STACK],
}));

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// The vector of the APIC timer, the first after the exceptions, and the
/// one the APIC gives for interrupts it gave up on.
pub const TIMER: u8 = 0x20;
pub const SPURIOUS: u8 = 0xff;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// How many times the timer has interrupted. Only the test image counts.
#[cfg(feature = "qemu-test")]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

/// Loads the kernel's global descriptor table, task state segment and
/// interrupt descriptor table. Only the first call does anything.
pub fn install() {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: INSTALLED lets only the first call get here, and nothing
    // reads the tables before they are loaded below.
    let t = unsafe { &mut *TABLES.0.get() };
    let stack_top = (&raw const t.fault_stack) as u64 + FAULT_STACK as u64;
    t.tss.interrupt_stacks[0] = stack_top & !0xf;
    t.gdt = tables::gdt((&raw const t.tss) as u64);
    for (vector, stub) in STUBS.iter().enumerate() {
        let stack = u8::from(matches!(vector, 2 | 8 | 18));
        t.idt[vector] = Gate::interrupt(*stub as usize as u64, stack);
    }
    let timer: unsafe extern "C" fn() = timer_stub;
    let spurious: unsafe extern "C" fn() = spurious_stub;
    t.idt[usize::from(TIMER)] = Gate::interrupt(timer as usize as u64, 0);
    t.idt[usize::from(SPURIOUS)] = Gate::interrupt(spurious as usize as u64, 0);
    // SAFETY: the tables live in a static, so they stay where they are,
    // the GDT holds the selectors load_gdt uses and the task state at
    // TASK_STATE, and every present gate leads to a stub below.
    unsafe {
        x86::raw::load_gdt(&t.gdt);
        x86::raw::load_task_register(tables::TASK_STATE);
        x86::raw::load_idt(&t.idt);
    }
}

/// Loads the kernel's descriptor tables on another processor, which shares
/// them with the first. It has no task state of its own yet, so it runs
/// without the fault stack, and so may only halt.
pub fn load_on_another_core() {
    if !INSTALLED.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: install wrote the tables and nothing writes them again;
    // they live in a static.
    let t = unsafe { &*TABLES.0.get() };
    // SAFETY: the same tables the first processor runs on: permanent, with
    // the kernel's selectors, and every present gate leads to a stub.
    unsafe {
        x86::raw::load_gdt(&t.gdt);
        x86::raw::load_idt(&t.idt);
    }
}

/// Whether this processor runs on the kernel's descriptor tables. Only the
/// test image asks.
#[cfg(feature = "qemu-test")]
pub fn loaded_here() -> bool {
    // SAFETY: only the addresses of the tables are taken, not their
    // contents.
    let t = unsafe { &*TABLES.0.get() };
    x86::raw::gdt_register().base == (&raw const t.gdt) as u64
        && x86::raw::idt_register().base == (&raw const t.idt) as u64
}

/// Where the stubs bring every exception and interrupt. A timer tick is
/// counted and acknowledged; a spurious interrupt needs nothing.
extern "sysv64" fn exception(frame: &mut Frame) {
    if frame.vector == u64::from(TIMER) {
        TICKS.fetch_add(1, Ordering::AcqRel);
        x86::raw::end_of_interrupt();
        return;
    }
    if frame.vector == u64::from(SPURIOUS) {
        return;
    }
    #[cfg(feature = "qemu-test")]
    if expected::resume(frame) {
        return;
    }
    super::crash_from_exception(frame.vector, frame.error, x86::raw::cr2(), frame.rip)
}

/// Saves the registers, passes the frame to `exception`, puts the
/// registers back and returns from the interrupt.
#[unsafe(naked)]
unsafe extern "C" fn common() {
    naked_asm!(
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "cld",
        "call {handler}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "add rsp, 16",
        "iretq",
        handler = sym exception,
    );
}

/// The entry stubs, one per exception. Those the processor gives no error
/// code push a 0 in its place.
macro_rules! stubs {
    ($($vector:literal $kind:ident),* $(,)?) => {
        [$({
            #[unsafe(naked)]
            unsafe extern "C" fn stub() {
                stubs!(@body $vector $kind)
            }
            stub
        }),*]
    };
    (@body $vector:literal code) => {
        naked_asm!("push {v}", "jmp {common}", v = const $vector, common = sym common)
    };
    (@body $vector:literal none) => {
        naked_asm!("push 0", "push {v}", "jmp {common}", v = const $vector, common = sym common)
    };
}

#[unsafe(naked)]
unsafe extern "C" fn timer_stub() {
    naked_asm!("push 0", "push {v}", "jmp {common}", v = const TIMER, common = sym common)
}

#[unsafe(naked)]
unsafe extern "C" fn spurious_stub() {
    naked_asm!("push 0", "push {v}", "jmp {common}", v = const SPURIOUS, common = sym common)
}

static STUBS: [unsafe extern "C" fn(); 32] = stubs![
    0 none, 1 none, 2 none, 3 none, 4 none, 5 none, 6 none, 7 none,
    8 code, 9 none, 10 code, 11 code, 12 code, 13 code, 14 code, 15 none,
    16 none, 17 code, 18 none, 19 none, 20 none, 21 code, 22 none, 23 none,
    24 none, 25 none, 26 none, 27 none, 28 none, 29 code, 30 code, 31 none,
];

/// Faults the test image asks for, and the place to resume after each.
#[cfg(feature = "qemu-test")]
pub mod expected {
    use core::arch::asm;
    use core::sync::atomic::{AtomicU64, Ordering};

    use super::Frame;

    const NONE: u64 = u64::MAX;
    static VECTOR: AtomicU64 = AtomicU64::new(NONE);
    static RESUME: AtomicU64 = AtomicU64::new(0);
    static ERROR: AtomicU64 = AtomicU64::new(0);
    static ADDRESS: AtomicU64 = AtomicU64::new(0);

    /// What the handler saw: the vector, the error code and CR2.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Caught {
        pub vector: u64,
        pub error: u64,
        pub address: u64,
    }

    /// Resumes after a fault that was asked for.
    pub(super) fn resume(frame: &mut Frame) -> bool {
        if frame.vector != VECTOR.load(Ordering::Acquire) {
            return false;
        }
        VECTOR.store(NONE, Ordering::Release);
        ERROR.store(frame.error, Ordering::Release);
        ADDRESS.store(x86::raw::cr2(), Ordering::Release);
        frame.rip = RESUME.load(Ordering::Acquire);
        true
    }

    fn expect(vector: u64) {
        ERROR.store(u64::MAX, Ordering::Release);
        ADDRESS.store(u64::MAX, Ordering::Release);
        VECTOR.store(vector, Ordering::Release);
    }

    fn caught(vector: u64) -> Option<Caught> {
        if VECTOR.swap(NONE, Ordering::AcqRel) == vector {
            return None;
        }
        Some(Caught {
            vector,
            error: ERROR.load(Ordering::Acquire),
            address: ADDRESS.load(Ordering::Acquire),
        })
    }

    /// Runs `int3`.
    pub fn breakpoint() -> Option<Caught> {
        expect(3);
        // SAFETY: the breakpoint handler resumes at label 2, right after
        // the instruction.
        unsafe {
            asm!("lea {r}, [rip + 2f]", "mov [{resume}], {r}", "int3", "2:",
                r = out(reg) _, resume = in(reg) RESUME.as_ptr());
        }
        caught(3)
    }

    /// Runs `ud2`, which is no instruction at all.
    pub fn invalid_opcode() -> Option<Caught> {
        expect(6);
        // SAFETY: the handler resumes at label 2, past the ud2.
        unsafe {
            asm!("lea {r}, [rip + 2f]", "mov [{resume}], {r}", "ud2", "2:",
                r = out(reg) _, resume = in(reg) RESUME.as_ptr());
        }
        caught(6)
    }

    /// Reads address 0, which the kernel leaves unmapped.
    pub fn null_read() -> Option<Caught> {
        expect(14);
        // SAFETY: the read faults, nothing is read, and the handler resumes
        // at label 2.
        unsafe {
            asm!("lea {r}, [rip + 2f]", "mov [{resume}], {r}", "mov {r}, qword ptr [0]", "2:",
                r = out(reg) _, resume = in(reg) RESUME.as_ptr());
        }
        caught(14)
    }

    /// Divides by zero.
    pub fn divide_by_zero() -> Option<Caught> {
        expect(0);
        // SAFETY: the division faults before it writes eax or edx, and the
        // handler resumes at label 2.
        unsafe {
            asm!("lea {r}, [rip + 2f]", "mov [{resume}], {r}", "xor edx, edx", "mov eax, 1",
                "xor ecx, ecx", "div ecx", "2:",
                r = out(reg) _, resume = in(reg) RESUME.as_ptr(),
                out("eax") _, out("ecx") _, out("edx") _);
        }
        caught(0)
    }

    /// Loads a selector past the end of the global descriptor table.
    pub fn bad_selector() -> Option<Caught> {
        expect(13);
        // SAFETY: the load faults before DS changes, and the handler
        // resumes at label 2.
        unsafe {
            asm!("lea {r}, [rip + 2f]", "mov [{resume}], {r}", "mov {s:x}, 0x28", "mov ds, {s:x}", "2:",
                r = out(reg) _, resume = in(reg) RESUME.as_ptr(), s = out(reg) _);
        }
        caught(13)
    }
}
