//! Kernel threads on the first core: each with a stack of its own and a
//! priority, switched when one gives way, sleeps or ends, and at every
//! timer tick, when a thread of the same or a higher priority is ready. The
//! tick also wakes the sleepers whose time has come. An idle thread at the
//! lowest priority halts the processor when nothing else is ready.

use core::arch::naked_asm;

use kernel::thread::{self, STACK_PAGES, THREADS};
use sched::{LEVELS, RunQueue};

use super::memory::Spin;

/// The priority threads start at unless they ask for another.
pub const NORMAL: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Priorities go from 0 to 31.
    NoSuchPriority(u8),
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
    Ended,
}

#[derive(Clone, Copy)]
struct Thread {
    state: State,
    priority: u8,
    /// Where its stack pointer was when it last gave way.
    stack: u64,
    /// Whether its stack is mapped, which it stays once it is.
    mapped: bool,
}

struct Threads {
    queue: RunQueue<THREADS>,
    all: [Thread; THREADS],
    current: usize,
    started: bool,
}

/// Thread 0 is the one that started the kernel, running from the start.
static THREADS_: Spin<Threads> = Spin::new(Threads {
    queue: RunQueue::new(),
    all: {
        let free = Thread {
            state: State::Free,
            priority: 0,
            stack: 0,
            mapped: false,
        };
        let mut all = [free; THREADS];
        all[0] = Thread {
            state: State::Running,
            priority: NORMAL,
            stack: 0,
            mapped: true,
        };
        all
    },
    current: 0,
    started: false,
});

/// Starts the idle thread, once. Until then, nothing but the first thread
/// and those it starts can run, and sleeping only gives way.
pub fn start() {
    let first = !core::mem::replace(&mut THREADS_.lock().started, true);
    if first {
        let _ = spawn(idle, 0, 0);
    }
}

/// Starts `entry(argument)` as a thread of its own at `priority`, ready to
/// run once the running thread gives way, and returns its number. It ends
/// when the entry returns.
///
/// # Errors
///
/// When the priority is 32 or more, every thread there can be exists, or
/// no memory is left for its stack.
pub fn spawn(entry: extern "C" fn(u64), argument: u64, priority: u8) -> Result<usize, Error> {
    if usize::from(priority) >= LEVELS {
        return Err(Error::NoSuchPriority(priority));
    }
    // The new thread starts with interrupts as its maker has them.
    let flags = if x86::raw::interrupts_enabled() {
        0x202
    } else {
        0x2
    };
    x86::raw::without_interrupts(|| {
        let mut threads = THREADS_.lock();
        let slot = (1..THREADS)
            .find(|&s| matches!(threads.all[s].state, State::Free | State::Ended))
            .ok_or(Error::Full)?;
        let top = thread::stack_top(slot);
        if !threads.all[slot].mapped {
            super::memory::map_stack(top, STACK_PAGES).map_err(|_| Error::NoStack)?;
            threads.all[slot].mapped = true;
        }
        let at = thread::first_frame_at(top);
        let start = thread_start as *const () as usize as u64;
        let frame = thread::first_frame(flags, entry as usize as u64, argument, start);
        // SAFETY: the slot's stack is mapped and no thread runs on it: it
        // is free, or its thread ended and switched away for good.
        unsafe { (at as *mut [u64; 8]).write(frame) };
        threads.all[slot].state = State::Ready;
        threads.all[slot].priority = priority;
        threads.all[slot].stack = at;
        let _ = threads.queue.push(slot, priority);
        Ok(slot)
    })
}

/// Lets every thread of the same or a higher priority that is ready run
/// first.
pub fn yield_now() {
    give_way(State::Ready);
}

/// Lets other threads run until the timer has ticked `ticks` more times.
pub fn sleep(ticks: u64) {
    give_way(State::Sleeping(super::interrupts::ticks() + ticks));
}

/// Ends the running thread.
pub fn exit() -> ! {
    give_way(State::Ended);
    x86::raw::halt()
}

/// Called at every timer tick, from the interrupt: wakes the sleepers whose
/// time has come, and lets the first ready thread of the same or a higher
/// priority run in place of the running one, which goes to the end of its
/// queue. The interrupted thread goes on later from inside this call, and
/// returns from the interrupt to where it was.
pub fn tick() {
    give_way(State::Ready);
}

/// The running thread's number. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn current() -> usize {
    THREADS_.lock().current
}

/// Puts the running thread into `becomes`, wakes the sleepers whose time
/// has come, and switches to the first thread of the highest priority that
/// is ready, unless that is the running one.
fn give_way(becomes: State) {
    x86::raw::without_interrupts(|| {
        let next = {
            let mut threads = THREADS_.lock();
            let current = threads.current;
            threads.all[current].state = becomes;
            if becomes == State::Ready {
                let priority = threads.all[current].priority;
                let _ = threads.queue.push(current, priority);
            }
            // The running thread is among them, so a sleep that is over
            // already only gives way.
            let now = super::interrupts::ticks();
            for slot in 0..THREADS {
                if let State::Sleeping(until) = threads.all[slot].state
                    && until <= now
                {
                    threads.all[slot].state = State::Ready;
                    let priority = threads.all[slot].priority;
                    let _ = threads.queue.push(slot, priority);
                }
            }
            match threads.queue.pop() {
                Some((next, _)) if next != current => {
                    threads.all[next].state = State::Running;
                    threads.current = next;
                    let from = &raw mut threads.all[current].stack;
                    Some((from, threads.all[next].stack))
                }
                Some(_) => {
                    threads.all[current].state = State::Running;
                    None
                }
                None => {
                    // Nothing else can run yet, so the running thread goes
                    // on, whatever it asked for.
                    threads.all[current].state = State::Running;
                    None
                }
            }
        };
        if let Some((from, to)) = next {
            // SAFETY: interrupts are off and only this core runs threads,
            // so nothing touches the threads between the lock and the
            // switch; `to` is where the next thread's stack pointer was
            // kept, on its own mapped stack, and `from` stays in the
            // static for good.
            unsafe { switch(from, to) };
        }
    });
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

/// Where a new thread's first switch returns to: its entry gets its
/// argument, and the thread ends when the entry returns.
#[unsafe(naked)]
unsafe extern "C" fn thread_start() -> ! {
    naked_asm!(
        "mov rdi, r13",
        "call r12",
        "call {ended}",
        "ud2",
        ended = sym ended,
    )
}

extern "C" fn ended() -> ! {
    exit()
}

/// The thread that runs when nothing else is ready: it halts until the
/// next interrupt, then lets the sleepers whose time has come run.
extern "C" fn idle(_: u64) {
    loop {
        x86::raw::wait_for_interrupt();
        yield_now();
    }
}
