//! The kernel's heap: a region of memory in blocks of 16 bytes, one bit
//! each in a [`Bitmap`], handed out as offsets into the region. Nothing
//! here touches the memory itself, so the kernel's allocator only turns
//! offsets into addresses.

#![cfg_attr(not(test), no_std)]

use frames::{Bitmap, GiveBack};

/// The size of a block, and the alignment every allocation has at least.
pub const BLOCK: usize = 16;

/// The largest alignment the heap gives, since its region starts on a
/// boundary of this size.
pub const MAX_ALIGN: usize = 2 << 20;

/// Why memory could not be given back to the heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeError {
    /// The offset is not the start of a block.
    Misaligned(usize),
    /// The memory lies beyond the heap's region.
    OutOfRange(usize),
    /// A block of it is free already.
    NotTaken(usize),
}

/// Which blocks of the heap's region are free.
pub struct Heap<'a> {
    blocks: Bitmap<'a>,
}

impl<'a> Heap<'a> {
    /// How many words of bits a region of `size` bytes needs.
    #[must_use]
    pub fn words_for(size: usize) -> usize {
        size.div_ceil(BLOCK).div_ceil(64)
    }

    /// A heap over the whole blocks of `size` bytes, all free, its bits
    /// kept in `bits`.
    pub fn new(bits: &'a mut [u64], size: usize) -> Heap<'a> {
        let mut blocks = Bitmap::taken(bits);
        blocks.release(0..size / BLOCK);
        Heap { blocks }
    }

    /// How many bytes are free.
    #[must_use]
    pub fn free_bytes(&self) -> usize {
        self.blocks.free() * BLOCK
    }

    /// The offset of room for `size` bytes on a multiple of `align`, now
    /// taken, or `None` when there is none or `align` is not a power of
    /// two up to [`MAX_ALIGN`]. Even 0 bytes take a block, so each
    /// allocation has an offset of its own.
    pub fn alloc(&mut self, size: usize, align: usize) -> Option<usize> {
        if !align.is_power_of_two() || align > MAX_ALIGN {
            return None;
        }
        let first = self.blocks.take_run(blocks(size), align.div_ceil(BLOCK))?;
        Some(first * BLOCK)
    }

    /// Gives back the `size` bytes [`Heap::alloc`] handed out at `offset`.
    ///
    /// # Errors
    ///
    /// When `offset` is not the start of a block, or the memory reaches
    /// beyond the region or was free already; then nothing is freed.
    pub fn free(&mut self, offset: usize, size: usize) -> Result<(), FreeError> {
        if !offset.is_multiple_of(BLOCK) {
            return Err(FreeError::Misaligned(offset));
        }
        self.blocks
            .give_back(offset / BLOCK, blocks(size))
            .map_err(|e| match e {
                GiveBack::OutOfRange(_) => FreeError::OutOfRange(offset),
                GiveBack::NotTaken(block) => FreeError::NotTaken(block * BLOCK),
            })
    }
}

/// The blocks `size` bytes take, at least one.
fn blocks(size: usize) -> usize {
    size.max(1).div_ceil(BLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_bits_follow_the_size() {
        assert_eq!(Heap::words_for(0), 0);
        assert_eq!(Heap::words_for(1), 1);
        assert_eq!(Heap::words_for(64 * BLOCK), 1);
        assert_eq!(Heap::words_for(64 * BLOCK + 1), 2);
        assert_eq!(Heap::words_for(4 << 20), 4096);
    }

    #[test]
    fn only_whole_blocks_are_free() {
        let mut bits = [0; 2];
        let h = Heap::new(&mut bits, 100 * BLOCK + 15);
        assert_eq!(h.free_bytes(), 100 * BLOCK);
        let mut bits = [0; 1];
        assert_eq!(
            Heap::new(&mut bits, 1 << 20).free_bytes(),
            64 * BLOCK,
            "as far as the bits reach"
        );
    }

    #[test]
    fn allocations_take_whole_blocks_on_their_alignment() {
        let mut bits = [0; 4];
        let mut h = Heap::new(&mut bits, 256 * BLOCK);
        assert_eq!(h.alloc(1, 1), Some(0));
        assert_eq!(h.alloc(0, 8), Some(BLOCK), "nothing still takes a block");
        assert_eq!(h.alloc(17, 16), Some(2 * BLOCK), "two blocks");
        assert_eq!(h.alloc(16, 64), Some(64), "the next multiple of 64");
        assert_eq!(h.alloc(8, 8), Some(5 * BLOCK));
        assert_eq!(h.free_bytes(), (256 - 6) * BLOCK);
        assert_eq!(h.alloc(4096, 4096), None, "no second 4096 boundary");
        assert_eq!(h.alloc(1024, 1024), Some(1024));
        assert_eq!(h.alloc(8, 24), None, "not a power of two");
        assert_eq!(h.alloc(8, 0), None);
        assert_eq!(
            h.alloc(8, MAX_ALIGN * 2),
            None,
            "more than the region's own"
        );
        assert_eq!(h.alloc(256 * BLOCK, 16), None, "more than is free");
    }

    #[test]
    fn the_regions_own_alignment_is_the_largest() {
        let mut bits = [0; 1];
        let mut h = Heap::new(&mut bits, 64 * BLOCK);
        assert_eq!(h.alloc(1, MAX_ALIGN), Some(0), "the start of the region");
        assert_eq!(
            h.alloc(1, MAX_ALIGN),
            None,
            "the next boundary is past the end"
        );
        assert_eq!(h.alloc(1, MAX_ALIGN * 2), None);
    }

    #[test]
    fn freeing_gives_the_blocks_back() {
        let mut bits = [0; 1];
        let mut h = Heap::new(&mut bits, 64 * BLOCK);
        let a = h.alloc(40, 16).unwrap();
        let b = h.alloc(16, 16).unwrap();
        assert_eq!(h.free(a + 8, 40), Err(FreeError::Misaligned(a + 8)));
        assert_eq!(h.free(b, 64 * BLOCK), Err(FreeError::OutOfRange(b)));
        assert_eq!(
            h.free(a, 80),
            Err(FreeError::NotTaken(b + BLOCK)),
            "a's blocks, b's and the free one after it"
        );
        assert_eq!(h.free_bytes(), 60 * BLOCK, "and nothing was freed");
        assert_eq!(h.free(a, 40), Ok(()));
        assert_eq!(h.free(a, 40), Err(FreeError::NotTaken(a)), "twice");
        assert_eq!(
            h.free(b, 0),
            Ok(()),
            "a zero-sized allocation has its block"
        );
        assert_eq!(h.free_bytes(), 64 * BLOCK);
        assert_eq!(h.alloc(64 * BLOCK, 16), Some(0), "all of it again");
    }

    /// Random allocations and frees from a fixed seed: no two overlap,
    /// each keeps its alignment, and freeing everything frees it all.
    #[test]
    fn many_allocations_never_overlap() {
        let size = 1 << 16;
        let mut bits = vec![0; Heap::words_for(size)];
        let mut h = Heap::new(&mut bits, size);
        let mut live: BTreeMap<usize, usize> = BTreeMap::new();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state >> 32).unwrap()
        };
        for _ in 0..20_000 {
            if random() % 3 != 0 || live.is_empty() {
                let len = random() % 300;
                let align = 1 << (random() % 8);
                if let Some(at) = h.alloc(len, align) {
                    assert!(at.is_multiple_of(align) && at + len <= size);
                    let end = at + blocks(len) * BLOCK;
                    if let Some((&before, &l)) = live.range(..end).next_back() {
                        assert!(before + blocks(l) * BLOCK <= at, "{at} overlaps {before}");
                    }
                    live.insert(at, len);
                }
            } else {
                let &at = live.keys().nth(random() % live.len()).unwrap();
                let len = live.remove(&at).unwrap();
                h.free(at, len).unwrap();
            }
            let taken: usize = live.values().map(|&l| blocks(l) * BLOCK).sum();
            assert_eq!(h.free_bytes() + taken, size);
        }
        for (at, len) in live {
            h.free(at, len).unwrap();
        }
        assert_eq!(h.free_bytes(), size);
    }
}
