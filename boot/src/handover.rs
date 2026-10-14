//! The kernel's address space, worked out before the jump: what is mapped
//! where, and with which rights. The loader runs it with frames from the
//! firmware; the tests run it with frames on the heap.

use core::mem::{size_of, size_of_val};

use bootinfo::{BootInfo, HHDM_BASE, MemoryRegion, STACK_PAGES, STACK_TOP};
use elf::Segment;
use paging::{AddressSpace, Flags, Frames, SIZE_1G, SIZE_2M, SIZE_4K};
use x86::Features;

/// Everything the address space is built from.
#[derive(Clone, Copy, Debug)]
pub struct Layout<'a> {
    /// The loaded kernel image, as `Elf::span` gives it: the virtual
    /// address it starts at, and where its bytes are in physical memory.
    pub kernel_virtual: u64,
    pub kernel_physical: u64,
    pub segments: &'a [Segment],
    pub stack_physical: u64,
    /// The loader's own image, which has to stay where it is until the
    /// jump is done.
    pub loader: (u64, u64),
    /// The highest physical address the memory map reaches.
    pub memory_top: u64,
    pub features: Features,
}

/// How far the direct map goes: all of memory, and at least the first
/// 4 GiB, where firmware puts devices such as the local APIC. It ends on a
/// 1 GiB boundary.
#[must_use]
pub fn direct_map_end(memory_top: u64) -> u64 {
    memory_top.max(4 * SIZE_1G).div_ceil(SIZE_1G) * SIZE_1G
}

/// Builds the kernel's address space:
///
/// - each kernel segment with the rights it asks for, never writable and
///   executable at once
/// - all physical memory at [`HHDM_BASE`], writable, never executable
/// - the kernel stack below [`STACK_TOP`], with the page under it left
///   unmapped as a guard
/// - the loader's image at its own address, so the jump survives the
///   switch; nothing else of the lower half
///
/// # Errors
///
/// Fails when the tables run out of frames, or when two kernel segments
/// share a page, which the kernel's linker script rules out.
pub fn build(frames: &mut impl Frames, layout: &Layout<'_>) -> Result<AddressSpace, paging::Error> {
    let no_exec = if layout.features.no_execute {
        Flags::NO_EXECUTE
    } else {
        Flags(0)
    };
    let big = layout.features.pages_1g;
    let mut space = AddressSpace::new(frames)?;

    for s in layout.segments {
        let start = s.vaddr & !(SIZE_4K - 1);
        let end = s.end().div_ceil(SIZE_4K) * SIZE_4K;
        let mut flags = Flags::GLOBAL;
        if s.write {
            flags = flags | Flags::WRITABLE | no_exec;
        } else if !s.execute {
            flags = flags | no_exec;
        }
        let physical = layout.kernel_physical + (start - layout.kernel_virtual);
        space.map_range(frames, start, physical, end - start, flags, big)?;
    }

    let end = direct_map_end(layout.memory_top);
    space.map_range(
        frames,
        HHDM_BASE,
        0,
        end,
        Flags::WRITABLE | Flags::GLOBAL | no_exec,
        big,
    )?;

    let stack_size = STACK_PAGES * SIZE_4K;
    space.map_range(
        frames,
        STACK_TOP - stack_size,
        layout.stack_physical,
        stack_size,
        Flags::WRITABLE | no_exec,
        false,
    )?;

    let (base, size) = layout.loader;
    let first = base & !(SIZE_4K - 1);
    let last = (base + size).div_ceil(SIZE_4K) * SIZE_4K;
    space.map_range(frames, first, first, last - first, Flags(0), false)?;
    Ok(space)
}

/// How many page table frames [`build`] can need at most for `layout`:
/// the root, a few tables each for the kernel, its stack and the loader,
/// and the direct map's tables, which without 1 GiB pages are one level 2
/// table per GiB.
#[must_use]
pub fn frames_needed(layout: &Layout<'_>) -> u64 {
    let span = layout
        .segments
        .iter()
        .map(Segment::end)
        .max()
        .map_or(0, |end| end - layout.kernel_virtual);
    let end = direct_map_end(layout.memory_top);
    let direct = if layout.features.pages_1g {
        0
    } else {
        end / SIZE_1G
    };
    16 + span.div_ceil(SIZE_2M)
        + layout.loader.1.div_ceil(SIZE_2M)
        + end.div_ceil(512 * SIZE_1G)
        + direct
}

/// How many pages to set aside for the memory map read just before the
/// jump, when it is `map_size` bytes now: twice that and a page, because
/// everything allocated until then can split descriptors.
#[must_use]
pub fn map_buffer_pages(map_size: usize) -> usize {
    (2 * map_size).div_ceil(4096) + 1
}

/// Frames for page tables, handed out one by one from a block of pages
/// that was allocated, and cleared, at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pool {
    start: u64,
    count: u64,
    used: u64,
}

impl Pool {
    #[must_use]
    pub fn new(start: u64, count: u64) -> Pool {
        Pool {
            start,
            count,
            used: 0,
        }
    }

    /// The next frame, or `None` when all are used.
    pub fn take(&mut self) -> Option<u64> {
        if self.used == self.count {
            return None;
        }
        self.used += 1;
        Some(self.start + (self.used - 1) * SIZE_4K)
    }

    /// Whether `physical` is the start of a frame this pool handed out.
    #[must_use]
    pub fn handed_out(&self, physical: u64) -> bool {
        physical >= self.start
            && physical < self.start + self.used * SIZE_4K
            && (physical - self.start).is_multiple_of(SIZE_4K)
    }
}

/// Whether `entry` lies in a segment the kernel may run code from.
#[must_use]
pub fn entry_is_code(segments: &[Segment], entry: u64) -> bool {
    segments
        .iter()
        .any(|s| s.execute && !s.write && s.vaddr <= entry && entry < s.end())
}

/// Writes the boot information and the regions after it into the pages
/// the kernel finds them in.
///
/// # Panics
///
/// When `page` is too small for both, which the loader rules out by
/// sizing the pages for the most regions it hands over.
pub fn write_info(page: &mut [u8], info: &BootInfo, regions: &[MemoryRegion]) {
    let head = size_of::<BootInfo>();
    let size = size_of::<MemoryRegion>();
    assert!(
        page.len() >= head + size_of_val(regions),
        "the boot information pages are too small for {} regions",
        regions.len()
    );
    page[..head].copy_from_slice(&info.to_bytes());
    for (slot, region) in page[head..].chunks_exact_mut(size).zip(regions) {
        slot.copy_from_slice(&region.to_bytes());
    }
}

/// Why the kernel could not be started, in words for the person in front
/// of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A firmware call failed while doing what `doing` says.
    Firmware {
        doing: &'static str,
        status: uefi::Status,
    },
    Kernel(elf::Error),
    /// The entry point is not in a code segment.
    BadEntry(u64),
    Tables(paging::Error),
    /// The memory map has more regions than the boot information holds.
    Map(crate::memory::TooSmall),
}

impl core::fmt::Display for Problem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Problem::Firmware { doing, status } => write!(f, "{doing} failed: {status}"),
            Problem::Kernel(e) => write!(
                f,
                "the kernel file is not one this loader can start ({e:?})"
            ),
            Problem::BadEntry(at) => {
                write!(f, "the kernel's entry point {at:#x} is not in its code")
            }
            Problem::Tables(e) => write!(f, "the kernel's page tables could not be built ({e:?})"),
            Problem::Map(m) => write!(
                f,
                "the memory map has {} regions, more than the kernel can be handed",
                m.needed
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paging::PageSize;

    struct Heap {
        tables: Vec<Box<[u64; 512]>>,
        limit: usize,
    }

    const HEAP_BASE: u64 = 0x5_0000_0000;

    impl Heap {
        fn new() -> Heap {
            Heap {
                tables: Vec::new(),
                limit: usize::MAX,
            }
        }
    }

    impl Frames for Heap {
        fn allocate(&mut self) -> Option<u64> {
            if self.tables.len() == self.limit {
                return None;
            }
            self.tables.push(Box::new([0; 512]));
            Some(HEAP_BASE + (self.tables.len() as u64 - 1) * SIZE_4K)
        }

        fn table(&mut self, physical: u64) -> &mut [u64; 512] {
            &mut self.tables[usize::try_from((physical - HEAP_BASE) / SIZE_4K).unwrap()]
        }
    }

    const KERNEL: u64 = 0xffff_ffff_8000_0000;
    const KERNEL_PHYS: u64 = 0x20_0000;
    const STACK_PHYS: u64 = 0x30_0000;
    const P: Flags = Flags::PRESENT;
    const G: Flags = Flags::GLOBAL;
    const W: Flags = Flags::WRITABLE;
    const NX: Flags = Flags::NO_EXECUTE;

    fn segment(vaddr: u64, mem_size: u64, write: bool, execute: bool) -> Segment {
        Segment {
            vaddr,
            mem_size,
            file_offset: 0,
            file_size: 0,
            read: true,
            write,
            execute,
        }
    }

    fn kernel_segments() -> [Segment; 3] {
        [
            segment(KERNEL, 0x4a23, false, true),
            segment(KERNEL + 0x5000, 0x19e0, false, false),
            segment(KERNEL + 0x7000, 0x2698, true, false),
        ]
    }

    fn layout(segments: &[Segment], features: Features) -> Layout<'_> {
        Layout {
            kernel_virtual: KERNEL,
            kernel_physical: KERNEL_PHYS,
            segments,
            stack_physical: STACK_PHYS,
            loader: (0x7e00_0123, 0x2_0000),
            memory_top: 512 << 20,
            features,
        }
    }

    const ALL: Features = Features {
        no_execute: true,
        pages_1g: true,
    };

    fn built(features: Features) -> (Heap, AddressSpace) {
        let mut heap = Heap::new();
        let segments = kernel_segments();
        let space = build(&mut heap, &layout(&segments, features)).unwrap();
        (heap, space)
    }

    fn at(heap: &mut Heap, space: &AddressSpace, virt: u64) -> Option<(u64, Flags, PageSize)> {
        space.translate(heap, virt)
    }

    #[test]
    fn the_direct_map_covers_memory_and_the_first_4_gib() {
        assert_eq!(direct_map_end(0), 4 * SIZE_1G);
        assert_eq!(direct_map_end(512 << 20), 4 * SIZE_1G);
        assert_eq!(direct_map_end(4 * SIZE_1G), 4 * SIZE_1G);
        assert_eq!(direct_map_end(4 * SIZE_1G + 1), 5 * SIZE_1G);
        assert_eq!(direct_map_end(37 * SIZE_1G - 4096), 37 * SIZE_1G);
    }

    #[test]
    fn kernel_segments_get_their_own_rights() {
        let (mut heap, space) = built(ALL);
        let code = at(&mut heap, &space, KERNEL + 0x1234);
        assert_eq!(code, Some((KERNEL_PHYS + 0x1234, P | G, PageSize::Size4K)));
        let end_of_code = at(&mut heap, &space, KERNEL + 0x4fff);
        assert_eq!(
            end_of_code.map(|t| t.1),
            Some(P | G),
            "the last code page is whole"
        );
        let rodata = at(&mut heap, &space, KERNEL + 0x5010);
        assert_eq!(
            rodata,
            Some((KERNEL_PHYS + 0x5010, P | G | NX, PageSize::Size4K))
        );
        let bss = at(&mut heap, &space, KERNEL + 0x9697);
        assert_eq!(
            bss,
            Some((KERNEL_PHYS + 0x9697, P | G | W | NX, PageSize::Size4K))
        );
        assert_eq!(at(&mut heap, &space, KERNEL + 0xa000), None);
        assert_eq!(at(&mut heap, &space, KERNEL - 1), None);
    }

    #[test]
    fn nothing_is_writable_and_executable() {
        let mut heap = Heap::new();
        let both = [segment(KERNEL, 0x1000, true, true)];
        let space = build(&mut heap, &layout(&both, ALL)).unwrap();
        assert_eq!(
            at(&mut heap, &space, KERNEL).map(|t| t.1),
            Some(P | G | W | NX)
        );
    }

    #[test]
    fn all_memory_is_in_the_direct_map() {
        let (mut heap, space) = built(ALL);
        let rw = P | G | W | NX;
        assert_eq!(
            at(&mut heap, &space, HHDM_BASE),
            Some((0, rw, PageSize::Size1G))
        );
        let deep = at(&mut heap, &space, HHDM_BASE + 0x1234_5678);
        assert_eq!(deep, Some((0x1234_5678, rw, PageSize::Size1G)));
        let last = at(&mut heap, &space, HHDM_BASE + 4 * SIZE_1G - 1);
        assert_eq!(last.map(|t| t.0), Some(4 * SIZE_1G - 1));
        assert_eq!(at(&mut heap, &space, HHDM_BASE + 4 * SIZE_1G), None);
    }

    #[test]
    fn the_stack_has_a_guard_below() {
        let (mut heap, space) = built(ALL);
        let size = STACK_PAGES * SIZE_4K;
        let top = at(&mut heap, &space, STACK_TOP - 1);
        assert_eq!(
            top,
            Some((STACK_PHYS + size - 1, P | W | NX, PageSize::Size4K))
        );
        let bottom = at(&mut heap, &space, STACK_TOP - size);
        assert_eq!(bottom.map(|t| t.0), Some(STACK_PHYS));
        assert_eq!(
            at(&mut heap, &space, STACK_TOP - size - 1),
            None,
            "the guard page"
        );
        assert_eq!(at(&mut heap, &space, STACK_TOP), None);
    }

    #[test]
    fn only_the_loader_stays_in_the_lower_half() {
        let (mut heap, space) = built(ALL);
        let first = at(&mut heap, &space, 0x7e00_0000);
        assert_eq!(
            first,
            Some((0x7e00_0000, P, PageSize::Size4K)),
            "page aligned, read and run only"
        );
        let last = at(&mut heap, &space, 0x7e00_0123 + 0x2_0000 - 1);
        assert_eq!(last.map(|t| t.0), Some(0x7e02_0122));
        assert_eq!(at(&mut heap, &space, 0x7e02_1000), None);
        assert_eq!(at(&mut heap, &space, 0x7dff_ffff), None);
        assert_eq!(at(&mut heap, &space, 0), None, "no mapping at null");
    }

    #[test]
    fn without_no_execute_no_entry_uses_the_bit() {
        let features = Features {
            no_execute: false,
            pages_1g: true,
        };
        let (mut heap, space) = built(features);
        for virt in [
            KERNEL,
            KERNEL + 0x5000,
            KERNEL + 0x7000,
            HHDM_BASE,
            STACK_TOP - 1,
            0x7e00_0000,
        ] {
            let (_, flags, _) = at(&mut heap, &space, virt).unwrap();
            assert!(!flags.contains(NX), "{virt:#x}");
        }
        assert_eq!(
            at(&mut heap, &space, KERNEL + 0x7000).map(|t| t.1),
            Some(P | G | W)
        );
    }

    #[test]
    fn without_1g_pages_the_direct_map_uses_2m() {
        let features = Features {
            no_execute: true,
            pages_1g: false,
        };
        let (mut heap, space) = built(features);
        let page = at(&mut heap, &space, HHDM_BASE + SIZE_1G + 5);
        assert_eq!(
            page.map(|t| (t.0, t.2)),
            Some((SIZE_1G + 5, PageSize::Size2M))
        );
        let next = at(&mut heap, &space, HHDM_BASE + SIZE_1G + SIZE_2M);
        assert_eq!(
            next.map(|t| (t.0, t.2)),
            Some((SIZE_1G + SIZE_2M, PageSize::Size2M))
        );
    }

    #[test]
    fn segments_sharing_a_page_are_refused() {
        let mut heap = Heap::new();
        let shared = [
            segment(KERNEL, 0x800, false, true),
            segment(KERNEL + 0x800, 0x800, false, false),
        ];
        let result = build(&mut heap, &layout(&shared, ALL));
        assert_eq!(result.err(), Some(paging::Error::AlreadyMapped(KERNEL)));
    }

    #[test]
    fn running_out_of_frames_is_an_error() {
        let segments = kernel_segments();
        for limit in [0, 1, 4, 6] {
            let mut heap = Heap {
                limit,
                ..Heap::new()
            };
            let result = build(&mut heap, &layout(&segments, ALL));
            assert_eq!(
                result.err(),
                Some(paging::Error::OutOfFrames),
                "{limit} frames"
            );
        }
    }

    #[test]
    fn the_frame_estimate_is_always_enough() {
        let big_kernel = [segment(KERNEL, 9 * SIZE_2M + 0x123, true, false)];
        let small = kernel_segments();
        for segments in [&small[..], &big_kernel[..]] {
            for pages_1g in [true, false] {
                for memory_top in [0, 512 << 20, 37 * SIZE_1G, 1100 * SIZE_1G] {
                    let mut l = layout(
                        segments,
                        Features {
                            no_execute: true,
                            pages_1g,
                        },
                    );
                    l.memory_top = memory_top;
                    let needed = frames_needed(&l);
                    let mut heap = Heap {
                        limit: usize::try_from(needed).unwrap(),
                        ..Heap::new()
                    };
                    assert!(
                        build(&mut heap, &l).is_ok(),
                        "{needed} frames, 1 GiB {pages_1g}, top {memory_top:#x}"
                    );
                    let used = heap.tables.len() as u64;
                    assert!(
                        used + 32 >= needed,
                        "{used} used, {needed} estimated, 1 GiB {pages_1g}, top {memory_top:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_estimate_grows_with_what_it_maps() {
        let small = kernel_segments();
        let base = frames_needed(&layout(&small, ALL));
        let big_kernel = [segment(KERNEL, 3 * SIZE_2M, true, false)];
        assert_eq!(frames_needed(&layout(&big_kernel, ALL)), base + 2);
        let mut big_loader = layout(&small, ALL);
        big_loader.loader.1 = 3 * SIZE_2M;
        assert_eq!(frames_needed(&big_loader), base + 2);
        let no_1g = Features {
            no_execute: true,
            pages_1g: false,
        };
        assert_eq!(frames_needed(&layout(&small, no_1g)), base + 4);
        let mut huge = layout(&small, ALL);
        huge.memory_top = 1100 * SIZE_1G;
        assert_eq!(frames_needed(&huge), base + 2);
    }

    #[test]
    fn the_map_buffer_leaves_room_to_grow() {
        assert_eq!(map_buffer_pages(0), 1);
        assert_eq!(map_buffer_pages(2048), 2);
        assert_eq!(map_buffer_pages(2049), 3);
        for size in [1, 48, 4096, 10_000, 65_536, 1 << 20] {
            let room = map_buffer_pages(size) * 4096;
            assert!(room >= 2 * size + 4096 && room < 2 * size + 8192, "{size}");
        }
    }

    #[test]
    fn the_pool_hands_out_each_frame_once() {
        let mut pool = Pool::new(0x40_0000, 3);
        assert!(!pool.handed_out(0x40_0000), "nothing handed out yet");
        assert_eq!(pool.take(), Some(0x40_0000));
        assert_eq!(pool.take(), Some(0x40_1000));
        assert!(pool.handed_out(0x40_0000) && pool.handed_out(0x40_1000));
        assert!(!pool.handed_out(0x40_2000), "not yet handed out");
        assert_eq!(pool.take(), Some(0x40_2000));
        assert_eq!(pool.take(), None);
        assert_eq!(pool.take(), None);
        assert!(pool.handed_out(0x40_2000));
        assert!(!pool.handed_out(0x40_3000));
        assert!(!pool.handed_out(0x3f_f000));
        assert!(!pool.handed_out(0x40_0800), "not the start of a frame");
        assert_eq!(Pool::new(0x1000, 0).take(), None);
    }

    #[test]
    fn the_entry_has_to_be_in_code() {
        let segments = kernel_segments();
        assert!(entry_is_code(&segments, KERNEL));
        assert!(entry_is_code(&segments, KERNEL + 0x4a22));
        assert!(!entry_is_code(&segments, KERNEL + 0x4a23), "past the code");
        assert!(!entry_is_code(&segments, KERNEL - 1));
        assert!(!entry_is_code(&segments, KERNEL + 0x5000), "read-only data");
        assert!(!entry_is_code(&segments, KERNEL + 0x7000), "writable data");
        let both = [segment(KERNEL, 0x1000, true, true)];
        assert!(
            !entry_is_code(&both, KERNEL),
            "writable code does not count"
        );
        assert!(!entry_is_code(&[], KERNEL));
    }

    #[test]
    fn info_and_regions_land_where_the_kernel_reads_them() {
        let info = BootInfo {
            rsdp: 0xe_0000,
            ..BootInfo::new()
        };
        let regions = [
            MemoryRegion {
                start: 0x1000,
                pages: 2,
                kind: bootinfo::RegionKind::USABLE,
                reserved: 0,
            },
            MemoryRegion {
                start: 0x10_0000,
                pages: 7,
                kind: bootinfo::RegionKind::KERNEL,
                reserved: 0,
            },
        ];
        let mut page = vec![0xaa; 4096];
        write_info(&mut page, &info, &regions);
        assert_eq!(&page[..128], &info.to_bytes());
        assert_eq!(&page[128..152], &regions[0].to_bytes());
        assert_eq!(&page[152..176], &regions[1].to_bytes());
        assert!(
            page[176..].iter().all(|&b| b == 0xaa),
            "nothing past the last region"
        );
    }

    #[test]
    #[should_panic(expected = "too small for 2 regions")]
    fn too_small_pages_are_caught() {
        let region = MemoryRegion {
            start: 0,
            pages: 1,
            kind: bootinfo::RegionKind::USABLE,
            reserved: 0,
        };
        let mut page = vec![0; 128 + 24 + 10];
        write_info(&mut page, &BootInfo::new(), &[region, region]);
    }

    #[test]
    fn problems_read_as_sentences() {
        let cases = [
            (
                Problem::Firmware {
                    doing: "opening the kernel file",
                    status: uefi::Status::NOT_FOUND,
                },
                format!(
                    "opening the kernel file failed: {}",
                    uefi::Status::NOT_FOUND
                ),
            ),
            (
                Problem::Kernel(elf::Error::NotElf),
                "the kernel file is not one this loader can start (NotElf)".to_string(),
            ),
            (
                Problem::BadEntry(0x1234),
                "the kernel's entry point 0x1234 is not in its code".to_string(),
            ),
            (
                Problem::Tables(paging::Error::OutOfFrames),
                "the kernel's page tables could not be built (OutOfFrames)".to_string(),
            ),
            (
                Problem::Map(crate::memory::TooSmall { needed: 900 }),
                "the memory map has 900 regions, more than the kernel can be handed".to_string(),
            ),
        ];
        for (problem, want) in cases {
            assert_eq!(problem.to_string(), want);
        }
    }
}
