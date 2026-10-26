//! Starting the other processors. Each one gets INIT and then startup
//! interrupts that run the trampoline: a page below 1 MiB that takes it
//! from real mode through protected mode into long mode, on page tables
//! of its own that map the trampoline where it is and the kernel's half
//! as the kernel has it, and on into the kernel, which switches it to the
//! kernel's page tables and a stack of its own.

use alloc::vec::Vec;
use core::arch::{global_asm, naked_asm};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use bootinfo::{HHDM_BASE, MemoryRegion};
use kernel::CORE_STACK_PAGES;

use super::interrupts::{CORES, SPURIOUS, WAKE};

// The trampoline. It runs at whatever page it was copied to, so it finds
// itself from CS, which the startup interrupt sets to the page, and keeps
// that address in EBX; its stack is the end of its own page.
global_asm!(
    ".pushsection .text",
    ".global meridian_trampoline_start",
    ".global meridian_trampoline_end",
    ".global meridian_trampoline_cr3",
    ".global meridian_trampoline_entry",
    // Places in the trampoline as distances from its start, since a memory
    // operand may name only one symbol.
    ".set TRAMPOLINE_GDT, meridian_trampoline_gdt - meridian_trampoline_start",
    ".set TRAMPOLINE_GDT_POINTER, meridian_trampoline_gdt_pointer - meridian_trampoline_start",
    ".set TRAMPOLINE_32, meridian_trampoline_32 - meridian_trampoline_start",
    ".set TRAMPOLINE_64, meridian_trampoline_64 - meridian_trampoline_start",
    ".set TRAMPOLINE_CR3, meridian_trampoline_cr3 - meridian_trampoline_start",
    ".code16",
    "meridian_trampoline_start:",
    "cli",
    "cld",
    "mov ax, cs",
    "mov ds, ax",
    "mov ss, ax",
    "mov sp, 0xff0",
    "movzx ebx, ax",
    "shl ebx, 4",
    "lea eax, [ebx + TRAMPOLINE_GDT]",
    "mov dword ptr [TRAMPOLINE_GDT_POINTER + 2], eax",
    "lgdt [TRAMPOLINE_GDT_POINTER]",
    "mov eax, cr0",
    "or eax, 1",
    "mov cr0, eax",
    "lea eax, [ebx + TRAMPOLINE_32]",
    // push dword 0x08, the 32-bit code selector, then the 32-bit address,
    // and a 32-bit far return to them.
    ".byte 0x66, 0x6a, 0x08",
    "push eax",
    ".byte 0x66, 0xcb",
    ".code32",
    "meridian_trampoline_32:",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "lea esp, [ebx + 0xff0]",
    // PAE, the trampoline's own tables, long mode and no-execute pages,
    // then paging and write protection.
    "mov eax, cr4",
    "or eax, 0x20",
    "mov cr4, eax",
    "mov eax, [ebx + TRAMPOLINE_CR3]",
    "mov cr3, eax",
    "mov ecx, 0xc0000080",
    "rdmsr",
    "or eax, 0x900",
    "wrmsr",
    "mov eax, cr0",
    "or eax, 0x80010001",
    "mov cr0, eax",
    "lea eax, [ebx + TRAMPOLINE_64]",
    "push 0x18",
    "push eax",
    "retf",
    ".code64",
    "meridian_trampoline_64:",
    "mov rax, [rip + meridian_trampoline_entry]",
    "jmp rax",
    ".balign 8",
    "meridian_trampoline_gdt:",
    ".quad 0",
    ".quad 0x00cf9a000000ffff",
    ".quad 0x00cf92000000ffff",
    ".quad 0x00af9a000000ffff",
    "meridian_trampoline_gdt_pointer:",
    ".word 31",
    ".long 0",
    ".balign 8",
    "meridian_trampoline_cr3:",
    ".quad 0",
    "meridian_trampoline_entry:",
    ".quad 0",
    "meridian_trampoline_end:",
    ".popsection",
);

unsafe extern "C" {
    static meridian_trampoline_start: u8;
    static meridian_trampoline_end: u8;
    static meridian_trampoline_cr3: u8;
    static meridian_trampoline_entry: u8;
}

/// What a processor the trampoline brings into the kernel takes on: the
/// kernel's page tables, the top of its stack and its number.
static CR3: AtomicU64 = AtomicU64::new(0);
static STACK: AtomicU64 = AtomicU64::new(0);
static NUMBER: AtomicU64 = AtomicU64::new(0);
/// The lowest address of the stack in STACK, for the test image's
/// processors to check they run on it.
#[cfg(feature = "qemu-test")]
static STACK_BASE: AtomicU64 = AtomicU64::new(0);

/// How many other processors have come into the kernel, and how many of
/// them found themselves on the stack they were given and on the kernel's
/// descriptor tables.
static RUNNING: AtomicU32 = AtomicU32::new(0);

/// A job for one of the other cores: a function, what it works on, and
/// what came of it.
struct Mailbox {
    /// The job's function as an address, or 0 while none waits.
    job: AtomicUsize,
    argument: AtomicU64,
    result: AtomicU64,
    /// Whether the core has finished the last job it was sent.
    done: AtomicBool,
    /// Whether the core waits for jobs, and the APIC id that wakes it.
    ready: AtomicBool,
    apic_id: AtomicU32,
}

static MAILBOXES: [Mailbox; CORES] = [const {
    Mailbox {
        job: AtomicUsize::new(0),
        argument: AtomicU64::new(0),
        result: AtomicU64::new(0),
        done: AtomicBool::new(true),
        ready: AtomicBool::new(false),
        apic_id: AtomicU32::new(0),
    }
}; CORES];

/// Sends core `number` the job `work(argument)` and wakes it. Returns
/// false when that core does not wait for jobs or has not finished the
/// last one.
pub fn send(number: usize, work: fn(u64) -> u64, argument: u64) -> bool {
    let Some(mailbox) = MAILBOXES.get(number) else {
        return false;
    };
    if !mailbox.ready.load(Ordering::Acquire)
        || mailbox
            .done
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return false;
    }
    mailbox.argument.store(argument, Ordering::Release);
    mailbox.job.store(work as usize, Ordering::Release);
    // SAFETY: a ready core runs on the kernel's IDT, which has the handler
    // for WAKE.
    unsafe { x86::raw::send_interrupt(mailbox.apic_id.load(Ordering::Acquire), WAKE) };
    true
}

/// Waits up to `cycles` of the time stamp counter for core `number` to
/// finish the last job it was sent, and returns what came of it.
pub fn wait(number: usize, cycles: u64) -> Option<u64> {
    let mailbox = MAILBOXES.get(number)?;
    let finished = mailbox.ready.load(Ordering::Acquire)
        && x86::raw::wait_for(cycles, || mailbox.done.load(Ordering::Acquire));
    finished.then(|| mailbox.result.load(Ordering::Acquire))
}

/// The job that tells whether a core takes work: it answers with the
/// APIC id of the core that runs it.
fn own_id(_: u64) -> u64 {
    x86::raw::apic_id().map_or(u64::MAX, u64::from)
}

/// Runs the jobs core `number` is sent, and sleeps in between.
/// The core's idle thread: it runs the jobs the core is sent, sleeps until
/// the next interrupt when there is none, and lets the core's threads run.
fn work(mailbox: &Mailbox) -> ! {
    loop {
        x86::raw::disable_interrupts();
        let job = mailbox.job.swap(0, Ordering::AcqRel);
        if job == 0 {
            super::thread::halt_unless_ready();
        } else {
            // SAFETY: only send stores a job, and only a fn(u64) -> u64.
            let work = unsafe { core::mem::transmute::<usize, fn(u64) -> u64>(job) };
            let result = work(mailbox.argument.load(Ordering::Acquire));
            mailbox.result.store(result, Ordering::Release);
            mailbox.done.store(true, Ordering::Release);
        }
        super::thread::yield_now();
    }
}

/// Sends core `number` the interrupt `vector`.
pub fn interrupt(number: usize, vector: u8) {
    if let Some(mailbox) = MAILBOXES.get(number) {
        // SAFETY: every core runs on the kernel's IDT, which has a handler
        // for every vector the kernel sends: WAKE and SHOOTDOWN.
        unsafe { x86::raw::send_interrupt(mailbox.apic_id.load(Ordering::Acquire), vector) };
    }
}

/// Wakes core `number` with an interrupt, so it looks for threads that
/// became ready there.
pub fn wake(number: usize) {
    interrupt(number, WAKE);
}

/// The numbers of the other cores that came into the kernel.
pub fn others() -> impl Iterator<Item = usize> {
    MAILBOXES
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, m)| m.ready.load(Ordering::Acquire))
        .map(|(number, _)| number)
}
#[cfg(feature = "qemu-test")]
static SETTLED: AtomicU32 = AtomicU32::new(0);
/// How many of them answered the first job they were sent.
#[cfg(feature = "qemu-test")]
static ANSWERED: AtomicU32 = AtomicU32::new(0);
/// How many counter cycles starting them took. Only the test image keeps
/// it.
#[cfg(feature = "qemu-test")]
static TOOK: AtomicU64 = AtomicU64::new(0);

/// How many of them came while the kernel still waited for them, rather
/// than after it gave up on them. Only the test image keeps it.
#[cfg(feature = "qemu-test")]
static CAME: AtomicU32 = AtomicU32::new(0);

/// How many other processors have come into the kernel, and how many of
/// them settled on their own stack and the kernel's tables. Only the test
/// image asks.
#[cfg(feature = "qemu-test")]
pub fn running() -> (u32, u32, u32) {
    (
        RUNNING.load(Ordering::Acquire),
        SETTLED.load(Ordering::Acquire),
        ANSWERED.load(Ordering::Acquire),
    )
}

/// How many counter cycles starting the other processors took, and how
/// many of them came while the kernel waited for them. Only the test image
/// asks.
#[cfg(feature = "qemu-test")]
pub fn took() -> (u64, u32) {
    (TOOK.load(Ordering::Acquire), CAME.load(Ordering::Acquire))
}

/// Starts every usable processor the MADT lists but this one, one after
/// the other, and returns how many came into the kernel and answered a
/// first job with their own APIC id. Starts none
/// without the MADT, a local APIC, a known counter rate `tsc_hz`, room for
/// the trampoline below 640 KiB, or frames for their stacks.
pub fn start_others(regions: &[MemoryRegion], madt: Option<&[u8]>, tsc_hz: Option<u64>) -> u32 {
    let (Some(madt), Some(tsc_hz), Some(home)) = (madt, tsc_hz, kernel::trampoline_home(regions))
    else {
        return 0;
    };
    if !super::apic::enable() {
        return 0;
    }
    let Some(own) = x86::raw::apic_id() else {
        return 0;
    };
    MAILBOXES[0].apic_id.store(own, Ordering::Release);
    let mut others = Vec::new();
    let listed = acpi::each_processor(madt, |p| {
        if p.usable && p.apic_id != own {
            others.push(p.apic_id);
        }
    });
    if listed.is_err() {
        return 0;
    }
    let Ok(page) = u8::try_from(home / MemoryRegion::PAGE_SIZE) else {
        return 0;
    };
    place_trampoline(home);
    CR3.store(x86::raw::read_cr3(), Ordering::Release);
    #[cfg(feature = "qemu-test")]
    let begin = x86::raw::tsc();
    for (index, &id) in others.iter().enumerate() {
        let number = index + 1;
        let stack = kernel::core_stack_top(number, false);
        let fault_stack = kernel::core_stack_top(number, true);
        if super::interrupts::prepare_core(number, fault_stack).is_none() {
            break;
        }
        let mapped = super::memory::map_stack(stack, CORE_STACK_PAGES)
            .and_then(|()| super::memory::map_stack(fault_stack, CORE_STACK_PAGES));
        if mapped.is_err() {
            break;
        }
        #[cfg(feature = "qemu-test")]
        STACK_BASE.store(stack - CORE_STACK_PAGES * frames::FRAME, Ordering::Release);
        STACK.store(stack, Ordering::Release);
        NUMBER.store(number as u64, Ordering::Release);
        let Some(mailbox) = MAILBOXES.get(number) else {
            break;
        };
        mailbox.apic_id.store(id, Ordering::Release);
        let before = RUNNING.load(Ordering::Acquire);
        // INIT, then two startups, as the MultiProcessor Specification
        // asks; a processor that started at the first ignores the second.
        // SAFETY: id names another usable processor, which has waited
        // since the firmware started this one, and the page holds the
        // trampoline, which brings it into ap_main.
        unsafe { x86::raw::send_init(id) };
        x86::raw::spin(tsc_hz / 100);
        for _ in 0..2 {
            // SAFETY: as for the INIT above.
            unsafe { x86::raw::send_startup(id, page) };
            x86::raw::spin(tsc_hz / 5000);
        }
        // A core comes within microseconds on a machine of its own. A busy
        // host running Meridian in a virtual machine kept one away for more
        // than 100 ms, so each gets a second.
        let came = x86::raw::wait_for(tsc_hz, || RUNNING.load(Ordering::Acquire) > before);
        #[cfg(feature = "qemu-test")]
        CAME.fetch_add(u32::from(came), Ordering::AcqRel);
        #[cfg(not(feature = "qemu-test"))]
        let _ = came;
    }
    #[cfg(feature = "qemu-test")]
    TOOK.store(x86::raw::tsc() - begin, Ordering::Release);
    for number in 1..=others.len() {
        let _ = send(number, own_id, 0);
    }
    let answered = others
        .iter()
        .enumerate()
        .filter(|&(index, &id)| wait(index + 1, tsc_hz / 100) == Some(u64::from(id)))
        .count();
    let answered = u32::try_from(answered).unwrap_or(u32::MAX);
    #[cfg(feature = "qemu-test")]
    ANSWERED.store(answered, Ordering::Release);
    answered
}

/// Copies the trampoline to `home`, gives it page tables after it and
/// tells it where to go.
fn place_trampoline(home: u64) {
    const PAGE: usize = 4096;
    let start = &raw const meridian_trampoline_start as usize;
    let len = &raw const meridian_trampoline_end as usize - start;
    let offset = |symbol: *const u8| symbol as usize - start;
    let tables = [1, 2, 3].map(|n| home + n * PAGE as u64);
    // SAFETY: the trampoline's code lies between its two symbols in the
    // kernel's image, which stays mapped and is never written.
    let code = unsafe { core::slice::from_raw_parts(start as *const u8, len) };
    // SAFETY: trampoline_home found these pages free below 640 KiB, where
    // the frame allocator hands nothing out and nothing else writes; the
    // direct map covers them, and only this processor runs the kernel.
    let pages = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + home) as *mut u8, 4 * PAGE) };
    pages.fill(0);
    pages[..len].copy_from_slice(code);
    let mut put = |at: usize, value: u64| pages[at..at + 8].copy_from_slice(&value.to_le_bytes());
    put(offset(&raw const meridian_trampoline_cr3), tables[0]);
    put(
        offset(&raw const meridian_trampoline_entry),
        ap_entry as *const () as usize as u64,
    );
    // The first 2 MiB where they are, for the trampoline itself.
    put(PAGE, tables[1] | 0b11);
    put(2 * PAGE, tables[2] | 0b11);
    put(3 * PAGE, 0b1000_0011);
    // The kernel's half, through the kernel's own tables below it.
    let root = x86::raw::read_cr3() & !0xfff;
    // SAFETY: CR3 points at the kernel's root table, which the direct map
    // covers; reading it changes nothing.
    let kernel = unsafe { core::slice::from_raw_parts((HHDM_BASE + root) as *const u64, 512) };
    for (index, &value) in kernel.iter().enumerate().skip(256) {
        put(PAGE + index * 8, value);
    }
}

/// Where the trampoline jumps, in the kernel's half: onto the kernel's
/// page tables and the processor's own stack, and into `ap_main`.
#[unsafe(naked)]
unsafe extern "C" fn ap_entry() -> ! {
    naked_asm!(
        "mov rax, [rip + {cr3}]",
        "mov cr3, rax",
        "mov rsp, [rip + {stack}]",
        "mov rdi, [rip + {number}]",
        "call {main}",
        "ud2",
        cr3 = sym CR3,
        stack = sym STACK,
        number = sym NUMBER,
        main = sym ap_main,
    )
}

/// The other processors' life: the kernel's descriptor tables, their
/// APIC, a count, then the jobs they are sent.
extern "sysv64" fn ap_main(number: u64) -> ! {
    let number = usize::try_from(number).unwrap_or(0);
    super::interrupts::load_on_another_core(number);
    x86::raw::protect(x86::Protections::detect());
    super::fpu::start(super::fpu::saving());
    let _ = x86::raw::set_cpu_number(u32::try_from(number).unwrap_or(u32::MAX));
    let _ = super::apic::enable();
    x86::raw::set_spurious_vector(SPURIOUS);
    #[cfg(feature = "qemu-test")]
    settled(number);
    let Some(mailbox) = MAILBOXES.get(number) else {
        x86::raw::halt()
    };
    if !super::thread::join(number) {
        x86::raw::halt()
    }
    mailbox.ready.store(true, Ordering::Release);
    RUNNING.fetch_add(1, Ordering::AcqRel);
    work(mailbox)
}

/// Counts this processor as settled when it runs on the stack it was
/// given and on the kernel's descriptor tables.
#[cfg(feature = "qemu-test")]
fn settled(number: usize) {
    let here = 0u8;
    let base = STACK_BASE.load(Ordering::Acquire);
    let on_stack =
        (base..base + CORE_STACK_PAGES * frames::FRAME).contains(&((&raw const here) as u64));
    if on_stack && super::interrupts::loaded_here(number) {
        SETTLED.fetch_add(1, Ordering::AcqRel);
    }
}
