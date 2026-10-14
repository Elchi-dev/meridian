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
use canvas::{Canvas, Scratch};
use kernel::screen::Facts;
use x86::raw::Com1;

fn run(info: &BootInfo, regions: &[MemoryRegion], serial: &mut Com1) -> ! {
    let _ = kernel::greet(serial, regions);
    let mut screen = raw::screen(&info.framebuffer);
    if let Some(canvas) = screen.as_mut() {
        let brand = x86::brand();
        let facts = Facts {
            processor: brand.as_str(),
            regions,
        };
        if let Err(e) = kernel::screen::draw(canvas, &facts, &mut Scratch::new()) {
            let _ = writeln!(
                serial,
                "kernel: the status screen could not be drawn: {e:?}"
            );
        }
    }
    finish(info, regions, screen.as_ref(), serial)
}

#[cfg(feature = "qemu-test")]
fn finish(
    info: &BootInfo,
    regions: &[MemoryRegion],
    screen: Option<&Canvas<'_>>,
    serial: &mut Com1,
) -> ! {
    qemu_test::run(info, regions, screen, serial)
}

#[cfg(not(feature = "qemu-test"))]
fn finish(_: &BootInfo, _: &[MemoryRegion], _: Option<&Canvas<'_>>, serial: &mut Com1) -> ! {
    let _ = writeln!(serial, "There is nothing more to do yet. Halting.");
    x86::raw::halt()
}
