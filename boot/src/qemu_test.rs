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
        "boot.prepare.info",
        info.check().is_ok() && info.memory.count == 0,
        "the boot information should pass its check and wait for the final map",
    );
}
