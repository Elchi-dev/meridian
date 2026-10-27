//! Processes as kernel objects, the parts that need no processor: whether
//! each still runs or how it ended, and how many handles stand for it. A
//! process's slot is free again once it ended and the last handle to it
//! was closed, so the one who started it can ask how it ended for as long
//! as it holds the handle.

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It exited with this code.
    Exited(u64),
    /// The exception with this vector ended it.
    Faulted(u64),
}

/// Where a process is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Ended(Outcome),
}

/// A program's threads, for the one that leaves last: how many run, and
/// how the program ends once one of them ended it for all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Threads {
    running: usize,
    ending: Option<Outcome>,
}

impl Default for Threads {
    fn default() -> Self {
        Self::new()
    }
}

impl Threads {
    /// A program's first thread, which runs.
    #[must_use]
    pub const fn new() -> Threads {
        Threads {
            running: 1,
            ending: None,
        }
    }

    /// Another thread is to start, and says whether it may: not once the
    /// program is ending.
    pub fn start(&mut self) -> bool {
        if self.ending.is_some() {
            return false;
        }
        self.running += 1;
        true
    }

    /// A thread that was to start could not.
    pub fn not_started(&mut self) {
        self.running = self.running.saturating_sub(1).max(1);
    }

    /// The whole program is to end as `outcome`, unless it is ending
    /// already: its threads leave one by one.
    pub fn end(&mut self, outcome: Outcome) {
        self.ending.get_or_insert(outcome);
    }

    /// How the program ends, once it is ending.
    #[must_use]
    pub fn ending(&self) -> Option<Outcome> {
        self.ending
    }

    /// A thread leaves, having ended with `code` when it ended on its own,
    /// and answers how the program ended when it was the last: as it was
    /// ended for all, or else with that last thread's code. Once none
    /// runs, the program ended already, and nothing more is answered.
    pub fn leave(&mut self, code: u64) -> Option<Outcome> {
        let was = self.running;
        self.running = was.saturating_sub(1);
        (was == 1).then(|| self.ending.unwrap_or(Outcome::Exited(code)))
    }

    /// How many of its threads run.
    #[must_use]
    pub fn running(&self) -> usize {
        self.running
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    state: State,
    handles: u32,
    /// The exit code it was asked to end with.
    asked: Option<u64>,
}

/// Up to `N` processes that handles can stand for.
pub struct Table<const N: usize> {
    entries: [Option<Entry>; N],
}

impl<const N: usize> Default for Table<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Table<N> {
    #[must_use]
    pub const fn new() -> Self {
        Table { entries: [None; N] }
    }

    /// A slot for a process about to start, which one handle stands for:
    /// that of whoever starts it. None when every slot is taken.
    pub fn create(&mut self) -> Option<usize> {
        let slot = self.entries.iter().position(Option::is_none)?;
        self.entries[slot] = Some(Entry {
            state: State::Running,
            handles: 1,
            asked: None,
        });
        Some(slot)
    }

    /// Where process `slot` is, or none when there is no such process.
    #[must_use]
    pub fn state(&self, slot: usize) -> Option<State> {
        self.entries.get(slot)?.map(|e| e.state)
    }

    /// Process `slot` ended as `outcome`. Its slot goes once no handle
    /// stands for it.
    pub fn end(&mut self, slot: usize, outcome: Outcome) {
        if let Some(entry) = self.entries.get_mut(slot) {
            if let Some(e) = entry.as_mut().filter(|e| e.state == State::Running) {
                e.state = State::Ended(outcome);
            }
            self.free_if_done(slot);
        }
    }

    /// Asks process `slot` to end with `code`, unless it ended or was asked
    /// before.
    pub fn ask_to_end(&mut self, slot: usize, code: u64) {
        if let Some(e) = self.running(slot) {
            e.asked.get_or_insert(code);
        }
    }

    /// The exit code process `slot` was asked to end with, while it runs.
    #[must_use]
    pub fn asked(&self, slot: usize) -> Option<u64> {
        self.entries
            .get(slot)?
            .filter(|e| e.state == State::Running)?
            .asked
    }

    fn running(&mut self, slot: usize) -> Option<&mut Entry> {
        self.entries
            .get_mut(slot)?
            .as_mut()
            .filter(|e| e.state == State::Running)
    }

    /// One more handle stands for process `slot`. False when there is no
    /// such process.
    pub fn hold(&mut self, slot: usize) -> bool {
        let Some(e) = self.entries.get_mut(slot).and_then(Option::as_mut) else {
            return false;
        };
        e.handles = e.handles.saturating_add(1);
        true
    }

    /// A handle that stood for process `slot` was closed. Its slot goes
    /// once it ended and no handle stands for it.
    pub fn close(&mut self, slot: usize) {
        if let Some(e) = self.entries.get_mut(slot).and_then(Option::as_mut) {
            e.handles = e.handles.saturating_sub(1);
        }
        self.free_if_done(slot);
    }

    fn free_if_done(&mut self, slot: usize) {
        if let Some(entry) = self.entries.get_mut(slot)
            && entry.is_some_and(|e| e.handles == 0 && e.state != State::Running)
        {
            *entry = None;
        }
    }

    /// How many processes there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_runs_until_it_ends() {
        let mut t = Table::<4>::new();
        assert!(t.is_empty());
        let slot = t.create().unwrap();
        assert_eq!(t.state(slot), Some(State::Running));
        assert_eq!((t.len(), t.is_empty()), (1, false));
        t.end(slot, Outcome::Exited(42));
        assert_eq!(t.state(slot), Some(State::Ended(Outcome::Exited(42))));
        t.end(slot, Outcome::Faulted(14));
        assert_eq!(
            t.state(slot),
            Some(State::Ended(Outcome::Exited(42))),
            "a process ends once"
        );
    }

    #[test]
    fn a_process_that_ended_stays_while_a_handle_stands_for_it() {
        let mut t = Table::<4>::new();
        let slot = t.create().unwrap();
        t.end(slot, Outcome::Faulted(13));
        assert_eq!(t.len(), 1, "its starter can still ask");
        t.close(slot);
        assert_eq!(
            t.state(slot),
            None,
            "and once it closed the handle, it is gone"
        );
        assert!(t.is_empty());
    }

    #[test]
    fn a_process_that_ended_stays_until_its_last_handle_closed() {
        let mut t = Table::<4>::new();
        let slot = t.create().unwrap();
        assert!(t.hold(slot) && t.hold(slot));
        t.end(slot, Outcome::Exited(3));
        t.close(slot);
        t.close(slot);
        assert_eq!(
            t.state(slot),
            Some(State::Ended(Outcome::Exited(3))),
            "one handle is left"
        );
        t.close(slot);
        assert_eq!(t.state(slot), None);
        assert!(!t.hold(slot), "nothing to hold once it went");
        assert!(!t.hold(9), "nor where no slot is");
        assert!(t.is_empty());
    }

    #[test]
    fn a_process_whose_handle_was_closed_stays_while_it_runs() {
        let mut t = Table::<4>::new();
        let slot = t.create().unwrap();
        t.close(slot);
        assert_eq!(t.state(slot), Some(State::Running));
        t.close(slot);
        assert_eq!(
            t.state(slot),
            Some(State::Running),
            "closing too often changes nothing"
        );
        t.end(slot, Outcome::Exited(0));
        assert_eq!(t.state(slot), None, "ending is all that was left");
    }

    #[test]
    fn slots_are_taken_once_and_given_again() {
        let mut t = Table::<2>::new();
        let (a, b) = (t.create().unwrap(), t.create().unwrap());
        assert_ne!(a, b);
        assert_eq!(t.create(), None, "every slot taken");
        t.end(a, Outcome::Exited(1));
        t.close(a);
        assert_eq!(t.create(), Some(a));
        assert_eq!(
            t.state(b),
            Some(State::Running),
            "the other one is untouched"
        );
    }

    #[test]
    fn a_process_asked_to_end_keeps_the_first_code_it_was_given() {
        let mut t = Table::<2>::new();
        let slot = t.create().unwrap();
        assert_eq!(t.asked(slot), None);
        t.ask_to_end(slot, 77);
        t.ask_to_end(slot, 78);
        assert_eq!(t.asked(slot), Some(77));
        assert_eq!(t.state(slot), Some(State::Running), "until it does");
        t.end(slot, Outcome::Exited(77));
        assert_eq!(t.asked(slot), None, "one that ended runs no more");
        let other = t.create().unwrap();
        assert_eq!(t.asked(other), None, "the other one was not asked");
        t.end(other, Outcome::Exited(1));
        t.ask_to_end(other, 2);
        assert_eq!(
            t.state(other),
            Some(State::Ended(Outcome::Exited(1))),
            "one that ended stays as it ended"
        );
        t.ask_to_end(9, 1);
        assert_eq!(t.asked(9), None);
    }

    #[test]
    fn a_program_ends_with_its_last_thread() {
        let mut t = Threads::new();
        assert!(t.start() && t.start());
        assert_eq!(t.running(), 3);
        assert_eq!(t.leave(1), None);
        assert_eq!(t.leave(2), None);
        assert_eq!(
            t.leave(3),
            Some(Outcome::Exited(3)),
            "with the last one's code"
        );
        assert_eq!(t.running(), 0);
        assert_eq!(Threads::default().leave(0), Some(Outcome::Exited(0)));
    }

    #[test]
    fn a_program_ended_for_all_ends_as_it_was_ended() {
        let mut t = Threads::new();
        assert!(t.start());
        t.end(Outcome::Faulted(14));
        t.end(Outcome::Exited(9));
        assert_eq!(
            t.ending(),
            Some(Outcome::Faulted(14)),
            "the first end counts"
        );
        assert!(!t.start(), "no thread starts in a program that ends");
        assert_eq!(t.running(), 2);
        assert_eq!(t.leave(5), None);
        assert_eq!(t.leave(6), Some(Outcome::Faulted(14)));
        assert_eq!(Threads::new().ending(), None);
    }

    #[test]
    fn a_program_ends_once() {
        let mut t = Threads::new();
        assert_eq!(t.leave(2), Some(Outcome::Exited(2)));
        assert_eq!(t.leave(3), None, "no thread was left to leave");
        assert_eq!(t.running(), 0);
        assert!(t.start(), "nothing ended it for all");
        assert_eq!(t.leave(4), Some(Outcome::Exited(4)));
    }

    #[test]
    fn a_process_in_a_slot_given_again_was_asked_nothing() {
        let mut t = Table::<1>::new();
        let slot = t.create().unwrap();
        t.ask_to_end(slot, 9);
        t.end(slot, Outcome::Exited(9));
        t.close(slot);
        assert_eq!(t.create(), Some(slot), "the slot is given again");
        assert_eq!(t.asked(slot), None, "and the new process was asked nothing");
        assert_eq!(t.state(slot), Some(State::Running));
        t.ask_to_end(slot, 10);
        assert_eq!(t.asked(slot), Some(10));
        t.end(slot, Outcome::Exited(10));
        t.close(slot);
        assert!(t.is_empty(), "with one handle, as any new process has");
    }

    #[test]
    fn a_thread_that_could_not_start_is_not_counted() {
        let mut t = Threads::new();
        assert!(t.start());
        t.not_started();
        assert_eq!(t.running(), 1);
        t.not_started();
        assert_eq!(t.running(), 1, "the one that asked still runs");
        assert_eq!(t.leave(4), Some(Outcome::Exited(4)));
    }

    #[test]
    fn slots_that_do_not_exist_hold_nothing() {
        let mut t = Table::<2>::new();
        assert_eq!(t.state(0), None);
        assert_eq!(t.state(7), None);
        t.end(7, Outcome::Exited(0));
        t.close(7);
        t.close(1);
        assert!(t.is_empty());
        assert_eq!(Table::<3>::default().len(), 0);
    }
}
