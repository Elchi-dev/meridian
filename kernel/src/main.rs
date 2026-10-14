//! The Meridian kernel. The loader enters it at `kernel_start` with paging
//! on, interrupts off and the boot information in `rdi`;
//! docs/architecture/boot.md describes the handover.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

#[cfg(feature = "qemu-test")]
mod qemu_test;
#[allow(unsafe_code)]
mod raw;

use core::fmt::Write;

use bootinfo::{BootInfo, MemoryRegion};
use canvas::Scratch;
use kernel::display::Display;
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
    let free = raw::memory::take(regions);
    let _ = kernel::hand_out(serial, free);
    let machine = machine::read(&raw::DirectMap(regions), info.rsdp);
    let tsc_hz = machine
        .pm_timer
        .and_then(|(port, bits)| machine::calibrate(|| raw::pm_timer(port), bits, x86::raw::tsc));
    let _ = kernel::describe(serial, &machine, tsc_hz);
    let screen = raw::screen(&info.framebuffer);
    #[cfg(feature = "qemu-test")]
    qemu_test::before_drawing(screen.as_ref());
    let mut display = screen.map(|s| Display::new(s, raw::back_buffer(info, regions)));
    if let Some(display) = display.as_mut() {
        let brand = x86::brand();
        let facts = Facts {
            processor: brand.as_str(),
            cores: machine.cores,
            up: up(tsc_hz),
            regions,
        };
        let drawn = display.frame(|c| kernel::screen::draw(c, &facts, &mut Scratch::new()));
        if let Err(e) = drawn {
            let _ = writeln!(
                serial,
                "kernel: the status screen could not be drawn: {e:?}"
            );
        }
    }
    finish(
        info,
        regions,
        &machine,
        tsc_hz,
        display.as_mut(),
        free,
        serial,
    )
}

#[cfg(feature = "qemu-test")]
fn finish(
    info: &BootInfo,
    regions: &[MemoryRegion],
    machine: &Machine,
    tsc_hz: Option<u64>,
    display: Option<&mut Display<'_>>,
    free: Option<usize>,
    serial: &mut Com1,
) -> ! {
    qemu_test::run(
        info,
        regions,
        machine,
        tsc_hz,
        display.map(|d| &*d),
        free,
        serial,
    )
}

/// Keeps the uptime on the screen current. There are no interrupts yet to
/// wake the processor, so it waits by spinning.
#[cfg(not(feature = "qemu-test"))]
fn finish(
    _: &BootInfo,
    _: &[MemoryRegion],
    _: &Machine,
    tsc_hz: Option<u64>,
    display: Option<&mut Display<'_>>,
    _: Option<usize>,
    serial: &mut Com1,
) -> ! {
    let _ = writeln!(serial, "There is nothing more to do yet.");
    let (Some(display), Some(mut shown)) = (display, up(tsc_hz)) else {
        x86::raw::halt()
    };
    let mut scratch = Scratch::new();
    let (x, y, width, height) = kernel::screen::uptime_area(display.screen().width());
    loop {
        if let Some(now) = kernel::clock::to_redraw(shown, up(tsc_hz).unwrap_or(shown)) {
            let _ = display.update(x, y, width, height, |c| {
                kernel::screen::uptime(c, Some(now), &mut scratch)
            });
            shown = now;
        }
        core::hint::spin_loop();
    }
}
