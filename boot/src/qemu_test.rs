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

    let hello = system
        .boot_volume()
        .and_then(|root| root.open("/meridian/programs/hello.mexec"))
        .and_then(|file| file.info());
    uefi::qemu_test::Report::check(
        &mut report,
        "boot.programs.file",
        hello.is_ok_and(|i| !i.directory && i.size > 4096),
        "sextant should put the programs into meridian/programs",
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
    let bases: [Option<u64>; 2] = core::array::from_fn(|_| {
        crate::prepare(system)
            .ok()
            .map(|again| again.info.kernel.virtual_base)
    });
    let first = ready.info.kernel.virtual_base;
    report.check(
        "boot.prepare.random",
        bases.iter().all(Option::is_some) && bases.iter().any(|&b| b != Some(first)),
        "of three times the kernel is prepared, at least two should pick another base",
    );
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
            && kernel.virtual_base % (2 << 20) == 0
            && kernel.virtual_base >= boot::handover::KERNEL_AREA
            && kernel.virtual_base + kernel.size
                <= boot::handover::KERNEL_AREA + boot::handover::KERNEL_AREA_SIZE,
        "the kernel should be whole pages, on a 2 MiB boundary of the top 2 GiB",
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
        "boot.prepare.programs",
        info.modules.count == 2
            && info.modules.list >= bootinfo::HHDM_BASE
            && info.modules.list % PAGE_SIZE == 0,
        "hello and its copy on test images should be listed for the kernel",
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
pub fn systems_found(
    cards: &[boot::screen::Card<'_>],
    firmware_settings: bool,
    log: &boot::log::Log,
    serial: &mut impl Write,
) {
    use boot::screen::Kind;
    use uefi::qemu_test::Report;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    report.check(
        "boot.systems.found",
        cards.len() == 3
            && cards[0].kind == Kind::Meridian
            && cards[1].kind == Kind::Linux
            && cards[1].name == "example",
        "the example system on the test image should be found as a second card",
    );
    report.check(
        "boot.systems.firmware",
        cards
            .get(2)
            .is_some_and(|c| c.kind == Kind::Other && c.name == FIRMWARE_EXAMPLE),
        "the boot option the test image added should be the third card",
    );
    let said = |line: &str| log.lines().any(|l| l == line);
    report.check(
        "boot.log.found",
        said("Found example: /EFI/example/grubx64.efi")
            && said("Found Firmware example in the firmware's boot options: Boot0100, \\EFI\\other\\tool.efi"),
        "the log should name each system found, and where",
    );
    report.check(
        "boot.firmware_settings.offered",
        firmware_settings,
        "OVMF can open its settings at the next start, so F should be offered",
    );
    stop_on_failure(&mut report);
}

/// What a 64-bit variable holds: its attributes and value, if it is there.
fn read_u64(system: &System, name: &[u16]) -> Option<(uefi::variable::Attributes, Option<u64>)> {
    let mut buf = [0u8; 8];
    system
        .variable(name, &uefi::guid::GLOBAL_VARIABLE, &mut buf)
        .ok()
        .map(|(attributes, data)| (attributes, uefi::variable::u64_value(data)))
}

/// Asks for the firmware settings the way F does, without the restart,
/// checks what reached `OsIndications` and puts it back as it was.
pub fn firmware_settings_request(system: &System, serial: &mut impl Write) {
    use uefi::guid::GLOBAL_VARIABLE;
    use uefi::qemu_test::Report;
    use uefi::variable::{Attributes, OS_INDICATIONS};

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let lasting = Attributes::NON_VOLATILE
        .with(Attributes::BOOTSERVICE_ACCESS)
        .with(Attributes::RUNTIME_ACCESS);
    let original = read_u64(system, &OS_INDICATIONS);
    // Another bit first, which the request has to keep: the one that says
    // the firmware may check certificates' time stamps.
    let other = system.set_variable(
        &OS_INDICATIONS,
        &GLOBAL_VARIABLE,
        lasting,
        &2u64.to_le_bytes(),
    );
    let requested = crate::request_firmware_settings(system);
    report.check(
        "boot.firmware_settings.request",
        other.is_ok()
            && requested.is_ok()
            && read_u64(system, &OS_INDICATIONS) == Some((lasting, Some(3))),
        "F should add the settings bit to OsIndications, kept across a restart",
    );
    let undone = match original {
        Some((attributes, Some(bits))) => system.set_variable(
            &OS_INDICATIONS,
            &GLOBAL_VARIABLE,
            attributes,
            &bits.to_le_bytes(),
        ),
        _ => system.set_variable(
            &OS_INDICATIONS,
            &GLOBAL_VARIABLE,
            Attributes::default(),
            &[],
        ),
    };
    report.check(
        "boot.firmware_settings.undone",
        undone.is_ok() && read_u64(system, &OS_INDICATIONS) == original,
        "the test should leave OsIndications as it found it",
    );
    stop_on_failure(&mut report);
}

/// The test image's boot.meri sets every value away from its default:
/// 2500 ms, the example system, and the log first. The test image's boot
/// screen keeps to its own script, so these are only checked.
pub fn settings_read(
    settings: &boot::settings::Settings<'_>,
    default: usize,
    serial: &mut impl Write,
) {
    use boot::settings::Start;
    use uefi::qemu_test::Report;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    report.check(
        "boot.settings.read",
        settings.timeout == 3
            && settings.verbose
            && matches!(settings.start, Start::Named(name) if name == "example"),
        "the test image's boot.meri should be read: 2500ms, the example system, verbose",
    );
    report.check(
        "boot.settings.default",
        default == 1,
        "the example system, the second card, should be chosen first",
    );
    stop_on_failure(&mut report);
}

/// Remembers a system the way `default = last` does, reads it back, and
/// leaves nothing behind.
pub fn remember_last(system: &System, serial: &mut impl Write) {
    use boot::settings::{LAST_SYSTEM, Settings, Start};
    use uefi::guid::MERIDIAN_VARIABLE;
    use uefi::qemu_test::Report;
    use uefi::variable::Attributes;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let choices = crate::Choices {
        firmware_settings: false,
        settings: Settings {
            start: Start::Last,
            ..Settings::default()
        },
        default: 0,
    };
    let mut quiet = canvas::Text::<128>::new();
    let mut buf = [0u8; 64];
    let before = crate::remembered(system, &mut buf).is_some();
    crate::remember(system, &choices, None, "Probe system", &mut quiet);
    let mut buf = [0u8; 64];
    report.check(
        "boot.settings.last",
        !before
            && crate::remembered(system, &mut buf) == Some("Probe system")
            && quiet.as_str().is_empty(),
        "with default = last, the started system should be remembered in LastSystem",
    );
    let cleared = system.set_variable(&LAST_SYSTEM, &MERIDIAN_VARIABLE, Attributes::default(), &[]);
    let mut buf = [0u8; 64];
    report.check(
        "boot.settings.last.cleared",
        cleared.is_ok() && crate::remembered(system, &mut buf).is_none(),
        "the test should leave no remembered system behind",
    );
    stop_on_failure(&mut report);
}

/// The example system was started from the boot screen and came back.
pub fn other_came_back(system: &System, status: uefi::Status, serial: &mut impl Write) {
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
    let mut buf = [0u8; 64];
    report.check(
        "boot.settings.last.untouched",
        crate::remembered(system, &mut buf).is_none(),
        "the test image's settings name a system, so nothing should be remembered",
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
        // The log comes first; log_screen goes on from there.
        report.press("v");
    } else if round == 1 {
        // The firmware's example, the third card.
        report.press("right");
        report.press("right");
        report.press("ret");
    } else {
        report.press("ret");
    }
    if report.failed > 0 {
        let _ = writeln!(report.out, "TESTS DONE {} failed", report.failed);
        x86::raw::qemu_exit(false);
    }
}

/// Checks the log that V showed, keeps a picture of it, and goes back to
/// the menu to start the example system.
pub fn log_screen(system: &System, frame: &[u32], screen: &[u32], serial: &mut impl Write) {
    use boot::screen::{LOG_LINE, LOG_MARGIN, LOG_TOP};
    use canvas::palette::{NIGHT, TANGERINE};
    use uefi::qemu_test::Report;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let (night, tangerine) = (
        NIGHT.encode(canvas::Format::Bgr),
        TANGERINE.encode(canvas::Format::Bgr),
    );
    let width = system.framebuffer().map_or(0, |fb| fb.width as usize);
    let orange = frame.iter().filter(|&&p| p == tangerine).count();
    report.check(
        "boot.log.drawn",
        frame.first() == Some(&night) && (64 * 6..64 * 6 + 200).contains(&orange),
        "the log should be dark, with only the orange line under its title",
    );
    // The banner, the screen, Secure Boot, the firmware settings and the
    // example system make at least six lines.
    let line_ink = |line: usize| {
        let baseline = LOG_TOP + line * LOG_LINE;
        (baseline - 12..baseline + 3)
            .flat_map(|y| (LOG_MARGIN..LOG_MARGIN + 300).map(move |x| y * width + x))
            .filter(|&i| frame.get(i).is_some_and(|&p| p != night))
            .count()
    };
    report.check(
        "boot.log.lines",
        width > 0 && (0..6).all(|line| line_ink(line) > 50),
        "the log should show what the loader said, one line per row",
    );
    report.check(
        "boot.log.presented",
        width > 0 && screen.get(..width) == frame.get(..width),
        "the log should be on the screen",
    );
    let _ = writeln!(report.out, "SCREEN boot-log");
    let answer = system.wait_for_key();
    report.check(
        "boot.log.kept",
        answer.is_ok_and(|k| k.unicode_char == 0x0d),
        "sextant should answer SCREEN with a carriage return, read as Enter",
    );
    // Any key goes back to the menu; menu_after_log goes on from there.
    report.press("spc");
    if report.failed > 0 {
        let _ = writeln!(report.out, "TESTS DONE {} failed", report.failed);
        x86::raw::qemu_exit(false);
    }
}

/// Checks that a key closed the log and brought the menu back, then
/// starts the example system.
pub fn menu_after_log(frame: &[u32], serial: &mut impl Write) {
    use canvas::palette::TANGERINE;
    use uefi::qemu_test::Report;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let tangerine = TANGERINE.encode(canvas::Format::Bgr);
    report.check(
        "boot.log.closed",
        frame.iter().filter(|&&p| p == tangerine).count() > 5000,
        "a key on the log should bring the cards back",
    );
    report.press("right");
    report.press("ret");
    stop_on_failure(&mut report);
}

/// The description of the boot option the test image adds.
const FIRMWARE_EXAMPLE: &str = "Firmware example";
/// Its number, far from the ones OVMF uses.
const FIRMWARE_EXAMPLE_NUMBER: u16 = 0x0100;

/// Adds a boot option for the third copy of the loader to the firmware's
/// list, as an installer of another system would, before the loader looks
/// for systems. It also asks for it as `BootNext`, reads that back and
/// takes it back, since starting it that way needs a restart.
pub fn add_firmware_option(system: &System, serial: &mut impl Write) {
    use uefi::guid::GLOBAL_VARIABLE;
    use uefi::load_option::{self, ACTIVE, BOOT_NEXT};
    use uefi::qemu_test::Report;
    use uefi::variable::Attributes;

    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let lasting = Attributes::NON_VOLATILE
        .with(Attributes::BOOTSERVICE_ACCESS)
        .with(Attributes::RUNTIME_ACCESS);
    // The example the third card stands for, and two that must get no
    // card: an inactive one, and one for a loader the scan finds anyway.
    let options = [
        (
            FIRMWARE_EXAMPLE_NUMBER,
            ACTIVE,
            FIRMWARE_EXAMPLE,
            "/EFI/other/tool.efi",
        ),
        (
            FIRMWARE_EXAMPLE_NUMBER + 1,
            0,
            "Switched off",
            "/EFI/other/tool.efi",
        ),
        (
            FIRMWARE_EXAMPLE_NUMBER + 2,
            ACTIVE,
            "Scanned already",
            "/EFI/example/grubx64.efi",
        ),
    ];
    let mut added = true;
    for (number, attributes, description, file) in options {
        added &= write_option(system, number, attributes, description, file).is_ok();
    }
    let ordered = append_to_boot_order(system, &options.map(|o| o.0));
    report.check(
        "boot.firmware_option.added",
        added && ordered.is_ok(),
        "the test image should add its boot options and put them last in BootOrder",
    );
    let number = FIRMWARE_EXAMPLE_NUMBER;

    let asked = crate::request_boot_next(system, number);
    let mut next = [0u8; 2];
    let back = system
        .variable(&BOOT_NEXT, &GLOBAL_VARIABLE, &mut next)
        .ok()
        .map(|(attributes, data)| {
            (
                attributes,
                load_option::numbers(data).and_then(|mut n| n.next()),
            )
        });
    let undone = system.set_variable(&BOOT_NEXT, &GLOBAL_VARIABLE, Attributes::default(), &[]);
    let mut gone = [0u8; 2];
    report.check(
        "boot.firmware.boot_next",
        asked.is_ok()
            && back == Some((lasting, Some(number)))
            && undone.is_ok()
            && system
                .variable(&BOOT_NEXT, &GLOBAL_VARIABLE, &mut gone)
                .is_err(),
        "an option the loader cannot start itself should be asked for as BootNext",
    );
    stop_on_failure(&mut report);
}

/// Writes boot option `number` for `file` on this partition, as an
/// installer would.
fn write_option(
    system: &System,
    number: u16,
    attributes: u32,
    description: &str,
    file: &str,
) -> Result<(), uefi::Status> {
    use uefi::raw::image::PATH_BYTES;

    let mut base = [0u8; PATH_BYTES];
    let mut path = [0u8; PATH_BYTES];
    let base = system.boot_device_path(&mut base)?;
    let len = uefi::device_path::with_file(base, file, &mut path)
        .ok_or(uefi::Status::BUFFER_TOO_SMALL)?;
    write_option_at(system, number, attributes, description, &path[..len])
}

/// Writes boot option `number` for the loader at the whole device path
/// `path`.
fn write_option_at(
    system: &System,
    number: u16,
    attributes: u32,
    description: &str,
    path: &[u8],
) -> Result<(), uefi::Status> {
    let len = path.len();
    let mut option = [0u8; 2048];
    let mut at = 0;
    let mut put = |bytes: &[u8]| {
        option[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(&attributes.to_le_bytes());
    put(&u16::try_from(len).unwrap_or(0).to_le_bytes());
    for unit in description.encode_utf16().chain([0]) {
        put(&unit.to_le_bytes());
    }
    put(path);
    let lasting = uefi::variable::Attributes::NON_VOLATILE
        .with(uefi::variable::Attributes::BOOTSERVICE_ACCESS)
        .with(uefi::variable::Attributes::RUNTIME_ACCESS);
    system.set_variable(
        &uefi::load_option::variable(number),
        &uefi::guid::GLOBAL_VARIABLE,
        lasting,
        &option[..at],
    )
}

/// The boot options for the loader's copy on a second disk: one with its
/// whole device path, and one with the short path that starts at the
/// partition, which only the firmware can follow.
const SECOND_DISK: u16 = 0x0103;
const SECOND_DISK_SHORT: u16 = 0x0104;
const SECOND_DISK_NAMES: [&str; 2] = ["Second disk", "Second disk, short"];
/// Where the Go test puts the loader's copy on the second disk.
const SECOND_DISK_FILE: &str = "/EFI/second/loader.efi";

/// With a second disk, which the Go test of boot options on another drive
/// gives the machine: adds the two options for the loader's copy there,
/// finds them as the boot screen does, starts the one with the whole path
/// and sees it come back, then starts the short one, which the loader
/// leaves to the firmware with a restart. Without a second disk it does
/// nothing.
pub fn second_disk(system: &System, serial: &mut impl Write) {
    use uefi::device_path::{from_partition, partition, with_file};
    use uefi::load_option::ACTIVE;
    use uefi::qemu_test::Report;
    use uefi::raw::image::PATH_BYTES;

    let mut own = [0u8; PATH_BYTES];
    let own = system.boot_device_path(&mut own).ok().and_then(partition);
    let mut other = [0u8; PATH_BYTES];
    let mut other_len = 0;
    let _ = system.file_system_paths(|p| {
        let theirs = partition(p);
        if other_len == 0 && theirs.is_some() && theirs != own {
            other[..p.len()].copy_from_slice(p);
            other_len = p.len();
        }
    });
    if other_len == 0 {
        return;
    }
    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let mut whole = [0u8; PATH_BYTES];
    let whole = with_file(&other[..other_len], SECOND_DISK_FILE, &mut whole).map(|n| &whole[..n]);
    let added = whole.is_some_and(|whole| {
        let [name, short_name] = SECOND_DISK_NAMES;
        write_option_at(system, SECOND_DISK, ACTIVE, name, whole).is_ok()
            && from_partition(whole).is_some_and(|short| {
                write_option_at(system, SECOND_DISK_SHORT, ACTIVE, short_name, short).is_ok()
            })
            && append_to_boot_order(system, &[SECOND_DISK, SECOND_DISK_SHORT]).is_ok()
    });
    report.check(
        "boot.second_disk.options",
        added,
        "the options for the copy on the second disk should be written and put in BootOrder",
    );
    stop_on_failure(&mut report);

    let mut found: [Option<crate::Other>; boot::screen::SYSTEM_LIMIT] = Default::default();
    crate::find_systems(system, &mut found);
    let card = |number: u16| {
        found
            .iter()
            .flatten()
            .find(|o| o.option.as_ref().is_some_and(|f| f.number == number))
    };
    let (Some(whole), Some(short)) = (card(SECOND_DISK), card(SECOND_DISK_SHORT)) else {
        report.check(
            "boot.second_disk.found",
            false,
            "both options on the second disk should get cards",
        );
        stop_on_failure(&mut report);
        return;
    };
    report.check(
        "boot.second_disk.found",
        [whole.name.as_str(), short.name.as_str()] == SECOND_DISK_NAMES,
        "both options on the second disk should get cards with their descriptions",
    );
    let status = crate::start_other(system, whole, report.out);
    report.check(
        "boot.second_disk.whole_path",
        status == CHAINED,
        "the copy on the second disk should start from its whole path and come back",
    );
    stop_on_failure(&mut report);
    let status = crate::start_other(system, short, report.out);
    let _ = writeln!(report.out, "the short option came back with {status}");
    report.check(
        "boot.second_disk.restart",
        false,
        "the short option should have been left to the firmware with a restart",
    );
    stop_on_failure(&mut report);
}

/// In the copy on the second disk, when the firmware started it through
/// the short option after the restart: checks it came from the partition
/// that option names and that the firmware used up `BootNext`, then ends
/// the run.
pub fn after_restart(system: &System, serial: &mut impl Write) {
    use uefi::device_path::partition;
    use uefi::guid::GLOBAL_VARIABLE;
    use uefi::load_option::{self, BOOT_CURRENT, BOOT_NEXT, LoadOption};
    use uefi::qemu_test::Report;
    use uefi::raw::image::PATH_BYTES;

    let mut two = [0u8; 2];
    let current = system
        .variable(&BOOT_CURRENT, &GLOBAL_VARIABLE, &mut two)
        .ok()
        .and_then(|(_, data)| load_option::numbers(data)?.next());
    if current != Some(SECOND_DISK_SHORT) {
        return;
    }
    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    let mut own = [0u8; PATH_BYTES];
    let own = system.boot_device_path(&mut own).ok().and_then(partition);
    let mut data = [0u8; 2048];
    let named = system
        .variable(
            &load_option::variable(SECOND_DISK_SHORT),
            &GLOBAL_VARIABLE,
            &mut data,
        )
        .ok()
        .and_then(|(_, data)| LoadOption::parse(data))
        .and_then(|option| partition(option.path));
    let mut next = [0u8; 2];
    let used_up = system
        .variable(&BOOT_NEXT, &GLOBAL_VARIABLE, &mut next)
        .is_err();
    report.check(
        "boot.second_disk.short_path",
        own.is_some() && own == named && used_up,
        "after the restart the firmware should start the copy from the partition the short option names",
    );
    let _ = writeln!(report.out, "TESTS DONE {} failed", report.failed);
    x86::raw::qemu_exit(report.failed == 0);
}

/// Puts `numbers` at the end of `BootOrder`.
fn append_to_boot_order(system: &System, numbers: &[u16]) -> Result<(), uefi::Status> {
    use uefi::guid::GLOBAL_VARIABLE;
    use uefi::load_option::BOOT_ORDER;
    use uefi::variable::Attributes;

    let mut order = [0u8; 256];
    let mut listed = system
        .variable(&BOOT_ORDER, &GLOBAL_VARIABLE, &mut order)
        .map_or(0, |(_, data)| data.len());
    for number in numbers {
        let slot = order
            .get_mut(listed..listed + 2)
            .ok_or(uefi::Status::BUFFER_TOO_SMALL)?;
        slot.copy_from_slice(&number.to_le_bytes());
        listed += 2;
    }
    let lasting = Attributes::NON_VOLATILE
        .with(Attributes::BOOTSERVICE_ACCESS)
        .with(Attributes::RUNTIME_ACCESS);
    system.set_variable(&BOOT_ORDER, &GLOBAL_VARIABLE, lasting, &order[..listed])
}

/// Where the Go test of enrolling a key puts the write for each Secure
/// Boot database, in the order the writes have to be made, with the name
/// of the check for each.
const KEY_FILES: [(&str, &[u16], &uefi::Guid, &str); 3] = [
    (
        "/meridian/keys/db.auth",
        &uefi::variable::ALLOWED,
        &uefi::guid::IMAGE_SECURITY_DATABASE,
        "boot.enroll.db",
    ),
    (
        "/meridian/keys/KEK.auth",
        &uefi::variable::KEY_EXCHANGE_KEY,
        &uefi::guid::GLOBAL_VARIABLE,
        "boot.enroll.kek",
    ),
    (
        "/meridian/keys/PK.auth",
        &uefi::variable::PLATFORM_KEY,
        &uefi::guid::GLOBAL_VARIABLE,
        "boot.enroll.pk",
    ),
];
/// Copies of the loader the Go test leaves unsigned, and signs with a key
/// nobody enrolled.
const UNSIGNED_COPY: &str = "/EFI/unsigned/tool.efi";
const FOREIGN_COPY: &str = "/EFI/foreign/tool.efi";

/// When the partition carries the writes of the Go test of enrolling a
/// key: in Setup Mode it makes them and restarts; after that, with Secure
/// Boot on, it checks that the firmware refuses a program the key did not
/// sign and starts one it did, and ends the run. Without them it does
/// nothing.
pub fn enrollment(system: &System, serial: &mut impl Write) {
    let Ok(root) = system.boot_volume() else {
        return;
    };
    if root.open(KEY_FILES[2].0).is_err() {
        return;
    }
    let mut report = Serial {
        out: serial,
        failed: 0,
    };
    if flag(system, &uefi::variable::SETUP_MODE) == Some(1) {
        enroll(system, &root, &mut report);
    } else {
        enforced(system, &mut report);
    }
}

/// A one byte global variable such as `SetupMode`.
fn flag(system: &System, name: &[u16]) -> Option<u8> {
    let mut one = [0u8; 1];
    system
        .variable(name, &uefi::guid::GLOBAL_VARIABLE, &mut one)
        .ok()
        .and_then(|(_, data)| data.first().copied())
}

fn enroll<W: Write>(system: &System, root: &uefi::raw::file::File<'_>, report: &mut Serial<'_, W>) {
    use uefi::qemu_test::Report;
    use uefi::variable::Attributes;

    let attributes = Attributes::NON_VOLATILE
        .with(Attributes::BOOTSERVICE_ACCESS)
        .with(Attributes::RUNTIME_ACCESS)
        .with(Attributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS);
    for (file, name, vendor, check) in KEY_FILES {
        let mut data = [0u8; 4096];
        let written = root
            .open(file)
            .and_then(|mut f| f.read_all(&mut data))
            .and_then(|n| system.set_variable(name, vendor, attributes, &data[..n]));
        report.check(
            check,
            written.is_ok(),
            "the firmware should take the signed write while in Setup Mode",
        );
    }
    report.check(
        "boot.enroll.user_mode",
        flag(system, &uefi::variable::SETUP_MODE) == Some(0),
        "writing the platform key should end Setup Mode",
    );
    stop_on_failure(report);
    let status = system.reset(uefi::raw::table::ResetType::COLD);
    report.check(
        "boot.enroll.restart",
        false,
        "the machine should restart to turn Secure Boot on",
    );
    let _ = writeln!(report.out, "the restart came back with {status}");
    stop_on_failure(report);
}

fn enforced<W: Write>(system: &System, report: &mut Serial<'_, W>) {
    use uefi::Status;
    use uefi::qemu_test::Report;

    report.check(
        "boot.enroll.secure_boot",
        flag(system, &uefi::variable::SECURE_BOOT) == Some(1)
            && flag(system, &uefi::variable::SETUP_MODE) == Some(0),
        "after the restart Secure Boot should be on, with the key enrolled",
    );
    let refused = |file| {
        matches!(
            system.load_image(file).err(),
            Some(Status::ACCESS_DENIED | Status::SECURITY_VIOLATION)
        )
    };
    report.check(
        "boot.enroll.unsigned_refused",
        refused(UNSIGNED_COPY),
        "the firmware should refuse a copy of the loader nobody signed",
    );
    report.check(
        "boot.enroll.foreign_refused",
        refused(FOREIGN_COPY),
        "the firmware should refuse a copy signed with a key it does not know",
    );
    let started = system.load_image(boot::LOADER_PATH).and_then(|mut image| {
        image.set_options(&CHAIN_OPTIONS)?;
        Ok(image.start())
    });
    report.check(
        "boot.enroll.signed_started",
        started == Ok(CHAINED),
        "the loader, signed with the enrolled key, should start again under Secure Boot",
    );
    let _ = writeln!(report.out, "TESTS DONE {} failed", report.failed);
    x86::raw::qemu_exit(report.failed == 0);
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
