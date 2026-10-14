//! The loader built as a QEMU test image. It runs the QEMU checks of every
//! crate it contains against real firmware and reports over the serial
//! port, one line each, in the format sextant reads:
//! `TEST <name> ok` or `TEST <name> FAILED <why>`. A `PRESS <key>` line
//! asks sextant to type that key. When every check passed it starts the
//! kernel's test build, which checks the handover and ends the run with
//! `TESTS DONE`; otherwise the loader ends it here.

use core::fmt::Write;
use core::mem::size_of;

use boot::handover::Problem;
use bootinfo::{BootInfo, MemoryRegion, STACK_PAGES, STACK_TOP};
use uefi::guid::ACPI_20_TABLE;
use uefi::memory::PAGE_SIZE;
use uefi::raw::System;

struct Serial<'a, W: Write> {
    out: &'a mut W,
    failed: u32,
}

impl<W: Write> uefi::qemu_test::Report for Serial<'_, W> {
    fn check(&mut self, name: &str, ok: bool, why: &str) {
        if ok {
            let _ = writeln!(self.out, "TEST {name} ok");
        } else {
            self.failed += 1;
            let _ = writeln!(self.out, "TEST {name} FAILED {why}");
        }
    }

    fn press(&mut self, key: &str) {
        let _ = writeln!(self.out, "PRESS {key}");
    }
}

/// Runs the checks and comes back only when all of them passed.
pub fn checks(system: &System, serial: &mut impl Write) {
    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    uefi::qemu_test::run(system, &mut report);
    x86::qemu_test::run(&mut |name, ok, why| {
        uefi::qemu_test::Report::check(&mut report, name, ok, why);
    });
    x86::qemu_test::firmware_tables(&mut |name, ok, why| {
        uefi::qemu_test::Report::check(&mut report, name, ok, why);
    });

    let (major, _, _) = boot::uefi_version(system.header().revision);
    uefi::qemu_test::Report::check(
        &mut report,
        "boot.uefi_version",
        major == 2,
        "the loader misreads the firmware's UEFI version",
    );

    let kernel = system
        .boot_volume()
        .and_then(|root| root.open(boot::KERNEL_PATH))
        .and_then(|file| file.info());
    uefi::qemu_test::Report::check(
        &mut report,
        "boot.kernel.file",
        kernel.is_ok_and(|i| !i.directory && i.size > 4096),
        "sextant should put the kernel next to the loader",
    );

    prepare(system, &mut report);
    chain(system, &mut report);

    let failed = report.failed;
    if failed > 0 {
        let _ = writeln!(report.out, "TESTS DONE {failed} failed");
        x86::raw::qemu_exit(false);
    }
}

/// Ends the run when the loader could not start the kernel.
pub fn kernel_not_started(problem: &Problem, serial: &mut impl Write) -> ! {
    let _ = writeln!(serial, "TEST boot.kernel FAILED {problem}");
    let _ = writeln!(serial, "TESTS DONE 1 failed");
    x86::raw::qemu_exit(false)
}

/// Runs the loader's preparation once on its own and checks what it hands
/// to the jump. The real start prepares again, so this costs a second copy
/// of the kernel in memory and nothing else.
fn prepare<W: Write>(system: &System, report: &mut Serial<'_, W>) {
    use uefi::qemu_test::Report;

    let ready = match crate::prepare(system) {
        Ok(ready) => ready,
        Err(problem) => {
            let _ = writeln!(report.out, "boot: {problem}");
            report.check(
                "boot.prepare",
                false,
                "the loader could not prepare the kernel",
            );
            return;
        }
    };
    report.check("boot.prepare", true, "");
    let info = ready.info;
    let kernel = info.kernel;
    report.check(
        "boot.prepare.root",
        ready.root != 0 && ready.root % PAGE_SIZE == 0,
        "the root page table should be a whole page",
    );
    report.check(
        "boot.prepare.kernel",
        kernel.size > 0
            && kernel.physical % PAGE_SIZE == 0
            && kernel.size % PAGE_SIZE == 0
            && kernel.virtual_base == 0xffff_ffff_8000_0000,
        "the kernel should be whole pages, placed where it was linked",
    );
    report.check(
        "boot.prepare.entry",
        (kernel.virtual_base..kernel.virtual_base + kernel.size).contains(&ready.entry),
        "the entry point should lie inside the kernel",
    );
    report.check(
        "boot.prepare.stack",
        info.stack.top == STACK_TOP && info.stack.bottom == STACK_TOP - STACK_PAGES * PAGE_SIZE,
        "the stack should end at STACK_TOP and be STACK_PAGES long",
    );
    report.check(
        "boot.prepare.firmware",
        info.rsdp == system.configuration_table(&ACPI_20_TABLE).unwrap_or(1)
            && info.uefi_system_table == system.table_address(),
        "the ACPI root pointer and the system table should be passed on",
    );
    report.check(
        "boot.prepare.room",
        ready.page_at % PAGE_SIZE == 0
            && ready.page.len()
                >= size_of::<BootInfo>() + crate::REGION_LIMIT * size_of::<MemoryRegion>()
            && ready.map.len() >= system.memory_map_size().unwrap_or(usize::MAX),
        "the boot information and the memory map should have room",
    );
    report.check(
        "boot.prepare.framebuffer",
        info.framebuffer == boot::handover::framebuffer(system.framebuffer().ok())
            && info.framebuffer.base != 0,
        "the framebuffer the firmware set up should be passed on",
    );
    report.check(
        "boot.prepare.info",
        info.check().is_ok() && info.memory.count == 0,
        "the boot information should pass its check and wait for the final map",
    );
}

/// The options the loader's copy is started with.
pub fn chain_options() -> &'static [u16] {
    &CHAIN_OPTIONS
}

/// The example system sextant puts on test images is found, behind
/// Meridian.
pub fn systems_found(cards: &[boot::screen::Card<'_>], serial: &mut impl Write) {
    use boot::screen::Kind;
    use uefi::qemu_test::Report;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    report.check(
        "boot.systems.found",
        cards.len() == 2
            && cards[0].kind == Kind::Meridian
            && cards[1].kind == Kind::Linux
            && cards[1].name == "example",
        "the example system on the test image should be found as a second card",
    );
    stop_on_failure(&mut report);
}

/// The example system was started from the boot screen and came back.
pub fn other_came_back(status: uefi::Status, serial: &mut impl Write) {
    use uefi::qemu_test::Report;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    report.check(
        "boot.systems.started",
        status == CHAINED,
        "choosing the example system should start it, and it should come back with ABORTED",
    );
    stop_on_failure(&mut report);
}

/// Ends the run when a check failed: the kernel would start next and
/// report only its own failures.
fn stop_on_failure<W: Write>(report: &mut Serial<'_, W>) {
    if report.failed > 0 {
        let _ = writeln!(report.out, "TESTS DONE {} failed", report.failed);
        x86::raw::qemu_exit(false);
    }
}

/// The boot screen's first frame: check it, have sextant keep it, and
/// choose with the keys a person would press. The first time round that
/// is Right and Enter, which starts the example system; when it has come
/// back, Enter starts Meridian.
pub fn boot_screen(
    system: &System,
    round: u32,
    frame: &[u32],
    screen: &[u32],
    serial: &mut impl Write,
) {
    use canvas::palette::{NIGHT, TANGERINE};
    use uefi::qemu_test::Report;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let fb = system.framebuffer();
    // OVMF's framebuffer is blue, green, red, as uefi.gop.format checks.
    let (night, tangerine) = (
        NIGHT.encode(canvas::Format::Bgr),
        TANGERINE.encode(canvas::Format::Bgr),
    );
    report.check(
        "boot.screen.drawn",
        frame.first() == Some(&night) && frame.iter().filter(|&&p| p == tangerine).count() > 5000,
        "the boot screen should be dark, with the chosen card's orange tile and frame",
    );
    let (width, stride) = fb.map_or((0, 0), |fb| (fb.width as usize, fb.stride as usize));
    let same = width > 0
        && frame
            .chunks_exact(width)
            .enumerate()
            .all(|(row, pixels)| screen.get(row * stride..row * stride + width) == Some(pixels));
    report.check(
        "boot.screen.presented",
        same,
        "the screen should show exactly the frame drawn off it, row for row",
    );
    report.check(
        "boot.screen.shown",
        system.pixels().is_err() && width > 0,
        "the screen should still be held by the boot screen while it is shown",
    );
    if round == 0 {
        let _ = writeln!(report.out, "SCREEN boot");
        let answer = system.wait_for_key();
        report.check(
            "boot.screen.kept",
            answer.is_ok_and(|k| k.unicode_char == 0x0d),
            "sextant should answer SCREEN with a carriage return, read as Enter",
        );
        report.press("right");
    }
    report.press("ret");
    if report.failed > 0 {
        let _ = writeln!(report.out, "TESTS DONE {} failed", report.failed);
        x86::raw::qemu_exit(false);
    }
}

/// What the loader's copy returns when it was started as a test.
pub const CHAINED: uefi::Status = uefi::Status::ABORTED;

/// The options the test starts the loader's copy with.
static CHAIN_OPTIONS: [u16; 11] = {
    let text = b"chain test\0";
    let mut units = [0u16; 11];
    let mut i = 0;
    while i < text.len() {
        units[i] = text[i] as u16;
        i += 1;
    }
    units
};

/// Whether this image was started by the chain check below.
pub fn is_chained(system: &System) -> bool {
    system.load_options() == CHAIN_OPTIONS
}

/// Loads a copy of the loader from the same partition and starts it with
/// options that make it come back at once: what the boot screen does to
/// start another system, checked end to end.
fn chain<W: Write>(system: &System, report: &mut Serial<'_, W>) {
    use uefi::qemu_test::Report;

    let started = system.load_image(boot::LOADER_PATH).and_then(|mut image| {
        image.set_options(&CHAIN_OPTIONS)?;
        Ok(image.start())
    });
    report.check(
        "boot.chain.start",
        started == Ok(CHAINED),
        "the loader's copy should start, see its options and return ABORTED",
    );
}
