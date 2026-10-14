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
use kernel::machine::{self, Machine};
use kernel::screen::Facts;
use x86::raw::Com1;

/// Whole seconds since the machine was switched on, from the time stamp
/// counter, once its rate `tsc_hz` is known.
fn up(tsc_hz: Option<u64>) -> Option<u64> {
    x86::raw::tsc().checked_div(tsc_hz?)
}

fn run(info: &BootInfo, regions: &[MemoryRegion], serial: &mut Com1) -> ! {
    let _ = kernel::greet(serial, regions);
    let machine = machine::read(&raw::DirectMap(regions), info.rsdp);
    let tsc_hz = machine
        .pm_timer
        .and_then(|(port, bits)| machine::calibrate(|| raw::pm_timer(port), bits, x86::raw::tsc));
    let _ = kernel::describe(serial, &machine, tsc_hz);
    let mut screen = raw::screen(&info.framebuffer);
    if let Some(canvas) = screen.as_mut() {
        let brand = x86::brand();
        let facts = Facts {
            processor: brand.as_str(),
            cores: machine.cores,
            up: up(tsc_hz),
            regions,
        };
        if let Err(e) = kernel::screen::draw(canvas, &facts, &mut Scratch::new()) {
            let _ = writeln!(
                serial,
                "kernel: the status screen could not be drawn: {e:?}"
            );
        }
    }
    finish(info, regions, &machine, tsc_hz, screen.as_mut(), serial)
}

#[cfg(feature = "qemu-test")]
fn finish(
    info: &BootInfo,
    regions: &[MemoryRegion],
    machine: &Machine,
    tsc_hz: Option<u64>,
    screen: Option<&mut Canvas<'_>>,
    serial: &mut Com1,
) -> ! {
    qemu_test::run(info, regions, machine, tsc_hz, screen.map(|c| &*c), serial)
}

/// Keeps the uptime on the screen current. There are no interrupts yet to
/// wake the processor, so it waits by spinning.
#[cfg(not(feature = "qemu-test"))]
fn finish(
    _: &BootInfo,
    _: &[MemoryRegion],
    _: &Machine,
    tsc_hz: Option<u64>,
    screen: Option<&mut Canvas<'_>>,
    serial: &mut Com1,
) -> ! {
    let _ = writeln!(serial, "There is nothing more to do yet.");
    let (Some(canvas), Some(mut shown)) = (screen, up(tsc_hz)) else {
        x86::raw::halt()
    };
    let mut scratch = Scratch::new();
    loop {
        if let Some(now) = kernel::clock::to_redraw(shown, up(tsc_hz).unwrap_or(shown)) {
            let _ = kernel::screen::uptime(canvas, Some(now), &mut scratch);
            shown = now;
        }
        core::hint::spin_loop();
    }
}
