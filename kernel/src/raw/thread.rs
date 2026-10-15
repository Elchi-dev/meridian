//! Kernel threads on every core. Each has a stack of its own, a priority
//! and the core it runs on, and is switched when it gives way, sleeps or
//! ends, and at every timer tick of its core when a thread of the same or
//! a higher priority is ready there; the tick also wakes the core's
//! sleepers whose time has come. Each core has an idle thread that runs
//! when nothing else is ready there: on the first core one of its own, on
//! the others the loop that also runs their jobs.

use core::arch::naked_asm;

use kernel::thread::{self, STACK_PAGES, State, Switch, Table};
pub use kernel::thread::{Error, NORMAL};

use super::interrupts::{CORES, this_core};
use super::memory::Spin;

static THREADS: Spin<Table<CORES>> = Spin::new(Table::new());

/// Gives the first core its idle thread, once.
pub fn start() {
    x86::raw::without_interrupts(|| {
        let mut threads = THREADS.lock();
        if threads.has_idle(0) {
            return;
        }
        // The idle thread halts with interrupts on, waiting for the next.
        if let Ok(slot) = make(&mut threads, 0, 0, idle, 0, 0, 0x202) {
            threads.make_idle(slot);
        }
    });
}

/// Makes the running code on another core that core's idle thread, so
/// threads can be started there. Returns false when no thread is left.
pub fn join(core: usize) -> bool {
    x86::raw::without_interrupts(|| THREADS.lock().join(core).is_ok())
}

/// Starts `entry(argument)` as a thread of its own at `priority` on
/// `core`, ready to run once that core's running thread gives way, wakes
/// that core for it, and returns its number. It ends when the entry
/// returns.
///
/// # Errors
///
/// When the priority is 32 or more, the core runs no threads, every
/// thread there can be exists, or no memory is left for its stack.
pub fn spawn_on(
    core: usize,
    entry: extern "C" fn(u64),
    argument: u64,
    priority: u8,
) -> Result<usize, Error> {
    spawn_in(core, 0, entry, argument, priority)
}

/// Starts a thread as [`spawn_on`] does, in the address space whose level
/// 4 table is at `root`, or the kernel's for 0.
///
/// # Errors
///
/// As for [`spawn_on`].
pub fn spawn_in(
    core: usize,
    root: u64,
    entry: extern "C" fn(u64),
    argument: u64,
    priority: u8,
) -> Result<usize, Error> {
    // The new thread starts with interrupts as its maker has them.
    let flags = if x86::raw::interrupts_enabled() {
        0x202
    } else {
        0x2
    };
    let slot = x86::raw::without_interrupts(|| {
        let mut threads = THREADS.lock();
        threads.can_start(core, priority)?;
        let slot = make(&mut threads, core, root, entry, argument, priority, flags)?;
        threads.queue(slot);
        Ok(slot)
    })?;
    if core != this_core() {
        super::smp::wake(core);
    }
    Ok(slot)
}

/// Takes a free thread, gives it a stack whose first switch lands in
/// `entry(argument)` with `flags`, and makes it ready on `core` in the
/// address space at `root` without queueing it.
fn make(
    threads: &mut Table<CORES>,
    core: usize,
    root: u64,
    entry: extern "C" fn(u64),
    argument: u64,
    priority: u8,
    flags: u64,
) -> Result<usize, Error> {
    let slot = threads.free().ok_or(Error::Full)?;
    let top = thread::stack_top(slot);
    if !threads.has_stack(slot) {
        super::memory::map_stack(top, STACK_PAGES).map_err(|_| Error::NoStack)?;
    }
    let at = thread::first_frame_at(top);
    let start = thread_start as *const () as usize as u64;
    let frame = thread::first_frame(flags, entry as usize as u64, argument, start);
    // SAFETY: the slot's stack is mapped and no thread runs on it: it is
    // free, or its thread ended and its core switched away for good.
    unsafe { (at as *mut [u64; 8]).write(frame) };
    threads.prepare(slot, core, priority, root, at);
    Ok(slot)
}

/// Lets every thread of the same or a higher priority that is ready on
/// this core run first.
pub fn yield_now() {
    give_way(State::Ready);
}

/// Lets other threads run until the timer has ticked `ticks` more times.
pub fn sleep(ticks: u64) {
    give_way(State::Sleeping(super::interrupts::ticks() + ticks));
}

/// Ends the running thread.
pub fn exit() -> ! {
    give_way(State::Ending);
    x86::raw::halt()
}

/// Called at every timer tick, from the interrupt: wakes this core's
/// sleepers whose time has come, and lets the first ready thread of the
/// same or a higher priority run in place of the running one, which goes
/// to the end of its queue. The interrupted thread goes on later from
/// inside this call, and returns from the interrupt to where it was.
pub fn tick() {
    give_way(State::Ready);
}

/// The level 4 table of the running thread's address space, or 0 for the
/// kernel's.
pub fn current_root() -> u64 {
    let threads = THREADS.lock();
    threads.root(threads.current(this_core()))
}

/// Whether thread `slot` has started and not ended. Only the test image
/// asks.
#[cfg(feature = "qemu-test")]
pub fn alive(slot: usize) -> bool {
    THREADS.lock().alive(slot)
}

/// The running thread's number.
pub fn current() -> usize {
    THREADS.lock().current(this_core())
}

/// Lets this core's table pick the next thread as [`Table::give_way`]
/// says, with the running one put into `becomes`, and switches to it.
fn give_way(becomes: State) {
    x86::raw::without_interrupts(|| {
        let core = this_core();
        let next = {
            let mut threads = THREADS.lock();
            let now = super::interrupts::ticks();
            threads
                .give_way(core, becomes, now)
                .map(|Switch { from, to }| {
                    super::interrupts::set_kernel_stack(thread::stack_top(to));
                    let kernel = super::memory::kernel_root();
                    let cr3 = x86::raw::read_cr3();
                    if let Some(root) = thread::root_to_load(cr3, threads.root(to), kernel) {
                        // SAFETY: every address space shares the kernel's half,
                        // where this code and every thread's kernel stack are.
                        unsafe { x86::raw::load_cr3(root) };
                    }
                    (threads.stack_at(from), threads.stack(to))
                })
        };
        if let Some((from, to)) = next {
            // SAFETY: interrupts are off and only this core switches the
            // threads that run on it, so nothing touches either between
            // the lock and the switch; `to` is where the next thread's
            // stack pointer was kept, on its own mapped stack, and `from`
            // stays in the static for good.
            unsafe { switch(from, to) };
            arrived();
        }
    });
}

/// Frees the thread this core just switched away from for good, now that
/// nothing runs on its stack any more.
extern "C" fn arrived() {
    THREADS.lock().arrived(this_core());
}

/// Keeps the running thread's flags and the registers a function has to
/// keep on its stack, saves its stack pointer at `from`, and takes the
/// next thread's back from its stack at `to`.
#[unsafe(naked)]
unsafe extern "C" fn switch(from: *mut u64, to: u64) {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "pushfq",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "popfq",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

/// Where a new thread's first switch returns to: the thread it replaced is
/// freed if it ended, its entry gets its argument, and the thread ends
/// when the entry returns.
#[unsafe(naked)]
unsafe extern "C" fn thread_start() -> ! {
    naked_asm!(
        "call {arrived}",
        "mov rdi, r13",
        "call r12",
        "call {ended}",
        "ud2",
        arrived = sym arrived,
        ended = sym ended,
    )
}

extern "C" fn ended() -> ! {
    exit()
}

/// The first core's idle thread: it halts until the next interrupt unless
/// a thread is ready, then lets whatever became ready run.
extern "C" fn idle(_: u64) {
    loop {
        x86::raw::disable_interrupts();
        halt_unless_ready();
        yield_now();
    }
}

/// Halts this core until the next interrupt, unless a thread is ready on
/// it. Interrupts have to be off when it is called: a thread made ready
/// between the look and the halt then wakes it, since `sti` and `hlt` go
/// together.
pub fn halt_unless_ready() {
    let ready = THREADS.lock().ready_on(this_core());
    if !ready {
        // SAFETY: every core runs on the kernel's IDT, and its APIC only
        // delivers the timer, WAKE and spurious interrupts, which have
        // handlers.
        unsafe { x86::raw::enable_interrupts_and_wait() };
    }
}
