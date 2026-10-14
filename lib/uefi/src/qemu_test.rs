//! Checks of the firmware wrappers against real firmware (OVMF), run from
//! a test image in QEMU. Each check reports its name, its result and what a
//! failure means.
//!
//! OVMF mirrors its text console to the serial port, so everything written
//! here also shows up in the serial log, where sextant compares it with
//! [`CONSOLE_PROBE`].

use core::fmt::Write;

use crate::Status;
use crate::guid::{GLOBAL_VARIABLE, MERIDIAN_VARIABLE};
use crate::memory::{DESCRIPTOR_SIZE, MemoryMap, MemoryType, PAGE_BYTES, PAGE_SIZE};
use crate::raw::{File, System};
use crate::text::scan;
use crate::variable::{
    Attributes, BOOT_TO_FIRMWARE_UI, OS_INDICATIONS_SUPPORTED, SECURE_BOOT, name, u64_value,
};

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
    files(system, report);
    tables(system, report);
    graphics(system, report);
    images(system, report);
    variables(system, report);
    boot_options(system, report);

    report.check(
        "uefi.keyboard.nothing_waiting",
        system.read_key() == Ok(None),
        "with no key pressed, reading should find nothing and not wait",
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
    report.press("left");
    let mut left = None;
    for _ in 0..200 {
        if let Ok(Some(key)) = system.read_key() {
            left = Some(key);
            break;
        }
        let _ = system.stall(10_000);
    }
    report.check(
        "uefi.keyboard.read_key",
        left.is_some_and(|k| k.scan_code == scan::LEFT),
        "a key pressed while polling should be read without waiting for it",
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
    console_on_screen(system, report);
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

/// Where sextant puts the loader on every image it builds.
const LOADER_PATH: &str = "/EFI/BOOT/BOOTX64.EFI";

fn files(system: &System, report: &mut impl Report) {
    let Ok(root) = system.boot_volume() else {
        report.check(
            "uefi.file.volume",
            false,
            "the volume the loader came from should open",
        );
        return;
    };
    report.check("uefi.file.volume", true, "");
    report.check(
        "uefi.file.root",
        root.info().is_ok_and(|i| i.directory),
        "the root of the volume should say it is a directory",
    );
    report.check(
        "uefi.file.missing",
        root.open("/no/such/file").err() == Some(Status::NOT_FOUND),
        "opening a file that is not there should say NOT_FOUND",
    );
    let long = [b'a'; 300];
    report.check(
        "uefi.file.long_path",
        root.open(core::str::from_utf8(&long).unwrap_or("")).err()
            == Some(Status::INVALID_PARAMETER),
        "a path longer than the limit should be refused before the firmware sees it",
    );

    long_name(&root, report);

    let Ok(mut loader) = root.open(LOADER_PATH) else {
        report.check("uefi.file.open", false, "the loader's own file should open");
        return;
    };
    report.check("uefi.file.open", true, "");
    let size = match loader.info() {
        Ok(info) if !info.directory && info.size > 4096 => info.size,
        _ => {
            report.check(
                "uefi.file.info",
                false,
                "the loader's file should be a file of some size",
            );
            return;
        }
    };
    report.check("uefi.file.info", true, "");
    directories(&root, size, report);
    listing(&root, report);

    let pages = usize::try_from(size.div_ceil(PAGE_SIZE)).unwrap_or(0) + 1;
    let Ok(mut buf) = system.allocate_pages(MemoryType::LOADER_DATA, pages) else {
        report.check("uefi.file.read", false, "no memory to read the loader into");
        return;
    };
    let read = loader.read_all(buf.bytes());
    report.check(
        "uefi.file.read",
        read.is_ok_and(|n| n as u64 == size),
        "reading into a larger buffer should stop at the end of the file",
    );
    report.check(
        "uefi.file.eof",
        loader.read(buf.bytes()) == Ok(0),
        "a read at the end of the file should return nothing",
    );
    let bytes = buf.bytes();
    let pe_at = usize::try_from(u32::from_le_bytes([
        bytes[0x3c],
        bytes[0x3d],
        bytes[0x3e],
        bytes[0x3f],
    ]))
    .unwrap_or(0);
    report.check(
        "uefi.file.contents",
        bytes.starts_with(b"MZ") && bytes.get(pe_at..pe_at + 4) == Some(b"PE\0\0"),
        "the loader's file should read back as a PE image",
    );
    report.check(
        "uefi.file.chunks",
        root.open("efi/boot/bootx64.efi")
            .is_ok_and(|again| same_in_chunks(again, &bytes[..pages * PAGE_BYTES - PAGE_BYTES])),
        "the file should read the same in small pieces through a relative, lower case path",
    );
    let _ = buf.free();
}

/// Whether reading `file` 1000 bytes at a time gives `want`, partly
/// beyond the file's end; `want` covers whole pages, so its tail is zero.
fn same_in_chunks(mut file: File<'_>, want: &[u8]) -> bool {
    let mut chunk = [0u8; 1000];
    let mut at = 0;
    loop {
        match file.read(&mut chunk) {
            Ok(0) => return want[at..].iter().all(|&b| b == 0),
            Ok(n) if at + n <= want.len() && chunk[..n] == want[at..at + n] => at += n,
            _ => return false,
        }
    }
}

/// Lists the EFI folder, which holds BOOT, and BOOT, which holds the
/// loader.
fn listing(root: &File<'_>, report: &mut impl Report) {
    use crate::raw::file::EntryBuffer;

    let mut buf: EntryBuffer = [0; size_of::<EntryBuffer>()];
    let (mut boot, mut dots, mut count) = (false, 0, 0);
    if let Ok(mut efi) = root.open("EFI") {
        while let Ok(Some(entry)) = efi.next_entry(&mut buf) {
            count += 1;
            boot |= entry.is_named("BOOT") && entry.info.directory;
            dots += usize::from(entry.is_named(".") || entry.is_named(".."));
        }
    }
    report.check(
        "uefi.file.list",
        boot && dots == 2 && count >= 3,
        "the EFI folder should list ., .. and the BOOT folder",
    );
    let mut loader = false;
    if let Ok(mut folder) = root.open("EFI/BOOT") {
        while let Ok(Some(entry)) = folder.next_entry(&mut buf) {
            loader |=
                entry.is_named("bootx64.efi") && !entry.info.directory && entry.info.size > 4096;
        }
        report.check(
            "uefi.file.list_end",
            folder.next_entry(&mut buf).is_ok_and(|e| e.is_none()),
            "a listing that has ended should stay ended",
        );
    }
    report.check(
        "uefi.file.list_files",
        loader,
        "the BOOT folder should list the loader with its size",
    );
}

/// Opens the loader again one directory at a time, and twice at once.
fn directories(root: &File<'_>, size: u64, report: &mut impl Report) {
    let efi = root.open("EFI");
    report.check(
        "uefi.file.directory",
        efi.as_ref()
            .is_ok_and(|d| d.info().is_ok_and(|i| i.directory)),
        "the EFI folder should open and say it is a directory",
    );
    let loader = efi
        .and_then(|efi| efi.open("BOOT"))
        .and_then(|boot| boot.open("BOOTX64.EFI"));
    report.check(
        "uefi.file.relative",
        loader
            .as_ref()
            .is_ok_and(|f| f.info().is_ok_and(|i| i.size == size)),
        "opening relative to a directory should reach the same file",
    );
    let Ok(mut first) = loader else {
        return;
    };
    report.check(
        "uefi.file.empty_read",
        first.read(&mut []) == Ok(0),
        "a read into no room should return nothing and succeed",
    );

    let Ok(mut second) = root.open(LOADER_PATH) else {
        report.check(
            "uefi.file.positions",
            false,
            "the loader should open a second time",
        );
        return;
    };
    let (mut a, mut b, mut c) = ([0u8; 16], [0u8; 16], [0u8; 16]);
    let reads = (first.read(&mut a), second.read(&mut b), first.read(&mut c));
    report.check(
        "uefi.file.positions",
        reads == (Ok(16), Ok(16), Ok(16)) && a == b && a != c && a.starts_with(b"MZ"),
        "each open file should keep its own position",
    );
}

/// Something in the image's own data, to find the image in memory by.
static MARKER: u8 = 0x4d;

fn tables(system: &System, report: &mut impl Report) {
    let acpi = system.configuration_table(&crate::guid::ACPI_20_TABLE);
    report.check(
        "uefi.config.acpi",
        acpi.is_some_and(|a| a != 0),
        "OVMF publishes ACPI 2.0 tables",
    );
    report.check(
        "uefi.config.missing",
        system
            .configuration_table(&crate::guid::FILE_INFO)
            .is_none(),
        "a GUID that names no table should find nothing",
    );

    let marker = (&raw const MARKER) as u64;
    let function: fn(&MemoryMap<'_>, MemoryType, u64, u64) -> bool = covers;
    let code = function as usize as u64;
    report.check(
        "uefi.image.range",
        system.image_range().is_ok_and(|(base, size)| {
            (base..base + size).contains(&marker) && (base..base + size).contains(&code)
        }),
        "the loaded image's range should hold its own code and data",
    );
}

fn graphics(system: &System, report: &mut impl Report) {
    let fb = system.framebuffer();
    report.check(
        "uefi.gop",
        fb.is_ok_and(|fb| fb.width >= 640 && fb.height >= 480 && fb.is_consistent()),
        "OVMF should offer a framebuffer of at least 640 by 480",
    );
    report.check(
        "uefi.gop.format",
        fb.is_ok_and(|fb| fb.format == crate::raw::graphics::PixelFormat::Bgr),
        "QEMU's standard VGA under OVMF stores pixels as blue, green, red",
    );
    let Ok(mut pixels) = system.pixels() else {
        report.check(
            "uefi.gop.write",
            false,
            "the framebuffer's pixels should be reachable",
        );
        return;
    };
    report.check(
        "uefi.gop.one_at_a_time",
        system.pixels().err() == Some(Status::ACCESS_DENIED),
        "a second way to write the pixels must wait for the first",
    );
    let (width, height, stride) = (
        pixels.fb.width as usize,
        pixels.fb.height as usize,
        pixels.fb.stride as usize,
    );
    let row = &mut pixels.as_mut_slice()[(height - 1) * stride..][..width];
    let mut kept = [0u32; 64];
    kept.copy_from_slice(&row[..64]);
    for (i, p) in (0u32..).zip(row[..64].iter_mut()) {
        *p = 0x00c0_ffee ^ i;
    }
    let back = (0u32..).zip(&row[..64]).all(|(i, &p)| p == 0x00c0_ffee ^ i);
    row[..64].copy_from_slice(&kept);
    report.check(
        "uefi.gop.write",
        back,
        "pixels written to the last row should read back",
    );
    report.check(
        "uefi.gop.same_mode",
        system.framebuffer() == Ok(pixels.fb),
        "the pixels should describe the mode the firmware reports",
    );
    corners(&mut pixels, report);
    drop(pixels);
    report.check(
        "uefi.gop.released",
        system.pixels().is_ok(),
        "the pixels should be free again once the first way is gone",
    );
}

/// Writes the four corners and the pixel under each top corner, then
/// reads them back. A wrong stride or row count shows up here as a pixel
/// written in the wrong place or outside the framebuffer.
fn corners(pixels: &mut crate::raw::graphics::Pixels<'_>, report: &mut impl Report) {
    let (w, h, stride) = (
        pixels.fb.width as usize,
        pixels.fb.height as usize,
        pixels.fb.stride as usize,
    );
    let spots = [
        (0, 0),
        (w - 1, 0),
        (0, h - 1),
        (w - 1, h - 1),
        (0, 1),
        (w - 1, 1),
    ];
    let slice = pixels.as_mut_slice();
    let mut kept = [0u32; 6];
    for (k, &(x, y)) in kept.iter_mut().zip(&spots) {
        *k = slice[y * stride + x];
    }
    for (i, &(x, y)) in (0u32..).zip(&spots) {
        slice[y * stride + x] = 0x0010_2030 + i;
    }
    let back = (0u32..)
        .zip(&spots)
        .all(|(i, &(x, y))| slice[y * stride + x] == 0x0010_2030 + i);
    for (&k, &(x, y)) in kept.iter().zip(&spots) {
        slice[y * stride + x] = k;
    }
    report.check(
        "uefi.gop.corners",
        back && slice.len() == stride * (h - 1) + w,
        "each corner and the row below the top should hold its own pixel",
    );
}

/// The boot partition's device path, and loading images from it without
/// starting them: the loader's test image starts one, with options.
fn images(system: &System, report: &mut impl Report) {
    use crate::device_path::nodes;
    use crate::raw::image::PATH_BYTES;

    let mut path = [0u8; PATH_BYTES];
    let base = system.boot_device_path(&mut path).map(<[u8]>::len);
    let whole = base
        .ok()
        .and_then(|n| nodes(&path[..n]).map(|nodes| (n, nodes.len())));
    report.check(
        "uefi.image.device_path",
        whole.is_some_and(|(n, before_end)| before_end + 4 == n && n > 4),
        "the boot partition's device path should be nodes and an end node",
    );
    let last_type = whole.and_then(|(_, before_end)| {
        let nodes = &path[..before_end];
        let mut at = 0;
        let mut last = None;
        while at + 4 <= nodes.len() {
            last = Some(nodes[at]);
            at += usize::from(u16::from_le_bytes([nodes[at + 2], nodes[at + 3]])).max(4);
        }
        last
    });
    report.check(
        "uefi.image.partition",
        last_type == Some(4),
        "the path should end at a media node: the partition or the CD's boot image",
    );
    let mut tiny = [0u8; 8];
    let mut exact = [0u8; PATH_BYTES];
    let fits = whole.is_some_and(|(n, _)| {
        system.boot_device_path(&mut exact[..n]).is_ok()
            && system.boot_device_path(&mut exact[..n - 1]).err() == Some(Status::BUFFER_TOO_SMALL)
    });
    report.check(
        "uefi.image.device_path_exact",
        fits,
        "a buffer exactly as long as the path should hold it, and one byte less not",
    );
    report.check(
        "uefi.image.device_path_room",
        system.boot_device_path(&mut tiny).err() == Some(Status::BUFFER_TOO_SMALL),
        "a buffer too small for the path should be refused, not cut",
    );
    report.check(
        "uefi.image.missing",
        system.load_image("/EFI/no/such.efi").err() == Some(Status::NOT_FOUND),
        "loading a file that is not there should say NOT_FOUND",
    );
    report.check(
        "uefi.image.not_efi",
        system.load_image("/meridian/kernel").is_err(),
        "the kernel is an ELF file, which the firmware should refuse to load",
    );
    report.check(
        "uefi.image.folder",
        system.load_image("/EFI/BOOT").is_err(),
        "a folder is no image",
    );
    let options: [u16; 4] = [u16::from(b'a'), 0x20ac, u16::from(b'b'), 0];
    let set = system.load_image(LOADER_PATH).and_then(|mut image| {
        let before = image.options().0.len();
        image.set_options(&options)?;
        Ok((before, image.options().0 == options))
    });
    report.check(
        "uefi.image.set_options",
        set.is_ok_and(|(before, after)| before == 0 && after),
        "options handed to a loaded image should be what it reads",
    );
    let fresh = system
        .load_image(LOADER_PATH)
        .map(|image| image.options().0.is_empty());
    report.check(
        "uefi.image.own_options",
        fresh == Ok(true) && system.load_options() != options,
        "options belong to the image they were set on, not to the next or to this one",
    );
    let handle = system.load_image(LOADER_PATH).map(|image| {
        let handle = image.handle();
        (handle, system.is_loaded(handle))
    });
    report.check(
        "uefi.image.unloaded",
        handle.is_ok_and(|(handle, while_held)| while_held && !system.is_loaded(handle)),
        "an image dropped without starting should be unloaded",
    );
    let first = system.load_image(LOADER_PATH).map(drop);
    let again = system.load_image(LOADER_PATH).map(drop);
    report.check(
        "uefi.image.load",
        first.is_ok() && again.is_ok(),
        "the loader's own file should load, unload unstarted, and load again",
    );
}

fn variables(system: &System, report: &mut impl Report) {
    let runtime = Attributes::BOOTSERVICE_ACCESS.with(Attributes::RUNTIME_ACCESS);
    let mut buf = [0u8; 32];
    let supported = system
        .variable(&OS_INDICATIONS_SUPPORTED, &GLOBAL_VARIABLE, &mut buf)
        .ok()
        .filter(|(attributes, _)| attributes.contains(runtime))
        .and_then(|(_, data)| u64_value(data));
    report.check(
        "uefi.variable.read",
        supported.is_some(),
        "OVMF has OsIndicationsSupported: 8 bytes, readable at runtime",
    );
    report.check(
        "uefi.variable.firmware_ui",
        supported.is_some_and(|bits| bits & BOOT_TO_FIRMWARE_UI != 0),
        "OVMF can open its settings at the next start",
    );
    let mut small = [0u8; 4];
    report.check(
        "uefi.variable.too_small",
        system
            .variable(&OS_INDICATIONS_SUPPORTED, &GLOBAL_VARIABLE, &mut small)
            .err()
            == Some(Status::BUFFER_TOO_SMALL),
        "8 bytes do not fit in 4",
    );
    let secure_boot = system.variable(&SECURE_BOOT, &GLOBAL_VARIABLE, &mut buf);
    report.check(
        "uefi.variable.secure_boot",
        matches!(secure_boot, Ok((_, [0])) | Err(Status::NOT_FOUND)),
        "the test firmware does not check signatures",
    );

    let probe: [u16; 14] = name("MeridianProbe");
    report.check(
        "uefi.variable.missing",
        system.variable(&probe, &MERIDIAN_VARIABLE, &mut buf).err() == Some(Status::NOT_FOUND),
        "a variable nobody set is not found",
    );
    let wrote = system.set_variable(&probe, &MERIDIAN_VARIABLE, runtime, b"probe 1");
    let back = system.variable(&probe, &MERIDIAN_VARIABLE, &mut buf);
    report.check(
        "uefi.variable.write",
        wrote.is_ok() && back == Ok((runtime, b"probe 1".as_slice())),
        "a variable that was set reads back with its attributes and data",
    );
    let deleted = system.set_variable(&probe, &MERIDIAN_VARIABLE, Attributes::default(), &[]);
    report.check(
        "uefi.variable.delete",
        deleted.is_ok()
            && system.variable(&probe, &MERIDIAN_VARIABLE, &mut buf).err()
                == Some(Status::NOT_FOUND),
        "setting no data with no attributes deletes the variable",
    );
}

fn boot_options(system: &System, report: &mut impl Report) {
    use crate::load_option::{self, BOOT_CURRENT, BOOT_ORDER, LoadOption};

    let mut buf = [0u8; 128];
    let mut order = [0u16; 64];
    let mut count = 0;
    if let Some(numbers) = system
        .variable(&BOOT_ORDER, &GLOBAL_VARIABLE, &mut buf)
        .ok()
        .and_then(|(_, data)| load_option::numbers(data))
    {
        for (slot, n) in order.iter_mut().zip(numbers) {
            *slot = n;
            count += 1;
        }
    }
    let order = &order[..count];
    report.check(
        "uefi.boot_order",
        !order.is_empty(),
        "OVMF lists its boot options in BootOrder",
    );
    let mut two = [0u8; 2];
    let current = system
        .variable(&BOOT_CURRENT, &GLOBAL_VARIABLE, &mut two)
        .ok()
        .and_then(|(_, data)| load_option::numbers(data)?.next());
    report.check(
        "uefi.boot_current",
        current.is_some_and(|n| order.contains(&n)),
        "the option the firmware started this image with should be in BootOrder",
    );
    let mut whole = 0;
    for &n in order {
        let mut data = [0u8; 1024];
        let option = system
            .variable(&load_option::variable(n), &GLOBAL_VARIABLE, &mut data)
            .ok()
            .and_then(|(_, data)| LoadOption::parse(data));
        whole += usize::from(option.is_some_and(|o| {
            o.description().next().is_some() && crate::device_path::walk(o.path).is_some()
        }));
    }
    report.check(
        "uefi.boot_options",
        !order.is_empty() && whole == order.len(),
        "every option in BootOrder should read as a description and a whole device path",
    );
    report.check(
        "uefi.image.broken_path",
        system.load_image_at(&[4, 4, 2, 0]).err() == Some(Status::INVALID_PARAMETER),
        "a device path without an end node should not reach the firmware",
    );
}

/// The test image keeps a file at its root whose path, with the leading
/// slash, is 255 characters: as long as [`crate::raw::file::PATH_LIMIT`]
/// allows, with a name long enough to fill most of what its information
/// can hold.
fn long_name(root: &File<'_>, report: &mut impl Report) {
    let mut path = [b'x'; 255];
    path[0] = b'/';
    path[1..11].copy_from_slice(b"long-name-");
    let path = core::str::from_utf8(&path).unwrap_or("");
    let mut text = [0u8; 16];
    let read = root.open(path).and_then(|mut file| {
        let size = file.info()?.size;
        let n = file.read_all(&mut text)?;
        Ok((size, n))
    });
    report.check(
        "uefi.file.long_name",
        read == Ok((10, 10)) && &text[..10] == b"long name\n",
        "a file whose path is as long as a path may be should open, report its size and read",
    );
}

/// OVMF draws its text console into the framebuffer, somewhere it chooses,
/// so what the console does shows there: the screen is one colour after
/// clearing, has ink after text, and is one colour again after the next
/// clear.
fn console_on_screen(system: &System, report: &mut impl Report) {
    let Some(mut console) = system.console() else {
        return;
    };
    let plain = || -> Option<bool> {
        let mut pixels = system.pixels().ok()?;
        let screen = pixels.as_mut_slice();
        let first = *screen.first()?;
        Some(screen.iter().all(|&p| p == first))
    };
    let _ = console.clear();
    let blank = plain();
    let _ = console.write_str("MMMMMMMM");
    let written = plain();
    let _ = console.clear();
    let cleared = plain();
    report.check(
        "uefi.console.on_screen",
        blank == Some(true) && written == Some(false) && cleared == Some(true),
        "text should show in the framebuffer, and clearing should take it away",
    );
}
