//! The Meridian bootloader, a UEFI application. Today it reports what it
//! runs on and hands control back to the firmware; loading the kernel is
//! the next milestone (docs/ROADMAP.md, M1).

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(feature = "qemu-test")]
mod qemu_test;
#[allow(unsafe_code)]
mod raw;

use core::fmt::Write;

use boot::{Out, banner};
use uefi::Status;
use uefi::raw::System;

fn run(system: &System, serial: &mut impl Write) -> Status {
    let mut out = Out {
        screen: system.console(),
        serial,
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

    finish(system, &mut out)
}

#[cfg(feature = "qemu-test")]
fn finish<S: Write>(system: &System, out: &mut Out<'_, S>) -> Status {
    qemu_test::run(system, out.serial)
}

#[cfg(not(feature = "qemu-test"))]
fn finish<S: Write>(system: &System, out: &mut Out<'_, S>) -> Status {
    let _ = writeln!(
        out,
        "\nThere is no kernel to start yet. Press any key to return to the firmware."
    );
    match system.wait_for_key() {
        Ok(_) => {
            let _ = writeln!(out, "Returning to the firmware.");
            Status::SUCCESS
        }
        Err(status) => status,
    }
}
