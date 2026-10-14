//! The loader built as a QEMU test image. It runs the QEMU checks of every
//! crate it contains against real firmware and reports over the serial
//! port, one line each, in the format sextant reads:
//! `TEST <name> ok` or `TEST <name> FAILED <why>`, then `TESTS DONE`.
//! A `PRESS <key>` line asks sextant to type that key.

use core::fmt::Write;

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

pub fn run(system: &System, serial: &mut impl Write) -> ! {
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

    let failed = report.failed;
    let _ = writeln!(report.out, "TESTS DONE {failed} failed");
    x86::raw::qemu_exit(failed == 0)
}
