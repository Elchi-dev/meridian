//! The Meridian kernel. The loader enters it at `kernel_start` with paging
//! on, interrupts off and the boot information in `rdi`;
//! docs/architecture/boot.md describes the handover.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(feature = "qemu-test")]
mod qemu_test;
#[allow(unsafe_code)]
mod raw;

use core::fmt::Write;

use bootinfo::{BootInfo, MemoryRegion};

fn run(info: &BootInfo, regions: &[MemoryRegion], serial: &mut impl Write) -> ! {
    let _ = kernel::greet(serial, regions);
    finish(info, regions, serial)
}

#[cfg(feature = "qemu-test")]
fn finish(info: &BootInfo, regions: &[MemoryRegion], serial: &mut impl Write) -> ! {
    qemu_test::run(info, regions, serial)
}

#[cfg(not(feature = "qemu-test"))]
fn finish(_: &BootInfo, _: &[MemoryRegion], serial: &mut impl Write) -> ! {
    let _ = writeln!(serial, "There is nothing more to do yet. Halting.");
    x86::raw::halt()
}
