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
use boot::screen::{Card, Cards, Kind, SYSTEM_LIMIT};
use boot::settings::{self, Settings, Start};
use boot::{KERNEL_PATH, Out, banner, firmware, memory, systems};
use bootinfo::{
    BootInfo, KERNEL_MEMORY_TYPE, KernelImage, MemoryRegion, STACK_PAGES, STACK_TOP, Stack,
};
use canvas::Text;
use canvas::{Canvas, Format, Scratch};
use core::fmt::Write;
use uefi::Status;
use uefi::device_path;
use uefi::guid::{ACPI_20_TABLE, GLOBAL_VARIABLE, MERIDIAN_VARIABLE};
use uefi::load_option::{self, BOOT_CURRENT, BOOT_NEXT, BOOT_ORDER, LoadOption};
use uefi::memory::{MemoryType, PAGE_BYTES, PAGE_SIZE};
use uefi::raw::System;
use uefi::raw::file::EntryBuffer;
use uefi::raw::graphics::PixelFormat;
use uefi::raw::image::PATH_BYTES;
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
    let mut supported = [0u8; 8];
    let firmware_settings = system
        .variable(&OS_INDICATIONS_SUPPORTED, &GLOBAL_VARIABLE, &mut supported)
        .is_ok_and(|(_, data)| offers_firmware_ui(data));
    let _ = describe(&system, firmware_settings, &mut log.tee(&mut out));
    let mut settings_file = [0u8; settings::LIMIT];
    let settings = read_settings(&system, &mut settings_file, &mut log.tee(&mut out));

    #[cfg(feature = "qemu-test")]
    {
        qemu_test::checks(&system, serial);
        qemu_test::add_firmware_option(&system, serial);
    }

    let mut others: [Option<Other>; SYSTEM_LIMIT] = Default::default();
    find_systems(&system, &mut others);
    let cards = Cards::new(others.iter().flatten().map(|o| Card {
        kind: o.kind,
        name: o.name.as_str(),
    }));
    let cards = cards.as_slice();
    log_found(&others, &mut log.tee(serial));
    let mut last_name = [0u8; 64];
    let last = remembered(&system, &mut last_name);
    let choices = Choices {
        firmware_settings,
        settings,
        default: settings::chosen(settings.start, cards.iter().map(|c| c.name), last),
    };
    #[cfg(feature = "qemu-test")]
    {
        qemu_test::systems_found(cards, firmware_settings, &log, serial);
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
        let status = start_other(&system, other, &mut log.tee(serial));
        let _ = writeln!(
            log.tee(serial),
            "{} came back with {status}",
            other.name.as_str()
        );
        #[cfg(feature = "qemu-test")]
        qemu_test::other_came_back(&system, status, serial);
        round += 1;
    }

    let (system, problem) = match prepare(&system) {
        Ok(ready) => raw::start(system, ready),
        Err(problem) => (system, problem),
    };
    give_up(&system, &problem, serial)
}

/// The log's lines about the machine, read from the firmware.
fn describe(system: &System, firmware_settings: bool, out: &mut impl Write) -> core::fmt::Result {
    let screen = system.framebuffer().ok().map(|fb| (fb.width, fb.height));
    let mut buf = [0u8; 8];
    let secure_boot = system
        .variable(&SECURE_BOOT, &GLOBAL_VARIABLE, &mut buf)
        .map(|(_, data)| data);
    boot::describe(out, screen, secure_boot, firmware_settings)
}

/// Another system, found on the partition or among the firmware's boot
/// options.
struct Other {
    kind: Kind,
    name: Text<48>,
    /// Its loader, from the root of its partition.
    path: Text<160>,
    /// The boot option it is, when the firmware listed it; otherwise its
    /// loader is on this partition.
    option: Option<FirmwareOption>,
}

/// One of the firmware's boot options: its number, and the device path of
/// the loader it starts.
struct FirmwareOption {
    number: u16,
    device: [u8; PATH_BYTES],
    len: usize,
}

/// Writes each system found to the log, with where it was found.
fn log_found(others: &[Option<Other>], out: &mut impl Write) {
    for other in others.iter().flatten() {
        let _ = match &other.option {
            None => writeln!(
                out,
                "Found {}: {}",
                other.name.as_str(),
                other.path.as_str()
            ),
            Some(FirmwareOption { number, .. }) => writeln!(
                out,
                "Found {} in the firmware's boot options: Boot{number:04X}, {}",
                other.name.as_str(),
                other.path.as_str()
            ),
        };
    }
}

/// Adds the systems among the firmware's boot options that have no card
/// yet to `found`.
fn find_firmware_systems(system: &System, found: &mut [Option<Other>; SYSTEM_LIMIT]) {
    let mut two = [0u8; 2];
    let current = system
        .variable(&BOOT_CURRENT, &GLOBAL_VARIABLE, &mut two)
        .ok()
        .and_then(|(_, data)| load_option::numbers(data)?.next());
    let mut own = [0u8; PATH_BYTES];
    let own = system.boot_device_path(&mut own).ok();
    let mut order = [0u8; 256];
    let Some(order) = system
        .variable(&BOOT_ORDER, &GLOBAL_VARIABLE, &mut order)
        .ok()
        .and_then(|(_, data)| load_option::numbers(data))
    else {
        return;
    };
    for number in order {
        if found.iter().all(Option::is_some) {
            break;
        }
        let mut data = [0u8; 2048];
        let Some(option) = system
            .variable(&load_option::variable(number), &GLOBAL_VARIABLE, &mut data)
            .ok()
            .and_then(|(_, data)| LoadOption::parse(data))
        else {
            continue;
        };
        let known =
            |key: &str| firmware::known_here(key, found.iter().flatten().map(|o| o.path.as_str()));
        if firmware::verdict(&option, number, current, own, known) != firmware::Verdict::Offered {
            continue;
        }
        let (Some(file), Some(mut path)) = (
            device_path::file(option.path).and_then(firmware::key),
            device_path::file(option.path).map(|chars| {
                let mut text = Text::<160>::new();
                for c in chars {
                    let _ = text.write_char(c);
                }
                text
            }),
        ) else {
            continue;
        };
        if path.as_str().is_empty() {
            let _ = path.write_str(file.as_str());
        }
        let mut device = [0u8; PATH_BYTES];
        let Some(slot) = device.get_mut(..option.path.len()) else {
            continue;
        };
        slot.copy_from_slice(option.path);
        let other = Other {
            kind: firmware::kind(file.as_str()),
            name: firmware::name(&option, file.as_str()),
            path,
            option: Some(FirmwareOption {
                number,
                device,
                len: option.path.len(),
            }),
        };
        put(found, other);
    }
}

/// Looks through the folders of \EFI for the loaders of other systems,
/// then through the firmware's boot options, and puts what it finds into
/// `found`.
fn find_systems(system: &System, found: &mut [Option<Other>; SYSTEM_LIMIT]) {
    scan_partition(system, found);
    find_firmware_systems(system, found);
}

/// Puts `other` into the first free place of `found`, if there is one.
fn put(found: &mut [Option<Other>], other: Other) {
    if let Some(slot) = found.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(other);
    }
}

/// Looks through the folders of \EFI on the partition this loader came
/// from for the loaders of other systems.
fn scan_partition(system: &System, found: &mut [Option<Other>; SYSTEM_LIMIT]) {
    let Ok(root) = system.boot_volume() else {
        return;
    };
    let Ok(mut efi) = root.open("EFI") else {
        return;
    };
    let mut buf: EntryBuffer = [0; size_of::<EntryBuffer>()];
    while found.iter().any(Option::is_none) {
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
            put(
                found,
                Other {
                    kind,
                    name,
                    path,
                    option: None,
                },
            );
        }
    }
}

/// Starts another system's loader and returns its status if it comes
/// back. The test image hands it the options that make the loader's own
/// copy return at once. A firmware boot option whose path the firmware
/// cannot load from here, such as the short paths Windows writes, is left
/// to the firmware: it starts it after a restart.
fn start_other(system: &System, other: &Other, out: &mut impl Write) -> Status {
    let image = match &other.option {
        None => system.load_image(other.path.as_str()),
        Some(option) => system.load_image_at(&option.device[..option.len]),
    };
    #[cfg(feature = "qemu-test")]
    let image = image.and_then(|mut image| {
        image.set_options(qemu_test::chain_options())?;
        Ok(image)
    });
    match (image, &other.option) {
        (Ok(image), _) => image.start(),
        (Err(status), Some(option)) if firmware::needs_the_firmware(status) => {
            let _ = writeln!(
                out,
                "{} cannot be started from here ({status}); the firmware starts it after a restart",
                other.name.as_str()
            );
            match request_boot_next(system, option.number) {
                Ok(()) => system.reset(ResetType::COLD),
                Err(refused) => refused,
            }
        }
        (Err(status), _) => status,
    }
}

/// Asks the firmware to start boot option `number` at the next start, once.
fn request_boot_next(system: &System, number: u16) -> Result<(), Status> {
    system.set_variable(&BOOT_NEXT, &GLOBAL_VARIABLE, LASTING, &number.to_le_bytes())
}

/// Variables that survive a restart and that the system can read later.
const LASTING: Attributes = Attributes::NON_VOLATILE
    .with(Attributes::BOOTSERVICE_ACCESS)
    .with(Attributes::RUNTIME_ACCESS);

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

/// Reads `\meridian\boot.meri` into `buf` and says in the log what it
/// found. Without the file, or with one that cannot be read, the defaults
/// count.
fn read_settings<'b>(system: &System, buf: &'b mut [u8], out: &mut impl Write) -> Settings<'b> {
    let read = system.boot_volume().and_then(|root| {
        let mut file = root.open(settings::PATH)?;
        if !settings::fits(file.info()?.size) {
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
                "Settings: boot.meri is longer than {} bytes, so the defaults count",
                settings::LIMIT
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
    if let Err(status) = system.set_variable(
        &settings::LAST_SYSTEM,
        &MERIDIAN_VARIABLE,
        LASTING,
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
            let back = Canvas::new(frame.words(), width, height, width, format);
            let front = Canvas::new(
                screen.as_mut_slice(),
                width,
                height,
                fb.stride as usize,
                format,
            );
            if let (Some(back), Some(mut front)) = (back, front) {
                back.copy_to(&mut front, 0, 0, width, height);
            }
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
    let wanted = (asked | BOOT_TO_FIRMWARE_UI).to_le_bytes();
    system.set_variable(&OS_INDICATIONS, &GLOBAL_VARIABLE, LASTING, &wanted)
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

/// Room in kernel memory for one frame of `screen`, or 0 without either.
/// The kernel draws its first frame there and copies it whole, so the boot
/// screen stays until that frame replaces it; without the room it draws
/// on the screen itself.
fn back_buffer(system: &System, screen: Option<uefi::raw::graphics::Framebuffer>) -> u64 {
    screen
        .and_then(|fb| {
            let bytes = u64::from(fb.width) * u64::from(fb.height) * 4;
            system.allocate_pages(KERNEL_TYPE, pages_for(bytes)).ok()
        })
        .map_or(0, |pages| pages.start())
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
    let back_buffer = back_buffer(system, screen);
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
        back_buffer,
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
