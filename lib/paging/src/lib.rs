//! Builds `x86_64` four-level page tables. The loader uses it to make the
//! kernel's address space before it hands over; the kernel will use it for
//! its own. Where the tables live in physical memory, and how to reach
//! them, is up to the caller's [`Frames`].

#![cfg_attr(not(test), no_std)]

use core::ops::BitOr;

pub const SIZE_4K: u64 = 0x1000;
pub const SIZE_2M: u64 = 0x20_0000;
pub const SIZE_1G: u64 = 0x4000_0000;

/// Bits of a page table entry, from the Intel SDM, volume 3, section 4.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags(pub u64);

impl Flags {
    pub const PRESENT: Flags = Flags(1);
    pub const WRITABLE: Flags = Flags(1 << 1);
    pub const USER: Flags = Flags(1 << 2);
    pub const WRITE_THROUGH: Flags = Flags(1 << 3);
    pub const NO_CACHE: Flags = Flags(1 << 4);
    pub const GLOBAL: Flags = Flags(1 << 8);
    pub const NO_EXECUTE: Flags = Flags(1 << 63);

    /// Set on a level 3 or level 2 entry that maps a page itself.
    const HUGE: u64 = 1 << 7;
    const ADDRESS: u64 = 0x000f_ffff_ffff_f000;

    #[must_use]
    pub fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Flags {
    type Output = Flags;

    fn bitor(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSize {
    Size4K,
    Size2M,
    Size1G,
}

impl PageSize {
    #[must_use]
    pub fn bytes(self) -> u64 {
        match self {
            PageSize::Size4K => SIZE_4K,
            PageSize::Size2M => SIZE_2M,
            PageSize::Size1G => SIZE_1G,
        }
    }

    /// The table level whose entry maps a page of this size: 1 for 4 KiB,
    /// 2 for 2 MiB, 3 for 1 GiB.
    fn level(self) -> usize {
        match self {
            PageSize::Size4K => 1,
            PageSize::Size2M => 2,
            PageSize::Size1G => 3,
        }
    }
}

/// Physical memory as the page tables see it.
pub trait Frames {
    /// A fresh, zeroed 4 KiB frame, as its physical address, or `None`
    /// when memory runs out.
    fn allocate(&mut self) -> Option<u64>;

    /// The 512 entries of the table in the frame at `physical`.
    fn table(&mut self, physical: u64) -> &mut [u64; 512];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    OutOfFrames,
    /// Bits 48 to 63 of the address do not all equal bit 47.
    NotCanonical(u64),
    /// An address or length is not a multiple of the page size used.
    Misaligned(u64),
    AlreadyMapped(u64),
    /// A larger page already covers the address.
    HugePageInTheWay(u64),
}

#[must_use]
pub fn is_canonical(virt: u64) -> bool {
    let top = virt >> 47;
    top == 0 || top == 0x1_ffff
}

fn index(virt: u64, level: usize) -> usize {
    ((virt >> (12 + 9 * (level - 1))) & 0x1ff) as usize
}

/// One address space, identified by its level 4 table.
pub struct AddressSpace {
    root: u64,
}

impl AddressSpace {
    /// Starts an empty address space.
    ///
    /// # Errors
    ///
    /// Fails when there is no frame for the level 4 table.
    pub fn new(frames: &mut impl Frames) -> Result<AddressSpace, Error> {
        let root = frames.allocate().ok_or(Error::OutOfFrames)?;
        Ok(AddressSpace { root })
    }

    /// The physical address of the level 4 table, which goes into CR3.
    #[must_use]
    pub fn root(&self) -> u64 {
        self.root
    }

    /// Maps one page. Tables on the way are created as needed; they allow
    /// everything, so the leaf entry alone decides what the page allows.
    ///
    /// # Errors
    ///
    /// Fails on a non-canonical or misaligned address, a page that is
    /// already mapped, a larger page in the way, or missing frames.
    pub fn map(
        &mut self,
        frames: &mut impl Frames,
        virt: u64,
        phys: u64,
        size: PageSize,
        flags: Flags,
    ) -> Result<(), Error> {
        if !is_canonical(virt) {
            return Err(Error::NotCanonical(virt));
        }
        if !virt.is_multiple_of(size.bytes()) {
            return Err(Error::Misaligned(virt));
        }
        if !phys.is_multiple_of(size.bytes()) || phys & !Flags::ADDRESS != 0 {
            return Err(Error::Misaligned(phys));
        }
        let mut table = self.root;
        for level in (size.level() + 1..=4).rev() {
            let i = index(virt, level);
            let entry = frames.table(table)[i];
            if entry & Flags::PRESENT.0 == 0 {
                let next = frames.allocate().ok_or(Error::OutOfFrames)?;
                let through = Flags::PRESENT | Flags::WRITABLE | Flags::USER;
                frames.table(table)[i] = next | through.0;
                table = next;
            } else if entry & Flags::HUGE != 0 {
                return Err(Error::HugePageInTheWay(virt));
            } else {
                table = entry & Flags::ADDRESS;
            }
        }
        let leaf = &mut frames.table(table)[index(virt, size.level())];
        if *leaf & Flags::PRESENT.0 != 0 {
            return Err(Error::AlreadyMapped(virt));
        }
        let huge = if size == PageSize::Size4K {
            0
        } else {
            Flags::HUGE
        };
        *leaf = phys | flags.0 | Flags::PRESENT.0 | huge;
        Ok(())
    }

    /// Maps `len` bytes from `virt` to `phys`, with the largest pages both
    /// addresses allow at each step. 1 GiB pages are only used when
    /// `allow_1g` says the processor has them.
    ///
    /// # Errors
    ///
    /// Fails like [`AddressSpace::map`], and when the range is not made of
    /// whole 4 KiB pages.
    pub fn map_range(
        &mut self,
        frames: &mut impl Frames,
        virt: u64,
        phys: u64,
        len: u64,
        flags: Flags,
        allow_1g: bool,
    ) -> Result<(), Error> {
        if !len.is_multiple_of(SIZE_4K) {
            return Err(Error::Misaligned(len));
        }
        let mut done = 0;
        while done < len {
            let (v, p, left) = (virt + done, phys + done, len - done);
            let fits = |size: u64| v.is_multiple_of(size) && p.is_multiple_of(size) && left >= size;
            let size = if allow_1g && fits(SIZE_1G) {
                PageSize::Size1G
            } else if fits(SIZE_2M) {
                PageSize::Size2M
            } else {
                PageSize::Size4K
            };
            self.map(frames, v, p, size, flags)?;
            done += size.bytes();
        }
        Ok(())
    }

    /// Where `virt` leads: the physical address, the leaf entry's flags
    /// and the size of the page it belongs to.
    #[must_use]
    pub fn translate(&self, frames: &mut impl Frames, virt: u64) -> Option<(u64, Flags, PageSize)> {
        let leaf = |entry: u64, size: PageSize| {
            let base = entry & Flags::ADDRESS & !(size.bytes() - 1);
            let flags = Flags(entry & !Flags::ADDRESS & !Flags::HUGE);
            (base + (virt & (size.bytes() - 1)), flags, size)
        };
        let mut table = self.root;
        for level in [4, 3, 2] {
            let entry = frames.table(table)[index(virt, level)];
            if entry & Flags::PRESENT.0 == 0 {
                return None;
            }
            // The huge bit is reserved at level 4 and means nothing there.
            if entry & Flags::HUGE != 0 && level != 4 {
                let size = if level == 3 {
                    PageSize::Size1G
                } else {
                    PageSize::Size2M
                };
                return Some(leaf(entry, size));
            }
            table = entry & Flags::ADDRESS;
        }
        let entry = frames.table(table)[index(virt, 1)];
        (entry & Flags::PRESENT.0 != 0).then(|| leaf(entry, PageSize::Size4K))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames on the heap, handed out from a made-up physical address so
    /// a test sees real-looking addresses and a mix-up with indices shows.
    struct Heap {
        base: u64,
        tables: Vec<Box<[u64; 512]>>,
        limit: usize,
    }

    impl Heap {
        fn new() -> Heap {
            Heap::with_limit(usize::MAX)
        }

        fn with_limit(limit: usize) -> Heap {
            Heap {
                base: 0x7_3000_0000,
                tables: Vec::new(),
                limit,
            }
        }

        fn used(&self) -> usize {
            self.tables.len()
        }
    }

    impl Frames for Heap {
        fn allocate(&mut self) -> Option<u64> {
            if self.tables.len() == self.limit {
                return None;
            }
            self.tables.push(Box::new([0; 512]));
            Some(self.base + (self.tables.len() as u64 - 1) * SIZE_4K)
        }

        fn table(&mut self, physical: u64) -> &mut [u64; 512] {
            assert_eq!(
                physical % SIZE_4K,
                0,
                "table address {physical:#x} is not page aligned"
            );
            let at = (physical - self.base) / SIZE_4K;
            &mut self.tables[usize::try_from(at).unwrap()]
        }
    }

    const RW: Flags = Flags(Flags::WRITABLE.0 | Flags::NO_EXECUTE.0);
    const KERNEL: u64 = 0xffff_ffff_8000_0000;
    const HHDM: u64 = 0xffff_8000_0000_0000;

    fn space(heap: &mut Heap) -> AddressSpace {
        AddressSpace::new(heap).unwrap()
    }

    #[test]
    fn canonical_addresses() {
        for virt in [0, 0x1000, 0x7fff_ffff_ffff, HHDM, KERNEL, u64::MAX] {
            assert!(is_canonical(virt), "{virt:#x}");
        }
        for virt in [
            0x8000_0000_0000,
            0xffff_7fff_ffff_ffff,
            0x0001_0000_0000_0000,
            0x8000_0000_0000_0000,
        ] {
            assert!(!is_canonical(virt), "{virt:#x}");
        }
    }

    #[test]
    fn index_takes_nine_bits_per_level() {
        let virt = (0x1a3 << 39) | (0x0f2 << 30) | (0x155 << 21) | (0x0aa << 12) | 0x123;
        assert_eq!(index(virt, 4), 0x1a3);
        assert_eq!(index(virt, 3), 0x0f2);
        assert_eq!(index(virt, 2), 0x155);
        assert_eq!(index(virt, 1), 0x0aa);
    }

    #[test]
    fn page_sizes() {
        assert_eq!(PageSize::Size4K.bytes(), 4096);
        assert_eq!(PageSize::Size2M.bytes(), 2 * 1024 * 1024);
        assert_eq!(PageSize::Size1G.bytes(), 1024 * 1024 * 1024);
    }

    #[test]
    fn flags_combine() {
        let both = Flags::WRITABLE | Flags::NO_EXECUTE;
        assert_eq!(both.0, 2 | 1 << 63);
        assert!(both.contains(Flags::WRITABLE));
        assert!(both.contains(Flags::NO_EXECUTE));
        assert!(both.contains(both));
        assert!(!both.contains(Flags::USER));
        assert!(!Flags::WRITABLE.contains(both));
    }

    #[test]
    fn new_space_is_empty() {
        let mut heap = Heap::new();
        let space = space(&mut heap);
        assert_eq!(heap.used(), 1);
        assert_eq!(space.root(), heap.base);
        assert_eq!(space.translate(&mut heap, 0), None);
        assert_eq!(space.translate(&mut heap, KERNEL), None);
        assert!(heap.tables[0].iter().all(|&e| e == 0));
    }

    #[test]
    fn new_space_without_frames() {
        let mut heap = Heap::with_limit(0);
        assert!(matches!(
            AddressSpace::new(&mut heap),
            Err(Error::OutOfFrames)
        ));
    }

    #[test]
    fn maps_a_4k_page() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, KERNEL + 0x5000, 0x20_3000, PageSize::Size4K, RW)
            .unwrap();
        assert_eq!(heap.used(), 4, "one table per level below the root");

        let want = Some((0x20_3000 + 0x7ff, RW | Flags::PRESENT, PageSize::Size4K));
        assert_eq!(space.translate(&mut heap, KERNEL + 0x57ff), want);
        assert_eq!(space.translate(&mut heap, KERNEL + 0x4fff), None);
        assert_eq!(space.translate(&mut heap, KERNEL + 0x6000), None);
    }

    #[test]
    fn leaf_entry_bits() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, 0x40_0000, 0x1234_5000, PageSize::Size4K, RW)
            .unwrap();
        // Root, level 3, level 2, level 1, in the order they were made.
        assert_eq!(heap.tables[3][0], 0x1234_5000 | RW.0 | 1);

        space
            .map(
                &mut heap,
                0x4000_0000,
                0x8000_0000,
                PageSize::Size2M,
                Flags::WRITABLE,
            )
            .unwrap();
        assert_eq!(heap.tables[4][0], 0x8000_0000 | 0b1000_0011);
    }

    #[test]
    fn tables_on_the_way_allow_everything() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, KERNEL, 0x10_0000, PageSize::Size4K, Flags(0))
            .unwrap();
        let through = (Flags::PRESENT | Flags::WRITABLE | Flags::USER).0;
        let l4 = heap.tables[0][511];
        let l3 = heap.tables[1][510];
        let l2 = heap.tables[2][0];
        assert_eq!(l4, (heap.base + SIZE_4K) | through);
        assert_eq!(l3, (heap.base + 2 * SIZE_4K) | through);
        assert_eq!(l2, (heap.base + 3 * SIZE_4K) | through);

        let (_, flags, _) = space.translate(&mut heap, KERNEL).unwrap();
        assert_eq!(
            flags,
            Flags::PRESENT,
            "the leaf keeps only what was asked for"
        );
    }

    #[test]
    fn neighbours_share_tables() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        for page in 0..8 {
            let virt = KERNEL + page * SIZE_4K;
            space
                .map(
                    &mut heap,
                    virt,
                    0x30_0000 + page * SIZE_4K,
                    PageSize::Size4K,
                    RW,
                )
                .unwrap();
        }
        assert_eq!(heap.used(), 4);
        space
            .map(&mut heap, KERNEL + SIZE_2M, 0x50_0000, PageSize::Size4K, RW)
            .unwrap();
        assert_eq!(heap.used(), 5, "a new level 1 table, nothing else");
        space
            .map(&mut heap, 0x1000, 0x1000, PageSize::Size4K, RW)
            .unwrap();
        assert_eq!(heap.used(), 8, "the lower half gets its own tables");

        for page in 0..8 {
            let (phys, ..) = space.translate(&mut heap, KERNEL + page * SIZE_4K).unwrap();
            assert_eq!(phys, 0x30_0000 + page * SIZE_4K);
        }
        assert_eq!(space.translate(&mut heap, 0x1abc).unwrap().0, 0x1abc);
    }

    #[test]
    fn maps_a_2m_page() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, HHDM + SIZE_2M, 6 * SIZE_2M, PageSize::Size2M, RW)
            .unwrap();
        assert_eq!(heap.used(), 3);

        let want = Some((
            6 * SIZE_2M + 0x1f_f123,
            RW | Flags::PRESENT,
            PageSize::Size2M,
        ));
        assert_eq!(space.translate(&mut heap, HHDM + SIZE_2M + 0x1f_f123), want);
        assert_eq!(space.translate(&mut heap, HHDM + SIZE_2M - 1), None);
        assert_eq!(space.translate(&mut heap, HHDM + 2 * SIZE_2M), None);
    }

    #[test]
    fn maps_a_1g_page() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(
                &mut heap,
                HHDM + 3 * SIZE_1G,
                SIZE_1G,
                PageSize::Size1G,
                Flags::NO_EXECUTE,
            )
            .unwrap();
        assert_eq!(heap.used(), 2);

        let want = Some((
            SIZE_1G + 0x3fff_ffff,
            Flags::NO_EXECUTE | Flags::PRESENT,
            PageSize::Size1G,
        ));
        assert_eq!(space.translate(&mut heap, HHDM + 4 * SIZE_1G - 1), want);
        assert_eq!(space.translate(&mut heap, HHDM + 4 * SIZE_1G), None);
    }

    #[test]
    fn rejects_non_canonical() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        let virt = 0x8000_0000_0000;
        assert_eq!(
            space.map(&mut heap, virt, 0, PageSize::Size4K, RW),
            Err(Error::NotCanonical(virt))
        );
        assert_eq!(heap.used(), 1);
    }

    #[test]
    fn rejects_misaligned() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        let cases = [
            (0x1001, 0x1000, PageSize::Size4K, 0x1001),
            (0x1000, 0x1800, PageSize::Size4K, 0x1800),
            (0x1000, 0, PageSize::Size2M, 0x1000),
            (SIZE_2M, 0x1000, PageSize::Size2M, 0x1000),
            (SIZE_2M, 0, PageSize::Size1G, SIZE_2M),
            (SIZE_1G, SIZE_2M, PageSize::Size1G, SIZE_2M),
        ];
        for (virt, phys, size, bad) in cases {
            assert_eq!(
                space.map(&mut heap, virt, phys, size, RW),
                Err(Error::Misaligned(bad)),
                "{virt:#x} {phys:#x}"
            );
        }
        assert_eq!(heap.used(), 1);
    }

    #[test]
    fn rejects_physical_beyond_52_bits() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        let phys = 1 << 52;
        assert_eq!(
            space.map(&mut heap, 0, phys, PageSize::Size4K, RW),
            Err(Error::Misaligned(phys))
        );
        let top = (1 << 52) - SIZE_4K;
        space.map(&mut heap, 0, top, PageSize::Size4K, RW).unwrap();
        assert_eq!(space.translate(&mut heap, 0x10).unwrap().0, top + 0x10);
    }

    #[test]
    fn rejects_double_map() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, 0x5000, 0x9000, PageSize::Size4K, RW)
            .unwrap();
        assert_eq!(
            space.map(&mut heap, 0x5000, 0xa000, PageSize::Size4K, RW),
            Err(Error::AlreadyMapped(0x5000))
        );
        assert_eq!(
            space.translate(&mut heap, 0x5000).unwrap().0,
            0x9000,
            "the first mapping stays"
        );
    }

    #[test]
    fn rejects_large_page_over_small_ones() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, SIZE_2M + 0x3000, 0, PageSize::Size4K, RW)
            .unwrap();
        assert_eq!(
            space.map(&mut heap, SIZE_2M, 0, PageSize::Size2M, RW),
            Err(Error::AlreadyMapped(SIZE_2M))
        );
        assert_eq!(
            space.map(&mut heap, 0, 0, PageSize::Size1G, RW),
            Err(Error::AlreadyMapped(0))
        );
    }

    #[test]
    fn rejects_small_page_inside_large_one() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, SIZE_2M, 0, PageSize::Size2M, RW)
            .unwrap();
        let inside = SIZE_2M + 0x7000;
        assert_eq!(
            space.map(&mut heap, inside, 0, PageSize::Size4K, RW),
            Err(Error::HugePageInTheWay(inside))
        );

        space
            .map(&mut heap, KERNEL & !(SIZE_1G - 1), 0, PageSize::Size1G, RW)
            .unwrap();
        assert_eq!(
            space.map(&mut heap, KERNEL, 0, PageSize::Size2M, RW),
            Err(Error::HugePageInTheWay(KERNEL))
        );
        assert_eq!(
            space.map(&mut heap, KERNEL + 0x1000, 0, PageSize::Size4K, RW),
            Err(Error::HugePageInTheWay(KERNEL + 0x1000))
        );
    }

    #[test]
    fn running_out_of_frames_midway() {
        let mut heap = Heap::with_limit(3);
        let mut space = space(&mut heap);
        assert_eq!(
            space.map(&mut heap, KERNEL, 0, PageSize::Size4K, RW),
            Err(Error::OutOfFrames)
        );
        assert_eq!(space.translate(&mut heap, KERNEL), None);
        space
            .map(&mut heap, KERNEL, 0, PageSize::Size2M, RW)
            .unwrap();
    }

    #[test]
    fn level_4_ignores_the_huge_bit() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, KERNEL, 0x6000, PageSize::Size4K, RW)
            .unwrap();
        heap.tables[0][511] |= Flags::HUGE;
        assert_eq!(
            space.translate(&mut heap, KERNEL).unwrap(),
            (0x6000, RW | Flags::PRESENT, PageSize::Size4K)
        );
    }

    /// Every page `map_range` made, as (virtual, physical, size), found by
    /// walking the range with `translate`.
    fn pages(
        space: &AddressSpace,
        heap: &mut Heap,
        virt: u64,
        len: u64,
    ) -> Vec<(u64, u64, PageSize)> {
        let mut out = Vec::new();
        let mut at = virt;
        while at < virt + len {
            let (phys, _, size) = space.translate(heap, at).unwrap();
            out.push((at, phys, size));
            at += size.bytes();
        }
        out
    }

    #[test]
    fn range_uses_large_pages_where_they_fit() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        let (virt, phys) = (SIZE_1G - 2 * SIZE_4K, 0x8000_0000 - 2 * SIZE_4K);
        let len = 2 * SIZE_4K + SIZE_1G + 2 * SIZE_2M + SIZE_4K;
        space
            .map_range(&mut heap, virt, phys, len, RW, true)
            .unwrap();

        let got = pages(&space, &mut heap, virt, len);
        let mut want = vec![
            (virt, phys, PageSize::Size4K),
            (virt + SIZE_4K, phys + SIZE_4K, PageSize::Size4K),
            (SIZE_1G, 0x8000_0000, PageSize::Size1G),
            (2 * SIZE_1G, 0x8000_0000 + SIZE_1G, PageSize::Size2M),
            (
                2 * SIZE_1G + SIZE_2M,
                0x8000_0000 + SIZE_1G + SIZE_2M,
                PageSize::Size2M,
            ),
        ];
        want.push((
            2 * SIZE_1G + 2 * SIZE_2M,
            0x8000_0000 + SIZE_1G + 2 * SIZE_2M,
            PageSize::Size4K,
        ));
        assert_eq!(got, want);
        assert_eq!(space.translate(&mut heap, virt + len), None);
        assert_eq!(space.translate(&mut heap, virt - 1), None);
    }

    #[test]
    fn range_without_1g_pages() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map_range(&mut heap, HHDM, 0, SIZE_1G, RW, false)
            .unwrap();
        let got = pages(&space, &mut heap, HHDM, SIZE_1G);
        assert_eq!(got.len(), 512);
        assert!(
            got.iter()
                .all(|&(v, p, size)| size == PageSize::Size2M && v - HHDM == p)
        );
    }

    #[test]
    fn range_follows_the_worse_aligned_address() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map_range(&mut heap, SIZE_2M, SIZE_2M + SIZE_4K, SIZE_2M, RW, true)
            .unwrap();
        let got = pages(&space, &mut heap, SIZE_2M, SIZE_2M);
        assert_eq!(got.len(), 512);
        assert!(
            got.iter()
                .all(|&(v, p, size)| size == PageSize::Size4K && p == v + SIZE_4K)
        );

        let virt = 4 * SIZE_2M + SIZE_4K;
        space
            .map_range(&mut heap, virt, 4 * SIZE_2M, SIZE_2M, RW, true)
            .unwrap();
        let got = pages(&space, &mut heap, virt, SIZE_2M);
        assert_eq!(got.len(), 512);
        assert!(
            got.iter()
                .all(|&(v, p, size)| size == PageSize::Size4K && v == p + SIZE_4K)
        );
    }

    #[test]
    fn large_page_attribute_bits_are_not_address() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, 0, 3 * SIZE_2M, PageSize::Size2M, RW)
            .unwrap();
        // Bit 12 of a large page entry picks the memory type, it is no
        // part of the address.
        heap.tables[2][0] |= 1 << 12;
        assert_eq!(
            space.translate(&mut heap, 0x42).unwrap().0,
            3 * SIZE_2M + 0x42
        );
    }

    #[test]
    fn range_stops_large_pages_at_the_end() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map_range(&mut heap, 0, 0, SIZE_2M - SIZE_4K, RW, true)
            .unwrap();
        assert_eq!(pages(&space, &mut heap, 0, SIZE_2M - SIZE_4K).len(), 511);
        assert_eq!(space.translate(&mut heap, SIZE_2M - SIZE_4K), None);
    }

    #[test]
    fn range_must_be_whole_pages() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        assert_eq!(
            space.map_range(&mut heap, 0, 0, 0x1800, RW, true),
            Err(Error::Misaligned(0x1800))
        );
        assert_eq!(heap.used(), 1);
        space.map_range(&mut heap, 0, 0, 0, RW, true).unwrap();
        assert_eq!(heap.used(), 1, "an empty range maps nothing");
    }

    #[test]
    fn range_reports_the_first_failure() {
        let mut heap = Heap::new();
        let mut space = space(&mut heap);
        space
            .map(&mut heap, 0x3000, 0, PageSize::Size4K, RW)
            .unwrap();
        assert_eq!(
            space.map_range(&mut heap, 0x1000, 0x1000, 0x4000, RW, true),
            Err(Error::AlreadyMapped(0x3000))
        );
        assert_eq!(
            space.translate(&mut heap, 0x2000).unwrap().0,
            0x2000,
            "pages before it stay mapped"
        );
        assert_eq!(space.translate(&mut heap, 0x4000), None);
    }
}
