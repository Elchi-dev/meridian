//! Kernel threads, the parts that need no processor: where each thread's
//! stack lies, what a new thread's stack holds before it first runs, and
//! which thread each core runs next.

use sched::{LEVELS, RunQueue};

/// How many threads there can be at once, the first one and every core's
/// idle thread included.
pub const THREADS: usize = 256;

/// Where the threads' stacks lie. Each gets 64 KiB of address space, of
/// which only the top 32 KiB are mapped, so a stack that runs over faults
/// instead of writing into the next one.
pub const STACKS: u64 = 0xffff_fc80_0000_0000;

/// The pages of each thread's stack.
pub const STACK_PAGES: u64 = 8;

/// The top of the stack of thread `slot`.
#[must_use]
pub fn stack_top(slot: usize) -> u64 {
    STACKS + (slot as u64 + 1) * 2 * STACK_PAGES * 4096
}

/// The words a new thread's stack starts with, lowest first, in the order
/// a switch takes them back: its flags, the six registers a switch keeps,
/// with `entry` in r12 and `argument` in r13, and `start`, where the
/// switch returns to.
#[must_use]
pub fn first_frame(flags: u64, entry: u64, argument: u64, start: u64) -> [u64; 8] {
    [flags, 0, 0, argument, entry, 0, 0, start]
}

/// Where that frame goes below the stack's top `top`: low enough that the
/// stack is aligned to 16 bytes again when `start` calls the entry, as the
/// calling convention wants.
#[must_use]
pub fn first_frame_at(top: u64) -> u64 {
    top - 80
}

/// The priority threads start at unless they ask for another.
pub const NORMAL: u8 = 8;

/// No thread, where a core has none.
pub const NOBODY: usize = usize::MAX;

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
pub enum State {
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
    /// The level 4 table of its address space, or 0 for the kernel's.
    root: u64,
}

struct Core {
    queue: RunQueue<THREADS>,
    current: usize,
    idle: usize,
    /// A thread that ended and is switched away from, to be freed once
    /// the switch is over.
    retiring: usize,
}

/// A core going from one thread to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switch {
    pub from: usize,
    pub to: usize,
}

/// Every thread, and what each of `CORES` cores runs and has ready.
/// Thread 0 is the one that started the kernel, running on the first core
/// from the start; the other cores run threads once they join.
pub struct Table<const CORES: usize> {
    all: [Thread; THREADS],
    cores: [Core; CORES],
}

impl<const CORES: usize> Default for Table<CORES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CORES: usize> Table<CORES> {
    #[must_use]
    pub const fn new() -> Self {
        let free = Thread {
            state: State::Free,
            priority: 0,
            core: 0,
            stack: 0,
            mapped: false,
            root: 0,
        };
        let mut all = [free; THREADS];
        all[0] = Thread {
            state: State::Running,
            priority: NORMAL,
            ..free
        };
        let mut cores = [const {
            Core {
                queue: RunQueue::new(),
                current: NOBODY,
                idle: NOBODY,
                retiring: NOBODY,
            }
        }; CORES];
        cores[0].current = 0;
        Table { all, cores }
    }

    /// A thread that is free to start, if there is one. Thread 0 never is.
    #[must_use]
    pub fn free(&self) -> Option<usize> {
        (1..THREADS).find(|&s| matches!(self.all[s].state, State::Free | State::Ended))
    }

    /// Whether thread `slot` has its stack mapped already.
    #[must_use]
    pub fn has_stack(&self, slot: usize) -> bool {
        self.all.get(slot).is_some_and(|t| t.mapped)
    }

    /// Makes the code running on `core` that core's idle thread, so
    /// threads can be started there, and returns its number.
    ///
    /// # Errors
    ///
    /// When the core does not exist or every thread there can be exists.
    pub fn join(&mut self, core: usize) -> Result<usize, Error> {
        if core >= CORES {
            return Err(Error::NoSuchCore(core));
        }
        let slot = self.free().ok_or(Error::Full)?;
        self.all[slot] = Thread {
            state: State::Running,
            priority: 0,
            core,
            root: 0,
            ..self.all[slot]
        };
        self.cores[core].current = slot;
        self.cores[core].idle = slot;
        Ok(slot)
    }

    /// Whether a thread can start on `core` at `priority`.
    ///
    /// # Errors
    ///
    /// When the priority is 32 or more, or the core runs no threads.
    pub fn can_start(&self, core: usize, priority: u8) -> Result<(), Error> {
        if usize::from(priority) >= LEVELS {
            return Err(Error::NoSuchPriority(priority));
        }
        if self.cores.get(core).is_none_or(|c| c.current == NOBODY) {
            return Err(Error::NoSuchCore(core));
        }
        Ok(())
    }

    /// Makes free thread `slot` ready on `core` at `priority`, in the
    /// address space whose level 4 table is at `root`, or the kernel's for
    /// 0. Its stack is mapped now for good, and its stack pointer is at
    /// `stack`. It waits in no queue yet.
    pub fn prepare(&mut self, slot: usize, core: usize, priority: u8, root: u64, stack: u64) {
        self.all[slot] = Thread {
            state: State::Ready,
            priority,
            core,
            stack,
            mapped: true,
            root,
        };
    }

    /// Puts ready thread `slot` at the end of its priority's queue on its
    /// core.
    pub fn queue(&mut self, slot: usize) {
        let Thread { priority, core, .. } = self.all[slot];
        let _ = self.cores[core].queue.push(slot, priority);
    }

    /// Whether `core` has its idle thread.
    #[must_use]
    pub fn has_idle(&self, core: usize) -> bool {
        self.cores.get(core).is_some_and(|c| c.idle != NOBODY)
    }

    /// Makes thread `slot` the idle thread of its core, the one that runs
    /// when nothing else is ready there.
    pub fn make_idle(&mut self, slot: usize) {
        let core = self.all[slot].core;
        self.cores[core].idle = slot;
    }

    /// Puts the thread running on `core` into `becomes`, wakes the core's
    /// sleepers whose time has come by `now`, and picks the next: the
    /// first thread of the highest priority ready there, or when there is
    /// none, the running one if it can go on, or else the idle thread.
    /// Answers the switch when another thread runs next. An ending thread
    /// is freed once the core arrived in the next one.
    pub fn give_way(&mut self, core: usize, becomes: State, now: u64) -> Option<Switch> {
        let current = self.cores[core].current;
        let idle = self.cores[core].idle;
        self.all[current].state = becomes;
        if becomes == State::Ready && current != idle {
            self.queue(current);
        }
        // The running thread is among them, so a sleep that is over
        // already only gives way.
        for slot in 0..THREADS {
            if self.all[slot].core == core
                && let State::Sleeping(until) = self.all[slot].state
                && until <= now
            {
                self.all[slot].state = State::Ready;
                self.queue(slot);
            }
        }
        let next = match self.cores[core].queue.pop() {
            Some((next, _)) => next,
            None if self.all[current].state == State::Ready || idle == NOBODY => current,
            None => idle,
        };
        self.all[next].state = State::Running;
        if next == current {
            return None;
        }
        self.cores[core].current = next;
        if becomes == State::Ending {
            self.cores[core].retiring = current;
        }
        Some(Switch {
            from: current,
            to: next,
        })
    }

    /// Frees the thread `core` just switched away from, if it ended, now
    /// that nothing runs on its stack any more.
    pub fn arrived(&mut self, core: usize) {
        let retiring = core::mem::replace(&mut self.cores[core].retiring, NOBODY);
        if let Some(t) = self.all.get_mut(retiring) {
            t.state = State::Ended;
        }
    }

    /// Whether a thread waits to run on `core`.
    #[must_use]
    pub fn ready_on(&self, core: usize) -> bool {
        !self.cores[core].queue.is_empty()
    }

    /// The thread running on `core`.
    #[must_use]
    pub fn current(&self, core: usize) -> usize {
        self.cores[core].current
    }

    /// The level 4 table of thread `slot`'s address space, or 0 for the
    /// kernel's.
    #[must_use]
    pub fn root(&self, slot: usize) -> u64 {
        self.all[slot].root
    }

    /// Where thread `slot`'s stack pointer is kept while it does not run.
    #[must_use]
    pub fn stack(&self, slot: usize) -> u64 {
        self.all[slot].stack
    }

    /// The same, for a switch to write when the thread gives way.
    pub fn stack_at(&mut self, slot: usize) -> *mut u64 {
        &raw mut self.all[slot].stack
    }

    /// Whether thread `slot` has started and not ended.
    #[must_use]
    pub fn alive(&self, slot: usize) -> bool {
        self.all
            .get(slot)
            .is_some_and(|t| matches!(t.state, State::Ready | State::Running | State::Sleeping(_)))
    }
}

/// The level 4 table to load for a thread whose address space is at
/// `root`, or the kernel's at `kernel` for 0, when cr3 holds `cr3`: none
/// when that table is loaded already, since loading it again only throws
/// away the cached translations. The low 12 bits of cr3 are flags.
#[must_use]
pub fn root_to_load(cr3: u64, root: u64, kernel: u64) -> Option<u64> {
    let root = if root == 0 { kernel } else { root };
    (cr3 & !0xfff != root).then_some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Two = Table<2>;

    /// A table whose second core joined, with its idle thread.
    fn two() -> Box<Two> {
        let mut t = Box::new(Two::new());
        let idle = t.free().unwrap();
        t.prepare(idle, 0, 0, 0, 0x100);
        t.make_idle(idle);
        assert_eq!(t.join(1), Ok(2));
        t
    }

    /// Starts a thread on `core` at `priority`, the way the kernel does.
    fn spawn(t: &mut Two, core: usize, priority: u8) -> usize {
        t.can_start(core, priority).unwrap();
        let slot = t.free().unwrap();
        t.prepare(slot, core, priority, 0, 0x1000 * slot as u64);
        t.queue(slot);
        slot
    }

    #[test]
    fn the_kernel_starts_as_thread_0_on_the_first_core() {
        let t = Box::new(Two::new());
        assert_eq!(t.current(0), 0);
        assert!(t.alive(0));
        assert_eq!(t.root(0), 0, "in the kernel's space");
        assert_eq!(t.free(), Some(1), "never thread 0");
        assert!(!t.has_idle(0) && !t.has_idle(1));
        assert_eq!(t.can_start(0, NORMAL), Ok(()));
        assert_eq!(t.can_start(1, NORMAL), Err(Error::NoSuchCore(1)));
        assert!(!t.ready_on(0));
    }

    #[test]
    fn a_core_that_joins_idles_in_a_thread_of_its_own() {
        let mut t = two();
        assert_eq!(t.current(1), 2);
        assert!(t.has_idle(1) && t.has_idle(0));
        assert!(t.alive(2));
        assert_eq!(t.can_start(1, NORMAL), Ok(()));
        let slot = spawn(&mut t, 1, NORMAL);
        assert!(t.ready_on(1) && !t.ready_on(0));
        assert_eq!(
            t.give_way(1, State::Ready, 0),
            Some(Switch { from: 2, to: slot })
        );
        assert_eq!(
            t.give_way(1, State::Ending, 0),
            Some(Switch { from: slot, to: 2 })
        );
        assert_eq!(t.current(1), 2, "back in its idle thread");
    }

    #[test]
    fn no_core_joins_that_does_not_exist_or_finds_no_thread_left() {
        let mut t = two();
        assert_eq!(t.join(2), Err(Error::NoSuchCore(2)));
        assert_eq!(t.join(usize::MAX), Err(Error::NoSuchCore(usize::MAX)));
        assert_eq!(t.free(), Some(3), "it took none");
        while let Some(slot) = t.free() {
            t.prepare(slot, 0, NORMAL, 0, 0);
        }
        let mut late = Box::new(Two::new());
        while let Some(slot) = late.free() {
            late.prepare(slot, 0, NORMAL, 0, 0);
        }
        assert_eq!(late.join(1), Err(Error::Full));
        assert_eq!(late.can_start(1, NORMAL), Err(Error::NoSuchCore(1)));
        assert_eq!(late.current(1), NOBODY);
    }

    #[test]
    fn threads_start_only_at_a_priority_there_is() {
        let t = two();
        assert_eq!(t.can_start(0, 31), Ok(()));
        assert_eq!(t.can_start(0, 0), Ok(()));
        assert_eq!(t.can_start(0, 32), Err(Error::NoSuchPriority(32)));
        assert_eq!(t.can_start(1, 255), Err(Error::NoSuchPriority(255)));
        assert_eq!(t.can_start(2, NORMAL), Err(Error::NoSuchCore(2)));
    }

    #[test]
    fn a_thread_alone_goes_on_when_it_gives_way() {
        let mut t = two();
        assert_eq!(
            t.give_way(0, State::Ready, 0),
            None,
            "not to the idle thread"
        );
        assert_eq!(t.current(0), 0);
        assert!(t.alive(0));
        assert!(!t.ready_on(0), "and it waits in no queue");
    }

    #[test]
    fn threads_of_one_priority_take_turns() {
        let mut t = two();
        let a = spawn(&mut t, 0, NORMAL);
        let b = spawn(&mut t, 0, NORMAL);
        let mut order = Vec::new();
        for _ in 0..6 {
            if let Some(s) = t.give_way(0, State::Ready, 0) {
                order.push(s.to);
            }
        }
        assert_eq!(order, [a, b, 0, a, b, 0]);
    }

    #[test]
    fn the_highest_priority_ready_goes_first_and_lower_ones_wait() {
        let mut t = two();
        let low = spawn(&mut t, 0, 2);
        assert_eq!(t.give_way(0, State::Ready, 0), None, "2 is lower than 8");
        let high = spawn(&mut t, 0, 20);
        assert_eq!(
            t.give_way(0, State::Ready, 0),
            Some(Switch { from: 0, to: high })
        );
        assert_eq!(t.give_way(0, State::Ready, 0), None, "it stays the highest");
        assert_eq!(
            t.give_way(0, State::Ending, 0),
            Some(Switch { from: high, to: 0 })
        );
        assert_eq!(
            t.give_way(0, State::Sleeping(5), 0),
            Some(Switch { from: 0, to: low })
        );
    }

    #[test]
    fn sleepers_wake_when_their_time_has_come() {
        let mut t = two();
        assert_eq!(
            t.give_way(0, State::Sleeping(10), 3),
            Some(Switch { from: 0, to: 1 })
        );
        assert!(t.alive(0), "sleeping is alive");
        assert_eq!(
            t.give_way(0, State::Ready, 9),
            None,
            "the idle thread goes on"
        );
        assert_eq!(t.current(0), 1);
        assert_eq!(
            t.give_way(0, State::Ready, 10),
            Some(Switch { from: 1, to: 0 })
        );
        assert!(!t.ready_on(0), "the idle thread waits in no queue");
    }

    #[test]
    fn a_sleep_that_is_over_already_only_gives_way() {
        let mut t = two();
        let other = spawn(&mut t, 0, NORMAL);
        assert_eq!(
            t.give_way(0, State::Sleeping(4), 4),
            Some(Switch { from: 0, to: other })
        );
        assert_eq!(
            t.give_way(0, State::Ready, 4),
            Some(Switch { from: other, to: 0 })
        );
        let mut alone = two();
        assert_eq!(alone.give_way(0, State::Sleeping(4), 5), None);
    }

    #[test]
    fn a_core_wakes_only_its_own_sleepers() {
        let mut t = two();
        let there = spawn(&mut t, 1, NORMAL);
        assert_eq!(
            t.give_way(1, State::Ready, 0),
            Some(Switch { from: 2, to: there })
        );
        assert_eq!(
            t.give_way(1, State::Sleeping(5), 0),
            Some(Switch { from: there, to: 2 })
        );
        assert_eq!(t.give_way(0, State::Ready, 100), None, "core 0 leaves it");
        assert_eq!(
            t.give_way(1, State::Ready, 100),
            Some(Switch { from: 2, to: there })
        );
    }

    #[test]
    fn an_ending_thread_is_freed_once_its_core_arrived_elsewhere() {
        let mut t = two();
        let a = spawn(&mut t, 0, NORMAL);
        assert_eq!(
            t.give_way(0, State::Ready, 0),
            Some(Switch { from: 0, to: a })
        );
        assert_eq!(
            t.give_way(0, State::Ending, 0),
            Some(Switch { from: a, to: 0 })
        );
        assert!(!t.alive(a));
        assert_ne!(t.free(), Some(a), "its stack is still in use");
        t.arrived(0);
        assert_eq!(t.free(), Some(a));
        assert!(t.has_stack(a), "the stack stays mapped for the next");
        t.arrived(0);
        assert_eq!(t.free(), Some(a), "arriving again frees nothing else");
        assert!(t.alive(0) && t.alive(1));
    }

    #[test]
    fn arriving_after_a_plain_switch_frees_nobody() {
        let mut t = two();
        let a = spawn(&mut t, 0, NORMAL);
        assert_eq!(
            t.give_way(0, State::Ready, 0),
            Some(Switch { from: 0, to: a })
        );
        t.arrived(0);
        assert!(t.alive(0) && t.alive(a));
    }

    #[test]
    fn an_ending_thread_with_nobody_to_run_ends_in_the_idle_thread() {
        let mut t = two();
        assert_eq!(
            t.give_way(0, State::Ending, 0),
            Some(Switch { from: 0, to: 1 })
        );
        t.arrived(0);
        assert!(!t.alive(0));
        assert_eq!(t.free(), Some(3), "thread 0 is never handed out again");
    }

    #[test]
    fn without_an_idle_thread_the_running_one_goes_on() {
        let mut t = Box::new(Two::new());
        assert_eq!(t.give_way(0, State::Sleeping(10), 0), None);
        assert_eq!(t.current(0), 0);
        assert!(t.alive(0));
    }

    #[test]
    fn threads_keep_their_space_and_stack() {
        let mut t = two();
        let slot = t.free().unwrap();
        assert!(!t.has_stack(slot));
        t.prepare(slot, 1, NORMAL, 0x7000, 0xffff_fc80_0003_ffb0);
        assert!(t.has_stack(slot));
        assert_eq!(t.root(slot), 0x7000);
        assert_eq!(t.stack(slot), 0xffff_fc80_0003_ffb0);
        let at = t.stack_at(slot).cast_const();
        assert_eq!(at, &raw const t.all[slot].stack, "where a switch keeps it");
        assert_ne!(at, &raw const t.all[slot + 1].stack);
        assert!(!t.has_stack(THREADS), "no such thread");
        assert!(!t.alive(THREADS));
    }

    #[test]
    fn cr3_is_loaded_only_for_another_space() {
        let kernel = 0x10_0000;
        let user = 0x7_3000;
        assert_eq!(root_to_load(kernel, 0, kernel), None);
        assert_eq!(root_to_load(kernel | 0x18, 0, kernel), None, "flags aside");
        assert_eq!(root_to_load(kernel, user, kernel), Some(user));
        assert_eq!(root_to_load(user | 0x8, user, kernel), None);
        assert_eq!(root_to_load(user, 0, kernel), Some(kernel));
        assert_eq!(root_to_load(user | 0xfff, kernel, kernel), Some(kernel));
    }

    #[test]
    fn each_thread_stack_has_a_guard_below_and_meets_no_other() {
        assert_eq!(stack_top(0), 0xffff_fc80_0001_0000);
        assert_eq!(stack_top(1), 0xffff_fc80_0002_0000);
        for slot in 1..THREADS {
            assert_eq!(
                stack_top(slot) - stack_top(slot - 1),
                0x1_0000,
                "32 KiB of stack, 32 KiB of guard"
            );
        }
        assert_eq!(
            STACKS >> 39 & 0x1ff,
            505,
            "a slot of the root table of its own"
        );
        assert_eq!((stack_top(THREADS - 1) - 1) >> 39 & 0x1ff, 505);
        assert!(stack_top(THREADS - 1) <= crate::APIC_PAGE);
    }

    #[test]
    fn there_are_threads_left_when_every_core_idles() {
        // Every core keeps one thread as its idle one, and the first core
        // keeps the one that started the kernel too.
        let kept = crate::CORES + 1;
        assert!(
            THREADS - kept >= 128,
            "{} threads left on the most cores",
            THREADS - kept
        );
        assert!(
            u16::try_from(THREADS).is_ok(),
            "lib/sched numbers threads in 16 bits"
        );
    }

    #[test]
    fn a_new_thread_starts_with_its_entry_and_argument_ready() {
        let frame = first_frame(0x202, 0xffff_ffff_8000_1000, 42, 0xffff_ffff_8000_2000);
        assert_eq!(
            frame,
            [
                0x202,
                0,
                0,
                42,
                0xffff_ffff_8000_1000,
                0,
                0,
                0xffff_ffff_8000_2000
            ]
        );
    }

    #[test]
    fn the_stack_is_aligned_when_the_entry_is_called() {
        for slot in [0, 1, THREADS - 1] {
            let top = stack_top(slot);
            let at = first_frame_at(top);
            // The switch takes eight words, then start pushes its return
            // address with the call: the entry has to see 8 more than a
            // multiple of 16.
            let at_call = at + 8 * 8;
            assert_eq!(at_call % 16, 0, "slot {slot}");
            assert_eq!((at_call - 8) % 16, 8);
            assert!(at_call < top, "the frame stays below the top");
            assert!(top - at <= STACK_PAGES * 4096);
        }
    }
}
