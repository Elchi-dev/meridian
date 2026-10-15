//! Kernel threads on every core. Each has a stack of its own, a priority
//! and the core it runs on, and is switched when it gives way, sleeps or
//! ends, and at every timer tick of its core when a thread of the same or
//! a higher priority is ready there; the tick also wakes the core's
//! sleepers whose time has come. Each core has an idle thread that runs
//! when nothing else is ready there: on the first core one of its own, on
//! the others the loop that also runs their jobs.

use core::arch::naked_asm;

use kernel::thread::{self, STACK_PAGES, THREADS};
use sched::{LEVELS, RunQueue};

use super::interrupts::{CORES, this_core};
use super::memory::Spin;

/// The priority threads start at unless they ask for another.
pub const NORMAL: u8 = 8;

/// No thread, where a core has none.
const NOBODY: usize = usize::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Priorities go from 0 to 31.
    NoSuchPriority(u8),
    /// The core does not exist or runs no threads.
    NoSuchCore(usize),
    /// Every thread there can be exists.
    Full,
    /// No memory was left for the thread's stack.
    NoStack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Free,
    Ready,
    Running,
    /// Waits until the timer has ticked this many times since it started.
    Sleeping(u64),
    /// Has ended, but its core is still switching away from its stack.
    Ending,
    Ended,
}

#[derive(Clone, Copy)]
struct Thread {
    state: State,
    priority: u8,
    core: usize,
    /// Where its stack pointer was when it last gave way.
    stack: u64,
    /// Whether its stack is mapped, which it stays once it is.
    mapped: bool,
}

struct Core {
    queue: RunQueue<THREADS>,
    current: usize,
    idle: usize,
    /// A thread that ended and is switched away from, to be freed once
    /// the switch is over.
    retiring: usize,
}

struct Threads {
    all: [Thread; THREADS],
    cores: [Core; CORES],
}

/// Thread 0 is the one that started the kernel, running on the first core
/// from the start.
static THREADS_: Spin<Threads> = Spin::new(Threads {
    all: {
        let free = Thread {
            state: State::Free,
            priority: 0,
            core: 0,
            stack: 0,
            mapped: false,
        };
        let mut all = [free; THREADS];
        all[0] = Thread {
            state: State::Running,
            priority: NORMAL,
            core: 0,
            stack: 0,
            mapped: true,
        };
        all
    },
    cores: {
        let mut cores = [const {
            Core {
                queue: RunQueue::new(),
                current: NOBODY,
                idle: NOBODY,
                retiring: NOBODY,
            }
        }; CORES];
        cores[0].current = 0;
        cores
    },
});

/// Gives the first core its idle thread, once.
pub fn start() {
    x86::raw::without_interrupts(|| {
        let mut threads = THREADS_.lock();
        if threads.cores[0].idle != NOBODY {
            return;
        }
        // The idle thread halts with interrupts on, waiting for the next.
        if let Ok(slot) = make(&mut threads, 0, idle, 0, 0, 0x202) {
            threads.cores[0].idle = slot;
        }
    });
}

/// Makes the running code on another core that core's idle thread, so
/// threads can be started there. Returns false when no thread is left.
pub fn join(core: usize) -> bool {
    x86::raw::without_interrupts(|| {
        let mut threads = THREADS_.lock();
        let Some(slot) = free_slot(&threads) else {
            return false;
        };
        threads.all[slot].state = State::Running;
        threads.all[slot].priority = 0;
        threads.all[slot].core = core;
        let Some(c) = threads.cores.get_mut(core) else {
            threads.all[slot].state = State::Free;
            return false;
        };
        c.current = slot;
        c.idle = slot;
        true
    })
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
    if usize::from(priority) >= LEVELS {
        return Err(Error::NoSuchPriority(priority));
    }
    // The new thread starts with interrupts as its maker has them.
    let flags = if x86::raw::interrupts_enabled() {
        0x202
    } else {
        0x2
    };
    let slot = x86::raw::without_interrupts(|| {
        let mut threads = THREADS_.lock();
        if threads.cores.get(core).is_none_or(|c| c.current == NOBODY) {
            return Err(Error::NoSuchCore(core));
        }
        let slot = make(&mut threads, core, entry, argument, priority, flags)?;
        let _ = threads.cores[core].queue.push(slot, priority);
        Ok(slot)
    })?;
    if core != this_core() {
        super::smp::wake(core);
    }
    Ok(slot)
}

/// Takes a free thread, gives it a stack whose first switch lands in
/// `entry(argument)` with `flags`, and makes it ready on `core` without
/// queueing it.
fn make(
    threads: &mut Threads,
    core: usize,
    entry: extern "C" fn(u64),
    argument: u64,
    priority: u8,
    flags: u64,
) -> Result<usize, Error> {
    let slot = free_slot(threads).ok_or(Error::Full)?;
    let top = thread::stack_top(slot);
    if !threads.all[slot].mapped {
        super::memory::map_stack(top, STACK_PAGES).map_err(|_| Error::NoStack)?;
        threads.all[slot].mapped = true;
    }
    let at = thread::first_frame_at(top);
    let start = thread_start as *const () as usize as u64;
    let frame = thread::first_frame(flags, entry as usize as u64, argument, start);
    // SAFETY: the slot's stack is mapped and no thread runs on it: it is
    // free, or its thread ended and its core switched away for good.
    unsafe { (at as *mut [u64; 8]).write(frame) };
    let t = &mut threads.all[slot];
    t.state = State::Ready;
    t.priority = priority;
    t.core = core;
    t.stack = at;
    Ok(slot)
}

fn free_slot(threads: &Threads) -> Option<usize> {
    (1..THREADS).find(|&s| matches!(threads.all[s].state, State::Free | State::Ended))
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

/// The running thread's number. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn current() -> usize {
    THREADS_.lock().cores[this_core()].current
}

/// Puts the running thread into `becomes`, wakes this core's sleepers
/// whose time has come, and switches to the first thread of the highest
/// priority ready on this core, or to its idle thread when there is none
/// and the running one cannot go on.
fn give_way(becomes: State) {
    x86::raw::without_interrupts(|| {
        let core = this_core();
        let next = {
            let mut threads = THREADS_.lock();
            let current = threads.cores[core].current;
            let idle = threads.cores[core].idle;
            threads.all[current].state = becomes;
            if becomes == State::Ready && current != idle {
                let priority = threads.all[current].priority;
                let _ = threads.cores[core].queue.push(current, priority);
            }
            // The running thread is among them, so a sleep that is over
            // already only gives way.
            let now = super::interrupts::ticks();
            for slot in 0..THREADS {
                if threads.all[slot].core == core
                    && let State::Sleeping(until) = threads.all[slot].state
                    && until <= now
                {
                    threads.all[slot].state = State::Ready;
                    let priority = threads.all[slot].priority;
                    let _ = threads.cores[core].queue.push(slot, priority);
                }
            }
            let next = match threads.cores[core].queue.pop() {
                Some((next, _)) => next,
                None if threads.all[current].state == State::Ready || idle == NOBODY => current,
                None => idle,
            };
            threads.all[next].state = State::Running;
            if next == current {
                None
            } else {
                threads.cores[core].current = next;
                if becomes == State::Ending {
                    threads.cores[core].retiring = current;
                }
                let from = &raw mut threads.all[current].stack;
                Some((from, threads.all[next].stack))
            }
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
    let mut threads = THREADS_.lock();
    let core = this_core();
    let retiring = core::mem::replace(&mut threads.cores[core].retiring, NOBODY);
    if let Some(t) = threads.all.get_mut(retiring) {
        t.state = State::Ended;
    }
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

/// The first core's idle thread: it halts until the next interrupt, then
/// lets whatever became ready run.
extern "C" fn idle(_: u64) {
    loop {
        x86::raw::wait_for_interrupt();
        yield_now();
    }
}
