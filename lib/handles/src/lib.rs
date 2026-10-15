//! Handle tables. A process reaches kernel objects only through handles:
//! numbers that stand for an object and the rights the process has to it.
//! Every slot of a table carries a generation, which goes up each time the
//! slot is freed, so a handle that was closed never reaches the object
//! that later takes its slot. Nothing is allocated; a table holds up to
//! `N` handles.

#![cfg_attr(not(test), no_std)]

/// What a handle allows, one bit each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(pub u32);

impl Rights {
    pub const READ: Rights = Rights(1);
    pub const WRITE: Rights = Rights(1 << 1);
    /// Making another handle to the same object.
    pub const DUPLICATE: Rights = Rights(1 << 2);
    /// Handing the handle to another process.
    pub const TRANSFER: Rights = Rights(1 << 3);
    pub const ALL: Rights = Rights(0b1111);

    /// Whether every right of `other` is among these.
    #[must_use]
    pub fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for Rights {
    type Output = Rights;
    fn bitor(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }
}

/// A handle as a process sees it: the slot in the low 16 bits, the slot's
/// generation above, and never 0, so 0 can mean no handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub u32);

impl Handle {
    fn new(slot: u16, generation: u16) -> Handle {
        Handle(u32::from(generation) << 16 | (u32::from(slot) + 1))
    }

    fn slot(self) -> Option<usize> {
        (self.0 & 0xffff)
            .checked_sub(1)
            .and_then(|s| usize::try_from(s).ok())
    }

    fn generation(self) -> u16 {
        u16::try_from(self.0 >> 16).unwrap_or(u16::MAX)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No handle has this number, or it was closed.
    NoSuchHandle(Handle),
    /// The handle does not allow what was asked.
    Denied(Handle),
    /// The table is full.
    Full,
}

#[derive(Clone, Copy)]
struct Entry {
    /// The object, or `None` while the slot is free.
    object: Option<u64>,
    rights: Rights,
    generation: u16,
}

/// The handles one process holds, up to `N` of them.
pub struct Table<const N: usize> {
    entries: [Entry; N],
}

impl<const N: usize> Default for Table<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Table<N> {
    /// A table holding no handle.
    #[must_use]
    pub const fn new() -> Self {
        Table {
            entries: [Entry {
                object: None,
                rights: Rights(0),
                generation: 0,
            }; N],
        }
    }

    /// A new handle to `object` with `rights`.
    ///
    /// # Errors
    ///
    /// When the table is full.
    pub fn insert(&mut self, object: u64, rights: Rights) -> Result<Handle, Error> {
        let slot = self
            .entries
            .iter()
            .take(0xffff)
            .position(|e| e.object.is_none())
            .ok_or(Error::Full)?;
        let number = u16::try_from(slot).map_err(|_| Error::Full)?;
        let entry = &mut self.entries[slot];
        entry.object = Some(object);
        entry.rights = rights;
        Ok(Handle::new(number, entry.generation))
    }

    /// The object `handle` stands for, when it allows `needed`.
    ///
    /// # Errors
    ///
    /// When there is no such handle, or it does not allow `needed`.
    pub fn get(&self, handle: Handle, needed: Rights) -> Result<u64, Error> {
        let entry = self.entry(handle)?;
        if !entry.rights.contains(needed) {
            return Err(Error::Denied(handle));
        }
        entry.object.ok_or(Error::NoSuchHandle(handle))
    }

    /// The rights `handle` has.
    ///
    /// # Errors
    ///
    /// When there is no such handle.
    pub fn rights(&self, handle: Handle) -> Result<Rights, Error> {
        Ok(self.entry(handle)?.rights)
    }

    /// Closes `handle` and returns the object it stood for. Its number
    /// never stands for anything again until its slot has gone round all
    /// 65536 generations.
    ///
    /// # Errors
    ///
    /// When there is no such handle.
    pub fn close(&mut self, handle: Handle) -> Result<u64, Error> {
        let entry = handle
            .slot()
            .and_then(|s| self.entries.get_mut(s))
            .filter(|e| e.generation == handle.generation())
            .ok_or(Error::NoSuchHandle(handle))?;
        let object = entry.object.take().ok_or(Error::NoSuchHandle(handle))?;
        entry.rights = Rights(0);
        entry.generation = entry.generation.wrapping_add(1);
        Ok(object)
    }

    /// A second handle to the object `handle` stands for, with `rights`,
    /// which have to be among the first one's, which has to allow
    /// duplicating.
    ///
    /// # Errors
    ///
    /// When there is no such handle, it does not allow duplicating or does
    /// not have `rights`, or the table is full.
    pub fn duplicate(&mut self, handle: Handle, rights: Rights) -> Result<Handle, Error> {
        let have = self.rights(handle)?;
        if !have.contains(rights | Rights::DUPLICATE) {
            return Err(Error::Denied(handle));
        }
        let object = self.get(handle, Rights::DUPLICATE)?;
        self.insert(object, rights)
    }

    /// How many handles the table holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| e.object.is_some()).count()
    }

    /// Whether the table holds no handle.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Closes every handle, handing each object to `closed`, as when the
    /// process ends.
    pub fn close_all(&mut self, mut closed: impl FnMut(u64)) {
        for entry in &mut self.entries {
            if let Some(object) = entry.object.take() {
                entry.rights = Rights(0);
                entry.generation = entry.generation.wrapping_add(1);
                closed(object);
            }
        }
    }

    fn entry(&self, handle: Handle) -> Result<&Entry, Error> {
        handle
            .slot()
            .and_then(|s| self.entries.get(s))
            .filter(|e| e.object.is_some() && e.generation == handle.generation())
            .ok_or(Error::NoSuchHandle(handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const RW: Rights = Rights(Rights::READ.0 | Rights::WRITE.0);

    #[test]
    fn a_handle_reaches_its_object_with_the_rights_it_has() {
        let mut t = Table::<4>::new();
        let h = t.insert(0xabc, RW).unwrap();
        assert_ne!(h.0, 0, "0 is no handle");
        assert_eq!(t.get(h, Rights::READ), Ok(0xabc));
        assert_eq!(t.get(h, RW), Ok(0xabc));
        assert_eq!(t.get(h, Rights(0)), Ok(0xabc), "asking for nothing");
        assert_eq!(t.get(h, Rights::DUPLICATE), Err(Error::Denied(h)));
        assert_eq!(
            t.get(h, Rights::ALL),
            Err(Error::Denied(h)),
            "all of them, not some"
        );
        assert_eq!(t.rights(h), Ok(RW));
        assert_eq!((t.len(), t.is_empty()), (1, false));
        assert!(Table::<4>::default().is_empty());
    }

    #[test]
    fn a_closed_handle_never_reaches_the_next_object_in_its_slot() {
        let mut t = Table::<1>::new();
        let old = t.insert(1, Rights::ALL).unwrap();
        assert_eq!(t.close(old), Ok(1));
        assert_eq!(t.get(old, Rights(0)), Err(Error::NoSuchHandle(old)));
        assert_eq!(t.close(old), Err(Error::NoSuchHandle(old)), "closing twice");
        let new = t.insert(2, Rights::ALL).unwrap();
        assert_ne!(new, old, "the same slot, a new generation");
        assert_eq!(t.get(old, Rights(0)), Err(Error::NoSuchHandle(old)));
        assert_eq!(t.get(new, Rights(0)), Ok(2));
        assert_eq!(t.rights(old), Err(Error::NoSuchHandle(old)));
    }

    #[test]
    fn made_up_handles_reach_nothing() {
        let mut t = Table::<4>::new();
        let h = t.insert(7, Rights::ALL).unwrap();
        for bad in [
            Handle(0),
            Handle(5),
            Handle(0xffff),
            Handle(h.0 + (1 << 16)),
            Handle(h.0 + 1),
        ] {
            assert_eq!(
                t.get(bad, Rights(0)),
                Err(Error::NoSuchHandle(bad)),
                "{bad:?}"
            );
            assert_eq!(t.close(bad), Err(Error::NoSuchHandle(bad)), "{bad:?}");
        }
        assert_eq!(t.get(h, Rights(0)), Ok(7));
        assert_eq!(t.len(), 1, "closing made up handles closed nothing");
    }

    #[test]
    fn duplicates_have_at_most_the_rights_of_the_first() {
        let mut t = Table::<4>::new();
        let h = t.insert(9, RW | Rights::DUPLICATE).unwrap();
        let read = t.duplicate(h, Rights::READ).unwrap();
        assert_ne!(read, h);
        assert_eq!(
            (t.get(read, Rights::READ), t.rights(read)),
            (Ok(9), Ok(Rights::READ))
        );
        assert_eq!(
            t.duplicate(h, Rights::TRANSFER),
            Err(Error::Denied(h)),
            "more than it has"
        );
        assert_eq!(
            t.duplicate(read, Rights::READ),
            Err(Error::Denied(read)),
            "no right to duplicate"
        );
        assert_eq!(t.close(h), Ok(9));
        assert_eq!(
            t.get(read, Rights::READ),
            Ok(9),
            "a duplicate outlives the first"
        );
        assert_eq!(t.duplicate(h, Rights::READ), Err(Error::NoSuchHandle(h)));
    }

    #[test]
    fn a_full_table_takes_more_once_one_is_closed() {
        let mut t = Table::<3>::new();
        let handles: Vec<Handle> = (0..3).map(|i| t.insert(i, Rights::ALL).unwrap()).collect();
        assert_eq!(t.insert(3, Rights::ALL), Err(Error::Full));
        let mut full = Table::<1>::new();
        let only = full.insert(0, Rights::ALL).unwrap();
        assert_eq!(full.duplicate(only, Rights::READ), Err(Error::Full));
        t.close(handles[1]).unwrap();
        let again = t.insert(3, Rights::ALL).unwrap();
        assert_eq!(t.get(again, Rights(0)), Ok(3));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn closing_everything_hands_each_object_over_once() {
        let mut t = Table::<5>::new();
        let handles: Vec<Handle> = [10, 20, 30]
            .iter()
            .map(|&o| t.insert(o, Rights::ALL).unwrap())
            .collect();
        t.close(handles[1]).unwrap();
        let mut closed = Vec::new();
        t.close_all(|o| closed.push(o));
        assert_eq!(closed, [10, 30]);
        assert!(t.is_empty());
        for h in handles {
            assert_eq!(t.get(h, Rights(0)), Err(Error::NoSuchHandle(h)));
        }
        let mut none = Vec::new();
        t.close_all(|o| none.push(o));
        assert_eq!(none, Vec::<u64>::new(), "nothing is left to close");
    }

    #[test]
    fn a_slot_comes_round_to_its_first_number_after_65536_closes() {
        let mut t = Table::<1>::new();
        let first = t.insert(0, Rights::ALL).unwrap();
        t.close(first).unwrap();
        for _ in 1..65_536 {
            let h = t.insert(0, Rights::ALL).unwrap();
            assert_ne!(h, first);
            t.close(h).unwrap();
        }
        assert_eq!(t.insert(0, Rights::ALL), Ok(first));
    }

    #[test]
    fn handle_numbers_hold_their_slot_and_generation() {
        let mut t = Table::<3>::new();
        let a = t.insert(1, Rights::ALL).unwrap();
        let b = t.insert(2, Rights::ALL).unwrap();
        assert_eq!((a.0, b.0), (1, 2), "slots counted from 1, generation 0");
        t.close(a).unwrap();
        let c = t.insert(3, Rights::ALL).unwrap();
        assert_eq!(c.0, 1 << 16 | 1, "the same slot, one generation on");
        let mut other = Table::<3>::new();
        assert_eq!(
            other.insert(9, Rights::READ),
            Ok(Handle(1)),
            "every process numbers its own"
        );
        assert_eq!(
            other.get(b, Rights(0)),
            Err(Error::NoSuchHandle(b)),
            "another table's handle"
        );
    }

    #[test]
    fn rights_combine_and_contain() {
        assert_eq!(Rights::READ | Rights::WRITE, RW);
        assert!(RW.contains(Rights::READ) && RW.contains(RW) && RW.contains(Rights(0)));
        assert!(!Rights::READ.contains(RW));
        assert_eq!(
            Rights::ALL,
            Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER
        );
        assert_eq!(
            [
                Rights::READ.0,
                Rights::WRITE.0,
                Rights::DUPLICATE.0,
                Rights::TRANSFER.0
            ],
            [1, 2, 4, 8]
        );
    }

    #[test]
    fn ten_thousand_steps_agree_with_a_map() {
        let mut t = Table::<16>::new();
        let mut model: HashMap<u32, (u64, Rights)> = HashMap::new();
        let mut handed: Vec<Handle> = Vec::new();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        for step in 0..10_000u64 {
            match random(3) {
                0 => {
                    let rights = Rights(u32::try_from(random(16)).unwrap());
                    match t.insert(step, rights) {
                        Ok(h) => {
                            assert!(
                                model.insert(h.0, (step, rights)).is_none(),
                                "step {step}: a live number again"
                            );
                            handed.push(h);
                        }
                        Err(e) => assert_eq!((e, model.len()), (Error::Full, 16), "step {step}"),
                    }
                }
                1 if !handed.is_empty() => {
                    let h = handed[usize::try_from(random(handed.len() as u64)).unwrap()];
                    let want = model
                        .remove(&h.0)
                        .map(|(o, _)| o)
                        .ok_or(Error::NoSuchHandle(h));
                    assert_eq!(t.close(h), want, "step {step}");
                }
                _ if !handed.is_empty() => {
                    let h = handed[usize::try_from(random(handed.len() as u64)).unwrap()];
                    let needed = Rights(u32::try_from(random(16)).unwrap());
                    let want = match model.get(&h.0) {
                        None => Err(Error::NoSuchHandle(h)),
                        Some(&(_, r)) if !r.contains(needed) => Err(Error::Denied(h)),
                        Some(&(o, _)) => Ok(o),
                    };
                    assert_eq!(t.get(h, needed), want, "step {step}");
                }
                _ => {}
            }
            assert_eq!(t.len(), model.len(), "step {step}");
        }
    }
}
