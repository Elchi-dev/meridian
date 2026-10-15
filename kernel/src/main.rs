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
    raw::memory::prepare_kernel_half();
    let _ = kernel::hand_out(serial, free);
    let machine = machine::read(&raw::DirectMap(regions), info.rsdp);
    let hpet = machine.hpet.and_then(raw::hpet::start);
    let acpi_timer = machine
        .pm_timer
        .and_then(|(port, bits)| machine::calibrate(|| raw::pm_timer(port), bits, x86::raw::tsc));
    let tsc = machine::measure(acpi_timer, || {
        machine::calibrate_against(hpet?, raw::hpet::count, x86::raw::tsc)
    });
    let tsc_hz = tsc.map(|t| t.hz);
    let _ = kernel::describe(serial, &machine, tsc);
    raw::pci::start(machine.ecam);
    let mut controller = None;
    raw::pci::each(|at, h| {
        let _ = serial.write_str("kernel: ");
        let _ = kernel::pci_function(serial, at, &h);
        if (h.class, h.subclass, h.interface) == (0x01, 0x06, 0x01) && controller.is_none() {
            controller = Some((at, h.bar(5)));
        }
    });
    if let Some((at, pci::Bar::Memory { address, .. })) = controller {
        raw::pci::enable(at);
        raw::ahci::start(address);
        for drive in raw::ahci::drives().iter().flatten() {
            let _ = serial.write_str("kernel: ");
            let _ = kernel::drive_line(serial, drive.port, drive.attached, drive.identity.as_ref());
            let mut sector = [0u8; 512];
            if raw::ahci::read_sectors(drive.port, 1, &mut sector).is_ok()
                && sector.starts_with(b"EFI PART")
            {
                let _ = writeln!(serial, "kernel: AHCI port {}: a GPT disk", drive.port);
            }
        }
    }
    raw::keep_modules(info, regions);
    #[cfg(feature = "qemu-test")]
    qemu_test::before_cores();
    let memory = raw::DirectMap(regions);
    let others = raw::smp::start_others(regions, machine::madt(&memory, info.rsdp), tsc_hz);
    let _ = kernel::running(serial, machine.cores, others);
    let screen = raw::screen(&info.framebuffer);
    #[cfg(feature = "qemu-test")]
    qemu_test::before_drawing(screen.as_ref());
    let mut display = screen.map(|s| Display::new(s, raw::back_buffer(info, regions)));
    if let Some(display) = display.as_mut() {
        let brand = x86::brand();
        let facts = Facts {
            processor: brand.as_str(),
            cores: machine.cores,
            running: kernel::with_this_one(others),
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

/// Keeps the uptime on the screen current. With the timer it sleeps a
/// tenth of a second between looks, while the idle thread halts the
/// processor; without one it spins.
#[cfg(not(feature = "qemu-test"))]
fn finish(
    info: &BootInfo,
    regions: &[MemoryRegion],
    _: &Machine,
    tsc_hz: Option<u64>,
    display: Option<&mut Display<'_>>,
    _: Option<usize>,
    serial: &mut Com1,
) -> ! {
    let _ = writeln!(serial, "There is nothing more to do yet.");
    let timer = raw::timer::start(tsc_hz);
    let _ = kernel::waiting(serial, timer);
    raw::thread::start();
    for (module, file) in raw::modules(info, regions) {
        if let Err(e) = raw::user::start_file(0, file, 0) {
            let name = core::str::from_utf8(module.name()).unwrap_or("?");
            let _ = writeln!(serial, "kernel: {name} did not start: {e:?}");
        }
    }
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
        if timer.is_some() {
            raw::thread::sleep(kernel::clock::between_looks(raw::timer::HZ));
        } else {
            core::hint::spin_loop();
        }
    }
}
