//! The physical frame allocator: one bit for each 4 KiB frame of memory,
//! set while the frame is free, built from the loader's memory map.

#![cfg_attr(not(test), no_std)]
// Meridian runs on 64-bit processors only, where frame numbers, which are
// u64 like the addresses they come from, fit a usize.
#![allow(clippy::cast_possible_truncation)]

const _: () = assert!(usize::BITS == 64);

/// The size of a frame.
pub const FRAME: u64 = 4096;

/// Nothing below this is handed out: the first MiB holds what firmware
/// leaves behind and, later, the code that starts the other cores.
pub const LOW: u64 = 1 << 20;

/// Why units could not be given back to a [`Bitmap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GiveBack {
    /// The unit lies beyond the ones the bitmap covers.
    OutOfRange(usize),
    /// The unit is free already.
    NotTaken(usize),
}

/// One bit for each unit of something, such as a frame of memory or a
/// block of the kernel's heap, set while the unit is free.
pub struct Bitmap<'a> {
    bits: &'a mut [u64],
    free: usize,
    /// The word the next search for a single unit starts at.
    next: usize,
}

impl<'a> Bitmap<'a> {
    /// A bitmap over `bits` with every unit taken.
    pub fn taken(bits: &'a mut [u64]) -> Bitmap<'a> {
        bits.fill(0);
        Bitmap {
            bits,
            free: 0,
            next: 0,
        }
    }

    /// How many units the bits stand for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bits.len() * 64
    }

    /// Whether there are no units at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    /// How many units are free.
    #[must_use]
    pub fn free(&self) -> usize {
        self.free
    }

    /// Whether `unit` is free; units beyond the bits never are.
    #[must_use]
    pub fn is_free(&self, unit: usize) -> bool {
        self.bits
            .get(unit / 64)
            .is_some_and(|w| w >> (unit % 64) & 1 == 1)
    }

    /// Marks `unit` free or taken and keeps the count of free units.
    fn set(&mut self, unit: usize, free: bool) {
        if self.is_free(unit) == free {
            return;
        }
        self.bits[unit / 64] ^= 1 << (unit % 64);
        if free {
            self.free += 1;
        } else {
            self.free -= 1;
        }
    }

    /// Marks the units of `range` free, as far as the bits reach.
    pub fn release(&mut self, range: core::ops::Range<usize>) {
        for unit in range.start..range.end.min(self.len()) {
            self.set(unit, true);
        }
    }

    /// Marks the units of `range` taken, as far as the bits reach.
    pub fn take(&mut self, range: core::ops::Range<usize>) {
        for unit in range.start..range.end.min(self.len()) {
            self.set(unit, false);
        }
    }

    /// A free unit, now taken, or `None` when there is none.
    pub fn take_one(&mut self) -> Option<usize> {
        let words = self.bits.len();
        for word in (self.next..words).chain(0..self.next) {
            let bits = self.bits[word];
            if bits != 0 {
                let unit = word * 64 + bits.trailing_zeros() as usize;
                self.set(unit, false);
                self.next = word;
                return Some(unit);
            }
        }
        None
    }

    /// `count` free units in a row, the first a multiple of `align`, now
    /// taken, or `None` when there are none, `count` is 0 or `align` is
    /// not a power of two.
    pub fn take_run(&mut self, count: usize, align: usize) -> Option<usize> {
        if count == 0 || !align.is_power_of_two() {
            return None;
        }
        let mut first = 0;
        while first + count <= self.len() {
            // The last taken unit in the way decides where to look next.
            match (first..first + count).rev().find(|&u| !self.is_free(u)) {
                None => {
                    self.take(first..first + count);
                    return Some(first);
                }
                Some(taken) => first = (taken + 1).next_multiple_of(align),
            }
        }
        None
    }

    /// Gives `count` units from `first` back, all of them or, when one is
    /// beyond the bits or free already, none.
    ///
    /// # Errors
    ///
    /// Names the first unit that cannot be given back.
    pub fn give_back(&mut self, first: usize, count: usize) -> Result<(), GiveBack> {
        let end = first.saturating_add(count);
        if end > self.len() {
            return Err(GiveBack::OutOfRange(first.max(self.len())));
        }
        if let Some(unit) = (first..end).find(|&u| self.is_free(u)) {
            return Err(GiveBack::NotTaken(unit));
        }
        self.release(first..end);
        Ok(())
    }
}

/// Why a frame could not be given back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeError {
    /// The address is not the start of a frame.
    Misaligned(u64),
    /// The frame lies beyond the memory the allocator covers.
    OutOfRange(u64),
    /// The frame is free already.
    NotTaken(u64),
}

/// Which frames of physical memory are free.
pub struct Frames<'a> {
    bits: Bitmap<'a>,
}

impl<'a> Frames<'a> {
    /// How many words of bits memory up to `top` needs.
    #[must_use]
    pub fn words_for(top: u64) -> usize {
        usize::try_from(top.div_ceil(FRAME).div_ceil(64)).unwrap_or(usize::MAX)
    }

    /// The frames `bits` covers, every one taken except those wholly
    /// inside one of the `free` ranges, given as start and length, and
    /// at or above [`LOW`].
    pub fn new(bits: &'a mut [u64], free: impl IntoIterator<Item = (u64, u64)>) -> Frames<'a> {
        let mut bits = Bitmap::taken(bits);
        for (start, len) in free {
            let first = start.max(LOW).div_ceil(FRAME);
            let end = start.saturating_add(len) / FRAME;
            bits.release(index(first)..index(end));
        }
        Frames { bits }
    }

    /// How many frames are free.
    #[must_use]
    pub fn free_frames(&self) -> usize {
        self.bits.free()
    }

    /// Takes the frames that hold any of `len` bytes from `start` out of
    /// the free ones, such as the frames these bits are kept in.
    pub fn reserve(&mut self, start: u64, len: u64) {
        let end = start.saturating_add(len).div_ceil(FRAME);
        self.bits.take(index(start / FRAME)..index(end));
    }

    /// A free frame, now taken, or `None` when there is none.
    pub fn alloc(&mut self) -> Option<u64> {
        self.bits.take_one().map(address)
    }

    /// `count` free frames in a row whose first is a multiple of `align`
    /// frames, now taken, or `None` when there are none, `count` is 0 or
    /// `align` is not a power of two.
    pub fn alloc_run(&mut self, count: u64, align: u64) -> Option<u64> {
        self.bits.take_run(index(count), index(align)).map(address)
    }

    /// Gives the frame at `address` back.
    ///
    /// # Errors
    ///
    /// When `address` is not the start of a frame, lies beyond the memory
    /// the allocator covers, or is free already.
    pub fn free(&mut self, address: u64) -> Result<(), FreeError> {
        if !address.is_multiple_of(FRAME) {
            return Err(FreeError::Misaligned(address));
        }
        self.bits
            .give_back(index(address / FRAME), 1)
            .map_err(|e| match e {
                GiveBack::OutOfRange(_) => FreeError::OutOfRange(address),
                GiveBack::NotTaken(_) => FreeError::NotTaken(address),
            })
    }
}

/// A frame number as an index into the bits; numbers past what a usize
/// holds are past every bitmap too.
fn index(frame: u64) -> usize {
    usize::try_from(frame).unwrap_or(usize::MAX)
}

fn address(frame: usize) -> u64 {
    frame as u64 * FRAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const MIB: u64 = 1 << 20;

    #[test]
    fn a_new_bitmap_has_every_unit_taken() {
        let mut words = [u64::MAX; 2];
        let b = Bitmap::taken(&mut words);
        assert_eq!((b.len(), b.free(), b.is_empty()), (128, 0, false));
        assert!(!b.is_free(0) && !b.is_free(127) && !b.is_free(128));
        assert!(Bitmap::taken(&mut []).is_empty());
    }

    #[test]
    fn releasing_and_taking_stop_at_the_end() {
        let mut words = [0; 2];
        let mut b = Bitmap::taken(&mut words);
        b.release(120..usize::MAX);
        assert_eq!(b.free(), 8);
        assert!(b.is_free(127) && !b.is_free(119) && !b.is_free(128));
        b.release(126..130);
        assert_eq!(b.free(), 8, "free already");
        b.take(125..usize::MAX);
        assert_eq!(b.free(), 5);
        b.take(0..0);
        assert_eq!(b.free(), 5);
    }

    #[test]
    fn units_come_back_all_or_none() {
        let mut words = [0; 1];
        let mut b = Bitmap::taken(&mut words);
        b.release(0..64);
        assert_eq!(b.take_run(10, 1), Some(0));
        assert_eq!(
            b.give_back(8, 4),
            Err(GiveBack::NotTaken(10)),
            "two of them were free"
        );
        assert_eq!(b.free(), 54, "and nothing came back");
        assert_eq!(b.give_back(58, 4), Err(GiveBack::NotTaken(58)));
        assert_eq!(b.take_run(4, 4), Some(12));
        assert_eq!(b.give_back(12, 4), Ok(()));
        b.take(60..64);
        assert_eq!(b.give_back(62, 4), Err(GiveBack::OutOfRange(64)));
        assert_eq!(
            b.give_back(usize::MAX, 2),
            Err(GiveBack::OutOfRange(usize::MAX))
        );
        assert_eq!(b.give_back(0, 10), Ok(()));
        assert_eq!(b.give_back(0, 0), Ok(()), "nothing to give back");
        assert_eq!(b.free(), 60);
    }

    #[test]
    fn runs_of_units_keep_to_their_alignment() {
        let mut words = [0; 4];
        let mut b = Bitmap::taken(&mut words);
        b.release(3..200);
        assert_eq!(b.take_run(2, 16), Some(16));
        assert_eq!(b.take_run(1, 1), Some(3));
        assert_eq!(b.take_run(13, 1), Some(18), "the first gap that holds it");
        assert_eq!(b.take_run(64, 64), Some(64));
        assert_eq!(b.take_run(64, 64), Some(128));
        assert_eq!(b.take_run(64, 64), None, "192 on would need units past 199");
        assert_eq!(b.take_run(8, 64), Some(192));
        assert_eq!(b.take_one(), Some(4));
    }

    #[test]
    fn the_bits_cover_memory_up_to_its_top() {
        assert_eq!(Frames::words_for(0), 0);
        assert_eq!(Frames::words_for(1), 1);
        assert_eq!(Frames::words_for(64 * FRAME), 1);
        assert_eq!(Frames::words_for(64 * FRAME + 1), 2);
        assert_eq!(Frames::words_for(4 << 30), (4 << 30) / FRAME as usize / 64);
        assert_eq!(
            Frames::words_for(u64::MAX),
            (u64::MAX / FRAME / 64 + 1) as usize
        );
    }

    #[test]
    fn only_whole_frames_inside_free_ranges_above_the_first_mib_are_free() {
        let mut bits = [u64::MAX; 8];
        let f = Frames::new(
            &mut bits,
            [
                (0, 0x9f000),
                (MIB + 0x800, 3 * FRAME),
                (MIB + 0x10_000, 2 * FRAME),
                (MIB + 0x10_000, FRAME),
            ],
        );
        assert_eq!(f.free_frames(), 2 + 2, "a ragged range loses its ends");
        let expected = [
            MIB / FRAME + 1,
            MIB / FRAME + 2,
            MIB / FRAME + 16,
            MIB / FRAME + 17,
        ];
        for frame in 0..f.bits.len() {
            assert_eq!(
                f.bits.is_free(frame),
                expected.contains(&(frame as u64)),
                "frame {frame}"
            );
        }
    }

    #[test]
    fn ranges_beyond_the_bits_are_cut_off() {
        let mut bits = [0; 5];
        let f = Frames::new(&mut bits, [(MIB, u64::MAX), (u64::MAX - 5, 10)]);
        assert_eq!(f.free_frames(), 5 * 64 - (MIB / FRAME) as usize);
        assert!(f.bits.is_free(5 * 64 - 1));
    }

    fn sample() -> Vec<u64> {
        vec![0; 16]
    }

    #[test]
    fn frames_are_handed_out_once_from_the_lowest() {
        let mut bits = sample();
        let mut f = Frames::new(&mut bits, [(MIB, 4 * FRAME), (2 * MIB, 2 * FRAME)]);
        assert_eq!(f.alloc(), Some(MIB));
        assert_eq!(f.alloc(), Some(MIB + FRAME));
        assert_eq!(f.free_frames(), 4);
        let rest: Vec<_> = core::iter::from_fn(|| f.alloc()).collect();
        assert_eq!(
            rest,
            [MIB + 2 * FRAME, MIB + 3 * FRAME, 2 * MIB, 2 * MIB + FRAME]
        );
        assert_eq!(f.free_frames(), 0);
        assert_eq!(f.alloc(), None);
    }

    #[test]
    fn the_search_goes_round_to_frames_given_back() {
        let mut bits = sample();
        let mut f = Frames::new(&mut bits, [(MIB, 70 * FRAME)]);
        let all: Vec<_> = core::iter::from_fn(|| f.alloc()).collect();
        assert_eq!(all.len(), 70);
        f.free(MIB).unwrap();
        assert_eq!(f.alloc(), Some(MIB), "behind where the search stopped");
        f.free(all[69]).unwrap();
        assert_eq!(f.alloc(), Some(all[69]));
    }

    #[test]
    fn freeing_checks_the_address() {
        let mut bits = sample();
        let mut f = Frames::new(&mut bits, [(MIB, 2 * FRAME)]);
        let a = f.alloc().unwrap();
        assert_eq!(f.free(a + 8), Err(FreeError::Misaligned(a + 8)));
        assert_eq!(
            f.free(16 * 64 * FRAME),
            Err(FreeError::OutOfRange(16 * 64 * FRAME))
        );
        assert_eq!(
            f.free((16 * 64 - 1) * FRAME),
            Ok(()),
            "the last frame covered"
        );
        assert_eq!(f.free(a), Ok(()));
        assert_eq!(f.free(a), Err(FreeError::NotTaken(a)), "twice");
        assert_eq!(f.free_frames(), 3);
    }

    #[test]
    fn runs_are_whole_and_aligned() {
        let mut bits = sample();
        let mut f = Frames::new(&mut bits, [(MIB, 600 * FRAME)]);
        assert_eq!(f.alloc_run(3, 1), Some(MIB));
        assert_eq!(
            f.alloc_run(4, 8),
            Some(MIB + 8 * FRAME),
            "skipping to the next multiple of 8"
        );
        assert_eq!(f.alloc(), Some(MIB + 3 * FRAME), "the gap stays free");
        assert_eq!(f.free_frames(), 600 - 3 - 4 - 1);
        assert_eq!(
            f.alloc_run(512, 512),
            None,
            "the next 2 MiB boundary leaves too few"
        );
        assert_eq!(f.alloc_run(0, 1), None);
        assert_eq!(f.alloc_run(1, 3), None, "alignments are powers of two");
        assert_eq!(f.alloc_run(1, 0), None);
    }

    #[test]
    fn a_run_may_end_at_the_last_frame_and_steps_over_holes() {
        let mut bits = vec![0; 5];
        let mut f = Frames::new(&mut bits, [(0, 320 * FRAME)]);
        f.reserve(MIB + 2 * FRAME, 1);
        let first = MIB / FRAME;
        assert_eq!(f.alloc_run(320 - first - 3, 1), Some(MIB + 3 * FRAME));
        assert_eq!(f.alloc_run(2, 1), Some(MIB));
        assert_eq!(f.alloc_run(1, 1), None);
        assert_eq!(f.free_frames(), 0);
    }

    #[test]
    fn reserving_takes_every_frame_it_touches() {
        let mut bits = sample();
        let mut f = Frames::new(&mut bits, [(MIB, 10 * FRAME)]);
        f.reserve(MIB + FRAME - 1, 2);
        assert_eq!(f.free_frames(), 8, "a frame on each side of the edge");
        f.reserve(MIB, FRAME);
        assert_eq!(f.free_frames(), 8, "taken already");
        f.reserve(MIB + 9 * FRAME, u64::MAX);
        assert_eq!(f.free_frames(), 7, "to the end of memory");
        f.reserve(0, 0);
        assert_eq!(f.free_frames(), 7);
        assert_eq!(f.alloc(), Some(MIB + 2 * FRAME));
    }

    /// Random allocations and frees against a set of the frames handed
    /// out, from a fixed seed.
    #[test]
    fn many_allocations_match_a_model() {
        let mut bits = vec![0; 64];
        let ranges = [
            (MIB, 300 * FRAME),
            (3 * MIB + 0x123, 900 * FRAME),
            (12 * MIB, 2000 * FRAME),
        ];
        let mut f = Frames::new(&mut bits, ranges);
        let total = f.free_frames();
        let mut taken = BTreeSet::new();
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            match random() % 4 {
                0 | 1 => {
                    if let Some(a) = f.alloc() {
                        assert!(
                            ranges.iter().any(|&(s, l)| s <= a && a + FRAME <= s + l),
                            "{a:#x} is free memory"
                        );
                        assert!(taken.insert(a), "{a:#x} handed out twice");
                    }
                }
                2 => {
                    let n = random() % 9 + 1;
                    let align = 1 << (random() % 4);
                    if let Some(a) = f.alloc_run(n, align) {
                        assert!(a.is_multiple_of(align * FRAME));
                        for i in 0..n {
                            assert!(taken.insert(a + i * FRAME), "run frame handed out twice");
                        }
                    }
                }
                _ => {
                    if let Some(&a) = taken.iter().nth((random() as usize) % taken.len().max(1)) {
                        taken.remove(&a);
                        f.free(a).unwrap();
                    }
                }
            }
            assert_eq!(f.free_frames() + taken.len(), total);
        }
    }
}
