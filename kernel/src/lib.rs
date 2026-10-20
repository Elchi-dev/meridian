//! The parts of the kernel that are plain computation, so they are tested
//! on the build machine. `main.rs` runs them on the real thing.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub use canvas::Text;

pub mod clock;
pub mod console;
pub mod disk;
pub mod display;
pub mod exceptions;
pub mod hpet;
pub mod log;
pub mod machine;
pub mod object;
pub mod panic;
pub mod process;
pub mod program;
pub mod screen;
pub use abi as syscall;
pub mod thread;
pub mod vfs;

use core::fmt::{self, Write};

use bootinfo::{BootInfo, MemoryRegion, RegionKind};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How many bytes of memory are of `kind`.
#[must_use]
pub fn bytes_of(regions: &[MemoryRegion], kind: RegionKind) -> u64 {
    regions
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.pages * MemoryRegion::PAGE_SIZE)
        .sum()
}

/// Whether the regions are sorted by address and none overlaps the next.
#[must_use]
pub fn is_tidy(regions: &[MemoryRegion]) -> bool {
    regions.windows(2).all(|w| w[0].end() <= w[1].start)
}

/// Whether one region of `kind` holds all of `len` bytes from `start`.
/// The loader merges neighbours of the same kind, so one is enough.
#[must_use]
pub fn covered(regions: &[MemoryRegion], kind: RegionKind, start: u64, len: u64) -> bool {
    regions
        .iter()
        .any(|r| r.kind == kind && r.start <= start && start.saturating_add(len) <= r.end())
}

/// How many pixels the room the loader set aside for drawing off the
/// screen holds: one frame of the framebuffer's size, when the loader gave
/// it and it lies in the kernel's own memory.
#[must_use]
pub fn back_buffer_pixels(info: &BootInfo, regions: &[MemoryRegion]) -> Option<usize> {
    let fb = &info.framebuffer;
    let pixels = u64::from(fb.width) * u64::from(fb.height);
    let bytes = pixels.checked_mul(4)?;
    let usable = info.back_buffer != 0
        && pixels > 0
        && covered(regions, RegionKind::KERNEL, info.back_buffer, bytes);
    usable.then(|| usize::try_from(pixels).ok()).flatten()
}

/// Whether the kernel may read `len` bytes from physical `start` through
/// the direct map: they lie in one region of the map, of any kind, and
/// below the end of memory or 4 GiB, which the loader always maps.
#[must_use]
pub fn readable(regions: &[MemoryRegion], start: u64, len: u64) -> bool {
    let Some(end) = start.checked_add(len) else {
        return false;
    };
    end <= ram_top(regions).max(4 << 30)
        && regions.iter().any(|r| r.start <= start && end <= r.end())
}

/// Where memory ends: the end of the last region that is not reserved.
/// Reserved regions above it are devices, such as PCI windows, and can be
/// far larger than all memory together.
#[must_use]
pub fn ram_top(regions: &[MemoryRegion]) -> u64 {
    regions
        .iter()
        .filter(|r| r.kind != RegionKind::RESERVED)
        .map(MemoryRegion::end)
        .max()
        .unwrap_or(0)
}

/// The regions that start below [`ram_top`]: memory, and the firmware's
/// pieces in between, without the devices above it.
#[must_use]
pub fn memory(regions: &[MemoryRegion]) -> &[MemoryRegion] {
    let top = ram_top(regions);
    let count = regions.iter().take_while(|r| r.start < top).count();
    &regions[..count]
}

/// The kernel's heap: 4 MiB of frames on a 2 MiB boundary, the largest
/// alignment it gives.
pub const HEAP_BYTES: usize = 4 << 20;

/// The free memory of the map, as start and length, for the frame
/// allocator.
pub fn free_ranges(regions: &[MemoryRegion]) -> impl Iterator<Item = (u64, u64)> + '_ {
    regions
        .iter()
        .filter(|r| r.kind == RegionKind::USABLE)
        .map(|r| (r.start, r.pages * MemoryRegion::PAGE_SIZE))
}

/// Where the frame allocator keeps its `bytes` of bits: the start of the
/// first free region above the first MiB with room for them.
#[must_use]
pub fn bits_home(regions: &[MemoryRegion], bytes: u64) -> Option<u64> {
    free_ranges(regions).find_map(|(start, len)| {
        let first = start.max(frames::LOW);
        (first.saturating_add(bytes) <= start + len).then_some(first)
    })
}

/// The pages the code that starts the other processors needs: its code
/// and data, and three page tables.
pub const TRAMPOLINE_PAGES: u64 = 4;

/// Where that code goes: the first free memory below the legacy video
/// memory at 640 KiB with room for it, past page 0, which a processor
/// cannot start at. The frame allocator never hands out memory there.
#[must_use]
pub fn trampoline_home(regions: &[MemoryRegion]) -> Option<u64> {
    const LEGACY: u64 = 0xa_0000;
    free_ranges(regions).find_map(|(start, len)| {
        let first = start
            .max(MemoryRegion::PAGE_SIZE)
            .next_multiple_of(MemoryRegion::PAGE_SIZE);
        let end = first + TRAMPOLINE_PAGES * MemoryRegion::PAGE_SIZE;
        (end <= (start + len).min(LEGACY)).then_some(first)
    })
}

/// How many frames the allocator starts with: every whole frame of free
/// memory above the first MiB.
#[must_use]
pub fn usable_frames(regions: &[MemoryRegion]) -> u64 {
    free_ranges(regions)
        .map(|(start, len)| {
            let first = start.max(frames::LOW).div_ceil(frames::FRAME);
            ((start + len) / frames::FRAME).saturating_sub(first)
        })
        .sum()
}

/// A number of bytes, written in the largest binary unit it reaches, with
/// one decimal when that is not a whole number: `1.5 GiB`, `512 MiB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size(pub u64);

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
        let (mut whole, mut rest, mut unit) = (self.0, 0, 0);
        while whole >= 1024 && unit + 1 < UNITS.len() {
            rest = whole % 1024;
            whole /= 1024;
            unit += 1;
        }
        let tenth = rest * 10 / 1024;
        if tenth == 0 {
            write!(f, "{whole} {}", UNITS[unit])
        } else {
            write!(f, "{whole}.{tenth} {}", UNITS[unit])
        }
    }
}

/// The first lines the kernel prints.
///
/// # Errors
///
/// Fails when the writer does.
pub fn greet(out: &mut impl Write, regions: &[MemoryRegion]) -> fmt::Result {
    writeln!(out, "Meridian kernel {VERSION}")?;
    writeln!(
        out,
        "Memory: {} free, {} taken by the kernel, {} regions",
        Size(bytes_of(regions, RegionKind::USABLE)),
        Size(bytes_of(regions, RegionKind::KERNEL)),
        regions.len()
    )
}

/// What the kernel has to hand out after taking its heap: `frames` free
/// frames, or nothing when there was no room.
///
/// # Errors
///
/// Those of `out`.
pub fn hand_out(out: &mut impl Write, frames: Option<usize>) -> fmt::Result {
    match frames {
        Some(n) => writeln!(
            out,
            "Memory to hand out: {} in frames, and a heap of {}",
            Size(n as u64 * frames::FRAME),
            Size(HEAP_BYTES as u64)
        ),
        None => writeln!(
            out,
            "kernel: there is no room for the frame allocator or the heap"
        ),
    }
}

/// How the kernel waits for time to pass: woken by a timer `period`
/// counts long, a hundred times a second, or spinning without one.
///
/// # Errors
///
/// Those of `out`.
pub fn waiting(out: &mut impl Write, period: Option<u32>) -> fmt::Result {
    match period {
        Some(p) => writeln!(
            out,
            "Timer: 100 interrupts a second, {p} APIC counts apart; the processor sleeps in between"
        ),
        None => writeln!(out, "Timer: none, so the uptime is kept by spinning"),
    }
}

/// Whether core `number` gets a task state of its own among `cores`: the
/// first core has the one the kernel starts with, and numbers past the
/// last have none.
#[must_use]
pub fn has_task_state(number: usize, cores: usize) -> bool {
    (1..cores).contains(&number)
}

/// How many cores run the kernel when `others` came in besides this one.
#[must_use]
pub fn with_this_one(others: u32) -> u32 {
    others.saturating_add(1)
}

/// How many processors run the kernel: this one and `others`, of the
/// `cores` the MADT lists.
///
/// # Errors
///
/// Those of `out`.
pub fn running(out: &mut impl Write, cores: Option<u32>, others: u32) -> fmt::Result {
    match cores {
        Some(n) => writeln!(out, "Cores running: {} of {n}", with_this_one(others)),
        None => writeln!(out, "Cores running: 1, as the MADT is missing"),
    }
}

/// What the kernel learned about the processor, for the serial log.
///
/// # Errors
///
/// Fails when the writer does.
pub fn describe(
    out: &mut impl Write,
    machine: &machine::Machine,
    tsc: Option<machine::Measured>,
) -> fmt::Result {
    match machine.cores {
        Some(n) => writeln!(out, "Cores: {n}")?,
        None => writeln!(out, "Cores: not listed in the ACPI tables")?,
    }
    match tsc {
        Some(t) => writeln!(
            out,
            "Time stamp counter: {} MHz, measured against {}",
            t.hz / 1_000_000,
            t.against
        ),
        None => writeln!(
            out,
            "Time stamp counter: not measured, neither the ACPI timer nor the HPET could be used"
        ),
    }
}

/// Writes the line the kernel logs for the PCI function at `at`: where it
/// is, who made it and what it is.
///
/// # Errors
///
/// When `out` fails.
pub fn pci_function(out: &mut impl Write, at: pci::Address, h: &pci::Header) -> fmt::Result {
    writeln!(
        out,
        "PCI {:02x}:{:02x}.{} {:04x}:{:04x} {}",
        at.bus,
        at.device,
        at.function,
        h.vendor,
        h.device,
        h.kind()
    )
}

/// Writes the line the kernel logs for the drive on AHCI port `port`: a
/// disk's model and size, or what else is attached.
///
/// # Errors
///
/// When `out` fails.
pub fn drive_line(
    out: &mut impl Write,
    port: usize,
    attached: ahci::Attached,
    identity: Option<&ahci::Identity>,
) -> fmt::Result {
    write!(out, "AHCI port {port}: ")?;
    match (attached, identity) {
        (ahci::Attached::Disk, Some(disk)) => writeln!(
            out,
            "{}, {} sectors of {} bytes",
            disk.model(),
            disk.sectors,
            disk.sector_size
        ),
        (ahci::Attached::Disk, None) => writeln!(out, "a disk that did not say what it is"),
        (ahci::Attached::Optical, _) => writeln!(out, "an optical drive"),
        (ahci::Attached::Other(signature), _) => {
            writeln!(out, "a device with signature {signature:#010x}")
        }
        (ahci::Attached::Nothing, _) => writeln!(out, "nothing"),
    }
}

/// Writes the line the kernel logs for partition `number` of the disk
/// `disk`, such as "AHCI port 0": its name, size, where it starts and its
/// type.
///
/// # Errors
///
/// When `out` fails.
pub fn partition_line(
    out: &mut impl Write,
    disk: &dyn fmt::Display,
    number: usize,
    p: &gpt::Partition,
) -> fmt::Result {
    write!(out, "{disk}, partition {number}: ")?;
    for c in p.name() {
        out.write_char(c)?;
    }
    writeln!(
        out,
        ", {} MiB from sector {}, type {}",
        (p.sectors() * 512) >> 20,
        p.first,
        p.kind
    )
}

/// Writes the line the kernel logs for an NVM Express controller and its
/// first namespace.
///
/// # Errors
///
/// When `out` fails.
pub fn nvme_line(
    out: &mut impl Write,
    controller: &nvme::Controller,
    namespace: &nvme::Namespace,
) -> fmt::Result {
    writeln!(
        out,
        "NVMe: {}, serial {}, {} blocks of {} bytes",
        controller.model(),
        controller.serial(),
        namespace.blocks,
        namespace.block_size
    )
}

/// The most cores that get a task state, and so are started.
pub const CORES: usize = 64;

/// Where the stacks of the other processors lie. Each stack gets 32 KiB
/// of address space, of which only the top 16 KiB are mapped, so a stack
/// that runs over faults instead of writing into the next one.
pub const CORE_STACKS: u64 = 0xffff_fe00_0000_0000;

/// The pages of each of those stacks.
pub const CORE_STACK_PAGES: u64 = 4;

/// Where an AHCI controller's registers are mapped, uncached: the two
/// pages 2 MiB into the slot for devices' registers, room for 32 ports.
pub const AHCI_REGISTERS: u64 = APIC_PAGE + (2 << 20);

/// Where an NVM Express controller's registers are mapped, uncached: the
/// two pages 4 MiB into the slot for devices' registers, room for the
/// doorbells of the queues Meridian makes.
pub const NVME_REGISTERS: u64 = APIC_PAGE + (4 << 20);

/// Where an I/O APIC's select register is once its page, which holds the
/// physical `address`, is mapped at [`IO_APIC_PAGE`].
#[must_use]
pub fn io_apic_select(address: u64) -> u64 {
    IO_APIC_PAGE + address % 4096
}

/// The input of the I/O APIC `io_apic`, which has `inputs` of them, that
/// the line `line` arrives at; none when it arrives at another one.
#[must_use]
pub fn input_of(io_apic: &acpi::IoApic, inputs: u32, line: &acpi::Route) -> Option<u32> {
    line.interrupt
        .checked_sub(io_apic.first)
        .filter(|&input| input < inputs)
}

/// Where the first I/O APIC's registers are mapped, uncached: the page
/// 6 MiB into the slot for devices' registers.
pub const IO_APIC_PAGE: u64 = APIC_PAGE + (6 << 20);

/// Where the configuration space of PCI's segment 0 is mapped, uncached,
/// page by page as the kernel reads it: in the slot for devices'
/// registers, 256 MiB after its start, with room for the 256 MiB of 256
/// buses.
pub const PCI_CONFIG: u64 = APIC_PAGE + (256 << 20);

/// Where the local APIC's registers are mapped, uncached, on processors
/// without an x2APIC: the first page of a slot of the root table for
/// devices' registers.
pub const APIC_PAGE: u64 = 0xffff_fd00_0000_0000;

/// Where the HPET's registers are mapped, uncached: the page after the
/// APIC's.
pub const HPET_PAGE: u64 = APIC_PAGE + 0x1000;

/// The top of the stack of processor `number`, or of the stack it takes
/// faults on.
#[must_use]
pub fn core_stack_top(number: usize, fault: bool) -> u64 {
    let space = 2 * CORE_STACK_PAGES * MemoryRegion::PAGE_SIZE;
    CORE_STACKS + (2 * number as u64 + u64::from(fault) + 1) * space
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: u64, pages: u64, kind: RegionKind) -> MemoryRegion {
        MemoryRegion {
            start,
            pages,
            kind,
            reserved: 0,
        }
    }

    fn sample() -> Vec<MemoryRegion> {
        vec![
            region(0, 0x9f, RegionKind::USABLE),
            region(0x10_0000, 0x100, RegionKind::KERNEL),
            region(0x20_0000, 0x1_0000, RegionKind::USABLE),
            region(0x1020_0000, 4, RegionKind::ACPI_RECLAIMABLE),
            region(0xfee0_0000, 1, RegionKind::RESERVED),
        ]
    }

    #[test]
    fn memory_ends_where_the_last_unreserved_region_does() {
        let mut r = sample();
        assert_eq!(ram_top(&r), 0x1020_0000 + 4 * 4096);
        r.push(region(0xfd_0000_0000, 0x30_0000, RegionKind::RESERVED));
        assert_eq!(
            ram_top(&r),
            0x1020_0000 + 4 * 4096,
            "a device window does not count"
        );
        assert_eq!(
            memory(&r).len(),
            4,
            "neither the window nor the APIC page above memory"
        );
        assert_eq!(ram_top(&[]), 0);
        assert_eq!(memory(&[]), []);
        let only_reserved = [region(0x1000, 1, RegionKind::RESERVED)];
        assert_eq!(memory(&only_reserved), []);
        let firmware_inside = [
            region(0, 4, RegionKind::USABLE),
            region(0x4000, 2, RegionKind::RESERVED),
            region(0x6000, 4, RegionKind::USABLE),
        ];
        assert_eq!(
            memory(&firmware_inside).len(),
            3,
            "firmware pieces inside memory stay"
        );
    }

    #[test]
    fn counts_bytes_per_kind() {
        let r = sample();
        assert_eq!(bytes_of(&r, RegionKind::USABLE), (0x9f + 0x1_0000) * 4096);
        assert_eq!(bytes_of(&r, RegionKind::KERNEL), 0x100 * 4096);
        assert_eq!(bytes_of(&r, RegionKind::UNUSABLE), 0);
        assert_eq!(bytes_of(&[], RegionKind::USABLE), 0);
    }

    #[test]
    fn tidy_means_sorted_without_overlap() {
        let mut r = sample();
        assert!(is_tidy(&r));
        assert!(is_tidy(&[]));
        assert!(is_tidy(&r[..1]));

        let touching = [
            region(0, 1, RegionKind::USABLE),
            region(0x1000, 1, RegionKind::KERNEL),
        ];
        assert!(is_tidy(&touching));
        let overlapping = [
            region(0, 2, RegionKind::USABLE),
            region(0x1000, 1, RegionKind::KERNEL),
        ];
        assert!(!is_tidy(&overlapping));

        r.swap(1, 2);
        assert!(!is_tidy(&r));
    }

    #[test]
    fn coverage_needs_one_region_of_the_kind() {
        let r = sample();
        assert!(covered(&r, RegionKind::KERNEL, 0x10_0000, 0x10_0000));
        assert!(covered(&r, RegionKind::KERNEL, 0x18_0000, 0x1000));
        assert!(
            !covered(&r, RegionKind::KERNEL, 0x10_0000, 0x10_0001),
            "one byte past the end"
        );
        assert!(
            !covered(&r, RegionKind::KERNEL, 0xf_f000, 0x2000),
            "starts before the region"
        );
        assert!(
            !covered(&r, RegionKind::USABLE, 0x10_0000, 0x1000),
            "wrong kind"
        );
        assert!(covered(&r, RegionKind::RESERVED, 0xfee0_0000, 0));
        assert!(
            !covered(&r, RegionKind::USABLE, u64::MAX - 4, 16),
            "no wrap around"
        );
    }

    #[test]
    fn the_frame_allocator_starts_from_the_free_memory_above_one_mib() {
        let r = sample();
        assert_eq!(
            free_ranges(&r).collect::<Vec<_>>(),
            [(0, 0x9f * 4096), (0x20_0000, 0x1000_0000)]
        );
        assert_eq!(usable_frames(&r), 0x1_0000, "the low region gives nothing");
        assert_eq!(bits_home(&r, 0x2000), Some(0x20_0000));
        assert_eq!(
            bits_home(&r, 0x1000_0000),
            Some(0x20_0000),
            "exactly the region"
        );
        assert_eq!(bits_home(&r, 0x1000_0001), None);
        assert_eq!(bits_home(&r, u64::MAX), None);
        // A region across the first MiB counts from there on.
        let across = [region(0xf_0000, 0x20, RegionKind::USABLE)];
        assert_eq!(usable_frames(&across), 0x10);
        assert_eq!(bits_home(&across, 0x10_000), Some(0x10_0000));
        assert_eq!(bits_home(&across, 0x10_001), None);
        assert_eq!(HEAP_BYTES % (2 << 20), 0, "a whole number of 2 MiB");
    }

    #[test]
    fn the_trampoline_goes_low_but_not_to_page_0() {
        assert_eq!(
            trampoline_home(&sample()),
            Some(0x1000),
            "page 0 is skipped"
        );
        let pages = |start, pages| region(start, pages, RegionKind::USABLE);
        assert_eq!(
            trampoline_home(&[pages(0x8000, 4)]),
            Some(0x8000),
            "exactly four pages"
        );
        assert_eq!(trampoline_home(&[pages(0x8000, 3)]), None);
        assert_eq!(
            trampoline_home(&[pages(0x8800, 5)]),
            Some(0x9000),
            "on a page"
        );
        assert_eq!(
            trampoline_home(&[pages(0x9c000, 8)]),
            Some(0x9c000),
            "up to 640 KiB"
        );
        assert_eq!(trampoline_home(&[pages(0x9d000, 8)]), None, "past it");
        assert_eq!(
            trampoline_home(&[pages(0x10_0000, 64)]),
            None,
            "above 1 MiB"
        );
        assert_eq!(
            trampoline_home(&[region(0x8000, 8, RegionKind::KERNEL), pages(0x20000, 4)]),
            Some(0x20000),
            "free memory only"
        );
        assert_eq!(trampoline_home(&[pages(0, 5)]), Some(0x1000));
        assert_eq!(trampoline_home(&[pages(0, 4)]), None, "page 0 leaves three");
    }

    #[test]
    fn the_back_buffer_is_one_frame_in_kernel_memory() {
        let r = sample();
        let with = |at: u64, width: u32, height: u32| {
            let mut info = BootInfo::new();
            info.back_buffer = at;
            info.framebuffer.width = width;
            info.framebuffer.height = height;
            back_buffer_pixels(&info, &r)
        };
        assert_eq!(
            with(0x10_0000, 512, 512),
            Some(512 * 512),
            "exactly the region"
        );
        assert_eq!(with(0x10_1000, 512, 512), None, "a page past its end");
        assert_eq!(with(0x10_1000, 512, 504), Some(512 * 504));
        assert_eq!(with(0, 512, 512), None, "no room given");
        assert_eq!(
            with(0x20_0000, 64, 64),
            None,
            "free memory is not the kernel's"
        );
        assert_eq!(with(0x10_0000, 0, 512), None, "no screen");
        assert_eq!(with(0x10_0000, 512, 0), None);
        assert_eq!(
            with(0x10_0000, u32::MAX, u32::MAX),
            None,
            "too big to count"
        );
    }

    #[test]
    fn only_mapped_regions_are_readable() {
        let mut r = sample();
        assert!(readable(&r, 0, 0x9f * 4096));
        assert!(readable(&r, 0x1020_0000, 4 * 4096), "ACPI tables");
        assert!(readable(&r, 0x1020_3fff, 1));
        assert!(!readable(&r, 0x1020_3fff, 2), "past the region's end");
        assert!(!readable(&r, 0x9f * 4096, 1), "the hole below 1 MiB");
        assert!(!readable(&r, 0x9e * 4096, 0x2000), "into the hole");
        assert!(readable(&r, 0xfee0_0000, 4096), "a device below 4 GiB");
        assert!(!readable(&r, u64::MAX, 2), "no wrap around");

        r.push(region(0x1_0000_0000, 0x100, RegionKind::USABLE));
        r.push(region(0xfd_0000_0000, 1, RegionKind::RESERVED));
        assert!(readable(&r, 0x1_0000_0000, 0x10_0000), "memory above 4 GiB");
        assert!(
            !readable(&r, 0xfd_0000_0000, 8),
            "a device window above memory is not mapped"
        );
        let high = [region(0x2_0000_0000, 1, RegionKind::RESERVED)];
        assert!(!readable(&high, 0x2_0000_0000, 8));
        let low = [region(0xffff_f000, 1, RegionKind::RESERVED)];
        assert!(readable(&low, 0xffff_f000, 4096), "up to 4 GiB exactly");
        assert!(!readable(&[], 0, 1));
    }

    #[test]
    fn sizes_pick_the_largest_unit() {
        let cases = [
            (0, "0 bytes"),
            (1023, "1023 bytes"),
            (1024, "1 KiB"),
            (1536, "1.5 KiB"),
            (1024 * 1024 - 1, "1023.9 KiB"),
            (512 << 20, "512 MiB"),
            ((1 << 30) + (1 << 29), "1.5 GiB"),
            ((2 << 30) - 1, "1.9 GiB"),
            (3 << 40, "3 TiB"),
            (5000 << 40, "5000 TiB"),
            (u64::MAX, "16777215.9 TiB"),
        ];
        for (bytes, want) in cases {
            assert_eq!(Size(bytes).to_string(), want, "{bytes}");
        }
    }

    #[test]
    fn greeting() {
        let mut out = String::new();
        greet(&mut out, &sample()).unwrap();
        assert_eq!(
            out,
            format!(
                "Meridian kernel {VERSION}\nMemory: 256.6 MiB free, 1 MiB taken by the kernel, 5 regions\n"
            )
        );
    }

    #[test]
    fn what_there_is_to_hand_out() {
        let mut out = String::new();
        hand_out(&mut out, Some(3 * 256 + 128)).unwrap();
        assert_eq!(
            out,
            "Memory to hand out: 3.5 MiB in frames, and a heap of 4 MiB\n"
        );
        out.clear();
        hand_out(&mut out, None).unwrap();
        assert_eq!(
            out,
            "kernel: there is no room for the frame allocator or the heap\n"
        );
    }

    #[test]
    fn other_cores_get_task_states_up_to_the_last() {
        assert!(!has_task_state(0, 64), "the first core has its own already");
        assert!(has_task_state(1, 64));
        assert!(has_task_state(63, 64));
        assert!(!has_task_state(64, 64), "one past the last");
        assert!(!has_task_state(1, 1));
    }

    #[test]
    fn this_core_counts_too() {
        assert_eq!(with_this_one(0), 1);
        assert_eq!(with_this_one(3), 4);
        assert_eq!(with_this_one(u32::MAX), u32::MAX);
    }

    #[test]
    fn how_many_cores_run() {
        let mut out = String::new();
        running(&mut out, Some(4), 3).unwrap();
        assert_eq!(out, "Cores running: 4 of 4\n");
        out.clear();
        running(&mut out, Some(12), 0).unwrap();
        assert_eq!(out, "Cores running: 1 of 12\n");
        out.clear();
        running(&mut out, None, 0).unwrap();
        assert_eq!(out, "Cores running: 1, as the MADT is missing\n");
    }

    #[test]
    fn how_the_kernel_waits() {
        let mut out = String::new();
        waiting(&mut out, Some(1_000_000)).unwrap();
        assert_eq!(
            out,
            "Timer: 100 interrupts a second, 1000000 APIC counts apart; the processor sleeps in between\n"
        );
        out.clear();
        waiting(&mut out, None).unwrap();
        assert_eq!(out, "Timer: none, so the uptime is kept by spinning\n");
    }

    #[test]
    fn the_machine_is_described() {
        let mut out = String::new();
        let found = machine::Machine {
            cores: Some(12),
            pm_timer: Some((0x408, 24)),
            hpet: None,
            ecam: None,
            io_apic: None,
            keyboard: None,
            i8042: None,
        };
        let measured = |hz, against| Some(machine::Measured { hz, against });
        describe(
            &mut out,
            &found,
            measured(2_994_999_999, machine::Source::AcpiTimer),
        )
        .unwrap();
        assert_eq!(
            out,
            "Cores: 12\nTime stamp counter: 2994 MHz, measured against the ACPI timer\n"
        );
        out.clear();
        describe(
            &mut out,
            &found,
            measured(3_549_000_000, machine::Source::Hpet),
        )
        .unwrap();
        assert_eq!(
            out,
            "Cores: 12\nTime stamp counter: 3549 MHz, measured against the HPET\n"
        );
        out.clear();
        describe(&mut out, &machine::Machine::default(), None).unwrap();
        assert_eq!(
            out,
            "Cores: not listed in the ACPI tables\nTime stamp counter: not measured, neither the ACPI timer nor the HPET could be used\n"
        );
    }

    #[test]
    fn greeting_stops_when_the_writer_fails() {
        struct Broken;
        impl Write for Broken {
            fn write_str(&mut self, _: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }
        assert!(greet(&mut Broken, &sample()).is_err());
        let measured = machine::Measured {
            hz: 1,
            against: machine::Source::Hpet,
        };
        assert!(describe(&mut Broken, &machine::Machine::default(), Some(measured)).is_err());
        let known = machine::Machine {
            cores: Some(1),
            pm_timer: None,
            hpet: None,
            ecam: None,
            io_apic: None,
            keyboard: None,
            i8042: None,
        };
        assert!(describe(&mut Broken, &known, None).is_err());
    }

    #[test]
    fn each_core_stack_has_a_guard_below_and_meets_no_other() {
        assert_eq!(core_stack_top(1, false), 0xffff_fe00_0001_8000);
        assert_eq!(core_stack_top(1, true), 0xffff_fe00_0002_0000);
        assert_eq!(core_stack_top(0, false), CORE_STACKS + 0x8000);
        let tops: Vec<u64> = (0..64)
            .flat_map(|n| [core_stack_top(n, false), core_stack_top(n, true)])
            .collect();
        for pair in tops.windows(2) {
            assert_eq!(
                pair[1] - pair[0],
                0x8000,
                "16 KiB of stack, 16 KiB of guard"
            );
        }
        let last = tops[tops.len() - 1];
        assert_eq!(last, 0xffff_fe00_0040_0000);
        assert!(last <= bootinfo::STACK_TOP - bootinfo::STACK_PAGES * MemoryRegion::PAGE_SIZE);
        assert_eq!(
            CORE_STACKS >> 39 & 0x1ff,
            508,
            "a slot of the root table nothing else uses"
        );
        assert_eq!((last - 1) >> 39 & 0x1ff, 508);
    }

    /// A writer that takes nothing.
    struct Full;

    impl Write for Full {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[test]
    fn a_pci_function_is_logged_by_where_it_is_and_what_it_is() {
        let mut bytes = [0u8; 64];
        bytes[0..4].copy_from_slice(&[0x86, 0x80, 0x22, 0x29]);
        bytes[9..12].copy_from_slice(&[0x01, 0x06, 0x01]);
        let h = pci::Header::read(&bytes).unwrap();
        let mut out = String::new();
        let at = pci::Address {
            bus: 0,
            device: 0x1f,
            function: 2,
        };
        pci_function(&mut out, at, &h).unwrap();
        assert_eq!(out, "PCI 00:1f.2 8086:2922 SATA controller (AHCI)\n");
        let mut out = String::new();
        let far = pci::Address {
            bus: 0xa3,
            device: 4,
            function: 7,
        };
        pci_function(&mut out, far, &h).unwrap();
        assert!(out.starts_with("PCI a3:04.7 "), "{out}");
        assert!(pci_function(&mut Full, at, &h).is_err());
    }

    #[test]
    fn each_drive_is_logged_with_what_it_says() {
        let mut data = [0u8; 512];
        for (i, pair) in b"QEMU HARDDISK                           "
            .chunks(2)
            .enumerate()
        {
            data[54 + 2 * i] = pair[1];
            data[55 + 2 * i] = pair[0];
        }
        data[120..124].copy_from_slice(&0x2_0000u32.to_le_bytes());
        let disk = ahci::Identity::read(&data);
        let line = |attached, identity| {
            let mut out = String::new();
            drive_line(&mut out, 2, attached, identity).unwrap();
            out
        };
        assert_eq!(
            line(ahci::Attached::Disk, Some(&disk)),
            "AHCI port 2: QEMU HARDDISK, 131072 sectors of 512 bytes\n"
        );
        assert_eq!(
            line(ahci::Attached::Disk, None),
            "AHCI port 2: a disk that did not say what it is\n"
        );
        assert_eq!(
            line(ahci::Attached::Optical, None),
            "AHCI port 2: an optical drive\n"
        );
        assert_eq!(
            line(ahci::Attached::Other(0x9669_0101), None),
            "AHCI port 2: a device with signature 0x96690101\n"
        );
        assert_eq!(
            line(ahci::Attached::Nothing, None),
            "AHCI port 2: nothing\n"
        );
        assert!(drive_line(&mut Full, 0, ahci::Attached::Optical, None).is_err());
    }

    #[test]
    fn an_io_apics_registers_are_where_its_address_falls_in_its_page() {
        assert_eq!(
            io_apic_select(0xfec0_0000),
            IO_APIC_PAGE,
            "at the start of a page, as q35's"
        );
        assert_eq!(io_apic_select(0xfec0_0400), IO_APIC_PAGE + 0x400);
        assert_eq!(io_apic_select(0xfec0_1fff), IO_APIC_PAGE + 0xfff);
    }

    #[test]
    fn a_line_arrives_at_the_input_of_its_io_apic() {
        let first = acpi::IoApic {
            id: 0,
            address: 0xfec0_0000,
            first: 0,
        };
        let second = acpi::IoApic {
            id: 1,
            address: 0xfec0_1000,
            first: 24,
        };
        let line = |interrupt| acpi::Route {
            interrupt,
            active_low: false,
            level: false,
        };
        assert_eq!(
            input_of(&first, 24, &line(1)),
            Some(1),
            "the keyboard on q35"
        );
        assert_eq!(input_of(&first, 24, &line(23)), Some(23), "its last input");
        assert_eq!(
            input_of(&first, 24, &line(24)),
            None,
            "the next I/O APIC's first"
        );
        assert_eq!(input_of(&second, 24, &line(25)), Some(1));
        assert_eq!(input_of(&second, 24, &line(1)), None, "below its first");
        assert_eq!(input_of(&second, 24, &line(47)), Some(23));
        assert_eq!(input_of(&second, 24, &line(48)), None);
    }

    #[test]
    fn the_io_apics_page_lies_past_nvmes() {
        assert_eq!(IO_APIC_PAGE >> 39 & 0x1ff, 506);
        const { assert!(IO_APIC_PAGE >= NVME_REGISTERS + 0x2000 && IO_APIC_PAGE + 0x1000 <= PCI_CONFIG) };
    }

    #[test]
    fn nvme_registers_lie_past_ahcis() {
        assert_eq!(NVME_REGISTERS >> 39 & 0x1ff, 506);
        const {
            assert!(
                NVME_REGISTERS >= AHCI_REGISTERS + 0x2000 && NVME_REGISTERS + 0x2000 <= PCI_CONFIG
            );
        };
        assert_eq!(NVME_REGISTERS % 4096, 0);
    }

    #[test]
    fn ahci_registers_lie_between_the_timers_and_pci() {
        assert_eq!(AHCI_REGISTERS >> 39 & 0x1ff, 506);
        const { assert!(AHCI_REGISTERS > HPET_PAGE && AHCI_REGISTERS + 0x2000 <= PCI_CONFIG) };
        assert_eq!(AHCI_REGISTERS % 4096, 0);
        assert!(
            ahci::port::base(31) + 0x80 <= 0x2000,
            "two pages hold every port"
        );
    }

    #[test]
    fn pci_configuration_space_lies_with_the_other_devices() {
        assert_eq!(PCI_CONFIG >> 39 & 0x1ff, 506);
        assert_eq!(
            (PCI_CONFIG + (256 << 20) - 1) >> 39 & 0x1ff,
            506,
            "all of it"
        );
        const { assert!(PCI_CONFIG > HPET_PAGE + 4096) };
        assert_eq!(PCI_CONFIG >> 47, 0x1_ffff, "in the kernel's half");
        assert_eq!(PCI_CONFIG % (2 << 20), 0);
    }

    #[test]
    fn the_apic_page_has_a_slot_of_its_own() {
        assert_eq!(HPET_PAGE, APIC_PAGE + 4096);
        assert_eq!(HPET_PAGE >> 39 & 0x1ff, 506);
        assert_eq!(APIC_PAGE % 4096, 0);
        assert_eq!(APIC_PAGE >> 39 & 0x1ff, 506);
        assert_eq!(APIC_PAGE >> 47, 0x1_ffff, "in the kernel's half");
        const { assert!(APIC_PAGE < CORE_STACKS && APIC_PAGE > bootinfo::HHDM_BASE) };
    }

    #[test]
    fn a_device_right_at_the_end_of_memory_is_left_out() {
        let at_top = [
            region(0, 4, RegionKind::USABLE),
            region(0x4000, 1, RegionKind::RESERVED),
        ];
        assert_eq!(ram_top(&at_top), 0x4000);
        assert_eq!(memory(&at_top).len(), 1, "it starts where memory ends");
    }
}
