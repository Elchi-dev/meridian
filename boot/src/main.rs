//! The Meridian bootloader, a UEFI application. It says what it runs on,
//! loads the kernel from `\meridian\kernel` next to itself, builds the
//! kernel's address space, leaves boot services and jumps
//! (docs/architecture/boot.md).

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(feature = "qemu-test")]
mod qemu_test;
#[allow(unsafe_code)]
mod raw;

use core::mem::size_of;

use boot::handover::{self, Layout, Problem};
use boot::{KERNEL_PATH, Out, banner, memory};
use bootinfo::{
    BootInfo, KERNEL_MEMORY_TYPE, KernelImage, MemoryRegion, STACK_PAGES, STACK_TOP, Stack,
};
use uefi::Status;
use uefi::guid::ACPI_20_TABLE;
use uefi::memory::{MemoryType, PAGE_BYTES, PAGE_SIZE};
use uefi::raw::System;
use x86::Features;
use x86::raw::Com1;

/// The most regions the boot information holds. Firmware maps have a few
/// hundred descriptors at most, and the loader merges neighbours of a kind.
const REGION_LIMIT: usize = 512;

const KERNEL_TYPE: MemoryType = MemoryType(KERNEL_MEMORY_TYPE);

fn run(system: System, serial: &mut Com1) -> Status {
    let mut out = Out {
        screen: system.console(),
        serial: &mut *serial,
    };
    if let Some(screen) = out.screen.as_mut() {
        let _ = screen.clear();
    }
    let _ = banner(
        &mut out,
        system.firmware_vendor(),
        system.firmware_revision(),
        system.header().revision,
    );

    #[cfg(feature = "qemu-test")]
    qemu_test::checks(&system, serial);

    let (system, problem) = match prepare(&system) {
        Ok(ready) => raw::start(system, ready),
        Err(problem) => (system, problem),
    };
    give_up(&system, &problem, serial)
}

/// Everything the jump needs, worked out while boot services still run.
struct Ready {
    root: u64,
    entry: u64,
    info: BootInfo,
    /// The pages the boot information goes into, and where they are.
    page: &'static mut [u8],
    page_at: u64,
    /// Room for the final memory map.
    map: &'static mut [u8],
}

fn firmware(doing: &'static str) -> impl FnOnce(Status) -> Problem {
    move |status| Problem::Firmware { doing, status }
}

fn pages_for(bytes: u64) -> usize {
    usize::try_from(bytes.div_ceil(PAGE_SIZE)).unwrap_or(usize::MAX)
}

/// Loads the kernel, gives it a stack and builds its page tables.
fn prepare(system: &System) -> Result<Ready, Problem> {
    let root = system
        .boot_volume()
        .map_err(firmware("opening the boot volume"))?;
    let mut file = root
        .open(KERNEL_PATH)
        .map_err(firmware("opening the kernel file"))?;
    let size = file
        .info()
        .map_err(firmware("reading the kernel file's size"))?
        .size;
    let mut buf = system
        .allocate_pages(MemoryType::LOADER_DATA, pages_for(size))
        .map_err(firmware("finding memory for the kernel file"))?;
    let got = file
        .read_all(buf.bytes())
        .map_err(firmware("reading the kernel file"))?;
    // A file cut short fails the ELF checks below, so the count read is
    // all that matters here.
    let data: &[u8] = &buf.bytes()[..got];
    let elf = elf::Elf::parse(data).map_err(Problem::Kernel)?;
    let (first, span) = elf.span().map_err(Problem::Kernel)?;
    let segments = elf.segments().map_err(Problem::Kernel)?;
    let entry = elf.entry();
    if !handover::entry_is_code(segments.as_slice(), entry) {
        return Err(Problem::BadEntry(entry));
    }
    let mut image = system
        .allocate_pages(KERNEL_TYPE, pages_for(span))
        .map_err(firmware("finding memory for the kernel"))?;
    elf.load(image.bytes(), first).map_err(Problem::Kernel)?;
    let _ = buf.free();

    let stack = system
        .allocate_pages(KERNEL_TYPE, pages_for(STACK_PAGES * PAGE_SIZE))
        .map_err(firmware("finding memory for the kernel's stack"))?;
    let loader = system
        .image_range()
        .map_err(firmware("finding the loader in memory"))?;
    let map_size = system
        .memory_map_size()
        .map_err(firmware("reading the memory map's size"))?;
    let map = system
        .allocate_pages(
            MemoryType::LOADER_DATA,
            handover::map_buffer_pages(map_size),
        )
        .map_err(firmware("finding memory for the memory map"))?
        .keep();
    let screen = system.framebuffer().ok();
    let memory_top = memory::memory_top(
        system
            .memory_map(map)
            .map_err(firmware("reading the memory map"))?
            .descriptors(),
    );

    let layout = Layout {
        kernel_virtual: first,
        kernel_physical: image.start(),
        segments: segments.as_slice(),
        stack_physical: stack.start(),
        loader,
        memory_top,
        framebuffer: screen.map_or((0, 0), |fb| (fb.base, fb.size)),
        features: Features::detect(),
    };
    let count = usize::try_from(handover::frames_needed(&layout)).unwrap_or(usize::MAX);
    let mut frames = raw::FirmwareFrames::new(system, count)
        .map_err(firmware("finding memory for page tables"))?;
    let space = handover::build(&mut frames, &layout).map_err(Problem::Tables)?;

    let info_bytes = size_of::<BootInfo>() + REGION_LIMIT * size_of::<MemoryRegion>();
    let page = system
        .allocate_pages(KERNEL_TYPE, info_bytes.div_ceil(PAGE_BYTES))
        .map_err(firmware("finding memory for the boot information"))?;
    let page_at = page.start();
    let info = BootInfo {
        kernel: KernelImage {
            physical: image.start(),
            virtual_base: first,
            size: span,
        },
        stack: Stack {
            bottom: STACK_TOP - STACK_PAGES * PAGE_SIZE,
            top: STACK_TOP,
        },
        rsdp: system.configuration_table(&ACPI_20_TABLE).unwrap_or(0),
        uefi_system_table: system.table_address(),
        framebuffer: handover::framebuffer(screen),
        ..BootInfo::new()
    };
    Ok(Ready {
        root: space.root(),
        entry,
        info,
        page: page.keep(),
        page_at,
        map,
    })
}

#[cfg(feature = "qemu-test")]
fn give_up(_: &System, problem: &Problem, serial: &mut Com1) -> Status {
    qemu_test::kernel_not_started(problem, serial)
}

#[cfg(not(feature = "qemu-test"))]
fn give_up(system: &System, problem: &Problem, serial: &mut Com1) -> Status {
    use core::fmt::Write;

    let mut out = Out {
        screen: system.console(),
        serial,
    };
    let _ = writeln!(out, "\nMeridian could not start: {problem}.");
    let _ = writeln!(out, "Press any key to return to the firmware.");
    match system.wait_for_key() {
        Ok(_) => Status::LOAD_ERROR,
        Err(status) => status,
    }
}
