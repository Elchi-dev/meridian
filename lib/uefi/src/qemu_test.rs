//! Checks of the firmware wrappers against real firmware (OVMF), run from
//! a test image in QEMU. Each check reports its name, its result and what a
//! failure means.
//!
//! OVMF mirrors its text console to the serial port, so everything written
//! here also shows up in the serial log, where sextant compares it with
//! [`CONSOLE_PROBE`].

use core::fmt::Write;

use crate::raw::System;
use crate::text::scan;

/// What a test image does with each check, and how it asks sextant to type.
pub trait Report {
    fn check(&mut self, name: &str, ok: bool, why: &str);
    /// Asks the runner to press a key, named as the QEMU monitor names it.
    fn press(&mut self, key: &str);
}

/// Longer than one 128-unit piece of the console encoder, so the probe only
/// arrives whole when splitting into pieces works.
pub const CONSOLE_PROBE: &str = concat!(
    "console probe: ",
    "0123456789abcdefghijklmnopqrstuvwxyz",
    "0123456789abcdefghijklmnopqrstuvwxyz",
    "0123456789abcdefghijklmnopqrstuvwxyz",
    "0123456789abcdefghijklmnopqrstuvwxyz",
    " end",
);

pub fn run(system: &System, report: &mut impl Report) {
    report.check(
        "uefi.vendor",
        system.firmware_vendor().eq("EDK II".chars()),
        "OVMF names itself EDK II; reading the vendor string is broken",
    );
    report.check(
        "uefi.table.revision",
        system.header().revision >> 16 == 2,
        "the system table should claim UEFI 2.x",
    );
    report.check(
        "uefi.stall",
        system.stall(1_000).is_ok() && system.stall(0).is_ok(),
        "Stall refused 1000 or 0 microseconds",
    );

    report.press("ret");
    let enter = system.wait_for_key();
    report.check(
        "uefi.keyboard.enter",
        enter.is_ok_and(|k| k.unicode_char == 0x0d && k.scan_code == scan::NULL),
        "Enter should arrive as carriage return without a scan code",
    );
    report.press("down");
    let down = system.wait_for_key();
    report.check(
        "uefi.keyboard.arrow",
        down.is_ok_and(|k| k.scan_code == scan::DOWN && k.unicode_char == 0),
        "the down arrow should arrive as its scan code",
    );

    let Some(mut console) = system.console() else {
        report.check("uefi.console", false, "OVMF always has a text console");
        return;
    };
    report.check("uefi.console", true, "");
    report.check(
        "uefi.console.clear",
        console.clear().is_ok(),
        "ClearScreen failed",
    );
    report.check(
        "uefi.console.write",
        writeln!(console, "{CONSOLE_PROBE}").is_ok(),
        "OutputString failed on a text longer than one piece",
    );
    report.check(
        "uefi.console.unknown_glyph",
        writeln!(console, "outside the basic plane: \u{1F600}").is_ok(),
        "a character the font lacks must only be a warning",
    );
}
