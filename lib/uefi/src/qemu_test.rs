//! Checks of the firmware wrappers against real firmware (OVMF), run from
//! a test image in QEMU. Each check reports its name, its result and what a
//! failure means.
//!
//! OVMF mirrors its text console to the serial port, so everything written
//! here also shows up in the serial log, where sextant compares it with
//! [`CONSOLE_PROBE`].

use core::fmt::Write;

use crate::Status;
use crate::memory::{DESCRIPTOR_SIZE, MemoryMap, MemoryType, PAGE_BYTES, PAGE_SIZE};
use crate::raw::System;
use crate::text::scan;

/// A type from the range UEFI leaves to the operating system, so the test
/// finds its own allocations in the memory map and nothing else.
const MARKED: MemoryType = MemoryType(0x8000_7e57);

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
    // Distributions name their builds differently ("EDK II" on Arch, with
    // the distribution in front on Ubuntu), but every OVMF says EDK II.
    let mut vendor = [0u8; 128];
    let mut len = 0;
    for c in system.firmware_vendor() {
        if len < vendor.len() && c.is_ascii() {
            vendor[len] = c as u8;
            len += 1;
        }
    }
    let vendor = core::str::from_utf8(&vendor[..len]).unwrap_or("");
    report.check(
        "uefi.vendor",
        vendor.contains("EDK II"),
        "an OVMF vendor name contains EDK II; reading the vendor string is broken",
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

    memory(system, report);

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

fn covers(map: &MemoryMap<'_>, ty: MemoryType, start: u64, pages: u64) -> bool {
    map.descriptors()
        .any(|d| d.ty == ty && d.physical_start <= start && d.end() >= start + pages * PAGE_SIZE)
}

fn memory(system: &System, report: &mut impl Report) {
    let size = system.memory_map_size();
    report.check(
        "uefi.memory_map.size",
        size.is_ok_and(|s| s >= DESCRIPTOR_SIZE),
        "GetMemoryMap should report a map of at least one descriptor",
    );
    let pages = size.unwrap_or(0).div_ceil(PAGE_BYTES) + 1;
    let Ok(mut buf) = system.allocate_pages(MemoryType::LOADER_DATA, pages) else {
        report.check(
            "uefi.pages.allocate",
            false,
            "AllocatePages failed for the map buffer",
        );
        return;
    };
    report.check(
        "uefi.pages.allocate",
        buf.start() != 0 && buf.start() % PAGE_SIZE == 0 && buf.count() == pages,
        "pages should come back page aligned and as many as asked for",
    );
    report.check(
        "uefi.pages.cleared",
        buf.bytes().iter().all(|&b| b == 0),
        "new pages should be cleared",
    );

    let Ok(marked) = system.allocate_pages(MARKED, 3) else {
        report.check(
            "uefi.pages.own_type",
            false,
            "AllocatePages refused an operating system type",
        );
        return;
    };
    let Ok(map) = system.memory_map(buf.bytes()) else {
        report.check(
            "uefi.memory_map.read",
            false,
            "GetMemoryMap failed with room to spare",
        );
        return;
    };
    report.check(
        "uefi.memory_map.read",
        map.descriptor_size >= DESCRIPTOR_SIZE && map.descriptor_version == 1,
        "the map should use version 1 descriptors of at least 40 bytes",
    );
    report.check(
        "uefi.memory_map.length",
        map.bytes.len() % map.descriptor_size.max(1) == 0 && map.descriptors().all(|d| d.pages > 0),
        "the map should end after its last descriptor, without empty entries",
    );
    report.check(
        "uefi.pages.own_type",
        covers(&map, MARKED, marked.start(), 3),
        "the map should show the three pages with the type they were allocated as",
    );
    let (start, key) = (marked.start(), map.key);

    report.check(
        "uefi.pages.free",
        marked.free().is_ok(),
        "FreePages refused pages it handed out",
    );
    match system.memory_map(buf.bytes()) {
        Ok(map) => {
            report.check(
                "uefi.memory_map.key",
                map.key != key,
                "the map key should change when the map does",
            );
            report.check(
                "uefi.pages.freed",
                !covers(&map, MARKED, start, 1),
                "freed pages should no longer show their old type",
            );
        }
        Err(_) => report.check(
            "uefi.memory_map.key",
            false,
            "GetMemoryMap failed the second time",
        ),
    }

    let mut tiny = [0u8; 8];
    report.check(
        "uefi.memory_map.too_small",
        system.memory_map(&mut tiny).err() == Some(Status::BUFFER_TOO_SMALL),
        "a buffer of 8 bytes should be refused as too small",
    );

    reuse(system, report);
    let _ = buf.free();
}

/// OVMF hands out the pages it just got back, so this sees whether old
/// contents survive into a new allocation.
fn reuse(system: &System, report: &mut impl Report) {
    if let Ok(mut dirty) = system.allocate_pages(MemoryType::LOADER_DATA, 1) {
        let at = dirty.start();
        dirty.bytes().fill(0xa5);
        let again = dirty
            .free()
            .ok()
            .and_then(|()| system.allocate_pages(MemoryType::LOADER_DATA, 1).ok());
        report.check(
            "uefi.pages.cleared_again",
            again.is_some_and(|mut p| p.start() == at && p.bytes().iter().all(|&b| b == 0)),
            "a page allocated again should come back cleared",
        );
    }
}
