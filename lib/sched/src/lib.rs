//! Which thread runs next: 32 priority levels, each a queue in the order
//! threads arrived, and a bit for every level with a thread waiting. The
//! highest such level goes first, and the threads of one level take turns.
//! Threads are numbers below the queue's capacity, and the queues are
//! linked through arrays, so nothing is allocated.

#![cfg_attr(not(test), no_std)]

/// How many priority levels there are; 31 is the highest.
pub const LEVELS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The thread's number is not below the queue's capacity.
    NoSuchThread(usize),
    /// The level is 32 or more.
    NoSuchLevel(u8),
    /// The thread waits already.
    AlreadyWaiting(usize),
    /// The thread does not wait.
    NotWaiting(usize),
}

/// The end of a queue, and a thread that does not wait.
const NONE: u16 = u16::MAX;

/// The threads waiting to run, for up to `N` of them.
pub struct RunQueue<const N: usize> {
    /// For each thread, the one after it in its level's queue.
    next: [u16; N],
    /// For each thread, the level it waits on, or [`NONE`].
    level: [u16; N],
    first: [u16; LEVELS],
    last: [u16; LEVELS],
    /// Bit n is set while level n has a thread waiting.
    waiting: u32,
    len: usize,
}

impl<const N: usize> Default for RunQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RunQueue<N> {
    /// A queue with nobody waiting.
    #[must_use]
    pub const fn new() -> Self {
        RunQueue {
            next: [NONE; N],
            level: [NONE; N],
            first: [NONE; LEVELS],
            last: [NONE; LEVELS],
            waiting: 0,
            len: 0,
        }
    }

    /// How many threads wait.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nobody waits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The highest level a thread waits on.
    #[must_use]
    pub fn highest(&self) -> Option<u8> {
        let top = u32::BITS.checked_sub(self.waiting.leading_zeros() + 1)?;
        u8::try_from(top).ok()
    }

    /// The level `thread` waits on, if it waits.
    #[must_use]
    pub fn level_of(&self, thread: usize) -> Option<u8> {
        self.level.get(thread).and_then(|&l| u8::try_from(l).ok())
    }

    /// Puts `thread` at the end of `level`'s queue.
    ///
    /// # Errors
    ///
    /// When the thread or the level does not exist, or the thread waits
    /// already.
    pub fn push(&mut self, thread: usize, level: u8) -> Result<(), Error> {
        let number = Self::number(thread)?;
        let l = usize::from(level);
        if l >= LEVELS {
            return Err(Error::NoSuchLevel(level));
        }
        if self.level[thread] != NONE {
            return Err(Error::AlreadyWaiting(thread));
        }
        self.level[thread] = u16::from(level);
        self.next[thread] = NONE;
        match self.last[l] {
            NONE => self.first[l] = number,
            last => self.next[usize::from(last)] = number,
        }
        self.last[l] = number;
        self.waiting |= 1 << l;
        self.len += 1;
        Ok(())
    }

    /// Takes the thread that runs next: the first of the highest level
    /// with a thread waiting, and its level.
    pub fn pop(&mut self) -> Option<(usize, u8)> {
        let level = self.highest()?;
        let thread = usize::from(self.first[usize::from(level)]);
        self.unlink(thread, level, NONE);
        Some((thread, level))
    }

    /// Takes `thread` out of its queue wherever it is, and returns the
    /// level it waited on.
    ///
    /// # Errors
    ///
    /// When the thread does not exist or does not wait.
    pub fn remove(&mut self, thread: usize) -> Result<u8, Error> {
        Self::number(thread)?;
        let level = self.level_of(thread).ok_or(Error::NotWaiting(thread))?;
        let mut before = NONE;
        let mut at = self.first[usize::from(level)];
        while usize::from(at) != thread {
            before = at;
            at = self.next[usize::from(at)];
        }
        self.unlink(thread, level, before);
        Ok(level)
    }

    /// Takes `thread`, which waits on `level` after `before`, or first
    /// when `before` is [`NONE`], out of that level's queue.
    fn unlink(&mut self, thread: usize, level: u8, before: u16) {
        let l = usize::from(level);
        let after = self.next[thread];
        match before {
            NONE => self.first[l] = after,
            b => self.next[usize::from(b)] = after,
        }
        if after == NONE {
            self.last[l] = before;
        }
        if self.first[l] == NONE {
            self.waiting &= !(1 << l);
        }
        self.level[thread] = NONE;
        self.next[thread] = NONE;
        self.len -= 1;
    }

    /// The thread's number as the queues keep it: below the capacity, in
    /// 16 bits, and not the one that marks an end.
    fn number(thread: usize) -> Result<u16, Error> {
        u16::try_from(thread)
            .ok()
            .filter(|&n| thread < N && n != NONE)
            .ok_or(Error::NoSuchThread(thread))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn nobody_waits_at_first() {
        let mut q = RunQueue::<4>::new();
        assert!(q.is_empty());
        assert_eq!((q.len(), q.highest(), q.pop()), (0, None, None));
        assert_eq!(q.level_of(0), None);
        assert!(RunQueue::<4>::default().is_empty());
    }

    #[test]
    fn the_highest_level_goes_first_and_a_level_in_order() {
        let mut q = RunQueue::<8>::new();
        for (thread, level) in [(0, 5), (1, 10), (2, 5), (3, 0), (4, 10), (5, 31)] {
            q.push(thread, level).unwrap();
        }
        assert_eq!((q.len(), q.highest()), (6, Some(31)));
        let order: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(order, [(5, 31), (1, 10), (4, 10), (0, 5), (2, 5), (3, 0)]);
        assert!(q.is_empty());
        q.push(3, 0).unwrap();
        assert!(!q.is_empty(), "one thread is enough to wait");
    }

    #[test]
    fn threads_of_one_level_take_turns() {
        let mut q = RunQueue::<3>::new();
        for t in 0..3 {
            q.push(t, 7).unwrap();
        }
        let mut ran = Vec::new();
        for _ in 0..7 {
            let (t, level) = q.pop().unwrap();
            ran.push(t);
            q.push(t, level).unwrap();
        }
        assert_eq!(ran, [0, 1, 2, 0, 1, 2, 0]);
    }

    #[test]
    fn a_thread_leaves_from_anywhere_in_its_queue() {
        let mut q = RunQueue::<5>::new();
        for t in 0..5 {
            q.push(t, 3).unwrap();
        }
        assert_eq!(q.remove(2), Ok(3), "from the middle");
        assert_eq!(q.remove(0), Ok(3), "from the front");
        assert_eq!(q.remove(4), Ok(3), "from the end");
        assert_eq!((q.len(), q.level_of(2), q.level_of(1)), (2, None, Some(3)));
        q.push(4, 3).unwrap();
        let order: Vec<_> = std::iter::from_fn(|| q.pop()).map(|(t, _)| t).collect();
        assert_eq!(order, [1, 3, 4], "the queue stays linked after each");
        assert_eq!(q.highest(), None, "the last to leave clears its level");
    }

    #[test]
    fn mistakes_are_refused_and_change_nothing() {
        let mut q = RunQueue::<2>::new();
        q.push(1, 4).unwrap();
        assert_eq!(q.push(2, 4), Err(Error::NoSuchThread(2)));
        assert_eq!(q.push(0, 32), Err(Error::NoSuchLevel(32)));
        assert_eq!(q.push(1, 9), Err(Error::AlreadyWaiting(1)));
        assert_eq!(q.remove(0), Err(Error::NotWaiting(0)));
        assert_eq!(q.remove(7), Err(Error::NoSuchThread(7)));
        assert_eq!((q.len(), q.highest(), q.level_of(1)), (1, Some(4), Some(4)));
        assert_eq!(q.pop(), Some((1, 4)));
        assert_eq!(
            q.remove(1),
            Err(Error::NotWaiting(1)),
            "a thread that ran waits no more"
        );
    }

    #[test]
    fn a_queue_as_wide_as_its_numbers_keeps_the_end_free() {
        let mut q = Box::new(RunQueue::<{ NONE as usize + 1 }>::new());
        assert_eq!(
            q.push(usize::from(NONE), 1),
            Err(Error::NoSuchThread(usize::from(NONE)))
        );
        assert_eq!(q.push(usize::from(NONE) - 1, 1), Ok(()));
        assert_eq!(
            q.push(usize::from(NONE) + 1, 1),
            Err(Error::NoSuchThread(usize::from(NONE) + 1))
        );
        assert_eq!(q.pop(), Some((usize::from(NONE) - 1, 1)));
    }

    #[test]
    fn the_lowest_and_highest_levels_work_alike() {
        let mut q = RunQueue::<2>::new();
        q.push(0, 0).unwrap();
        assert_eq!(q.highest(), Some(0));
        q.push(1, 31).unwrap();
        assert_eq!(q.highest(), Some(31));
        assert_eq!(q.pop(), Some((1, 31)));
        assert_eq!(q.highest(), Some(0));
        assert_eq!(q.pop(), Some((0, 0)));
        assert_eq!(q.highest(), None);
    }

    #[test]
    fn levels_keep_their_own_order_when_threads_mix() {
        let mut q = RunQueue::<10>::new();
        for t in 0..10 {
            q.push(t, if t % 2 == 0 { 20 } else { 12 }).unwrap();
        }
        assert_eq!(q.remove(4), Ok(20));
        assert_eq!(q.remove(7), Ok(12));
        q.push(4, 12).unwrap();
        let order: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(
            order,
            [
                (0, 20),
                (2, 20),
                (6, 20),
                (8, 20),
                (1, 12),
                (3, 12),
                (5, 12),
                (9, 12),
                (4, 12)
            ]
        );
    }

    #[test]
    fn a_level_stays_marked_until_its_last_thread_leaves() {
        let mut q = RunQueue::<3>::new();
        q.push(0, 9).unwrap();
        q.push(1, 9).unwrap();
        q.push(2, 2).unwrap();
        assert_eq!(q.remove(1), Ok(9));
        assert_eq!(q.highest(), Some(9));
        assert_eq!(q.remove(0), Ok(9));
        assert_eq!(q.highest(), Some(2));
        q.push(0, 9).unwrap();
        assert_eq!(
            q.pop(),
            Some((0, 9)),
            "an emptied level takes threads again"
        );
        assert_eq!(q.pop(), Some((2, 2)));
    }

    #[test]
    fn every_thread_can_wait_at_once() {
        let mut q = RunQueue::<64>::new();
        for t in 0..64 {
            q.push(t, u8::try_from(t % 32).unwrap()).unwrap();
        }
        assert_eq!(q.len(), 64);
        for level in (0..32u8).rev() {
            let t = usize::from(level);
            assert_eq!(q.pop(), Some((t, level)));
            assert_eq!(q.pop(), Some((t + 32, level)));
        }
        assert!(q.is_empty());
    }

    /// The same queue the simple way, to check the linked one against.
    struct Model {
        levels: Vec<VecDeque<usize>>,
    }

    impl Model {
        fn pop(&mut self) -> Option<(usize, u8)> {
            let level = (0..LEVELS).rev().find(|&l| !self.levels[l].is_empty())?;
            Some((self.levels[level].pop_front()?, u8::try_from(level).ok()?))
        }

        fn level_of(&self, thread: usize) -> Option<u8> {
            let level = self.levels.iter().position(|q| q.contains(&thread))?;
            u8::try_from(level).ok()
        }
    }

    #[test]
    fn ten_thousand_steps_agree_with_the_simple_way() {
        const THREADS: usize = 40;
        let mut q = RunQueue::<THREADS>::new();
        let mut model = Model {
            levels: vec![VecDeque::new(); LEVELS],
        };
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut random = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        for step in 0..10_000 {
            let thread = usize::try_from(random(THREADS as u64)).unwrap();
            match random(3) {
                0 => {
                    let level = u8::try_from(random(LEVELS as u64)).unwrap();
                    let waits = model.level_of(thread).is_some();
                    let pushed = q.push(thread, level);
                    if waits {
                        assert_eq!(pushed, Err(Error::AlreadyWaiting(thread)), "step {step}");
                    } else {
                        assert_eq!(pushed, Ok(()), "step {step}");
                        model.levels[usize::from(level)].push_back(thread);
                    }
                }
                1 => assert_eq!(q.pop(), model.pop(), "step {step}"),
                _ => {
                    let want = model.level_of(thread);
                    if let Some(level) = want {
                        model.levels[usize::from(level)].retain(|&t| t != thread);
                    }
                    assert_eq!(q.remove(thread).ok(), want, "step {step}");
                }
            }
            let waiting: usize = model.levels.iter().map(VecDeque::len).sum();
            let highest = (0..LEVELS).rev().find(|&l| !model.levels[l].is_empty());
            assert_eq!(q.len(), waiting, "step {step}");
            assert_eq!(q.highest().map(usize::from), highest, "step {step}");
        }
    }
}
