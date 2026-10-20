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

use alloc::boxed::Box;
use core::fmt::Write;

use bootinfo::{BootInfo, MemoryRegion};
use canvas::Scratch;
use kernel::display::Display;
use kernel::machine::{self, Machine};
use kernel::screen::Facts;
use raw::serial::Serial;

/// Whole seconds since the machine was switched on, from the time stamp
/// counter, once its rate `tsc_hz` is known.
fn up(tsc_hz: Option<u64>) -> Option<u64> {
    x86::raw::tsc().checked_div(tsc_hz?)
}

fn run(info: &BootInfo, regions: &[MemoryRegion], serial: &mut Serial) -> ! {
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
    let _ = kernel::registers(serial, raw::fpu::saving());
    start_storage(machine.ecam, info.boot_partition, serial);
    raw::keep_modules(info, regions);
    #[cfg(feature = "qemu-test")]
    qemu_test::before_cores();
    let memory = raw::DirectMap(regions);
    let others = raw::smp::start_others(regions, machine::madt(&memory, info.rsdp), tsc_hz);
    start_keyboard(&machine, serial);
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
    finish(info, regions, &machine, tsc_hz, display, free, serial)
}

/// Finds every PCI function, logs it, and starts the first AHCI and the
/// first NVM Express controller, logging each drive and what it holds,
/// and hangs the partition `boot` the loader came from in the tree.
fn start_storage(ecam: Option<acpi::Ecam>, boot: [u8; 16], serial: &mut Serial) {
    raw::pci::start(ecam);
    let mut found = kernel::disk::Controllers::default();
    raw::pci::each(|at, h| {
        let _ = serial.write_str("kernel: ");
        let _ = kernel::pci_function(serial, at, &h);
        found.saw(at, &h);
    });
    if let Some((at, pci::Bar::Memory { address, .. })) = found.nvme {
        raw::pci::enable(at);
        if let Some(drive) = raw::nvme::start(address) {
            let _ = serial.write_str("kernel: ");
            let _ = kernel::nvme_line(serial, &drive.controller, &drive.namespace);
            let mut read = |lba, into: &mut [u8]| raw::nvme::read(lba, into).is_ok();
            let _ = kernel::disk::describe(serial, &"NVMe", &mut read);
            mount_boot(serial, &"NVMe", boot, read);
        }
    }
    if let Some((at, pci::Bar::Memory { address, .. })) = found.ahci {
        raw::pci::enable(at);
        raw::ahci::start(address);
        for drive in raw::ahci::drives().iter().flatten() {
            let _ = serial.write_str("kernel: ");
            let _ = kernel::drive_line(serial, drive.port, drive.attached, drive.identity.as_ref());
            let port = drive.port;
            let mut read = move |lba, into: &mut [u8]| raw::ahci::read(port, lba, into).is_ok();
            let name = format_args!("AHCI port {port}");
            let _ = kernel::disk::describe(serial, &name, &mut read);
            mount_boot(serial, &name, boot, read);
        }
    }
}

/// Sets the PS/2 keyboard up and has the I/O APIC send its interrupt to
/// this processor, the first, once the keyboard answered. The APIC has to
/// be on, as it is once the other cores were started.
fn start_keyboard(machine: &Machine, serial: &mut Serial) {
    let (Some(io_apic), Some(line)) = (machine.io_apic, machine.keyboard) else {
        return;
    };
    let (Some(inputs), Some(apic)) = (raw::ioapic::start(io_apic.address), x86::raw::apic_id())
    else {
        return;
    };
    let Some(input) = kernel::input_of(&io_apic, inputs, &line) else {
        return;
    };
    if !raw::ps2::start() {
        let _ = writeln!(serial, "kernel: no PS/2 keyboard answered");
        return;
    }
    let destination = u8::try_from(apic).unwrap_or(0);
    let entry = x86::ioapic::entry(
        raw::interrupts::KEYBOARD,
        destination,
        line.active_low,
        line.level,
        false,
    );
    raw::ioapic::route(input, entry);
    let _ = writeln!(
        serial,
        "kernel: the PS/2 keyboard interrupts on input {input} of the I/O APIC"
    );
}

/// Hangs the partition `boot` the loader came from at /System/Boot, read
/// only, when it is on the disk `disk`, which `read` reads, and no volume
/// hangs there already. A loader that names none leaves the first EFI
/// system partition found.
fn mount_boot(
    serial: &mut Serial,
    disk: &dyn core::fmt::Display,
    boot: [u8; 16],
    mut read: impl FnMut(u64, &mut [u8]) -> bool + Copy + Send + 'static,
) {
    let Some(esp) = kernel::disk::Table::read(&mut read).and_then(|t| t.boot_volume(boot)) else {
        return;
    };
    let first = esp.first;
    let volume = Box::new(move |lba, into: &mut [u8]| read(first + lba, into));
    if raw::vfs::mount("/System/Boot", volume).is_ok() {
        let _ = writeln!(
            serial,
            "kernel: the partition the loader came from, on {disk}, hangs at /System/Boot, read only"
        );
    }
}

#[cfg(feature = "qemu-test")]
fn finish(
    info: &BootInfo,
    regions: &[MemoryRegion],
    machine: &Machine,
    tsc_hz: Option<u64>,
    display: Option<Display<'static>>,
    free: Option<usize>,
    serial: &mut Serial,
) -> ! {
    qemu_test::run(info, regions, machine, tsc_hz, display, free, serial)
}

/// Starts the programs, and gives the screen to the console when one of
/// them is the shell, which writes on it; otherwise it keeps the uptime
/// on the screen current. With the timer it sleeps a tenth of a second
/// between looks, while the idle thread halts the processor; without one
/// it spins.
#[cfg(not(feature = "qemu-test"))]
fn finish(
    info: &BootInfo,
    regions: &[MemoryRegion],
    _: &Machine,
    tsc_hz: Option<u64>,
    mut display: Option<Display<'static>>,
    _: Option<usize>,
    serial: &mut Serial,
) -> ! {
    let _ = writeln!(serial, "There is nothing more to do yet.");
    let timer = raw::timer::start(tsc_hz);
    let _ = kernel::waiting(serial, timer);
    raw::thread::start();
    let shell = raw::modules(info, regions).any(|(m, _)| kernel::is_shell(m.name()));
    if shell && let Some(screen) = display.take() {
        let _ = raw::console::start(screen);
    }
    for (module, file) in raw::modules(info, regions) {
        if let Err(e) = raw::user::start_file(0, file, 0) {
            let name = core::str::from_utf8(module.name()).unwrap_or("?");
            let _ = writeln!(serial, "kernel: {name} did not start: {e:?}");
        }
    }
    let (Some(display), Some(mut shown)) = (display.as_mut(), up(tsc_hz)) else {
        loop {
            raw::thread::sleep(raw::timer::HZ);
        }
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
