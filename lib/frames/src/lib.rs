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

/// Which frames of memory are free, one bit each in a slice of words.
pub struct Frames<'a> {
    bits: &'a mut [u64],
    free: usize,
    /// The word the next search for a single frame starts at.
    next: usize,
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
        bits.fill(0);
        let mut frames = Frames {
            bits,
            free: 0,
            next: 0,
        };
        for (start, len) in free {
            let first = start.max(LOW).div_ceil(FRAME);
            let end = (start.saturating_add(len) / FRAME).min(frames.covered());
            for frame in first..end {
                frames.set(frame, true);
            }
        }
        frames
    }

    /// How many frames the bits stand for.
    fn covered(&self) -> u64 {
        self.bits.len() as u64 * 64
    }

    fn is_free(&self, frame: u64) -> bool {
        let (word, bit) = (frame / 64, frame % 64);
        self.bits[word as usize] >> bit & 1 == 1
    }

    /// Marks `frame` free or taken and keeps the count of free frames.
    fn set(&mut self, frame: u64, free: bool) {
        if self.is_free(frame) == free {
            return;
        }
        let (word, bit) = ((frame / 64) as usize, frame % 64);
        self.bits[word] ^= 1 << bit;
        if free {
            self.free += 1;
        } else {
            self.free -= 1;
        }
    }

    /// How many frames are free.
    #[must_use]
    pub fn free_frames(&self) -> usize {
        self.free
    }

    /// Takes the frames that hold any of `len` bytes from `start` out of
    /// the free ones, such as the frames these bits are kept in.
    pub fn reserve(&mut self, start: u64, len: u64) {
        let first = start / FRAME;
        let end = start
            .saturating_add(len)
            .div_ceil(FRAME)
            .min(self.covered());
        for frame in first..end {
            self.set(frame, false);
        }
    }

    /// A free frame, now taken, or `None` when there is none.
    pub fn alloc(&mut self) -> Option<u64> {
        let words = self.bits.len();
        for word in (self.next..words).chain(0..self.next) {
            let bits = self.bits[word];
            if bits != 0 {
                let frame = word as u64 * 64 + u64::from(bits.trailing_zeros());
                self.set(frame, false);
                self.next = word;
                return Some(frame * FRAME);
            }
        }
        None
    }

    /// `count` free frames in a row whose first is a multiple of `align`
    /// frames, now taken, or `None` when there are none, `count` is 0 or
    /// `align` is not a power of two.
    pub fn alloc_run(&mut self, count: u64, align: u64) -> Option<u64> {
        if count == 0 || !align.is_power_of_two() {
            return None;
        }
        let mut first = 0;
        while first + count <= self.covered() {
            // The last taken frame in the way decides where to look next.
            match (first..first + count).rev().find(|&f| !self.is_free(f)) {
                None => {
                    for frame in first..first + count {
                        self.set(frame, false);
                    }
                    return Some(first * FRAME);
                }
                Some(taken) => first = (taken + 1).next_multiple_of(align),
            }
        }
        None
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
        let frame = address / FRAME;
        if frame >= self.covered() {
            return Err(FreeError::OutOfRange(address));
        }
        if self.is_free(frame) {
            return Err(FreeError::NotTaken(address));
        }
        self.set(frame, true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const MIB: u64 = 1 << 20;

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
        for frame in 0..f.covered() {
            assert_eq!(f.is_free(frame), expected.contains(&frame), "frame {frame}");
        }
    }

    #[test]
    fn ranges_beyond_the_bits_are_cut_off() {
        let mut bits = [0; 5];
        let f = Frames::new(&mut bits, [(MIB, u64::MAX), (u64::MAX - 5, 10)]);
        assert_eq!(f.free_frames(), 5 * 64 - (MIB / FRAME) as usize);
        assert!(f.is_free(5 * 64 - 1));
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
