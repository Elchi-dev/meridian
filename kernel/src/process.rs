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
