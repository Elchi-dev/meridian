//! Checks of the firmware wrappers against real firmware (OVMF), run from
//! a test image in QEMU. Each check reports its name, its result and what a
//! failure means.
//!
//! OVMF mirrors its text console to the serial port, so everything written
//! here also shows up in the serial log, where sextant compares it with
//! [`CONSOLE_PROBE`].

use core::fmt::Write;

use crate::raw::System;

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

pub fn run(system: &System, check: &mut impl FnMut(&str, bool, &str)) {
    check(
        "uefi.vendor",
        system.firmware_vendor().eq("EDK II".chars()),
        "OVMF names itself EDK II; reading the vendor string is broken",
    );
    check(
        "uefi.table.revision",
        system.header().revision >> 16 == 2,
        "the system table should claim UEFI 2.x",
    );
    check(
        "uefi.stall",
        system.stall(1_000).is_ok() && system.stall(0).is_ok(),
        "Stall refused 1000 or 0 microseconds",
    );

    let Some(mut console) = system.console() else {
        check("uefi.console", false, "OVMF always has a text console");
        return;
    };
    check("uefi.console", true, "");
    check(
        "uefi.console.clear",
        console.clear().is_ok(),
        "ClearScreen failed",
    );
    check(
        "uefi.console.write",
        writeln!(console, "{CONSOLE_PROBE}").is_ok(),
        "OutputString failed on a text longer than one piece",
    );
    check(
        "uefi.console.unknown_glyph",
        writeln!(console, "outside the basic plane: \u{1F600}").is_ok(),
        "a character the font lacks must only be a warning",
    );
}
