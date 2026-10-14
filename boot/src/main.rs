//! The Meridian bootloader, a UEFI application. It says what it runs on,
//! loads the kernel from `\meridian\kernel` next to itself, builds the
//! kernel's address space, leaves boot services and jumps
//! (docs/architecture/boot.md).

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(feature = "qemu-test")]
mod qemu_test;
#[allow(unsafe_code)]
mod raw;

use core::mem::size_of;

use boot::handover::{self, Layout, Problem};
use boot::log::Log;
use boot::menu::{Action, Key, Menu};
use boot::screen::{Card, Kind};
use boot::settings::{self, Settings, Start};
use boot::{KERNEL_PATH, Out, banner, memory, systems};
use bootinfo::{
    BootInfo, KERNEL_MEMORY_TYPE, KernelImage, MemoryRegion, STACK_PAGES, STACK_TOP, Stack,
};
use canvas::Text;
use canvas::{Canvas, Format, Scratch};
use core::fmt::Write;
use uefi::Status;
use uefi::guid::{ACPI_20_TABLE, GLOBAL_VARIABLE, MERIDIAN_VARIABLE};
use uefi::memory::{MemoryType, PAGE_BYTES, PAGE_SIZE};
use uefi::raw::System;
use uefi::raw::file::EntryBuffer;
use uefi::raw::graphics::PixelFormat;
use uefi::raw::table::ResetType;
use uefi::variable::{
    Attributes, BOOT_TO_FIRMWARE_UI, OS_INDICATIONS, OS_INDICATIONS_SUPPORTED, SECURE_BOOT,
    offers_firmware_ui, u64_value,
};
use x86::Features;
use x86::raw::Com1;

/// The most regions the boot information holds. Firmware maps have a few
/// hundred descriptors at most, and the loader merges neighbours of a kind.
const REGION_LIMIT: usize = 512;

const KERNEL_TYPE: MemoryType = MemoryType(KERNEL_MEMORY_TYPE);

fn run(system: System, serial: &mut Com1) -> Status {
    let mut log = Log::new();
    let mut out = Out {
        screen: system.console(),
        serial: &mut *serial,
    };
    if let Some(screen) = out.screen.as_mut() {
        let _ = screen.clear();
    }
    let _ = banner(
        &mut log.tee(&mut out),
        system.firmware_vendor(),
        system.firmware_revision(),
        system.header().revision,
    );
    let firmware_settings = firmware_settings_offered(&system);
    let _ = describe(&system, firmware_settings, &mut log.tee(&mut out));
    let mut settings_file = [0u8; SETTINGS_LIMIT];
    let settings = read_settings(&system, &mut settings_file, &mut log.tee(&mut out));

    #[cfg(feature = "qemu-test")]
    qemu_test::checks(&system, serial);

    let mut others: [Option<Other>; SYSTEM_LIMIT] = Default::default();
    let found = find_systems(&system, &mut others);
    let mut cards = [Card {
        kind: Kind::Meridian,
        name: "Meridian",
    }; SYSTEM_LIMIT + 1];
    for (card, other) in cards[1..].iter_mut().zip(others.iter().flatten()) {
        *card = Card {
            kind: other.kind,
            name: other.name.as_str(),
        };
    }
    let cards = &cards[..=found];
    for other in others.iter().flatten() {
        let _ = writeln!(
            log.tee(serial),
            "Found {}: {}",
            other.name.as_str(),
            other.path.as_str()
        );
    }
    let mut last_name = [0u8; 64];
    let last = remembered(&system, &mut last_name);
    let choices = Choices {
        firmware_settings,
        settings,
        default: settings::chosen(settings.start, cards.iter().map(|c| c.name), last),
    };
    #[cfg(feature = "qemu-test")]
    {
        qemu_test::systems_found(cards, firmware_settings, serial);
        qemu_test::firmware_settings_request(&system, serial);
        qemu_test::settings_read(&choices.settings, choices.default, serial);
        qemu_test::remember_last(&system, serial);
    }

    // Another system that comes back, or fails to start, brings the boot
    // screen back.
    let mut round = 0;
    loop {
        let chosen = show_menu(&system, cards, &choices, &log, round, serial);
        let chosen = match chosen {
            Choice::System(chosen) => {
                remember(
                    &system,
                    &choices,
                    last,
                    cards[chosen].name,
                    &mut log.tee(serial),
                );
                chosen
            }
            Choice::FirmwareSettings => {
                let status = open_firmware_settings(&system);
                let _ = writeln!(
                    log.tee(serial),
                    "The firmware would not open its settings: {status}"
                );
                continue;
            }
        };
        let Some(other) = chosen.checked_sub(1).and_then(|i| others[i].as_ref()) else {
            break;
        };
        let status = start_other(&system, other.path.as_str());
        let _ = writeln!(
            log.tee(serial),
            "{} came back with {status}",
            other.name.as_str()
        );
        #[cfg(feature = "qemu-test")]
        qemu_test::other_came_back(status, serial);
        round += 1;
    }

    let (system, problem) = match prepare(&system) {
        Ok(ready) => raw::start(system, ready),
        Err(problem) => (system, problem),
    };
    give_up(&system, &problem, serial)
}

/// The most other systems the boot screen offers.
const SYSTEM_LIMIT: usize = 8;

/// The log's lines about the machine, read from the firmware.
fn describe(system: &System, firmware_settings: bool, out: &mut impl Write) -> core::fmt::Result {
    let screen = system.framebuffer().ok().map(|fb| (fb.width, fb.height));
    let mut buf = [0u8; 8];
    let secure_boot = system
        .variable(&SECURE_BOOT, &GLOBAL_VARIABLE, &mut buf)
        .map(|(_, data)| data);
    boot::describe(out, screen, secure_boot, firmware_settings)
}

/// Another system found on the partition.
struct Other {
    kind: Kind,
    name: Text<48>,
    /// Its loader, from the root of the partition.
    path: Text<160>,
}

/// Looks through the folders of \EFI for the loaders of other systems and
/// returns how many it put into `found`.
fn find_systems(system: &System, found: &mut [Option<Other>; SYSTEM_LIMIT]) -> usize {
    let Ok(root) = system.boot_volume() else {
        return 0;
    };
    let Ok(mut efi) = root.open("EFI") else {
        return 0;
    };
    let mut buf: EntryBuffer = [0; size_of::<EntryBuffer>()];
    let mut count = 0;
    while count < SYSTEM_LIMIT {
        let Ok(Some(entry)) = efi.next_entry(&mut buf) else {
            break;
        };
        if !entry.info.directory {
            continue;
        }
        let mut folder = Text::<64>::new();
        for c in entry.name() {
            let _ = folder.write_char(c);
        }
        let folder = folder.as_str();
        let exists = |file: &str| {
            let mut path = Text::<160>::new();
            write!(path, "EFI/{folder}/{file}").is_ok() && root.open(path.as_str()).is_ok()
        };
        if let Some((kind, file)) = systems::classify(folder, exists) {
            let mut name = Text::new();
            let _ = name.write_str(systems::display_name(folder, kind));
            let mut path = Text::new();
            let _ = write!(path, "/EFI/{folder}/{file}");
            found[count] = Some(Other { kind, name, path });
            count += 1;
        }
    }
    count
}

/// Starts another system's loader and returns its status if it comes
/// back. The test image hands it the options that make the loader's own
/// copy return at once.
fn start_other(system: &System, path: &str) -> Status {
    let image = system.load_image(path);
    #[cfg(feature = "qemu-test")]
    let image = image.and_then(|mut image| {
        image.set_options(qemu_test::chain_options())?;
        Ok(image)
    });
    match image {
        Ok(image) => image.start(),
        Err(status) => status,
    }
}

/// The boot screen in the test image: no countdown, and the checks of the
/// first menu, the first log and the menu after it.
#[cfg(feature = "qemu-test")]
fn show_menu(
    system: &System,
    cards: &[Card<'_>],
    choices: &Choices<'_>,
    log: &Log,
    round: u32,
    serial: &mut Com1,
) -> Choice {
    let menu = Menu::new(cards.len(), 0, 0).with_firmware_settings(choices.firmware_settings);
    let (mut menus, mut logs) = (0, 0);
    choose(
        system,
        cards,
        menu,
        log,
        |shown, frame, screen| match shown {
            Shown::Menu => {
                menus += 1;
                match (menus, logs) {
                    (1, _) => qemu_test::boot_screen(system, round, frame, screen, serial),
                    (2, 1) => qemu_test::menu_after_log(frame, serial),
                    _ => {}
                }
            }
            Shown::Log => {
                logs += 1;
                if logs == 1 {
                    qemu_test::log_screen(system, frame, screen, serial);
                }
            }
        },
    )
}

/// The boot screen as the settings want it.
#[cfg(not(feature = "qemu-test"))]
fn show_menu(
    system: &System,
    cards: &[Card<'_>],
    choices: &Choices<'_>,
    log: &Log,
    _: u32,
    _: &mut Com1,
) -> Choice {
    let menu = Menu::new(cards.len(), choices.default, choices.settings.timeout)
        .with_firmware_settings(choices.firmware_settings)
        .with_log_open(choices.settings.verbose);
    choose(system, cards, menu, log, |_, _, _| {})
}

/// What the boot screen needs besides the cards.
struct Choices<'a> {
    firmware_settings: bool,
    settings: Settings<'a>,
    /// The card chosen when the screen opens.
    default: usize,
}

/// The most bytes of `boot.meri` the loader reads.
const SETTINGS_LIMIT: usize = 4096;

/// Reads `\meridian\boot.meri` into `buf` and says in the log what it
/// found. Without the file, or with one that cannot be read, the defaults
/// count.
fn read_settings<'b>(system: &System, buf: &'b mut [u8], out: &mut impl Write) -> Settings<'b> {
    let read = system.boot_volume().and_then(|root| {
        let mut file = root.open(settings::PATH)?;
        if file.info()?.size > buf.len() as u64 {
            return Err(Status::BUFFER_TOO_SMALL);
        }
        file.read_all(buf)
    });
    let settings = match read {
        Ok(len) => {
            let (settings, problem) = settings::read(&buf[..len]);
            if let Some(problem) = problem {
                let _ = writeln!(out, "Settings: boot.meri {problem}; the defaults stand in");
            }
            settings
        }
        Err(Status::NOT_FOUND) => {
            let _ = writeln!(
                out,
                "Settings: there is no boot.meri, so the defaults count"
            );
            Settings::default()
        }
        Err(Status::BUFFER_TOO_SMALL) => {
            let _ = writeln!(
                out,
                "Settings: boot.meri is longer than {SETTINGS_LIMIT} bytes, so the defaults count"
            );
            Settings::default()
        }
        Err(status) => {
            let _ = writeln!(out, "Settings: boot.meri could not be read: {status}");
            Settings::default()
        }
    };
    let _ = writeln!(out, "Settings: {settings}");
    settings
}

/// The name of the system started last, if the loader remembered one.
fn remembered<'b>(system: &System, buf: &'b mut [u8]) -> Option<&'b str> {
    let (_, name) = system
        .variable(&settings::LAST_SYSTEM, &MERIDIAN_VARIABLE, buf)
        .ok()?;
    core::str::from_utf8(name).ok()
}

/// Remembers which system starts, when the settings start the last one
/// and it is not the one remembered already. Firmware variables live in
/// flash, so nothing is written that would not change.
fn remember(
    system: &System,
    choices: &Choices<'_>,
    last: Option<&str>,
    name: &str,
    out: &mut impl Write,
) {
    if !matches!(choices.settings.start, Start::Last) || last == Some(name) {
        return;
    }
    let lasting = Attributes::NON_VOLATILE
        .with(Attributes::BOOTSERVICE_ACCESS)
        .with(Attributes::RUNTIME_ACCESS);
    if let Err(status) = system.set_variable(
        &settings::LAST_SYSTEM,
        &MERIDIAN_VARIABLE,
        lasting,
        name.as_bytes(),
    ) {
        let _ = writeln!(out, "Remembering {name} for next time failed: {status}");
    }
}

/// What the person chose on the boot screen.
enum Choice {
    System(usize),
    FirmwareSettings,
}

/// What a frame on the boot screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    Menu,
    Log,
}

/// Shows the boot screen until a system is chosen, or the firmware's
/// settings. Every frame is drawn off the screen first and then copied
/// whole. `on_frame` sees each finished frame, what it shows and the
/// screen after it. Without a screen, or without memory for the frame, the
/// first system starts at once.
fn choose(
    system: &System,
    cards: &[Card<'_>],
    mut menu: Menu,
    log: &Log,
    mut on_frame: impl FnMut(Shown, &[u32], &[u32]),
) -> Choice {
    let Ok(mut screen) = system.pixels() else {
        return Choice::System(0);
    };
    let fb = screen.fb;
    let (width, height) = (fb.width as usize, fb.height as usize);
    let Ok(mut frame) = system.allocate_pages(
        MemoryType::LOADER_DATA,
        (width * height * 4).div_ceil(PAGE_BYTES),
    ) else {
        return Choice::System(0);
    };
    let format = match fb.format {
        PixelFormat::Rgb => Format::Rgb,
        PixelFormat::Bgr => Format::Bgr,
    };
    let mut scratch = Scratch::new();
    let mut action = Action::Redraw;
    loop {
        let shown = match action {
            Action::Start(chosen) => {
                let _ = frame.free();
                return Choice::System(chosen);
            }
            Action::OpenFirmwareSettings => {
                let _ = frame.free();
                return Choice::FirmwareSettings;
            }
            Action::Redraw | Action::ShowLog => {
                let shown = if menu.log_open() {
                    Shown::Log
                } else {
                    Shown::Menu
                };
                if let Some(mut canvas) = Canvas::new(frame.words(), width, height, width, format) {
                    let _ = match shown {
                        Shown::Menu => boot::screen::draw(&mut canvas, cards, &menu, &mut scratch),
                        Shown::Log => {
                            let footer = boot::screen::log_footer(&menu, cards);
                            boot::screen::draw_log(&mut canvas, log, footer.as_str(), &mut scratch)
                        }
                    };
                }
                Some(shown)
            }
            Action::Wait => None,
        };
        if let Some(shown) = shown {
            boot::screen::present(
                frame.words(),
                screen.as_mut_slice(),
                width,
                height,
                fb.stride as usize,
            );
            on_frame(shown, frame.words(), screen.as_slice());
        }
        action = if let Ok(Some(key)) = system.read_key() {
            menu.key(Key::from_uefi(key))
        } else {
            let _ = system.stall(100_000);
            menu.tick(1)
        };
    }
}

/// Whether the firmware can be asked to open its settings at the next
/// start.
fn firmware_settings_offered(system: &System) -> bool {
    let mut buf = [0u8; 8];
    system
        .variable(&OS_INDICATIONS_SUPPORTED, &GLOBAL_VARIABLE, &mut buf)
        .is_ok_and(|(_, data)| offers_firmware_ui(data))
}

/// Asks the firmware to open its settings at the next start and restarts
/// the machine. Comes back only when the firmware refused.
fn open_firmware_settings(system: &System) -> Status {
    match request_firmware_settings(system) {
        Ok(()) => system.reset(ResetType::COLD),
        Err(status) => status,
    }
}

/// Sets the bit in `OsIndications` that asks the firmware to open its
/// settings at the next start, keeping the other bits.
fn request_firmware_settings(system: &System) -> Result<(), Status> {
    let mut buf = [0u8; 8];
    let asked = system
        .variable(&OS_INDICATIONS, &GLOBAL_VARIABLE, &mut buf)
        .ok()
        .and_then(|(_, data)| u64_value(data))
        .unwrap_or(0);
    let lasting = Attributes::NON_VOLATILE
        .with(Attributes::BOOTSERVICE_ACCESS)
        .with(Attributes::RUNTIME_ACCESS);
    let wanted = (asked | BOOT_TO_FIRMWARE_UI).to_le_bytes();
    system.set_variable(&OS_INDICATIONS, &GLOBAL_VARIABLE, lasting, &wanted)
}

/// Everything the jump needs, worked out while boot services still run.
struct Ready {
    root: u64,
    entry: u64,
    info: BootInfo,
    /// The pages the boot information goes into, and where they are.
    page: &'static mut [u8],
    page_at: u64,
    /// Room for the final memory map.
    map: &'static mut [u8],
}

fn firmware(doing: &'static str) -> impl FnOnce(Status) -> Problem {
    move |status| Problem::Firmware { doing, status }
}

fn pages_for(bytes: u64) -> usize {
    usize::try_from(bytes.div_ceil(PAGE_SIZE)).unwrap_or(usize::MAX)
}

/// Loads the kernel, gives it a stack and builds its page tables.
fn prepare(system: &System) -> Result<Ready, Problem> {
    let root = system
        .boot_volume()
        .map_err(firmware("opening the boot volume"))?;
    let mut file = root
        .open(KERNEL_PATH)
        .map_err(firmware("opening the kernel file"))?;
    let size = file
        .info()
        .map_err(firmware("reading the kernel file's size"))?
        .size;
    let mut buf = system
        .allocate_pages(MemoryType::LOADER_DATA, pages_for(size))
        .map_err(firmware("finding memory for the kernel file"))?;
    let got = file
        .read_all(buf.bytes())
        .map_err(firmware("reading the kernel file"))?;
    // A file cut short fails the ELF checks below, so the count read is
    // all that matters here.
    let data: &[u8] = &buf.bytes()[..got];
    let elf = elf::Elf::parse(data).map_err(Problem::Kernel)?;
    let (first, span) = elf.span().map_err(Problem::Kernel)?;
    let segments = elf.segments().map_err(Problem::Kernel)?;
    let entry = elf.entry();
    if !handover::entry_is_code(segments.as_slice(), entry) {
        return Err(Problem::BadEntry(entry));
    }
    let mut image = system
        .allocate_pages(KERNEL_TYPE, pages_for(span))
        .map_err(firmware("finding memory for the kernel"))?;
    elf.load(image.bytes(), first).map_err(Problem::Kernel)?;
    let _ = buf.free();

    let stack = system
        .allocate_pages(KERNEL_TYPE, pages_for(STACK_PAGES * PAGE_SIZE))
        .map_err(firmware("finding memory for the kernel's stack"))?;
    let loader = system
        .image_range()
        .map_err(firmware("finding the loader in memory"))?;
    let map_size = system
        .memory_map_size()
        .map_err(firmware("reading the memory map's size"))?;
    let map = system
        .allocate_pages(
            MemoryType::LOADER_DATA,
            handover::map_buffer_pages(map_size),
        )
        .map_err(firmware("finding memory for the memory map"))?
        .keep();
    let screen = system.framebuffer().ok();
    let memory_top = memory::memory_top(
        system
            .memory_map(map)
            .map_err(firmware("reading the memory map"))?
            .descriptors(),
    );

    let layout = Layout {
        kernel_virtual: first,
        kernel_physical: image.start(),
        segments: segments.as_slice(),
        stack_physical: stack.start(),
        loader,
        memory_top,
        framebuffer: screen.map_or((0, 0), |fb| (fb.base, fb.size)),
        features: Features::detect(),
    };
    let count = usize::try_from(handover::frames_needed(&layout)).unwrap_or(usize::MAX);
    let mut frames = raw::FirmwareFrames::new(system, count)
        .map_err(firmware("finding memory for page tables"))?;
    let space = handover::build(&mut frames, &layout).map_err(Problem::Tables)?;

    let info_bytes = size_of::<BootInfo>() + REGION_LIMIT * size_of::<MemoryRegion>();
    let page = system
        .allocate_pages(KERNEL_TYPE, info_bytes.div_ceil(PAGE_BYTES))
        .map_err(firmware("finding memory for the boot information"))?;
    let page_at = page.start();
    let info = BootInfo {
        kernel: KernelImage {
            physical: image.start(),
            virtual_base: first,
            size: span,
        },
        stack: Stack {
            bottom: STACK_TOP - STACK_PAGES * PAGE_SIZE,
            top: STACK_TOP,
        },
        rsdp: system.configuration_table(&ACPI_20_TABLE).unwrap_or(0),
        uefi_system_table: system.table_address(),
        framebuffer: handover::framebuffer(screen),
        ..BootInfo::new()
    };
    Ok(Ready {
        root: space.root(),
        entry,
        info,
        page: page.keep(),
        page_at,
        map,
    })
}

#[cfg(feature = "qemu-test")]
fn give_up(_: &System, problem: &Problem, serial: &mut Com1) -> Status {
    qemu_test::kernel_not_started(problem, serial)
}

#[cfg(not(feature = "qemu-test"))]
fn give_up(system: &System, problem: &Problem, serial: &mut Com1) -> Status {
    use core::fmt::Write;

    let mut out = Out {
        screen: system.console(),
        serial,
    };
    let _ = writeln!(out, "\nMeridian could not start: {problem}.");
    let _ = writeln!(out, "Press any key to return to the firmware.");
    match system.wait_for_key() {
        Ok(_) => Status::LOAD_ERROR,
        Err(status) => status,
    }
}
