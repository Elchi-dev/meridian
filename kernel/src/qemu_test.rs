//! The kernel built as a QEMU test image. The loader runs its own checks,
//! starts this kernel only when they all passed, and the kernel then checks
//! what it was handed and ends the run with `TESTS DONE`, in the format
//! sextant reads.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use bootinfo::{BootInfo, HHDM_BASE, MemoryRegion, RegionKind, STACK_PAGES, STACK_TOP};
use canvas::Canvas;
use canvas::palette::{GRASS, INK, NIGHT, SUN, TANGERINE};
use kernel::display::Display;
use kernel::machine::Machine;
use kernel::panic::Details;
use kernel::{bytes_of, covered, is_tidy};
use x86::raw::Com1;

use crate::raw::disk::CHECKS;
use crate::raw::interrupts::expected::{self, Caught};
use crate::raw::serial::Serial;
use crate::raw::{self, mapped, physical};

struct Report<'a, W: Write> {
    out: &'a mut W,
    failed: u32,
}

impl<W: Write> Report<'_, W> {
    fn check(&mut self, name: &str, ok: bool, why: &str) {
        if ok {
            let _ = writeln!(self.out, "TEST {name} ok");
        } else {
            self.failed += 1;
            let _ = writeln!(self.out, "TEST {name} FAILED {why}");
        }
    }
}

pub fn run(
    info: &BootInfo,
    regions: &[MemoryRegion],
    machine: &Machine,
    tsc_hz: Option<u64>,
    display: Option<Display<'static>>,
    free: Option<usize>,
    serial: &mut Serial,
) -> ! {
    let mut report = Report {
        out: serial,
        failed: 0,
    };
    report.check("kernel.entered", true, "");
    let screen = display.as_ref().map(Display::screen);

    let info_at = core::ptr::from_ref(info) as u64;
    report.check(
        "kernel.bootinfo.direct_map",
        info_at >= HHDM_BASE && info.hhdm == HHDM_BASE,
        "the boot information should be passed as an address in the direct map",
    );
    report.check(
        "kernel.memory.tidy",
        !regions.is_empty() && is_tidy(regions),
        "the memory map should be sorted, without overlaps",
    );
    report.check(
        "kernel.memory.usable",
        bytes_of(regions, RegionKind::USABLE) >= 128 << 20,
        "the test machines have at least 256 MiB, so at least half should be free",
    );
    report.check(
        "kernel.memory.owned",
        covered(
            regions,
            RegionKind::KERNEL,
            info.kernel.physical,
            info.kernel.size,
        ) && covered(
            regions,
            RegionKind::KERNEL,
            info_at - HHDM_BASE,
            size_of::<BootInfo>() as u64,
        ),
        "the kernel image and the boot information should be in kernel memory",
    );
    report.check(
        "kernel.image.mapped",
        info.kernel.size >= 4096
            && mapped(info.kernel.virtual_base, 4096) == physical(info.kernel.physical, 4096)
            && (info.kernel.virtual_base..info.kernel.virtual_base + info.kernel.size)
                .contains(&(run as *const () as usize as u64)),
        "the kernel should run from the base the loader chose, its virtual and physical first page the same memory",
    );

    let marker = 0u8;
    let sp = (&raw const marker) as u64;
    report.check(
        "kernel.stack",
        info.stack.top == STACK_TOP
            && info.stack.top - info.stack.bottom == STACK_PAGES * 4096
            && (info.stack.bottom..info.stack.top).contains(&sp),
        "the kernel should run on the stack the boot information describes",
    );
    report.check(
        "kernel.no_execute",
        x86::raw::efer() & x86::efer::NO_EXECUTE_ENABLE != 0,
        "the loader should turn on no-execute pages",
    );
    report.check(
        "kernel.write_protect",
        x86::raw::read_cr0() & x86::cr0::WRITE_PROTECT != 0,
        "the loader should make read-only pages read-only for the kernel too",
    );
    report.check(
        "kernel.interrupts_off",
        !x86::raw::interrupts_enabled(),
        "the kernel has no interrupt handlers yet, so interrupts must be off",
    );
    report.check(
        "kernel.rsdp",
        info.rsdp != 0 && physical(info.rsdp, 8) == b"RSD PTR ",
        "OVMF has ACPI tables, so the root pointer should be passed on",
    );

    // UEFI clears the boot services pointer, at offset 96 of the system
    // table, when ExitBootServices succeeds.
    report.check(
        "kernel.boot_services_gone",
        info.uefi_system_table != 0 && physical(info.uefi_system_table + 96, 8) == [0; 8],
        "the loader should have left the firmware's boot services",
    );

    machine_checks(&mut report, machine, tsc_hz);
    memory_checks(&mut report, regions, free);
    mapping_checks(&mut report);
    screen_checks(&mut report, info, screen);
    handover_screen_checks(&mut report, info, regions, display.as_ref());
    if screen.is_some() {
        let kept = keep_screen(&mut report, "status");
        report.check(
            "kernel.screen.kept",
            kept,
            "sextant should answer SCREEN with a carriage return on the serial port",
        );
    }

    console_checks(&mut report, display);
    device_checks(&mut report, info, regions);
    timer_checks(&mut report, tsc_hz, info, regions, machine);
    exception_checks(&mut report);
    protection_checks(&mut report);

    // The last check is the panic screen: the panic handler takes over,
    // checks what it drew and ends the run.
    FAILED.store(report.failed, Ordering::Release);
    STAGE.store(1, Ordering::Release);
    panic!("{PANIC_MESSAGE}");
}

const PANIC_MESSAGE: &str = "a test of the panic screen";
static FAILED: AtomicU32 = AtomicU32::new(0);
/// Which crash comes next: 1 for the panic, 2 for the stack overflow.
static STAGE: AtomicU32 = AtomicU32::new(0);

/// Called by the panic handler once the panic screen is drawn. The run
/// ends in two crashes: the panic above, then a stack overflow, which the
/// processor turns into a double fault on the fault stack.
pub fn after_panic(screen: Option<&Canvas<'_>>, details: &Details<'_>, serial: &mut Com1) -> ! {
    let mut report = Report {
        out: serial,
        failed: FAILED.load(Ordering::Acquire),
    };
    match STAGE.load(Ordering::Acquire) {
        1 => {
            report.check(
                "kernel.panic.message",
                details.message == PANIC_MESSAGE
                    && details.location.starts_with("kernel/src/qemu_test.rs:"),
                "the panic screen should get the message and where it came from",
            );
            crash_screen_checks(&mut report, screen, "panic");
            let look = |c: &Canvas<'_>| c.pixel(20, 20);
            let before = screen.and_then(look);
            raw::console::write("\u{c}written after the crash");
            report.check(
                "kernel.console.after_crash",
                before.is_some_and(|p| p != NIGHT) && screen.and_then(look) == before,
                "once a crash took the screen, the console should draw on it no more",
            );
            FAILED.store(report.failed, Ordering::Release);
            STAGE.store(2, Ordering::Release);
            raw::allow_another_crash();
            let _ = writeln!(report.out, "kernel: running out of stack on purpose");
            core::hint::black_box(overflow(0));
            report.check(
                "kernel.overflow",
                false,
                "running out of stack should have crashed",
            );
        }
        2 => {
            let here = 0u8;
            let on_fault_stack =
                raw::interrupts::fault_stack().contains(&((&raw const here) as u64));
            // The loader puts the kernel somewhere in the top 2 GiB.
            let in_kernel = details
                .location
                .strip_prefix("the instruction at 0x")
                .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                .is_some_and(|at| at >= 0xffff_ffff_8000_0000);
            report.check(
                "kernel.double_fault",
                details.message.starts_with("double fault: ") && in_kernel && on_fault_stack,
                "running out of stack should end on the panic screen as a double fault in the kernel, on the fault stack",
            );
            crash_screen_checks(&mut report, screen, "double-fault");
        }
        _ => report.check(
            "kernel.panic",
            false,
            "the kernel crashed before its checks were done",
        ),
    }
    let failed = report.failed;
    let _ = writeln!(report.out, "TESTS DONE {failed} failed");
    x86::raw::qemu_exit(failed == 0)
}

/// Calls itself until the stack runs into the guard page below it.
#[inline(never)]
fn overflow(depth: usize) -> usize {
    let pad = core::hint::black_box([depth; 32]);
    if core::hint::black_box(true) {
        overflow(depth + 1) + pad[depth % 32]
    } else {
        0
    }
}

/// The crash screen is sun yellow with a QR code on the right, and sextant
/// keeps a picture of it under `name`.
fn crash_screen_checks(report: &mut Report<'_, Com1>, screen: Option<&Canvas<'_>>, name: &str) {
    report.check(
        "kernel.panic.screen",
        screen.is_some_and(|c| {
            c.pixel(0, 0) == Some(SUN) && c.pixel(c.width() - 1, c.height() - 1) == Some(SUN)
        }),
        "the panic screen should be sun yellow",
    );
    report.check(
        "kernel.panic.code",
        screen.is_some_and(|c| {
            let right = (c.width() * 2 / 3..c.width())
                .flat_map(|x| (0..c.height() / 2).map(move |y| (x, y)));
            let (mut ink, mut light) = (0, 0);
            for (x, y) in right {
                match c.pixel(x, y) {
                    Some(INK) => ink += 1,
                    Some(p) if p.r == 0xff && p.g == 0xff && p.b == 0xff => light += 1,
                    _ => {}
                }
            }
            ink > 2000 && light > 2000
        }),
        "the QR code should be drawn on the right",
    );
    if screen.is_some() {
        let _ = writeln!(report.out, "SCREEN {name}");
        let answer = report.out.receive(1 << 24);
        report.check(
            "kernel.panic.kept",
            answer == Some(b'\r'),
            "sextant should answer SCREEN with a carriage return on the serial port",
        );
    }
}

/// The devices the kernel finds and drives, and what it reads from them.
fn device_checks<W: Write>(report: &mut Report<'_, W>, info: &BootInfo, regions: &[MemoryRegion]) {
    pci_checks(report);
    ahci_checks(report);
    nvme_checks(report);
    virtio_checks(report);
    virtio_file_checks(report, info, regions);
    write_checks(report);
    guard_checks(report, info.boot_partition);
    fat_checks(report, info, regions);
    vfs_checks(report, info.boot_partition);
    vfs_write_checks(report);
}

/// PCI through the MCFG's ECAM region: q35's host bridge at 00:00.0 and its
/// AHCI controller are found, with the controller's registers in memory,
/// every function once and in order, and where no function is, none.
fn pci_checks<W: Write>(report: &mut Report<'_, W>) {
    use pci::{Address, Bar};
    let mut found = alloc::vec::Vec::new();
    raw::pci::each(|at, h| found.push((at, h)));
    let host = found.first().is_some_and(|(at, h)| {
        at.bus == 0 && at.device == 0 && at.function == 0 && h.kind() == "host bridge"
    });
    let ahci = found
        .iter()
        .find(|(_, h)| (h.class, h.subclass, h.interface) == (0x01, 0x06, 0x01));
    let registers =
        ahci.is_some_and(|(_, h)| matches!(h.bar(5), Bar::Memory { address, .. } if address != 0));
    let enabled = ahci.is_some_and(|&(at, _)| {
        let before = raw::pci::command(at).unwrap_or(0);
        raw::pci::set_command(at, 0);
        let off = raw::pci::command(at) == Some(0);
        raw::pci::enable(at);
        let alone = raw::pci::command(at) == Some(0x6);
        raw::pci::set_command(at, before & !0x6);
        raw::pci::enable(at);
        off && alone && raw::pci::command(at) == Some(before | 0x6)
    });
    let in_order = found.windows(2).all(|pair| pair[0].0 < pair[1].0);
    let nobody = Address {
        bus: 0xff,
        device: 31,
        function: 7,
    };
    report.check(
        "kernel.pci.found",
        host && registers && in_order && raw::pci::vendor(nobody) == Some(pci::NOBODY),
        "q35's host bridge and AHCI controller should be found, each function once, the controller's registers in memory",
    );
    let bridge = found
        .iter()
        .find(|(at, h)| at.bus == 0 && h.kind() == "PCI bridge");
    let behind = bridge.is_some_and(|(_, b)| {
        found
            .iter()
            .any(|(at, h)| at.bus == b.secondary_bus && b.secondary_bus > 0 && h.vendor == 0x1af4)
    });
    report.check(
        "kernel.pci.bridge",
        behind && found.iter().all(|(at, _)| at.bus <= 1),
        "the root port the test machine has should lead to its random number generator, on a bus of its own",
    );
    let nowhere = bridge.is_some_and(|&(at, _)| {
        let Some((secondary, subordinate)) = raw::pci::set_buses(at, 0, 0) else {
            return false;
        };
        let mut seen = alloc::vec::Vec::new();
        raw::pci::each(|at, h| seen.push((at, h)));
        raw::pci::set_buses(at, secondary, subordinate);
        let mut again = 0;
        raw::pci::each(|_, _| again += 1);
        seen.windows(2).all(|pair| pair[0].0 < pair[1].0)
            && seen.iter().all(|(at, _)| at.bus == 0)
            && seen.iter().any(|(a, h)| *a == at && h.secondary_bus == 0)
            && again == found.len()
    });
    report.check(
        "kernel.pci.nowhere",
        nowhere,
        "a bridge whose bus is not above its own, as before anyone numbers it, should not be followed",
    );
    report.check(
        "kernel.pci.enabled",
        enabled,
        "enabling the AHCI controller should turn on its memory and bus master bits and leave the others",
    );
}

/// The drives on q35's AHCI controller. Booted from a disk, the first disk
/// says it is QEMU's, and its first sectors are the protective MBR and the
/// GPT header sextant wrote, its last the GPT's copy, and nothing past it
/// is read. Booted from a CD, the CD is there as an optical drive.
fn ahci_checks<W: Write>(report: &mut Report<'_, W>) {
    use ahci::Attached;
    use raw::ahci::{Error, read_sectors};
    let drives = raw::ahci::drives();
    let disk = drives
        .iter()
        .flatten()
        .find(|d| d.attached == Attached::Disk);
    let optical = drives
        .iter()
        .flatten()
        .any(|d| d.attached == Attached::Optical);
    let Some(disk) = disk else {
        report.check(
            "kernel.ahci.found",
            optical,
            "booted from a CD, the CD should be found as an optical drive",
        );
        return;
    };
    let identity = disk.identity;
    report.check(
        "kernel.ahci.found",
        identity.is_some_and(|i| {
            i.model() == "QEMU HARDDISK" && i.sectors > 2048 && i.sector_size == 512
        }),
        "the boot disk should say it is QEMU's, with more than 1 MiB of 512-byte sectors",
    );
    let sectors = identity.map_or(0, |i| i.sectors);
    let mut first = [0u8; 1024];
    let mut last = [0u8; 512];
    let read = read_sectors(disk.port, 0, &mut first).is_ok()
        && read_sectors(disk.port, sectors.saturating_sub(1), &mut last).is_ok();
    let mbr = first[510..512] == [0x55, 0xaa] && first[0x1c2] == 0xee;
    let gpt = first[512..520] == *b"EFI PART" && last[..8] == *b"EFI PART";
    let mut past = [0u8; 512];
    let refused = read_sectors(disk.port, sectors, &mut past) == Err(Error::OutOfRange)
        && read_sectors(disk.port, 0, &mut past[..100]) == Err(Error::OutOfRange);
    report.check(
        "kernel.ahci.read",
        read && mbr && gpt && refused,
        "the disk's first sectors should be the protective MBR and GPT header, its last the copy, and no sector past it read",
    );
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let mut large = alloc::vec![0u8; 17 * 4096];
    let mut by_sector = large.clone();
    let mut at_once = alloc::vec![0u8; 16 * 4096 + 512];
    let frames = free();
    let whole = raw::ahci::read(disk.port, 2040, &mut large).is_ok();
    let back = frames.is_some() && free() == frames;
    let each = (2040..)
        .zip(by_sector.chunks_mut(512))
        .all(|(lba, sector)| read_sectors(disk.port, lba, sector).is_ok());
    let one = read_sectors(disk.port, 2040, &mut at_once[..16 * 4096]).is_ok()
        && at_once[..16 * 4096] == large[..16 * 4096];
    let too_much = read_sectors(disk.port, 2040, &mut at_once) == Err(Error::OutOfRange);
    report.check(
        "kernel.ahci.large",
        whole
            && back
            && each
            && large == by_sector
            && large[8 * 512 + 510..8 * 512 + 512] == [0x55, 0xaa]
            && one
            && too_much,
        "seventeen frames should read as a sector at a time reads them, and give their frames back, sixteen in one command and no more",
    );
    let mut read = |lba, into: &mut [u8]| raw::ahci::read(disk.port, lba, into).is_ok();
    let table = kernel::disk::Table::read(&mut read);
    let listed = [
        (gpt::EFI_SYSTEM, 2048, 131_072, "EFI system partition"),
        (gpt::MERIDIAN_SYSTEM, 133_120, 69_632, "Meridian system"),
        (gpt::BASIC_DATA, 202_752, 2048, "Unlocked"),
        (gpt::MERIDIAN_SYSTEM, 204_800, 2048, "Not listed"),
    ];
    report.check(
        "kernel.gpt.read",
        table.is_some_and(|t| {
            t.header.backup + 1 == sectors
                && t.partitions().count() == listed.len()
                && t.partitions().zip(1..).zip(listed).all(
                    |(((number, p), want), (kind, first, size, name))| {
                        number == want
                            && p.kind == kind
                            && (p.first, p.sectors()) == (first, size)
                            && p.name().eq(name.chars())
                    },
                )
        }),
        "the boot disk's partition table should list sextant's EFI system partition of 64 MiB from 1 MiB on, then the test disk's Meridian system partition of 34 MiB, and its unlocked partition and one not listed, 1 MiB each",
    );
    ahci_port_checks(report, disk.port);
}

/// What an AHCI port sends back and how it gets over trouble: a read
/// leaves the disk's answer in the drive's own frame and gives its buffer
/// back, a command the disk refuses fails with an answer that says so and
/// leaves the port working, and a port left stopped with its addresses
/// above 4 GiB, as a firmware might leave it, works once started again.
fn ahci_port_checks<W: Write>(report: &mut Report<'_, W>, port: usize) {
    use ahci::port::{
        COMMAND, COMMAND_LIST_HIGH, FAILED, RECEIVE, RECEIVED_HIGH, RECEIVING, RUNNING, START,
    };
    use raw::ahci::{
        Error, answer, command, port_register, read_sectors, restart, set_port_register,
    };
    // A register FIS from the device, with the status and error registers.
    let answered = |failed: bool| {
        answer(port).is_some_and(|fis| {
            fis[0] == 0x34 && (fis[2] & u8::try_from(FAILED).unwrap_or(0) != 0) == failed
        })
    };
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let mut header = [0u8; 512];
    let before = free();
    let read = read_sectors(port, 1, &mut header).is_ok() && header[..8] == *b"EFI PART";
    report.check(
        "kernel.ahci.answer",
        read && answered(false) && before.is_some() && free() == before,
        "a read should leave the disk's answer in the drive's own frame and give its buffer back",
    );
    let refused = command(port, ahci::ata::NOP) == Err(Error::Failed);
    let aborted = answered(true) && answer(port).is_some_and(|fis| fis[3] & 0x04 != 0);
    let mut again = [0u8; 512];
    let works = read_sectors(port, 1, &mut again).is_ok() && again == header && answered(false);
    report.check(
        "kernel.ahci.error",
        refused && aborted && works,
        "NOP should fail with the disk saying it aborted it, and the port should read again afterwards",
    );
    let settings = port_register(port, COMMAND);
    set_port_register(port, COMMAND, settings & !(START | RECEIVE));
    let stopped = (0..1000).any(|_| port_register(port, COMMAND) & (RUNNING | RECEIVING) == 0);
    let highs = [COMMAND_LIST_HIGH, RECEIVED_HIGH];
    for high in highs {
        set_port_register(port, high, 1);
    }
    let above = highs.iter().all(|&high| port_register(port, high) == 1);
    let restarted = restart(port);
    let below = highs.iter().all(|&high| port_register(port, high) == 0);
    again.fill(0);
    let works = read_sectors(port, 1, &mut again).is_ok() && again == header && answered(false);
    report.check(
        "kernel.ahci.restart",
        stopped && above && restarted && below && works && port_register(port, COMMAND) == settings,
        "a port left stopped with its addresses above 4 GiB should get its own back, keep its other settings, and read again once started",
    );
}

/// The NVM Express controller a test machine with a disk has next to
/// AHCI, with the same image behind it: it says it is QEMU's, its
/// namespace is as big as the AHCI disk, and it reads what AHCI reads,
/// whether in one go over several frames, in as few commands as the
/// controller takes, or a block at a time, more often than its queues have
/// room, and nothing past the end. A machine booted from a CD has none.
fn nvme_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::nvme::{Error, read};
    let drives = raw::ahci::drives();
    let disk = drives
        .iter()
        .flatten()
        .find(|d| d.attached == ahci::Attached::Disk);
    let drive = raw::nvme::drive();
    let Some(disk) = disk else {
        report.check(
            "kernel.nvme.found",
            drive.is_none(),
            "booted from a CD, the machine should have no NVMe controller",
        );
        return;
    };
    let sectors = disk.identity.map_or(0, |i| i.sectors);
    report.check(
        "kernel.nvme.found",
        drive.is_some_and(|d| {
            (d.controller.model(), d.controller.serial()) == ("QEMU NVMe Ctrl", "meridian")
                && (d.namespace.blocks, d.namespace.block_size) == (sectors, 512)
        }),
        "the NVMe controller should say it is QEMU's, with the serial sextant gave it, and hold as many blocks as the AHCI disk",
    );
    let ahci_read = |lba, into: &mut [u8]| raw::ahci::read(disk.port, lba, into).is_ok();
    let mut by_nvme = alloc::vec![0u8; 25 * 512];
    let mut by_ahci = by_nvme.clone();
    let spanning = read(2040, &mut by_nvme).is_ok()
        && ahci_read(2040, &mut by_ahci)
        && by_nvme == by_ahci
        && by_nvme[8 * 512 + 510..8 * 512 + 512] == [0x55, 0xaa];
    let mut one_by_one = alloc::vec![0u8; 130 * 512];
    let mut whole = one_by_one.clone();
    let each = (0..130)
        .zip(one_by_one.chunks_mut(512))
        .all(|(lba, block)| read(lba, block).is_ok())
        && ahci_read(0, &mut whole)
        && one_by_one == whole
        && whole[512..520] == *b"EFI PART";
    let mut last = [0u8; 512];
    let end = read(sectors.saturating_sub(1), &mut last).is_ok() && last[..8] == *b"EFI PART";
    let mut room = [0u8; 1024];
    let refused = read(sectors, &mut room[..512]) == Err(Error::OutOfRange)
        && read(sectors.saturating_sub(1), &mut room) == Err(Error::OutOfRange)
        && read(u64::MAX, &mut room[..512]) == Err(Error::OutOfRange)
        && read(0, &mut room[..100]) == Err(Error::OutOfRange)
        && read(0, &mut []) == Err(Error::OutOfRange);
    report.check(
        "kernel.nvme.read",
        spanning && each && end && refused,
        "NVMe should read the same sectors as AHCI, up to the disk's last and no further",
    );
    let most = drive
        .and_then(|d| d.controller.transfer_pages)
        .map_or(16, |pages| pages.min(16));
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let mut by_nvme = alloc::vec![0u8; 17 * 4096];
    let mut by_ahci = by_nvme.clone();
    let (before, frames) = (raw::nvme::commands(), free());
    let large = read(2040, &mut by_nvme).is_ok();
    let sent = raw::nvme::commands().wrapping_sub(before);
    let back = frames.is_some() && free() == frames;
    let same = ahci_read(2040, &mut by_ahci)
        && by_nvme == by_ahci
        && by_nvme[8 * 512 + 510..8 * 512 + 512] == [0x55, 0xaa];
    report.check(
        "kernel.nvme.large",
        large && same && back && u32::from(sent) == 17u32.div_ceil(most),
        "seventeen frames should come in as few commands as the controller takes, sixteen frames each at most, as AHCI reads them, and the frames they came through should go back",
    );
}

/// The virtio block device, which test machines with a disk have, holding
/// the same disk once more, read only: as large as the AHCI disk, saying
/// it only reads, and reading the same sectors, a frame at a time and
/// across frames, up to its last and no further.
fn virtio_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::virtio::{Error, read};
    use virtio::status::{ACKNOWLEDGE, DRIVER, DRIVER_OK, FEATURES_OK};
    let drives = raw::ahci::drives();
    let disk = drives
        .iter()
        .flatten()
        .find(|d| d.attached == ahci::Attached::Disk);
    let drive = raw::virtio::drive();
    let Some(disk) = disk else {
        report.check(
            "kernel.virtio.found",
            drive.is_none(),
            "booted from a CD, the machine should have no virtio disk",
        );
        return;
    };
    let sectors = disk.identity.map_or(0, |i| i.sectors);
    let offered = raw::virtio::agreed().map_or(0, |(_, offered, ..)| offered);
    report.check(
        "kernel.virtio.found",
        drive.is_some_and(|d| d.sectors == sectors && d.read_only == virtio::read_only(offered)),
        "the virtio disk should hold as many sectors as the AHCI disk, and only read where it says so",
    );
    let ahci_read = |lba, into: &mut [u8]| raw::ahci::read(disk.port, lba, into).is_ok();
    let mut by_virtio = alloc::vec![0u8; 25 * 512];
    let mut by_ahci = by_virtio.clone();
    let spanning = read(2040, &mut by_virtio).is_ok()
        && ahci_read(2040, &mut by_ahci)
        && by_virtio == by_ahci
        && by_virtio[8 * 512 + 510..8 * 512 + 512] == [0x55, 0xaa];
    let mut one_by_one = alloc::vec![0u8; 20 * 512];
    let mut whole = one_by_one.clone();
    let each = (0..20)
        .zip(one_by_one.chunks_mut(512))
        .all(|(lba, block)| read(lba, block).is_ok())
        && ahci_read(0, &mut whole)
        && one_by_one == whole
        && whole[512..520] == *b"EFI PART";
    let mut last = [0u8; 512];
    let end = read(sectors.saturating_sub(1), &mut last).is_ok() && last[..8] == *b"EFI PART";
    let mut room = [0u8; 1024];
    let refused = read(sectors, &mut room[..512]) == Err(Error::OutOfRange)
        && read(sectors.saturating_sub(1), &mut room) == Err(Error::OutOfRange)
        && read(u64::MAX, &mut room[..512]) == Err(Error::OutOfRange)
        && read(0, &mut room[..100]) == Err(Error::OutOfRange)
        && read(0, &mut []) == Err(Error::OutOfRange);
    report.check(
        "kernel.virtio.read",
        spanning && each && end && refused,
        "virtio should read the same sectors as AHCI, up to the disk's last and no further",
    );
    // A request takes a descriptor for its header and one for its status
    // besides those for its frames.
    let pieces = raw::virtio::agreed().map_or(0, |(.., size, _)| size.saturating_sub(2));
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let mut by_virtio = alloc::vec![0u8; 17 * 4096];
    let mut by_ahci = by_virtio.clone();
    let (before, frames) = (raw::virtio::requests(), free());
    let large = read(2040, &mut by_virtio).is_ok();
    let sent = raw::virtio::requests().wrapping_sub(before);
    let back = frames.is_some() && free() == frames;
    let same = ahci_read(2040, &mut by_ahci)
        && by_virtio == by_ahci
        && by_virtio[8 * 512 + 510..8 * 512 + 512] == [0x55, 0xaa];
    report.check(
        "kernel.virtio.large",
        large && same && back && pieces > 0 && sent == 17u16.div_ceil(pieces),
        "seventeen frames should come in as few requests as the queue has descriptors for, as AHCI reads them, and the frames they came through should go back",
    );
    let wanted = virtio::features::VERSION_1 | offered & virtio::features::BLOCK_READ_ONLY;
    report.check(
        "kernel.virtio.ready",
        raw::virtio::agreed().is_some_and(|(status, _, taken, size, enabled)| {
            status == ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK
                && taken == wanted
                && (size, enabled) == (virtio::QUEUE_SIZE, 1)
        }),
        "the virtio disk should be acknowledged, driven and ready, with version 1 and its only reading agreed on, and its first queue of 8 entries on",
    );
    virtio_window_checks(report);
    report.check(
        "kernel.virtio.unsupported",
        raw::virtio::ask(0x7f) == Err(Error::Failed(virtio::block::UNSUPPORTED))
            && read(0, &mut room[..512]).is_ok(),
        "a request of a type no device knows should come back unsupported, and reads go on",
    );
}

/// Writing, on the test disk's unlocked partition: twelve sectors through
/// AHCI, more than a frame, of bytes no run wrote before, which
/// AHCI, NVM Express and virtio then read back as written; the last two
/// hold the disk read only and refuse to write, and AHCI writes nothing
/// past the disk's end.
fn write_checks<W: Write>(report: &mut Report<'_, W>) {
    let drives = raw::ahci::drives();
    let Some(disk) = drives
        .iter()
        .flatten()
        .find(|d| d.attached == ahci::Attached::Disk)
    else {
        return;
    };
    let port = disk.port;
    let mut read = |lba, into: &mut [u8]| raw::ahci::read(port, lba, into).is_ok();
    let Some(unlocked) = kernel::disk::Table::read(&mut read).and_then(|t| {
        t.partitions()
            .map(|(_, p)| p)
            .find(|p| p.name().eq("Unlocked".chars()))
    }) else {
        report.check(
            "kernel.disk.write",
            false,
            "a test disk should have an unlocked partition",
        );
        return;
    };
    let salt = x86::raw::tsc().to_le_bytes();
    let written: alloc::vec::Vec<u8> = (0..12 * 512)
        .map(|i: usize| salt[i % 8] ^ (i / 8).to_le_bytes()[0])
        .collect();
    let at = unlocked.first + 3;
    let wrote = raw::ahci::write(CHECKS, port, at, &written).is_ok();
    let back = |read: &dyn Fn(u64, &mut [u8]) -> bool| {
        let mut bytes = alloc::vec![0u8; written.len()];
        read(at, &mut bytes) && bytes == written
    };
    let through_ahci = back(&|lba, into| raw::ahci::read(port, lba, into).is_ok());
    let through_nvme = back(&|lba, into| raw::nvme::read(lba, into).is_ok());
    let through_virtio = back(&|lba, into| raw::virtio::read(lba, into).is_ok());
    let sectors = disk.identity.map_or(0, |i| i.sectors);
    let refused = raw::ahci::write(CHECKS, port, sectors, &written[..512])
        == Err(raw::ahci::Error::OutOfRange)
        && raw::ahci::write_sectors(CHECKS, port, at, &written[..100])
            == Err(raw::ahci::Error::OutOfRange);
    report.check(
        "kernel.disk.write",
        wrote && through_ahci && through_nvme && through_virtio && refused,
        "sectors AHCI wrote should read back as written through AHCI, NVMe and virtio, and none past the disk or short of a sector go",
    );
    copy_write_checks(report, port, &unlocked);
    write_place_checks(report, port, &unlocked, &written);
    write_refusal_checks(report, port, sectors);
}

/// Where AHCI's writes go: each of the twelve sectors at its own place,
/// one sector overwritten among them alone, the partition's last sector,
/// and seventeen frames, sixteen in one command, read back in one go.
fn write_place_checks<W: Write>(
    report: &mut Report<'_, W>,
    port: usize,
    unlocked: &gpt::Partition,
    written: &[u8],
) {
    use raw::ahci::{read, read_sectors, write, write_sectors};
    let at = unlocked.first + 3;
    let mut sector = [0u8; 512];
    let each = written
        .chunks(512)
        .zip(at..)
        .all(|(want, lba)| read_sectors(port, lba, &mut sector).is_ok() && sector == want);
    let one = [0x5a; 512];
    let mut after = written.to_vec();
    after[5 * 512..6 * 512].copy_from_slice(&one);
    let mut read_back = alloc::vec![0u8; written.len()];
    let alone = write_sectors(CHECKS, port, at + 5, &one).is_ok()
        && read(port, at, &mut read_back).is_ok()
        && read_back == after;
    let last = unlocked.first + unlocked.sectors() - 1;
    let mut end = [0u8; 512];
    let edge = write(CHECKS, port, last, &[0xa5; 512]).is_ok()
        && raw::virtio::read(last, &mut end).is_ok()
        && end == [0xa5; 512];
    // Seventeen frames: sixteen in one command and one more in a second.
    let large: alloc::vec::Vec<u8> = (0..17 * 4096)
        .map(|i: usize| i.to_le_bytes()[1] ^ i.to_le_bytes()[2] ^ 0x3c)
        .collect();
    let mut by_virtio = alloc::vec![0u8; large.len()];
    let many = write(CHECKS, port, at, &large).is_ok()
        && raw::virtio::read(at, &mut by_virtio).is_ok()
        && by_virtio == large;
    report.check(
        "kernel.disk.write_places",
        each && alone && edge && many,
        "each sector AHCI writes should land where it was meant to, one overwritten alone, the partition's last too, and seventeen frames in two commands",
    );
    // The W bit of the command header: set where data goes to the disk.
    let towards =
        |write: bool| raw::ahci::last_header(port).is_some_and(|h| h[0] & 0x40 != 0) == write;
    let out = write_sectors(CHECKS, port, at + 5, &one).is_ok() && towards(true);
    let back = read_sectors(port, at + 5, &mut sector).is_ok() && towards(false) && sector == one;
    report.check(
        "kernel.disk.write_direction",
        out && back,
        "the command header should tell the controller that a write's data goes to the disk and a read's comes from it",
    );
}

/// The virtio disk's common, device and notification registers each lie in
/// a page of their own, in that order, in the window the kernel's layout
/// keeps for them, uncached, and nothing lies next to it.
fn virtio_window_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::memory::translate;
    let at = |page: u64| kernel::VIRTIO_REGISTERS + page * 4096;
    let pages = [0, 1, 2].map(|page| translate(at(page)));
    let uncached = pages
        .iter()
        .all(|p| p.is_some_and(|(_, flags)| flags.contains(paging::Flags::NO_CACHE)));
    let apart = pages[0]
        .zip(pages[1])
        .zip(pages[2])
        .is_some_and(|((a, b), c)| a.0 != b.0 && b.0 != c.0);
    let alone = translate(at(0) - 4096).is_none() && translate(at(3)).is_none();
    report.check(
        "kernel.virtio.window",
        uncached && apart && alone,
        "the virtio disk's registers should take the three pages of the kernel's window for them, uncached, and nothing next to it",
    );
}

/// Where the machine lets its NVM Express and virtio disks write, into
/// copies of the disk QEMU keeps, as the virtio disk's features tell, each
/// reads its own writes back, sixteen frames of them at once, and the AHCI
/// disk never sees them. Elsewhere both refuse to write.
fn copy_write_checks<W: Write>(report: &mut Report<'_, W>, port: usize, unlocked: &gpt::Partition) {
    let offered = raw::virtio::agreed().map_or(0, |(_, offered, ..)| offered);
    let sector = [0u8; 512];
    if virtio::read_only(offered) {
        let refused = matches!(
            raw::nvme::write(CHECKS, unlocked.first, &sector),
            Err(raw::nvme::Error::Failed(_))
        ) && raw::virtio::write(CHECKS, unlocked.first, &sector)
            == Err(raw::virtio::Error::ReadOnly);
        report.check(
            "kernel.disk.copies",
            refused,
            "NVMe and virtio disks that only read should refuse to write",
        );
        return;
    }
    let large = |salt: u8| -> alloc::vec::Vec<u8> {
        (0..16 * 4096)
            .map(|i: usize| i.to_le_bytes()[1] ^ i.to_le_bytes()[0] ^ salt)
            .collect()
    };
    let (nvme_at, virtio_at) = (unlocked.first + 512, unlocked.first + 1024);
    let before = |at| {
        let mut sector = [0u8; 512];
        raw::ahci::read(port, at, &mut sector)
            .is_ok()
            .then_some(sector)
    };
    let (nvme_before, virtio_before) = (before(nvme_at), before(virtio_at));
    let mut back = alloc::vec![0u8; 16 * 4096];
    let nvme = raw::nvme::write(CHECKS, nvme_at, &large(0x11)).is_ok()
        && raw::nvme::read(nvme_at, &mut back).is_ok()
        && back == large(0x11);
    let by_virtio = raw::virtio::write(CHECKS, virtio_at, &large(0x22)).is_ok()
        && raw::virtio::read(virtio_at, &mut back).is_ok()
        && back == large(0x22);
    let apart = nvme_before.is_some()
        && before(nvme_at) == nvme_before
        && virtio_before.is_some()
        && before(virtio_at) == virtio_before;
    report.check(
        "kernel.disk.copies",
        nvme && by_virtio && apart,
        "NVMe and virtio disks that write to copies should read sixteen frames of their own writes back, which the AHCI disk never sees",
    );
}

/// What the disk layer lets through (decision record 0015) on the test
/// disk, whose lists say that Meridian made its EFI system partition and
/// its own and that the third is unlocked: those, each write within one.
/// The fourth, of Meridian's own type but not listed, the partition table
/// and whatever is past the end are refused, and nothing refused lands.
/// With no partition `boot` named, nothing may be written anywhere.
fn guard_checks<W: Write>(report: &mut Report<'_, W>, boot: [u8; 16]) {
    use kernel::disk::Why;
    use raw::disk::{Driver, find, write};
    let drives = raw::ahci::drives();
    let Some(drive) = drives
        .iter()
        .flatten()
        .find(|d| d.attached == ahci::Attached::Disk)
    else {
        return;
    };
    let ahci = Driver::Ahci(drive.port);
    let unnamed = raw::disk::trust([0; 16]).is_none();
    let mut nothing = true;
    raw::disk::each(|_, _, writable| nothing &= *writable == kernel::disk::Writable::default());
    report.check(
        "kernel.disk.trust",
        unnamed && nothing && raw::disk::trust(boot) == Some(Driver::Virtio),
        "with no partition named by the loader nothing should be written, and the lists should come from the disk it came from, through virtio where several controllers reach it",
    );
    let mut listed = alloc::vec::Vec::new();
    let mut whole = true;
    raw::disk::each(|_, driver, writable| {
        if driver == ahci {
            listed = writable
                .partitions
                .iter()
                .map(|&(n, _, why)| (n, why))
                .collect();
            whole = writable.whole;
        }
    });
    report.check(
        "kernel.disk.listed",
        listed == [(1, Why::Made), (2, Why::Made), (3, Why::Unlocked)] && !whole,
        "the lists on the test disk's system partition should let Meridian write the two partitions it made and the one unlocked, and no more",
    );
    let mut read = |lba, into: &mut [u8]| raw::disk::read_from(ahci, lba, into);
    let table = kernel::disk::Table::read(&mut read);
    let part = |n: usize| {
        table
            .as_ref()
            .and_then(|t| t.partitions().find(|&(m, _)| m == n))
            .map(|(_, p)| p)
    };
    let (Some(disk), Some(system), Some(unlocked), Some(other)) =
        (find(ahci), part(2), part(3), part(4))
    else {
        report.check(
            "kernel.disk.guard",
            false,
            "the test disk should be taken in with its four partitions",
        );
        return;
    };
    let bytes = |lba, len: usize| {
        let mut b = alloc::vec![0u8; len];
        read(lba, &mut b).then_some(b)
    };
    let salt = x86::raw::tsc().to_le_bytes();
    let mark: alloc::vec::Vec<u8> = (0..1024).map(|i: usize| salt[i % 8] ^ 0x5c).collect();
    let boot = bytes(system.first, 512);
    let made = boot
        .as_ref()
        .is_some_and(|b| write(disk, system.first, b).is_ok())
        && bytes(system.first, 512) == boot;
    let unlocked_ok = write(disk, unlocked.first, &mark[..512]).is_ok()
        && bytes(unlocked.first, 512).as_deref() == Some(&mark[..512]);
    report.check(
        "kernel.disk.guard",
        made && unlocked_ok,
        "a partition Meridian made and one unlocked should take writes through the disk layer",
    );
    let sectors = drive.identity.map_or(0, |i| i.sectors);
    guard_refusal_checks(
        report,
        drive.port,
        &mark,
        [system, unlocked, other],
        sectors,
    );
}

/// What the disk layer refuses on the AHCI disk on port `port`, of
/// `sectors` sectors, given its system, unlocked and unlisted partitions,
/// without writing any of it, and how a write it lets through fails.
fn guard_refusal_checks<W: Write>(
    report: &mut Report<'_, W>,
    port: usize,
    mark: &[u8],
    [system, unlocked, other]: [gpt::Partition; 3],
    sectors: u64,
) {
    use raw::disk::{Driver, Error, find, write};
    let Some(disk) = find(Driver::Ahci(port)) else {
        return;
    };
    let bytes = |lba, len: usize| {
        let mut b = alloc::vec![0u8; len];
        raw::disk::read_from(Driver::Ahci(port), lba, &mut b).then_some(b)
    };
    let refused = |lba: u64, len: usize| {
        let before = bytes(lba, 512);
        write(disk, lba, &mark[..len]) == Err(Error::Refused)
            && before.is_some()
            && bytes(lba, 512) == before
    };
    let others = refused(other.first, 512) && refused(other.last, 512);
    let tables = refused(0, 512) && refused(1, 512) && refused(sectors - 1, 512);
    let across = refused(unlocked.last, 1024) && refused(system.first - 1, 1024);
    let beyond = write(disk, sectors, &mark[..512]) == Err(Error::Refused)
        && write(disk, u64::MAX, &mark[..1024]) == Err(Error::Refused);
    report.check(
        "kernel.disk.guard_refused",
        others && tables && across && beyond,
        "the disk layer should refuse a partition neither made nor unlocked, though of Meridian's type, the partition tables, a write across two partitions, made ones too, and the sectors past the end, writing none of them",
    );
    let offered = raw::virtio::agreed().map_or(0, |(_, offered, ..)| offered);
    let others_write = !virtio::read_only(offered);
    let through = |driver| {
        find(driver).map(|d| write(d, unlocked.first + 1, &mark[..512]))
            == Some(if others_write {
                Ok(())
            } else {
                Err(Error::Failed)
            })
    };
    report.check(
        "kernel.disk.guard_drivers",
        through(Driver::Nvme)
            && through(Driver::Virtio)
            && write(usize::MAX, unlocked.first, &mark[..512]) == Err(Error::NoDisk),
        "a write the layer lets through should fail where the driver fails, as on a disk that only reads, and none should go to a disk there is not",
    );
}

/// What no driver writes: nothing, part of a sector, more than sixteen
/// frames at once where that is all a command takes, past the disk's end,
/// or to a port without a disk.
fn write_refusal_checks<W: Write>(report: &mut Report<'_, W>, port: usize, sectors: u64) {
    use raw::ahci::{Error, write, write_sectors};
    let room = alloc::vec![0u8; 16 * 4096 + 512];
    let ahci = write_sectors(CHECKS, port, 0, &[]) == Err(Error::OutOfRange)
        && write_sectors(CHECKS, port, 0, &room[..513]) == Err(Error::OutOfRange)
        && write_sectors(CHECKS, port, 0, &room) == Err(Error::OutOfRange)
        && write(CHECKS, port, sectors - 1, &room[..1024]) == Err(Error::OutOfRange)
        && write(CHECKS, port, u64::MAX, &room[..512]) == Err(Error::OutOfRange)
        && write(CHECKS, 31, 0, &room[..512]) == Err(Error::NoDisk);
    let nvme = raw::nvme::write(CHECKS, 0, &[]) == Err(raw::nvme::Error::OutOfRange)
        && raw::nvme::write(CHECKS, 0, &room[..100]) == Err(raw::nvme::Error::OutOfRange)
        && raw::nvme::write(CHECKS, sectors, &room[..512]) == Err(raw::nvme::Error::OutOfRange);
    report.check(
        "kernel.disk.write_refused",
        ahci && nvme,
        "no write should go out for nothing, part of a sector, too much at once, past the end, or to a port without a disk",
    );
}

/// FAT32 on the virtio disk: hello, read through its EFI system
/// partition, is byte for byte what the loader read through the firmware.
fn virtio_file_checks<W: Write>(
    report: &mut Report<'_, W>,
    info: &BootInfo,
    regions: &[MemoryRegion],
) {
    let Some((_, esp)) = esp_of_disk() else {
        return;
    };
    let read = |sector, into: &mut [u8]| raw::virtio::read(esp.first + sector, into).is_ok();
    let hello = fat::Fat::open(read).ok().and_then(|mut volume| {
        let entry = volume.find("meridian/programs/hello.mexec").ok()?;
        let mut bytes = alloc::vec![0u8; entry.size as usize];
        volume.read_file(&entry, &mut bytes).ok()?;
        Some(bytes)
    });
    let loaded = raw::modules(info, regions).find(|(m, _)| m.name() == b"hello.mexec");
    report.check(
        "kernel.virtio.file",
        hello
            .zip(loaded)
            .is_some_and(|(read, (_, file))| read.len() > 4096 && read == file),
        "hello read through FAT32 on the virtio disk should be what the loader read",
    );
}

/// The tree of files, from inside the kernel: the boot volume hangs at
/// /System/Boot and reads as AHCI and FAT32 read it straight, SYSTEM and a
/// token restricted to the low level both open it to read, and an open
/// file's slot is free again once it is closed, while one file more than
/// there are slots finds none. Booted from a CD, which the kernel cannot
/// read yet, nothing hangs there.
fn vfs_checks<W: Write>(report: &mut Report<'_, W>, boot: [u8; 16]) {
    use abi::file::READ;
    use raw::vfs::{self, Error};
    use security::{Integrity, Privileges, Restriction, Token};
    let settings = "/System/Boot/meridian/boot.meri";
    let system = Token::system();
    let disk = raw::ahci::drives()
        .iter()
        .flatten()
        .any(|d| d.attached == ahci::Attached::Disk);
    if !disk {
        report.check(
            "kernel.vfs.read",
            vfs::open(settings, &system, READ, 0) == Err(Error::NotFound),
            "booted from a CD, nothing should hang at /System/Boot",
        );
        return;
    }
    report.check(
        "kernel.boot.partition",
        boot != [0; 16] && esp_of_disk().is_some_and(|(_, esp)| esp.unique.0 == boot),
        "the loader should name the partition it came from, the EFI system partition sextant wrote",
    );
    let mut by_tree = [0u8; 128];
    let read = vfs::open(settings, &system, READ, 0).ok().and_then(|slot| {
        let len = vfs::read(slot, 0, &mut by_tree).ok();
        let size = vfs::info(slot);
        vfs::close(slot);
        len.zip(size)
    });
    let straight = esp_of_disk().and_then(|(port, esp)| {
        let mut volume = fat::Fat::open(|lba, into: &mut [u8]| {
            raw::ahci::read(port, esp.first + lba, into).is_ok()
        })
        .ok()?;
        let entry = volume.find("meridian/boot.meri").ok()?;
        let mut bytes = [0u8; 128];
        let len = volume.read_file(&entry, &mut bytes).ok()?;
        Some((bytes, len))
    });
    report.check(
        "kernel.vfs.read",
        read.zip(straight)
            .is_some_and(|((len, (size, directory)), (bytes, straight_len))| {
                len == straight_len
                    && size == len as u64
                    && !directory
                    && by_tree[..len] == bytes[..len]
            }),
        "boot.meri should read through /System/Boot as AHCI and FAT32 read it straight",
    );
    let low = system.restricted(Restriction {
        integrity: Integrity::Low,
        keep: Privileges::NONE,
    });
    let opened_low = vfs::open(settings, &low, READ, 0)
        .inspect(|&slot| vfs::close(slot))
        .is_ok();
    let again = (0..100).all(|_| {
        vfs::open(settings, &system, READ, 0)
            .inspect(|&slot| vfs::close(slot))
            .is_ok()
    });
    let mut held = alloc::vec::Vec::new();
    while let Ok(slot) = vfs::open(settings, &system, READ, 0) {
        held.push(slot);
        if held.len() > 64 {
            break;
        }
    }
    let full = held.len() == 64 && vfs::open(settings, &system, READ, 0) == Err(Error::NoRoom);
    for slot in held {
        vfs::close(slot);
    }
    mount_checks(report);
    list_checks(report);
    report.check(
        "kernel.vfs.slots",
        opened_low
            && again
            && full
            && vfs::open(settings, &system, READ, 0)
                .inspect(|&slot| vfs::close(slot))
                .is_ok(),
        "a closed file's slot should be free again, and the 65th file open at once find none",
    );
}

/// Writing files on /System, the test disk's system partition through
/// virtio, which takes them only where the machine lets virtio write to a
/// copy and hangs read only elsewhere, while /System/Boot always does.
fn vfs_write_checks<W: Write>(report: &mut Report<'_, W>) {
    use abi::file::{CREATE_NEW, READ, WRITE};
    use kernel::vfs::Refusal::ReadOnly;
    use raw::vfs::{self, Error};
    if esp_of_disk().is_none() {
        return;
    }
    let system = security::Token::system();
    let offered = raw::virtio::agreed().map_or(0, |(_, offered, ..)| offered);
    let made = vfs::open("/System/made.meri", &system, READ, 0).ok();
    let mut text = [0u8; 64];
    let lists = made.is_some_and(|slot| {
        let read = vfs::read(slot, 0, &mut text);
        vfs::close(slot);
        read.is_ok_and(|n| text[..n].starts_with(b"# The partitions Meridian made"))
    });
    let boot = vfs::open("/System/Boot/new.txt", &system, READ | WRITE, CREATE_NEW)
        == Err(Error::Refused(ReadOnly))
        && vfs::make_directory("/System/Boot/New", &system) == Err(Error::Refused(ReadOnly));
    report.check(
        "kernel.vfs.system",
        lists && boot,
        "the system partition should hang at /System with made.meri on it, and /System/Boot should take no writes",
    );
    if virtio::read_only(offered) {
        let refused = vfs::open("/System/new.txt", &system, READ | WRITE, CREATE_NEW)
            == Err(Error::Refused(ReadOnly))
            && vfs::open("/System/made.meri", &system, READ | WRITE, 0)
                == Err(Error::Refused(ReadOnly))
            && vfs::make_directory("/System/New", &system) == Err(Error::Refused(ReadOnly))
            && vfs::remove("/System/made.meri", &system) == Err(Error::Refused(ReadOnly));
        report.check(
            "kernel.vfs.write",
            refused,
            "/System on a virtio disk that only reads should hang read only and take no writes",
        );
        return;
    }
    file_write_checks(report);
    directory_write_checks(report);
}

/// A file made, written across clusters and past its end, read back, on
/// the disk as written and only on the virtio disk's copy, emptied, and
/// removed once it is closed.
fn file_write_checks<W: Write>(report: &mut Report<'_, W>) {
    use abi::file::{CREATE_ALWAYS, CREATE_NEW, OPEN_ALWAYS, READ, TRUNCATE_EXISTING, WRITE};
    use raw::vfs::{self, Error};
    let system = security::Token::system();
    let path = "/System/Written.txt";
    let bytes: alloc::vec::Vec<u8> = (0..10_000)
        .map(|i: usize| (i % 251).to_le_bytes()[0])
        .collect();
    let Ok(slot) = vfs::open(path, &system, READ | WRITE, CREATE_NEW) else {
        report.check(
            "kernel.vfs.write",
            false,
            "a new file should be made on /System",
        );
        return;
    };
    let wrote = vfs::write(slot, 0, &bytes) == Ok(bytes.len())
        && vfs::write(slot, 12_000, b"end") == Ok(3)
        && vfs::info(slot) == Some((12_003, false));
    let mut back = alloc::vec![0xffu8; 12_003];
    let mut done = 0;
    while let Ok(n @ 1..) = vfs::read(slot, done as u64, &mut back[done..]) {
        done += n;
    }
    let read_back = done == back.len()
        && back[..10_000] == bytes[..]
        && back[10_000..12_000].iter().all(|&b| b == 0)
        && back[12_000..] == *b"end";
    let mut on_disk = alloc::vec![0u8; 12_003];
    let virtio_has = on_volume(raw::disk::Driver::Virtio, "/Written.txt", &mut on_disk)
        == Some(12_003)
        && on_disk == back;
    let ahci = raw::ahci::drives().iter().flatten().map(|d| d.port).next();
    let ahci_has_not = ahci.is_some_and(|port| {
        on_volume(raw::disk::Driver::Ahci(port), "/Written.txt", &mut on_disk).is_none()
    });
    report.check(
        "kernel.vfs.write",
        wrote && read_back && virtio_has && ahci_has_not,
        "a file made on /System should take ten thousand bytes and three more past its end, read back so with zeros between, and be so on the virtio disk's copy alone",
    );
    let reader = vfs::open(path, &system, READ, 0);
    let refusals = vfs::open(path, &system, READ | WRITE, CREATE_NEW) == Err(Error::Exists)
        && reader.is_ok_and(|r| vfs::write(r, 0, b"x").is_err())
        && vfs::remove(path, &system) == Err(Error::InUse);
    reader.into_iter().chain([slot]).for_each(vfs::close);
    let emptied = vfs::open(path, &system, READ | WRITE, CREATE_ALWAYS)
        .inspect(|&s| vfs::close(s))
        .is_ok_and(|_| file_size(path) == Some(0));
    let gone = vfs::remove(path, &system).is_ok()
        && vfs::open(path, &system, READ, 0) == Err(Error::NotFound)
        && vfs::open(path, &system, READ | WRITE, TRUNCATE_EXISTING) == Err(Error::NotFound)
        && vfs::remove(path, &system) == Err(Error::NotFound);
    let again = vfs::open(path, &system, READ | WRITE, OPEN_ALWAYS)
        .inspect(|&s| vfs::close(s))
        .is_ok()
        && file_size(path) == Some(0)
        && vfs::remove(path, &system).is_ok();
    report.check(
        "kernel.vfs.write_ways",
        refusals && emptied && gone && again,
        "a file there already should not be made new, one opened to read should take no writes, an open one should stay, CREATE_ALWAYS should empty it, and once removed it should be gone until OPEN_ALWAYS makes it again",
    );
}

/// The size of the file at `path`, opened to read for a moment.
fn file_size(path: &str) -> Option<u64> {
    use raw::vfs;
    let slot = vfs::open(path, &security::Token::system(), abi::file::READ, 0).ok()?;
    let size = vfs::info(slot).map(|(size, _)| size);
    vfs::close(slot);
    size
}

/// Reads the file at `path` on the system partition behind `driver`
/// through a FAT32 reader of its own into `into`, and returns its size.
fn on_volume(driver: raw::disk::Driver, path: &str, into: &mut [u8]) -> Option<u32> {
    let mut read = |lba, into: &mut [u8]| raw::disk::read_from(driver, lba, into);
    let system = kernel::disk::Table::read(&mut read)?.first_of(gpt::MERIDIAN_SYSTEM)?;
    let mut fat = fat::Fat::open(|lba, into: &mut [u8]| {
        raw::disk::read_from(driver, system.first + lba, into)
    })
    .ok()?;
    let entry = fat.find(path).ok()?;
    fat.read_file(&entry, into).ok()?;
    Some(entry.size)
}

/// Directories made, one in another, a file in it, removed only once
/// they are empty; one cannot be made twice, nor where there is no
/// directory to hold it, nor written to; and a token at the low level may
/// make none.
fn directory_write_checks<W: Write>(report: &mut Report<'_, W>) {
    use abi::file::{CREATE_NEW, READ, WRITE};
    use raw::vfs::{self, Error};
    use security::{Integrity, Privileges, Restriction};
    let system = security::Token::system();
    let made = vfs::make_directory("/System/Folder", &system).is_ok()
        && vfs::make_directory("/System/Folder/Inner", &system).is_ok()
        && vfs::open(
            "/System/Folder/Inner/a.txt",
            &system,
            READ | WRITE,
            CREATE_NEW,
        )
        .inspect(|&s| vfs::close(s))
        .is_ok();
    let listed = vfs::open("/System/Folder", &system, READ, 0).is_ok_and(|slot| {
        let mut room = [0u8; 16];
        let found = vfs::list(slot, 0, &mut room) == Ok((5, true)) && &room[..5] == b"Inner";
        vfs::close(slot);
        found
    });
    let refused = vfs::make_directory("/System/Folder", &system) == Err(Error::Exists)
        && vfs::make_directory("/System/Nowhere/Inner", &system) == Err(Error::NotFound)
        && vfs::make_directory("/System", &system) == Err(Error::BadPath)
        && vfs::open("/System/Folder", &system, READ | WRITE, 0).is_ok_and(|slot| {
            let no = vfs::write(slot, 0, b"x") == Err(Error::NotAFile);
            vfs::close(slot);
            no
        })
        && vfs::remove("/System/Folder/Inner", &system) == Err(Error::NotEmpty);
    let low = system.restricted(Restriction {
        integrity: Integrity::Low,
        keep: Privileges::NONE,
    });
    let low_refused = matches!(
        vfs::make_directory("/System/Low", &low),
        Err(Error::Refused(kernel::vfs::Refusal::Denied(_)))
    ) && matches!(
        vfs::open("/System/Low.txt", &low, READ | WRITE, CREATE_NEW),
        Err(Error::Refused(kernel::vfs::Refusal::Denied(_)))
    );
    let removed = vfs::remove("/System/Folder/Inner/a.txt", &system).is_ok()
        && vfs::remove("/System/Folder/Inner", &system).is_ok()
        && vfs::remove("/System/Folder", &system).is_ok()
        && vfs::open("/System/Folder", &system, READ, 0) == Err(Error::NotFound);
    report.check(
        "kernel.vfs.directories",
        made && listed && refused && low_refused && removed,
        "directories should be made inside each other and listed, not twice, nowhere and not over the root, take no bytes, stay while they hold something, be refused to a low token, and go once empty",
    );
}

/// Mounting more: a place that is taken, in other capitals, takes no
/// second volume; a disk without FAT32 none; the boot volume read through
/// a second place reads the same files; and five volumes, /System and
/// /System/Boot among them, are all there is room for.
fn mount_checks<W: Write>(report: &mut Report<'_, W>) {
    use abi::file::READ;
    use raw::vfs;
    let system = security::Token::system();
    let reader = |port: usize, first: u64| -> raw::vfs::Reader {
        alloc::boxed::Box::new(move |lba, into: &mut [u8]| {
            raw::ahci::read(port, first + lba, into).is_ok()
        })
    };
    let mounted = esp_of_disk().is_some_and(|(port, esp)| {
        use kernel::vfs::MountError::Taken;
        use raw::vfs::MountError::{Full, NotFat, Place};
        let taken = vfs::mount("/system/boot", reader(port, esp.first), None) == Err(Place(Taken));
        let blank = vfs::mount("/Volumes/Blank", reader(port, 0), None) == Err(NotFat);
        let again = vfs::mount("/Volumes/Again", reader(port, esp.first), None).is_ok();
        let through = vfs::open("/Volumes/Again/meridian/boot.meri", &system, READ, 0)
            .inspect(|&slot| vfs::close(slot))
            .is_ok();
        let filled = vfs::mount("/Volumes/Third", reader(port, esp.first), None).is_ok()
            && vfs::mount("/Volumes/Fourth", reader(port, esp.first), None).is_ok();
        let full = vfs::mount("/Volumes/Fifth", reader(port, esp.first), None) == Err(Full);
        taken && blank && again && through && filled && full
    });
    report.check(
        "kernel.vfs.mount",
        mounted,
        "a place should take one volume, a disk without FAT32 none, and five volumes all there is room for",
    );
}

/// The programs' directory lists the four programs test images carry and
/// the second copy of each, none of them a directory.
fn list_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::vfs;
    let system = security::Token::system();
    let mut listed = alloc::vec::Vec::new();
    if let Ok(slot) = vfs::open(
        "/System/Boot/meridian/programs",
        &system,
        abi::file::READ,
        0,
    ) {
        let mut room = [0u8; 64];
        while let Ok((len, directory)) = vfs::list(slot, listed.len() as u64, &mut room) {
            listed.push((
                alloc::string::String::from_utf8_lossy(&room[..len]).into_owned(),
                directory,
            ));
        }
        vfs::close(slot);
    }
    let mut names: alloc::vec::Vec<&str> = listed.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    report.check(
        "kernel.vfs.list",
        names
            == [
                "exitwith.mexec",
                "exitwith_again.mexec",
                "hello.mexec",
                "hello_again.mexec",
                "init.mexec",
                "init_again.mexec",
                "shell.mexec",
                "shell_again.mexec",
                "threads.mexec",
                "threads_again.mexec",
                "whoami.mexec",
                "whoami_again.mexec",
            ]
            && listed.iter().all(|(_, directory)| !directory),
        "the programs' directory should list the six programs test images carry, each twice",
    );
}

/// The PS/2 keyboard, through the I/O APIC: the keyboard's input is let
/// through to this processor as the keyboard's vector, edge triggered and
/// active high as q35 has it, and each key sextant presses comes as its
/// events, a shift around the key it shifts and an arrow after its prefix.
/// The interrupts it comes by need interrupts on, as they are while the
/// timer runs. On a machine whose FADT says it has no 8042, the keyboard
/// is a USB one: nothing answers, nothing is let through and no key comes.
fn keyboard_checks<W: Write>(report: &mut Report<'_, W>, machine: &Machine, tsc_hz: Option<u64>) {
    use ps2::{Event, Key};
    let present = machine.i8042 != Some(false);
    let routed = machine
        .keyboard
        .zip(machine.io_apic)
        .is_some_and(|(line, io)| {
            let entry = raw::ioapic::entry(line.interrupt - io.first);
            entry & 0xff == u64::from(raw::interrupts::KEYBOARD)
                && entry & 1 << 16 == 0
                && entry & (1 << 13 | 1 << 15) == 0
        });
    report.check(
        "kernel.keyboard.routed",
        routed == present,
        "the keyboard's input of the I/O APIC should be let through as its vector, edge triggered and active high, if there is an 8042",
    );
    let held = x86::ioapic::entry(0, 0, false, false, true);
    let held_back = machine
        .keyboard
        .zip(machine.io_apic)
        .is_some_and(|(line, io)| {
            let keyboard = line.interrupt - io.first;
            (0..24)
                .filter(|&input| input != keyboard || !present)
                .all(|input| raw::ioapic::entry(input) == held)
        });
    // An input nothing drives, held back still, sent to another APIC for a
    // moment: its destination is in the entry's high half.
    let elsewhere = x86::ioapic::entry(0x30, 2, true, true, true);
    raw::ioapic::route(23, elsewhere);
    let read_back = raw::ioapic::entry(23) == elsewhere;
    raw::ioapic::route(23, held);
    report.check(
        "kernel.ioapic.held_back",
        held_back && read_back && raw::ioapic::entry(23) == held,
        "every other input of the I/O APIC should be held back and nothing else, and an entry read back as written, its destination too",
    );
    let wanted = ps2::config::FIRST_INTERRUPT | ps2::config::TRANSLATION;
    let unwanted = ps2::config::FIRST_CLOCK_OFF | ps2::config::SECOND_INTERRUPT;
    let configuration = x86::raw::without_interrupts(raw::ps2::configuration);
    let configured = configuration.is_some_and(|c| c & wanted == wanted && c & unwanted == 0);
    report.check(
        "kernel.keyboard.configured",
        if present { configured } else { configuration.is_none() },
        "the controller should give the keyboard its interrupt, its clock and translation, and the second port no interrupt; without an 8042 nothing answers",
    );
    while raw::ps2::next().is_some() {}
    let down = |key| Event { key, pressed: true };
    let up = |key| Event {
        key,
        pressed: false,
    };
    let presses = [
        ("a", alloc::vec![down(Key::A), up(Key::A)]),
        (
            "shift-z",
            alloc::vec![
                down(Key::LEFT_SHIFT),
                down(Key::Z),
                up(Key::Z),
                up(Key::LEFT_SHIFT)
            ],
        ),
        ("up", alloc::vec![down(Key::UP), up(Key::UP)]),
        ("f12", alloc::vec![down(Key::F12), up(Key::F12)]),
        ("kp_enter", alloc::vec![down(Key(0x58)), up(Key(0x58))]),
    ];
    let patience = tsc_hz.unwrap_or(1_000_000_000);
    let all = presses.iter().all(|(press, events)| {
        let wanted = if present { events.as_slice() } else { &[] };
        let _ = writeln!(report.out, "PRESS {press}");
        let mut got = alloc::vec::Vec::new();
        let start = x86::raw::tsc();
        while got.len() < wanted.len() && x86::raw::tsc() - start < patience {
            match raw::ps2::next() {
                Some(event) => got.push(event),
                None => core::hint::spin_loop(),
            }
        }
        raw::thread::sleep(2);
        while let Some(event) = raw::ps2::next() {
            got.push(event);
        }
        got == wanted
    });
    report.check(
        "kernel.keyboard.keys",
        all,
        "each key sextant presses should come as the keyboard's events, in order and nothing else, or nothing without an 8042",
    );
}

/// The console, once it takes the screen over: text written goes into its
/// grid and shows on the screen, light on the dark around it, a form feed
/// clears it, and text past its last row moves the rows up. sextant keeps
/// a picture of it.
fn console_checks<W: Write>(report: &mut Report<'_, W>, display: Option<Display<'static>>) {
    use canvas::palette::{NIGHT, SUN};
    let Some(display) = display else {
        return;
    };
    let started = raw::console::start(display).is_some();
    raw::console::write("Meridian\n");
    let shape = raw::console::shape();
    let lit = shape.is_some_and(|(_, (width, height))| {
        (16..16 + width).any(|x| {
            (16..16 + height).any(|y| raw::console::pixel(x, y).is_some_and(|c| c != NIGHT))
        })
    });
    report.check(
        "kernel.console.write",
        started
            && raw::console::row(0).as_deref() == Some("Meridian")
            && lit
            && raw::console::pixel(4, 4) == Some(NIGHT),
        "text written should go into the console's first row and show light on the dark around it",
    );
    let cell = |column: usize, row: usize| {
        let (width, height) = shape.map_or((0, 0), |(_, cell)| cell);
        raw::console::pixel(
            16 + column * width + width / 2,
            16 + row * height + height / 2,
        )
    };
    let waiting = cell(0, 1) == Some(SUN);
    raw::console::write("x");
    let typed = raw::console::row(1).as_deref() == Some("x") && cell(1, 1) == Some(SUN);
    raw::console::write("\u{8}");
    let erased = raw::console::row(1).as_deref() == Some("")
        && cell(0, 1) == Some(SUN)
        && cell(1, 1) == Some(NIGHT);
    raw::console::write("  \n  ");
    let one = cell(2, 1) == Some(NIGHT) && cell(2, 2) == Some(SUN);
    let columns = shape.map_or(0, |((columns, _), _)| columns);
    raw::console::write(&" ".repeat(columns - 2));
    let edge = cell(columns - 1, 2) == Some(SUN) && cell(0, 3) == Some(NIGHT);
    report.check(
        "kernel.console.cursor",
        waiting && typed && erased && one && edge,
        "the cursor should be a block of sun in the cell the next character goes to, in its row alone, in the last column while it waits at the edge, and a backspace should leave the dark behind",
    );
    console_cell_checks(report, shape);
    let rows = shape.map_or(0, |((_, rows), _)| rows);
    raw::console::write("\u{c}");
    let cleared = raw::console::row(0).as_deref() == Some("");
    for n in 0..=rows {
        raw::console::write(&alloc::format!("line {n}\n"));
    }
    report.check(
        "kernel.console.scroll",
        rows > 2 && cleared && raw::console::row(0).as_deref() == Some("line 2"),
        "a form feed should clear the console, and lines past its last row move the rows up",
    );
    let _ = keep_screen(report, "console");
}

/// Each character in its own cell: an M far to the right looks exactly as
/// one in the first column, and a character is rasterized once, however
/// often it is drawn.
fn console_cell_checks<W: Write>(
    report: &mut Report<'_, W>,
    shape: Option<((usize, usize), (usize, usize))>,
) {
    use canvas::palette::NIGHT;
    let Some(((columns, _), (width, height))) = shape else {
        return;
    };
    let far = columns.min(101) - 1;
    raw::console::write(&alloc::format!("\u{c}M{}M", " ".repeat(far - 1)));
    let cell = |column: usize| {
        (0..height).flat_map(move |dy| {
            (0..width).map(move |dx| raw::console::pixel(16 + column * width + dx, 16 + dy))
        })
    };
    let alike = cell(0).eq(cell(far));
    let inked = cell(0).filter(|&p| p.is_some_and(|p| p != NIGHT)).count();
    // An i sits well right of its pen, as narrow letters do in a
    // monospaced font.
    raw::console::write("\u{c}i");
    let placed =
        raw::console::drawn_alone('i').is_some_and(|alone| cell(0).eq(alone.into_iter().map(Some)));
    report.check(
        "kernel.console.cells",
        far > 50 && alike && inked > 10 && placed,
        "an M in a column far to the right should look exactly as one in the first, and an i in its cell as it looks drawn on its own",
    );
    let before = raw::console::rasterized();
    raw::console::write("\u{a7}\u{a7}\u{a7}\n");
    let once = raw::console::rasterized();
    raw::console::write("\u{a7}");
    report.check(
        "kernel.console.glyphs",
        once == before + 1 && raw::console::rasterized() == once,
        "a character should be rasterized once, however often it is drawn",
    );
}

/// Asks sextant to keep a picture of the screen as `name`, and says whether
/// it answered with a carriage return, as it does once it has. COM1 as it
/// is, not opened again: that would clear an answer that came quickly.
fn keep_screen<W: Write>(report: &mut Report<'_, W>, name: &str) -> bool {
    let _ = writeln!(report.out, "SCREEN {name}");
    Com1::already_open().receive(1 << 24) == Some(b'\r')
}

/// The shell, on the console, typed at with keys sextant presses: echo
/// shows its words, ls lists a directory with its subdirectories marked,
/// or booted from a CD says there is none, cat shows a file longer than
/// one read to its end, a command it does not know is named as one, a
/// program's name runs it from its file, and exit ends it.
fn shell_checks<W: Write>(
    report: &mut Report<'_, W>,
    info: &BootInfo,
    regions: &[MemoryRegion],
    machine: &Machine,
    tsc_hz: Option<u64>,
) {
    use raw::user::{self, seen};
    let Some((_, file)) = raw::modules(info, regions).find(|(m, _)| kernel::is_shell(m.name()))
    else {
        report.check("kernel.shell", false, "every image should carry the shell");
        return;
    };
    // Without the 8042 nothing could type at it.
    if raw::console::shape().is_none() || machine.i8042 == Some(false) {
        return;
    }
    raw::console::write("\u{c}");
    seen::forget();
    let started = user::start_file(0, file, 0);
    // sextant takes 50 ms a key, and a path is a few dozen of them.
    let patience = tsc_hz.unwrap_or(1_000_000_000) * 5;
    let mut typed = |keys: &[&str]| {
        for key in keys {
            let _ = writeln!(report.out, "PRESS {key}");
        }
        raw::thread::sleep(raw::timer::HZ / 5);
    };
    let rows = || -> alloc::vec::Vec<alloc::string::String> {
        let count = raw::console::shape().map_or(0, |((_, rows), _)| rows);
        (0..count).filter_map(raw::console::row).collect()
    };
    let waited_for = |line: &str| {
        let start = x86::raw::tsc();
        while x86::raw::tsc() - start < patience {
            if rows().iter().any(|r| r == line) {
                return true;
            }
            raw::thread::sleep(1);
        }
        false
    };
    let greeted = waited_for("Meridian shell. Type help to see what it knows.");
    typed(&["e", "c", "h", "o", "spc", "h", "i", "ret"]);
    let echoed = waited_for("> echo hi") && waited_for("hi");
    typed(&[
        "l", "s", "spc", "slash", "shift-s", "y", "s", "t", "e", "m", "slash", "shift-b", "o", "o",
        "t", "slash", "m", "e", "r", "i", "d", "i", "a", "n", "ret",
    ]);
    let disk = esp_of_disk().is_some();
    let listed = if disk {
        waited_for("boot.meri") && waited_for("programs/")
    } else {
        waited_for("/System/Boot/meridian: no such file")
    };
    // The shim's licences take three reads and more.
    typed(&[
        "c", "a", "t", "spc", "slash", "shift-s", "y", "s", "t", "e", "m", "slash", "shift-b", "o",
        "o", "t", "slash", "m", "e", "r", "i", "d", "i", "a", "n", "slash", "l", "i", "c", "e",
        "n", "c", "e", "s", "slash", "s", "h", "i", "m", "dot", "t", "x", "t", "ret",
    ]);
    let shown = if disk {
        waited_for(" OF THE POSSIBILITY OF SUCH DAMAGE.")
    } else {
        waited_for("/System/Boot/meridian/licences/shim.txt: no such file")
    };
    typed(&["n", "o", "p", "e", "ret"]);
    let refused = waited_for("nope: no such command");
    typed(&["s", "e", "r", "v", "i", "c", "e", "s", "ret"]);
    let alone = waited_for("services: init did not start this shell");
    // exitwith, given no channel, ends with 0xbad.
    typed(&["e", "x", "i", "t", "w", "i", "t", "h", "ret"]);
    let ran = if disk {
        waited_for("exitwith ended with 2989")
    } else {
        waited_for("exitwith: no such command")
    };
    typed(&["e", "x", "i", "t", "ret"]);
    let ended = started.is_ok_and(|slot| {
        let start = x86::raw::tsc();
        while raw::thread::alive(slot) && x86::raw::tsc() - start < patience {
            raw::thread::sleep(1);
        }
        !raw::thread::alive(slot) && seen::exit_of(slot) == 0
    });
    report.check(
        "kernel.shell",
        greeted && echoed && listed && shown && refused && alone && ran && ended,
        "the shell should greet, echo, list a directory, show a long file to its end, name a command it does not know, say init did not start it when asked for the services, run a program from its file and say how it ended, and end at exit",
    );
}

/// init, handed a channel end: of the services the test image lists, it
/// runs setup, which is whoami, once, starts whoami, which waits for setup
/// to end and ends at once, again and again but waiting longer each time,
/// and the shell once whoami started, and once the channel's other end closes,
/// it ends them and itself, so every frame they took comes back. Booted
/// from a CD it has no list, so it starts the shell alone. Asked over that
/// channel, and by its shell over the line init gave it, init says what
/// each service is doing.
fn init_checks<W: Write>(
    report: &mut Report<'_, W>,
    info: &BootInfo,
    regions: &[MemoryRegion],
    machine: &Machine,
    tsc_hz: Option<u64>,
) {
    use handles::Rights;
    use raw::user::{self, seen};
    let Some((_, file)) = raw::modules(info, regions).find(|(m, _)| kernel::is_init(m.name()))
    else {
        report.check("kernel.init", false, "every image should carry init");
        return;
    };
    let disk = esp_of_disk().is_some();
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let before = free();
    seen::forget();
    let channel = raw::ipc::create();
    let channels = raw::ipc::live();
    // init reads its start messages first: there are none but the end.
    let started = channel.and_then(|(ours, theirs)| {
        raw::ipc::send(ours, &[0], None).ok()?;
        user::start_file_holding(0, file, &[theirs], Rights::READ | Rights::WRITE).ok()
    });
    raw::thread::sleep(60);
    // init closes the line of each program that ended: what is left is
    // the shell's, and whoami's while it runs.
    let lines = raw::ipc::live().saturating_sub(channels);
    let lines_closed = if disk { lines <= 2 } else { lines <= 1 };
    let spawns = seen::calls(abi::SPAWN);
    let events = seen::events();
    let first = |line: &[u8]| events.iter().position(|e| e.starts_with(line));
    let ran = if disk {
        // setup and whoami run the same program; init's lines tell them apart.
        let setup = b"init: setup ended with 0";
        let once = events.iter().filter(|e| e.starts_with(setup)).count() == 1;
        let whoami = events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.starts_with(b"start whoami"))
            .nth(1)
            .map(|(at, _)| at);
        let waited = first(setup)
            .zip(whoami)
            .is_some_and(|(ended, started)| ended < started)
            && whoami
                .zip(first(b"start shell"))
                .is_some_and(|(whoami, shell)| whoami < shell);
        (6..=12).contains(&spawns) && once && waited && seen::logged(b"init: whoami ended with 0")
    } else {
        spawns == 1
    };
    init_services_checks(report, channel.map(|(ours, _)| ours), disk, machine, tsc_hz);
    if let Some((ours, _)) = channel {
        raw::ipc::close(ours);
    }
    let ended = ended_with(started, 0);
    let back = (0..100).any(|_| {
        raw::thread::sleep(1);
        free() == before
    });
    // init ends after its services, so its end is the last one logged.
    let last = seen::ended(b"user: init ended with 0\n");
    report.check(
        "kernel.init",
        ran && lines_closed && ended && back && last,
        "init should run a service once where it says so, start one that waits for it only after it ended and one that waits for a service that keeps running only after that started, start a service again whenever it ended, waiting longer each time, close the line of each program that ended, and end its services and itself once its channel closes",
    );
}

/// What init, holding the other end of `ours`, says about its services:
/// asked over that channel, each of the test image's services with what it
/// does and how often it started, and none past them; asked by its shell
/// with the services command, where keys can be typed, the same as lines
/// on the console.
fn init_services_checks<W: Write>(
    report: &mut Report<'_, W>,
    ours: Option<u64>,
    disk: bool,
    machine: &Machine,
    tsc_hz: Option<u64>,
) {
    use services::{About, State};
    let ask = |place: u8| -> Option<alloc::vec::Vec<u8>> {
        let ours = ours?;
        raw::ipc::send(ours, &services::ask(place), None).ok()?;
        let mut room = [0u8; services::MESSAGE];
        (0..100)
            .find_map(|_| match raw::ipc::receive(ours, &mut room) {
                Ok((len, _)) => Some(Some(room[..len].to_vec())),
                Err(channel::Error::Empty) => {
                    raw::thread::sleep(1);
                    None
                }
                Err(_) => Some(None),
            })
            .flatten()
    };
    let answers: alloc::vec::Vec<_> = (0..4).map(ask).collect();
    let told = |place: usize| {
        answers
            .get(place)
            .and_then(Option::as_deref)
            .and_then(|a| services::read(a).ok())
    };
    let shell = About {
        name: "shell",
        state: State::Running,
        starts: 1,
    };
    let asked = if disk {
        let setup = About {
            name: "setup",
            state: State::Ended,
            starts: 1,
        };
        told(0) == Some(Some(setup))
            && told(1).flatten().is_some_and(|a| {
                a.name == "whoami"
                    && matches!(a.state, State::Running | State::Resting)
                    && (5..=11).contains(&a.starts)
            })
            && told(2) == Some(Some(shell))
            && told(3) == Some(None)
    } else {
        told(0) == Some(Some(shell)) && told(1) == Some(None)
    };
    report.check(
        "kernel.init.services",
        asked,
        "asked over its channel, init should say what each service does and how often it started, and that none is past the last",
    );
    // Without the 8042 nothing could type at init's shell.
    if raw::console::shape().is_none() || machine.i8042 == Some(false) {
        return;
    }
    let patience = tsc_hz.unwrap_or(1_000_000_000) * 5;
    type_line(report, "services");
    let shown = |line: &str| {
        let start = x86::raw::tsc();
        while x86::raw::tsc() - start < patience {
            let count = raw::console::shape().map_or(0, |((_, rows), _)| rows);
            if (0..count).filter_map(raw::console::row).any(|r| r == line) {
                return true;
            }
            raw::thread::sleep(1);
        }
        false
    };
    let listed = if disk {
        shown("setup: ended, started once") && shown("shell: running, started once")
    } else {
        shown("shell: running, started once")
    };
    report.check(
        "kernel.init.shell_services",
        listed,
        "the shell init started should list init's services with the services command",
    );
    if disk {
        shell_file_checks(report, &shown);
    }
}

/// Files and directories made, written, shown and removed by what is typed
/// at the shell, `shown` waiting for each line it should show. On a machine
/// whose virtio disk only reads, /System hangs read only, and the shell
/// says so instead.
fn shell_file_checks<W: Write>(report: &mut Report<'_, W>, shown: &dyn Fn(&str) -> bool) {
    let offered = raw::virtio::agreed().map_or(0, |(_, offered, ..)| offered);
    if virtio::read_only(offered) {
        type_line(report, "md /System/New");
        let md = shown("/System/New: on a volume that is only read");
        type_line(report, "echo hi > /System/a");
        let echo = shown("/System/a: on a volume that is only read");
        type_line(report, "rm /System/made.meri");
        let rm = shown("/System/made.meri: on a volume that is only read");
        report.check(
            "kernel.shell.files",
            md && echo && rm,
            "typed at the shell, md, echo into a file and rm should say that /System is only read where its disk only reads",
        );
        return;
    }
    type_line(report, "md /System/New");
    type_line(report, "ls /System");
    let made = shown("New/");
    type_line(report, "echo hi > /System/New/a");
    type_line(report, "echo more >> /System/New/a");
    type_line(report, "cat /System/New/a");
    let mut held = [0u8; 16];
    let written = shown("more")
        && raw::vfs::open(
            "/System/New/a",
            &security::Token::system(),
            abi::file::READ,
            0,
        )
        .is_ok_and(|slot| {
            let read = raw::vfs::read(slot, 0, &mut held);
            raw::vfs::close(slot);
            read.is_ok_and(|n| held[..n] == *b"hi\nmore\n")
        });
    type_line(report, "rm /System/New");
    let stays = shown("/System/New: a directory that holds something");
    type_line(report, "rm /System/New/a");
    type_line(report, "rm /System/New");
    type_line(report, "cat /System/New/a");
    let gone = shown("/System/New/a: no such file")
        && raw::vfs::open(
            "/System/New",
            &security::Token::system(),
            abi::file::READ,
            0,
        ) == Err(raw::vfs::Error::NotFound);
    report.check(
        "kernel.shell.files",
        made && written && stays && gone,
        "typed at the shell, md should make a directory ls shows, echo should write a file and add to it, cat show it, and rm keep a directory that holds something and remove the file and then the directory",
    );
}

/// Asks sextant to type `line` and press return, a key for each
/// character as QEMU names keys on a US keyboard.
fn type_line<W: Write>(report: &mut Report<'_, W>, line: &str) {
    for c in line.chars().chain(['\n']) {
        let _ = match c {
            ' ' => writeln!(report.out, "PRESS spc"),
            '/' => writeln!(report.out, "PRESS slash"),
            '.' => writeln!(report.out, "PRESS dot"),
            '>' => writeln!(report.out, "PRESS shift-dot"),
            '\n' => writeln!(report.out, "PRESS ret"),
            c if c.is_ascii_uppercase() => {
                writeln!(report.out, "PRESS shift-{}", c.to_ascii_lowercase())
            }
            c => writeln!(report.out, "PRESS {c}"),
        };
    }
}

/// The AHCI port of the disk and the EFI system partition on it.
fn esp_of_disk() -> Option<(usize, gpt::Partition)> {
    let drives = raw::ahci::drives();
    let disk = drives
        .iter()
        .flatten()
        .find(|d| d.attached == ahci::Attached::Disk)?;
    let port = disk.port;
    let mut read = |lba, into: &mut [u8]| raw::ahci::read(port, lba, into).is_ok();
    let esp = kernel::disk::Table::read(&mut read)?.first_of(gpt::EFI_SYSTEM)?;
    Some((port, esp))
}

/// The EFI system partition, read by the kernel itself through AHCI, the
/// GPT and FAT32: the settings and the long name of test images are what
/// sextant wrote, the firmware's file is a PE file, and hello is byte for
/// byte what the loader read through the firmware.
fn fat_checks<W: Write>(report: &mut Report<'_, W>, info: &BootInfo, regions: &[MemoryRegion]) {
    let drives = raw::ahci::drives();
    let Some(disk) = drives
        .iter()
        .flatten()
        .find(|d| d.attached == ahci::Attached::Disk)
    else {
        return;
    };
    let port = disk.port;
    let mut read = |lba, into: &mut [u8]| raw::ahci::read(port, lba, into).is_ok();
    let Some(esp) = kernel::disk::Table::read(&mut read).and_then(|t| t.partitions().next()) else {
        report.check(
            "kernel.fat.read",
            false,
            "the boot disk should have a partition table",
        );
        return;
    };
    let read = |lba, into: &mut [u8]| raw::ahci::read(port, esp.1.first + lba, into).is_ok();
    let Ok(mut volume) = fat::Fat::open(read) else {
        report.check(
            "kernel.fat.read",
            false,
            "the EFI system partition should hold FAT32",
        );
        return;
    };
    let mut file = |path: &str, room: usize| {
        let entry = volume.find(path).ok()?;
        let mut bytes = alloc::vec![0u8; room.min(entry.size as usize)];
        volume.read_file(&entry, &mut bytes).ok()?;
        Some((entry, bytes))
    };
    let settings = file("meridian/boot.meri", 4096);
    let firmware = file("EFI/BOOT/BOOTX64.EFI", 2);
    let long = file(&alloc::format!("long-name-{}", "x".repeat(244)), 64);
    let hello = file("meridian/programs/hello.mexec", usize::MAX);
    let loaded = raw::modules(info, regions).find(|(m, _)| m.name() == b"hello.mexec");
    report.check(
        "kernel.fat.read",
        settings.is_some_and(|(_, s)| s == b"boot {\n    timeout = 2500ms\n    default = example\n    verbose = true\n}\n")
            && firmware.is_some_and(|(_, f)| f == b"MZ")
            && long.is_some_and(|(_, l)| l == b"long name\n")
            && hello.zip(loaded).is_some_and(|((e, h), (_, l))| e.size as usize == l.len() && h == l),
        "the kernel should read the EFI system partition's files as sextant wrote them and the loader read them",
    );
}

/// The kernel stays off the pages of ring 3: SMEP and SMAP are on, so
/// running code from such a page faults, and so does reading one, except
/// where the kernel copies from a program on purpose.
fn protection_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::interrupts::expected;
    x86::qemu_test::protections(&mut |name, ok, why| report.check(name, ok, why));
    let tried = raw::memory::with_user_page(|page| {
        let fetched = expected::call_at(page);
        let read = expected::read_at(page);
        let copied = x86::raw::with_user_access(|| expected::read_at(page));
        (page, fetched, read, copied, x86::raw::user_access())
    });
    report.check(
        "kernel.protect.smep",
        tried.is_some_and(|(page, fetched, ..)| {
            fetched.is_some_and(|c| c.error == 0x11 && c.address == page)
        }),
        "running code from a page of ring 3 should fault in the kernel",
    );
    report.check(
        "kernel.protect.smap",
        tried.is_some_and(|(page, _, read, copied, after)| {
            read.is_some_and(|c| c.error == 0x1 && c.address == page) && copied.is_none() && !after
        }),
        "reading a page of ring 3 should fault, except inside with_user_access, which clears AC again",
    );
    user_copy_checks(report);
}

/// A copy to or from a program stops at a page that is not there, as when
/// another of its threads gave the page back meanwhile, and answers how
/// much it did not copy instead of crashing the kernel.
fn user_copy_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::user::{copy_without_looking as copy, seen};
    let faults = seen::copy_faults();
    let tried = raw::memory::with_user_page(|page| {
        let mut room = [0u8; 8];
        let whole = copy(&mut room, page);
        let first = room[0];
        let short = copy(&mut room, page + 4096 - 4);
        let none = copy(&mut room, page + 2 * 4096);
        (whole, short, none, first, x86::raw::user_access())
    });
    report.check(
        "kernel.user.copy_stops",
        tried.is_some_and(|(whole, short, none, first, open)| {
            whole == 0 && short > 0 && none == 8 && first == 0xc3 && !open
        }) && seen::copy_faults() == faults + 2,
        "a copy from a program should stop where its pages do, say how much it left, and close the program's pages again",
    );
    let looked = raw::memory::with_user_page(|page| {
        let mut room = [0u8; 4];
        let kernels = room.as_ptr() as u64;
        let read = raw::user::copy_from_user(page, &mut room) && room[0] == 0xc3;
        let gap = raw::user::copy_from_user(page + 4096 - 2, &mut room);
        let kernel = raw::user::copy_from_user(kernels, &mut room);
        let written = raw::user::copy_to_user(page, &[1, 2]);
        let kept = raw::user::copy_from_user(page, &mut room) && room[..2] == [0xc3, 0];
        let empty = raw::user::copy_from_user(page + 2 * 4096, &mut []);
        (read, gap, kernel, written, kept, empty)
    });
    report.check(
        "kernel.user.copy_looks",
        looked == Some((true, false, false, false, true, true)) && seen::copy_faults() == faults + 2,
        "a system call's copy should take a program's bytes, refuse a gap, the kernel's half and bytes ring 3 may not write before anything faults, and copy nothing from nowhere",
    );
}

/// The kernel's own tables are loaded, and each exception a check asks for
/// reaches the handler with the right vector, error code and address.
fn exception_checks<W: Write>(report: &mut Report<'_, W>) {
    use crate::raw::interrupts::{KEYBOARD, SHOOTDOWN, SPURIOUS, SYSCALL, TIMER, WAKE};
    x86::qemu_test::tables(
        &[TIMER, WAKE, KEYBOARD, SHOOTDOWN, SYSCALL, SPURIOUS],
        raw::interrupts::CORES,
        &mut |name, ok, why| {
            report.check(name, ok, why);
        },
    );
    let caught = |c: Option<Caught>, vector: u64| c.is_some_and(|c| c.vector == vector);
    report.check(
        "kernel.exception.breakpoint",
        caught(expected::breakpoint(), 3),
        "int3 should reach the handler as vector 3 and come back",
    );
    report.check(
        "kernel.exception.invalid_opcode",
        caught(expected::invalid_opcode(), 6),
        "ud2 should reach the handler as vector 6",
    );
    report.check(
        "kernel.exception.divide",
        caught(expected::divide_by_zero(), 0),
        "dividing by zero should reach the handler as vector 0",
    );
    let null = expected::null_read();
    report.check(
        "kernel.exception.page_fault",
        null.is_some_and(|c| c.vector == 14 && c.error == 0 && c.address == 0),
        "reading address 0 should be a page fault on a missing page, with CR2 at 0",
    );
    let selector = expected::bad_selector();
    report.check(
        "kernel.exception.general_protection",
        selector.is_some_and(|c| c.vector == 13 && c.error == 0x28),
        "loading selector 0x28 should be a general protection fault naming it",
    );
}

fn machine_checks<W: Write>(report: &mut Report<'_, W>, machine: &Machine, tsc_hz: Option<u64>) {
    smp_checks(report, machine, tsc_hz);
    report.check(
        "kernel.acpi.cores",
        machine.cores.is_some_and(|n| n >= 1),
        "the MADT should list at least the processor the kernel runs on",
    );
    let pm_timer = machine.pm_timer;
    report.check(
        "kernel.acpi.pm_timer",
        pm_timer.is_some_and(|(port, bits)| port != 0 && (bits == 24 || bits == 32)),
        "the FADT should name the power management timer's port",
    );
    report.check(
        "kernel.tsc.calibrated",
        tsc_hz.is_some_and(|hz| (100_000_000..=10_000_000_000).contains(&hz)),
        "the time stamp counter should run between 100 MHz and 10 GHz",
    );
    // A second measurement, ten times as long, should agree with the
    // first to within a tenth.
    // A host that takes the processor away between two readings makes
    // one measurement long, never all three.
    let agrees = pm_timer.zip(tsc_hz).is_some_and(|((port, bits), hz)| {
        (0..3).any(|_| {
            let tenth = acpi::PM_TIMER_HZ / 10;
            let (pm_start, tsc_start) = (u64::from(raw::pm_timer(port)), x86::raw::tsc());
            while kernel::clock::ticks_between(pm_start, u64::from(raw::pm_timer(port)), bits)
                < tenth
            {
                core::hint::spin_loop();
            }
            let ms = (x86::raw::tsc() - tsc_start) / (hz / 1000);
            (90..=110).contains(&ms)
        })
    });
    report.check(
        "kernel.tsc.agrees",
        agrees,
        "100 ms on the ACPI timer should be 100 ms on the calibrated counter",
    );
    report.check(
        "kernel.uptime",
        crate::up(tsc_hz).is_some_and(|s| s < 600),
        "the test machine was switched on less than ten minutes ago",
    );
    hpet_checks(report, machine, tsc_hz);
}

/// The HPET: found in its table, mapped uncached, counting at a rate the
/// specification allows, and measuring the counter as the ACPI timer did.
fn hpet_checks<W: Write>(report: &mut Report<'_, W>, machine: &Machine, tsc_hz: Option<u64>) {
    use paging::Flags;
    let uncached = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::NO_EXECUTE;
    let page = raw::memory::translate(kernel::HPET_PAGE);
    report.check(
        "kernel.hpet.found",
        machine.hpet == Some(0xfed0_0000)
            && page.is_some_and(|(at, flags)| Some(at) == machine.hpet && flags.contains(uncached)),
        "the HPET should be where QEMU puts it, as its ACPI table says, and mapped uncached",
    );
    let reference = raw::hpet::reference();
    let first = raw::hpet::count();
    x86::raw::spin(1_000_000);
    let later = raw::hpet::count();
    report.check(
        "kernel.hpet.counts",
        reference.is_some_and(|r| r.bits == 64 && (1_000_000..=1_000_000_000).contains(&r.hz))
            && later > first,
        "the HPET's main counter should run, 64 bits wide, at a rate the specification allows",
    );
    let measured = reference
        .and_then(|r| kernel::machine::calibrate_against(r, raw::hpet::count, x86::raw::tsc));
    report.check(
        "kernel.tsc.hpet_agrees",
        measured.zip(tsc_hz).is_some_and(|(hpet, acpi)| hpet.abs_diff(acpi) < acpi / 50),
        "the counter measured against the HPET should run within 2% of its rate against the ACPI timer",
    );
}

/// The other cores: all running on their own task states when there is an
/// x2APIC to start them with, and none otherwise.
fn smp_checks<W: Write>(report: &mut Report<'_, W>, machine: &Machine, tsc_hz: Option<u64>) {
    if x86::raw::apic_base().is_none() {
        report.check(
            "kernel.smp.alone",
            raw::smp::running() == (0, 0, 0)
                && x86::raw::gdt_entries().get(x86::tables::SEGMENTS.len() + 2) == Some(&0),
            "without a local APIC no other core should be started or given a task state",
        );
        return;
    }
    let page = raw::memory::translate(kernel::APIC_PAGE);
    report.check(
        "kernel.apic.page",
        if x86::apic::supported(x86::raw::cpuid(1)[2]) {
            page.is_none()
        } else {
            use paging::Flags;
            let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::NO_EXECUTE;
            page.is_some_and(|(at, f)| Some(at) == x86::raw::apic_base() && f.contains(flags))
                && x86::raw::xapic_registers() == Some(kernel::APIC_PAGE)
        },
        "without an x2APIC the APIC's page should be mapped uncached for its registers, and with one not at all",
    );
    report.check(
        "kernel.smp.running",
        machine
            .cores
            .is_some_and(|n| n > 1 && raw::smp::running() == (n - 1, n - 1, n - 1)),
        "every other core the MADT lists should have come into the kernel, on its own stack and the kernel's tables, and answered with its own APIC id",
    );
    let entries = x86::raw::gdt_entries();
    let segments = x86::tables::SEGMENTS.len();
    let others = machine.cores.map_or(0, |n| n.saturating_sub(1) as usize);
    report.check(
        "kernel.smp.task_states",
        (1..=others).all(|i| {
            entries
                .get(segments + 2 * i)
                .is_some_and(|e| e >> 40 & 0xf == 0xb)
        }) && entries
            .get(segments + 2 * (others + 1))
            .is_some_and(|&e| e == 0),
        "each other core should have loaded a task state of its own, and none past them be set up",
    );
    // Each one waits 10 ms after its INIT and twice 0.2 ms after its
    // startups, and no longer when it starts at once.
    let guarded = |top: u64| {
        let guard = top - kernel::CORE_STACK_PAGES * 4096 - 8;
        expected::read_at(top - 8).is_none()
            && expected::read_at(guard).is_some_and(|c| c.error == 0 && c.address == guard)
    };
    report.check(
        "kernel.smp.guarded",
        (1..=others).all(|n| {
            guarded(kernel::core_stack_top(n, false)) && guarded(kernel::core_stack_top(n, true))
        }) && expected::read_at(kernel::core_stack_top(others + 1, false) - 8).is_some(),
        "each other core's stacks should be mapped, with nothing below them, and no more stacks than cores",
    );
    work_checks(report, others, tsc_hz.unwrap_or(0) / 10);
    // Each core ends the wait for it by coming, and none takes longer than
    // the second it may wait plus its pauses, which only a pause far too
    // long would pass. A bound on the time alone failed on a busy host,
    // which kept a core that starts in 10.4 ms away for more than 60.
    let (took, came) = raw::smp::took();
    report.check(
        "kernel.smp.quick",
        usize::try_from(came) == Ok(others)
            && tsc_hz.is_some_and(|hz| took < hz * 2 * (others as u64).max(1)),
        "starting the other cores should not wait out the time a core that never starts gets",
    );
}

static HOLD: AtomicBool = AtomicBool::new(false);

/// A job that keeps its core busy until the test lets it go.
fn held(argument: u64) -> u64 {
    while HOLD.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    argument + 1
}

/// A job whose result depends on every step of it.
fn squares(n: u64) -> u64 {
    (1..=n).map(|i| i * i).sum()
}

/// Sends the `others` other cores jobs, all at once and one while a core
/// is busy, and waits up to `cycles` for each.
fn work_checks<W: Write>(report: &mut Report<'_, W>, others: usize, cycles: u64) {
    use raw::smp::{send, wait};
    let n = |core: usize| 1000 * core as u64;
    let sent = (1..=others).all(|core| send(core, squares, n(core)));
    let done = (1..=others).all(|core| wait(core, cycles) == Some(squares(n(core))));
    report.check(
        "kernel.smp.work",
        others > 0 && sent && done,
        "each other core should run the job it is sent, alongside the rest, and give back its result",
    );
    HOLD.store(true, Ordering::Release);
    let taken = send(1, held, 41);
    let refused = !send(1, squares, 1);
    let busy = wait(1, cycles / 10).is_none();
    HOLD.store(false, Ordering::Release);
    let back = wait(1, cycles) == Some(42);
    report.check(
        "kernel.smp.busy",
        taken
            && refused
            && busy
            && back
            && !send(0, squares, 1)
            && !send(others + 1, squares, 1)
            && wait(others + 1, cycles).is_none()
            && !send(raw::interrupts::CORES, squares, 1),
        "a busy core should take no second job until it is done, and only other cores that run take jobs",
    );
    // Each job comes with an interrupt. Two that meet while one waits are
    // taken as one, so only at least one for each core is certain.
    report.check(
        "kernel.smp.slept",
        x86::raw::wait_for(cycles, || raw::interrupts::woken() >= others as u64),
        "the other cores should sleep between jobs and be woken for them",
    );
}

/// Notes whether the screen still shows the boot screen when the kernel
/// is about to draw: the boot screens' background in the corner and
/// something else across the middle, where the cards are.
pub fn before_drawing(screen: Option<&Canvas<'_>>) {
    let kept = screen.is_some_and(|c| {
        let middle = c.height() / 2;
        c.pixel(0, 0) == Some(NIGHT)
            && (0..c.width()).any(|x| c.pixel(x, middle).is_some_and(|p| p != NIGHT))
    });
    BOOT_SCREEN_KEPT.store(kept, Ordering::Release);
}

static BOOT_SCREEN_KEPT: AtomicBool = AtomicBool::new(false);

/// Notes whether the APIC is still as OVMF left it, before starting the
/// other cores turns it on.
pub fn before_cores() {
    APIC_OFF_BEFORE.store(x86::qemu_test::apic_is_off(), Ordering::Release);
}

static APIC_OFF_BEFORE: AtomicBool = AtomicBool::new(false);

fn handover_screen_checks<W: Write>(
    report: &mut Report<'_, W>,
    info: &BootInfo,
    regions: &[MemoryRegion],
    display: Option<&Display<'_>>,
) {
    report.check(
        "kernel.screen.boot_screen_stays",
        BOOT_SCREEN_KEPT.load(Ordering::Acquire),
        "the boot screen should still be on the screen when the kernel starts drawing",
    );
    report.check(
        "kernel.screen.back_buffer",
        info.back_buffer != 0 && display.is_some_and(|d| d.back().is_some()),
        "the loader should set aside room in kernel memory for a frame",
    );
    report.check(
        "kernel.screen.shown_whole",
        display.is_some_and(|d| {
            let (screen, Some(back)) = (d.screen(), d.back()) else {
                return false;
            };
            (0..screen.height())
                .step_by(7)
                .flat_map(|y| (0..screen.width()).step_by(5).map(move |x| (x, y)))
                .all(|(x, y)| screen.pixel(x, y) == back.pixel(x, y))
        }),
        "the first frame should be drawn off the screen and then shown as it was drawn",
    );
    report.check(
        "kernel.screen.back_buffer_once",
        raw::back_buffer(info, regions).is_none(),
        "the room for a frame should be handed out only once",
    );
}

/// Starts the APIC timer, sees it tick and wake the processor from hlt,
/// and stops it again, so the crashes after this run as before.
fn timer_checks<W: Write>(
    report: &mut Report<'_, W>,
    tsc_hz: Option<u64>,
    info: &BootInfo,
    regions: &[MemoryRegion],
    machine: &Machine,
) {
    use crate::raw::interrupts::ticks;
    use crate::raw::timer;

    use x86::apic;
    use x86::raw::read_apic;

    report.check(
        "x86.apic.off_before",
        APIC_OFF_BEFORE.load(Ordering::Acquire),
        "OVMF leaves the APIC in xAPIC mode, and nothing reaches its registers before the kernel turns it on",
    );
    x86::qemu_test::apic(&mut |name, ok, why| report.check(name, ok, why));
    if x86::raw::apic_base().is_none() {
        report.check(
            "kernel.timer.none",
            timer::start(tsc_hz).is_none() && !x86::raw::interrupts_enabled(),
            "without a local APIC the kernel should keep spinning, with interrupts off",
        );
        return;
    }
    let tenth = tsc_hz.unwrap_or(0) / 10;
    let period = timer::start(tsc_hz);
    report.check(
        "kernel.timer.apic",
        period.is_some()
            && (x86::raw::x2apic_enabled() || x86::raw::xapic_registers().is_some())
            && x86::raw::interrupts_enabled()
            && read_apic(apic::SPURIOUS) & 0x1ff == apic::spurious(crate::raw::interrupts::SPURIOUS)
            && read_apic(apic::LVT_TIMER) & 0x3_00ff == apic::periodic(crate::raw::interrupts::TIMER),
        "the APIC should be on with its spurious vector, its timer periodic on its vector, and interrupts let in",
    );
    // A host that takes the processor away during one tenth swallows its
    // ticks, never those of all three.
    let counted = (0..3).any(|_| {
        let first = ticks();
        x86::raw::spin(tenth);
        (5..=20).contains(&(ticks() - first))
    });
    report.check(
        "kernel.timer.ticks",
        counted,
        "the timer should interrupt about ten times in a tenth of a second",
    );
    let before = ticks();
    x86::raw::wait_for_interrupt();
    report.check(
        "kernel.timer.wakes",
        ticks() > before,
        "a halted processor should wake at the next tick",
    );
    thread_checks(report, tsc_hz.unwrap_or(0));
    fpu_checks(report);
    program_thread_checks(report);
    user_checks(report);
    entry_checks(report);
    given_back_checks(report);
    ipc_checks(report);
    program_checks(report, info, regions);
    let rows = raw::console::shape().map_or(0, |((_, rows), _)| rows);
    report.check(
        "kernel.console.from_ring_3",
        raw::console::shape().is_none()
            || (0..rows).any(|r| raw::console::row(r).as_deref() == Some("hello on the console")),
        "the line hello writes should show on the console",
    );
    keyboard_checks(report, machine, tsc_hz);
    shell_checks(report, info, regions, machine, tsc_hz);
    init_checks(report, info, regions, machine, tsc_hz);
    timer::stop();
    let stopped = ticks();
    x86::raw::spin(tenth);
    report.check(
        "kernel.timer.stopped",
        ticks() == stopped
            && !x86::raw::interrupts_enabled()
            && x86::raw::apic_timer_count() == 0
            && read_apic(apic::LVT_TIMER) & apic::MASKED != 0,
        "once stopped, the timer should count no more and interrupt no more",
    );
}

/// What the threads in the checks below leave behind: numbers in the order
/// they noted them, and a count of those that ran.
static TRAIL: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static NOTED: AtomicU32 = AtomicU32::new(0);
static RAN: AtomicU32 = AtomicU32::new(0);
static HERE: AtomicU64 = AtomicU64::new(0);

fn note(value: u64) {
    let at = NOTED.fetch_add(1, Ordering::AcqRel) as usize;
    if let Some(slot) = TRAIL.get(at) {
        slot.store(value, Ordering::Release);
    }
}

fn trail() -> ([u64; 8], usize) {
    let noted = NOTED.load(Ordering::Acquire) as usize;
    (TRAIL.each_ref().map(|t| t.load(Ordering::Acquire)), noted)
}

fn forget_the_trail() {
    NOTED.store(0, Ordering::Release);
    for t in &TRAIL {
        t.store(0, Ordering::Release);
    }
}

/// Notes its argument three times, giving way after each.
extern "C" fn taking_turns(me: u64) {
    for _ in 0..3 {
        note(me);
        raw::thread::yield_now();
    }
}

/// Notes its argument once and ends.
extern "C" fn noting(me: u64) {
    note(me);
}

/// Counts that it ran and ends.
extern "C" fn counting(_: u64) {
    RAN.fetch_add(1, Ordering::AcqRel);
}

static STOP: AtomicBool = AtomicBool::new(false);
static SPINS: AtomicU64 = AtomicU64::new(0);

/// Spins without ever giving way until told to stop, then counts that it
/// ran.
extern "C" fn spinning(_: u64) {
    while !STOP.load(Ordering::Acquire) {
        SPINS.fetch_add(1, Ordering::AcqRel);
        core::hint::spin_loop();
    }
    RAN.fetch_add(1, Ordering::AcqRel);
}

/// Threads that never give way: the timer has to take the processor from
/// one of the first thread's priority, and from one of a lower priority as
/// soon as the first thread's sleep is over.
fn preemption_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::interrupts::ticks;
    use raw::thread::{self, NORMAL};
    STOP.store(false, Ordering::Release);
    SPINS.store(0, Ordering::Release);
    RAN.store(0, Ordering::Release);
    let spinner = thread::spawn_on(0, spinning, 0, NORMAL);
    thread::yield_now();
    let spun = SPINS.load(Ordering::Acquire);
    STOP.store(true, Ordering::Release);
    thread::sleep(2);
    report.check(
        "kernel.thread.preempted",
        spinner.is_ok() && spun > 0 && RAN.load(Ordering::Acquire) == 1,
        "a thread that never gives way should still lose the processor at the next tick to one of its priority",
    );
    STOP.store(false, Ordering::Release);
    let low = thread::spawn_on(0, spinning, 0, 1);
    let at = ticks();
    thread::sleep(3);
    let slept = ticks() - at;
    STOP.store(true, Ordering::Release);
    thread::sleep(2);
    report.check(
        "kernel.thread.woken_on_time",
        low.is_ok() && (3..=5).contains(&slept) && RAN.load(Ordering::Acquire) == 2,
        "a sleeper should wake on time even while a lower thread keeps the processor busy",
    );
}

/// Keeps where its stack is.
extern "C" fn where_am_i(_: u64) {
    let here = 0u8;
    HERE.store((&raw const here) as u64, Ordering::Release);
}

/// Kernel threads on the first core, with the timer running: taking
/// turns, going by priority, sleeping on the timer, each on a stack of its
/// own with nothing below, and ending, so their numbers can be used again.
fn thread_checks<W: Write>(report: &mut Report<'_, W>, tsc_hz: u64) {
    use kernel::thread::{STACK_PAGES, THREADS, stack_top};
    use raw::interrupts::ticks;
    use raw::thread::{self, Error, NORMAL};
    thread::start();

    // With interrupts off no tick takes the processor in between, and the
    // threads started now keep them off, so the order is the one giving
    // way makes.
    forget_the_trail();
    let (a, b) = x86::raw::without_interrupts(|| {
        let a = thread::spawn_on(0, taking_turns, 1, NORMAL);
        let b = thread::spawn_on(0, taking_turns, 2, NORMAL);
        for _ in 0..20 {
            thread::yield_now();
        }
        (a, b)
    });
    let (noted, count) = trail();
    report.check(
        "kernel.thread.turns",
        a.is_ok_and(|a| a > 0)
            && b.is_ok_and(|b| b > 0)
            && a != b
            && thread::current() == 0
            && count == 6
            && noted[..6] == [1, 2, 1, 2, 1, 2],
        "two threads of the first one's priority should take turns each time one gives way",
    );

    forget_the_trail();
    let spawned = x86::raw::without_interrupts(|| {
        let spawned = [(10, 2), (20, 20), (30, 9)]
            .map(|(me, priority)| thread::spawn_on(0, noting, me, priority));
        thread::yield_now();
        spawned
    });
    let (before_sleep, early) = trail();
    let ticks_then = ticks();
    thread::sleep(3);
    let slept = ticks() - ticks_then;
    let (after_sleep, count) = trail();
    report.check(
        "kernel.thread.priority",
        spawned.iter().all(Result::is_ok)
            && early == 2
            && before_sleep[..2] == [20, 30]
            && count == 3
            && after_sleep[..3] == [20, 30, 10],
        "higher priorities should run first, and lower ones only once the first thread sleeps",
    );
    // With interrupts off nothing ticks, so a sleep that is over at once
    // has to come back without waiting for the timer.
    let (at, back) = x86::raw::without_interrupts(|| {
        let at = ticks();
        thread::sleep(0);
        (at, ticks())
    });
    report.check(
        "kernel.thread.sleep",
        (3..=30).contains(&slept) && thread::current() == 0 && back == at,
        "a sleeping thread should wake once the timer has ticked as often as it asked, and not before",
    );

    preemption_checks(report);

    let slot = thread::spawn_on(0, where_am_i, 0, NORMAL);
    thread::yield_now();
    let here = HERE.load(Ordering::Acquire);
    report.check(
        "kernel.thread.stack",
        slot.is_ok_and(|s| {
            let top = stack_top(s);
            let bottom = top - STACK_PAGES * 4096;
            (bottom..top).contains(&here)
                && raw::interrupts::expected::read_at(bottom - 8)
                    .is_some_and(|c| c.address == bottom - 8)
        }),
        "a thread should run on a stack of its own, with nothing mapped below it",
    );

    RAN.store(0, Ordering::Release);
    let mut started = 0;
    let full = loop {
        match thread::spawn_on(0, counting, 0, 1) {
            Ok(_) => started += 1,
            Err(e) => break e,
        }
        if started > THREADS {
            break Error::NoStack;
        }
    };
    thread::sleep(3);
    let again = thread::spawn_on(0, noting, 0, NORMAL);
    thread::yield_now();
    report.check(
        "kernel.thread.ended",
        full == Error::Full
            && started == THREADS - 2 - raw::smp::others().count()
            && RAN.load(Ordering::Acquire) as usize == started
            && again.is_ok()
            && thread::spawn_on(0, noting, 0, 32) == Err(Error::NoSuchPriority(32)),
        "every thread there can be should start and end, and an ended one's number should be used again",
    );
    other_core_checks(report, tsc_hz);
}

/// CR0, CR4, XCR0 and the number RDTSCP gives of each core, as `note_fpu`
/// found them there.
static FPU_SEEN: [[AtomicU64; 4]; 8] = [const { [const { AtomicU64::new(0) }; 4] }; 8];

/// Keeps CR0, CR4 and XCR0 of the core it runs on under its argument,
/// and the number RDTSCP finds for that core.
extern "C" fn note_fpu(slot: u64) {
    if let Some(seen) = usize::try_from(slot).ok().and_then(|s| FPU_SEEN.get(s)) {
        seen[0].store(x86::raw::read_cr0(), Ordering::Release);
        seen[1].store(x86::raw::read_cr4(), Ordering::Release);
        seen[2].store(raw::fpu::xcr0().unwrap_or(0), Ordering::Release);
        let number = x86::raw::cpu_number().map_or(u64::MAX, u64::from);
        seen[3].store(number, Ordering::Release);
    }
}

/// How often each of two threads paints its registers and looks again.
const ROUNDS: u64 = 64;

/// The rounds each of them found its registers as it painted them.
static KEPT: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Whether a thread found the registers a new one should start with.
static FRESH: AtomicBool = AtomicBool::new(false);

/// Where XSAVE puts the upper halves of the YMM registers: leaf 0xD of
/// CPUID, subleaf 2, says.
fn avx_at() -> usize {
    usize::try_from(core::arch::x86_64::__cpuid_count(0xd, 2).ebx).unwrap_or(0)
}

/// The XMM registers in an image, and with `avx` the upper halves of the
/// YMM registers after them.
fn vectors(image: &raw::fpu::Image, avx: bool) -> [u8; 512] {
    let mut all = [0; 512];
    all[..256].copy_from_slice(&image.0[160..416]);
    if avx {
        all[256..].copy_from_slice(&image.0[avx_at()..avx_at() + 256]);
    }
    all
}

/// Paints `image` for thread `me` in round `round`: the x87 control word
/// and MXCSR each with a rounding of their own, and vector registers with
/// values no other thread or round has. Returns the two controls.
fn paint(image: &mut raw::fpu::Image, me: u64, round: u64, avx: bool) -> (u16, u32) {
    image.0.fill(0);
    let turn = u16::try_from((me + round) % 4).unwrap_or(0);
    let control = x86::fpu::FRESH_CONTROL | turn << 10;
    let mxcsr = x86::fpu::FRESH_MXCSR | u32::from(turn) << 13;
    image.0[..2].copy_from_slice(&control.to_le_bytes());
    image.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    for (i, n) in (1..=32u128).enumerate() {
        let value = u128::from(me + 1) << 96 | u128::from(round) << 64 | n;
        let at = if i < 16 {
            160 + 16 * i
        } else {
            avx_at() + 16 * (i - 16)
        };
        if i < 16 || avx {
            image.0[at..at + 16].copy_from_slice(&value.to_le_bytes());
        }
    }
    let avx_part = if avx { x86::fpu::parts::AVX } else { 0 };
    let used = x86::fpu::parts::X87 | x86::fpu::parts::SSE | avx_part;
    image.0[512..520].copy_from_slice(&used.to_le_bytes());
    (control, mxcsr)
}

/// Paints its registers anew in every round, gives way or waits for the
/// timer to take the processor, and counts the rounds it finds them as it
/// painted them.
extern "C" fn keeps_its_registers(me: u64) {
    let parts = raw::fpu::xcr0();
    let avx = parts.is_some_and(|p| p & x86::fpu::parts::AVX != 0);
    let mut painted = raw::fpu::Image([0; 4096]);
    let mut seen = raw::fpu::Image([0; 4096]);
    let Some(kept) = usize::try_from(me).ok().and_then(|me| KEPT.get(me)) else {
        return;
    };
    for round in 0..ROUNDS {
        let controls = paint(&mut painted, me, round, avx);
        raw::fpu::load(&painted, parts);
        if round % 16 == 0 {
            let tick = raw::interrupts::ticks();
            while raw::interrupts::ticks() == tick {
                core::hint::spin_loop();
            }
        } else {
            raw::thread::yield_now();
        }
        seen.0.fill(0);
        raw::fpu::dump(&mut seen, parts);
        if raw::fpu::controls() != controls || vectors(&seen, avx) != vectors(&painted, avx) {
            return;
        }
        kept.fetch_add(1, Ordering::AcqRel);
    }
}

/// Looks at the registers it started with.
extern "C" fn finds_fresh_registers(_: u64) {
    let parts = raw::fpu::xcr0();
    let avx = parts.is_some_and(|p| p & x86::fpu::parts::AVX != 0);
    let mut seen = raw::fpu::Image([0; 4096]);
    raw::fpu::dump(&mut seen, parts);
    let fresh = raw::fpu::controls() == (x86::fpu::FRESH_CONTROL, x86::fpu::FRESH_MXCSR)
        && vectors(&seen, avx) == [0; 512];
    FRESH.store(fresh, Ordering::Release);
}

/// The floating point and vector registers: on on every core, with XSAVE
/// of every part the processor has where it has XSAVE, each of two threads
/// on one core finding its own however often they take turns, by giving
/// way or at the timer's tick, and a thread started where one of them was
/// starting with the registers power on gives.
fn fpu_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::thread::{self, NORMAL};
    use x86::cr0::{EMULATION, MONITOR_COPROCESSOR, NUMERIC_ERROR, TASK_SWITCHED};
    let saving = x86::fpu::Saving::detect();
    let parts = match saving {
        x86::fpu::Saving::Fx => 0,
        x86::fpu::Saving::X(on) => on,
    };
    note_fpu(0);
    let asked = raw::smp::others()
        .all(|core| thread::spawn_on(core, note_fpu, core as u64, NORMAL).is_ok());
    thread::sleep(3);
    let cores = 1 + raw::smp::others().count();
    let on = (0..cores).all(|core| {
        FPU_SEEN.get(core).is_some_and(|[cr0, cr4, xcr0, _]| {
            let cr0 = cr0.load(Ordering::Acquire);
            cr0 & (EMULATION | TASK_SWITCHED) == 0
                && cr0 & (MONITOR_COPROCESSOR | NUMERIC_ERROR)
                    == MONITOR_COPROCESSOR | NUMERIC_ERROR
                && cr4.load(Ordering::Acquire) & saving.cr4_bits() == saving.cr4_bits()
                && xcr0.load(Ordering::Acquire) == parts
        })
    });
    report.check(
        "kernel.fpu.on",
        asked && on,
        "every core should have the x87 unit and SSE on, and XSAVE with every part the processor has where it has XSAVE",
    );
    report.check(
        "kernel.cores.numbered",
        asked
            && (0..cores).all(|core| {
                FPU_SEEN
                    .get(core)
                    .is_some_and(|seen| seen[3].load(Ordering::Acquire) == core as u64)
            }),
        "RDTSCP should hand each core its own number",
    );
    let core = raw::smp::others().last().unwrap_or(0);
    let painters = [0, 1].map(|me| thread::spawn_on(core, keeps_its_registers, me, NORMAL));
    let start = x86::raw::tsc();
    while painters.iter().any(|p| p.is_ok_and(thread::alive)) && x86::raw::tsc() - start < 1 << 34 {
        thread::sleep(1);
    }
    report.check(
        "kernel.fpu.kept",
        KEPT.iter().all(|k| k.load(Ordering::Acquire) == ROUNDS),
        "each of two threads on one core should find its registers as it left them, after giving way and after the timer took the processor",
    );
    let fresh = thread::spawn_on(core, finds_fresh_registers, 0, NORMAL);
    thread::sleep(3);
    report.check(
        "kernel.fpu.fresh",
        fresh.is_ok_and(|slot| painters.contains(&Ok(slot))) && FRESH.load(Ordering::Acquire),
        "a thread started where an earlier one painted its registers should start with the registers power on gives",
    );
}

/// Runs `program` in ring 3 on the first core with `argument`, waits up
/// to a second for it to end, and returns its thread's number if it did.
fn ran(program: &'static raw::user::Program, argument: u64) -> Option<usize> {
    raw::user::seen::forget();
    let slot = raw::user::start(0, program, argument).ok()?;
    (0..100)
        .any(|_| {
            raw::thread::sleep(1);
            !raw::thread::alive(slot)
        })
        .then_some(slot)
}

/// Waits up to a second for the thread `slot` to end, and says whether it
/// ended with `code`.
fn ended_with(slot: Option<usize>, code: u64) -> bool {
    let ended = (0..100).any(|_| {
        raw::thread::sleep(1);
        slot.is_some_and(|s| !raw::thread::alive(s))
    });
    ended && slot.is_some_and(|s| raw::user::seen::exit_of(s) == code)
}

/// Channels between programs: one program talks to itself, two on
/// different cores talk to each other, a closed end shows as closed, a
/// handle a program never got reaches nothing, and every channel goes once
/// its ends are closed.
fn ipc_checks<W: Write>(report: &mut Report<'_, W>) {
    use handles::Rights;
    use kernel::syscall::{BAD_HANDLE, CLOSED, DENIED, FULL, TOO_SMALL};
    use raw::user::{probe, seen, start_holding};
    let ends = Rights::READ | Rights::WRITE;
    report.check(
        "kernel.ipc.pair",
        ended_with(ran(&probe::PAIR, 0), 5) && seen::logged(b"hello"),
        "a program should send itself a message through a channel it made",
    );
    let other = raw::smp::others().next().unwrap_or(0);
    let talk = raw::ipc::create().map(|(a, b)| {
        let receiver = start_holding(other, &probe::RECEIVER, &[b], ends).ok();
        let sender = start_holding(0, &probe::SENDER, &[a], ends).ok();
        (receiver, sender)
    });
    report.check(
        "kernel.ipc.between",
        talk.is_some_and(|(receiver, sender)| ended_with(receiver, 4) && ended_with(sender, 0))
            && seen::logged(b"ping"),
        "two programs on different cores should talk through a channel",
    );
    let left_alone = raw::ipc::create().and_then(|(a, b)| {
        let receiver = start_holding(0, &probe::RECEIVER, &[b], ends).ok();
        raw::ipc::close(a);
        receiver
    });
    report.check(
        "kernel.ipc.closed",
        ended_with(left_alone, CLOSED),
        "a program waiting at a channel whose other end closed should learn it is closed",
    );
    report.check(
        "kernel.ipc.bad_handle",
        ended_with(ran(&probe::BAD_HANDLE, 0), BAD_HANDLE),
        "a handle a program never got should reach nothing",
    );
    report.check(
        "kernel.ipc.full",
        ended_with(ran(&probe::FILL, 0), FULL),
        "a channel should take eight messages and refuse the ninth",
    );
    report.check(
        "kernel.ipc.too_small",
        ended_with(ran(&probe::SMALL_ROOM, 0), TOO_SMALL),
        "a message should stay when its room is too small, and read only memory be no room at all",
    );
    let denied = raw::ipc::create().and_then(|(a, b)| {
        let sender = start_holding(0, &probe::SENDER, &[a], Rights::READ).ok();
        raw::ipc::close(b);
        sender
    });
    report.check(
        "kernel.ipc.denied",
        ended_with(denied, DENIED),
        "a handle without the right to write should not send",
    );
    // A program hands one end of a channel of its own to another, through a
    // channel the kernel gave them; the other takes it and reads what was
    // sent into it. Handing over needs the right to.
    let carries = Rights::READ | Rights::WRITE | Rights::TRANSFER;
    let handover = raw::ipc::create().map(|(a, b)| {
        let taker = start_holding(other, &probe::TAKER, &[b], carries).ok();
        let passer = start_holding(0, &probe::PASSER, &[a], carries).ok();
        (taker, passer)
    });
    report.check(
        "kernel.ipc.handed_over",
        handover.is_some_and(|(taker, passer)| ended_with(taker, 2) && ended_with(passer, BAD_HANDLE))
            && seen::logged(b"hi"),
        "a handle sent through a channel should leave its sender and reach the program that takes it",
    );
    // Nobody takes the end handed over here: closing the channel it waits
    // in closes it too, so the channel it belongs to goes as well.
    let unclaimed = raw::ipc::create().and_then(|(a, b)| {
        let passer = start_holding(0, &probe::PASSER, &[a], carries).ok();
        let ended = ended_with(passer, BAD_HANDLE);
        raw::ipc::close(b);
        ended.then_some(())
    });
    report.check(
        "kernel.ipc.unclaimed",
        unclaimed.is_some() && raw::ipc::live() == 0,
        "a handle in a message nobody read should be closed with the channel it waited in",
    );
    report.check(
        "kernel.ipc.freed",
        raw::ipc::live() == 0,
        "every channel should go once both its ends are closed, by a program or by its end",
    );
}

/// Whether thread `slot` ended by a fault of `vector` rather than exiting.
fn ended_by(slot: Option<usize>, vector: u64) -> bool {
    use raw::user::seen;
    slot.is_some_and(|s| seen::fault_of(s) == vector && seen::exit_of(s) == u64::MAX)
}

/// Programs that do what ring 3 must not: each ends by the fault it
/// should, and only it.
fn user_fault_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::user::{probe, seen};
    let faults = ended_by;
    report.check(
        "kernel.user.faults",
        faults(ran(&probe::READ_KERNEL, 0), 14) && faults(ran(&probe::HALT, 0), 13),
        "reading the kernel's memory or halting in ring 3 should end only the program",
    );
    // A page fault's error code says the page was there (bit 0) and the
    // access came from ring 3 (bit 2): a write (bit 1) to the code, which
    // is read only, and fetching an instruction (bit 4) from the stack,
    // which nothing runs from.
    // A thread's number is used again once it ended, so each program is
    // looked at before the next one runs.
    let wrote = ran(&probe::WRITE_CODE, 0);
    report.check(
        "kernel.user.code_read_only",
        faults(wrote, 14) && wrote.is_some_and(|s| seen::error_of(s) == 0b111),
        "a program should not write over its own code",
    );
    let fetched = ran(&probe::RUN_STACK, 0);
    report.check(
        "kernel.user.stack_not_run",
        faults(fetched, 14) && fetched.is_some_and(|s| seen::error_of(s) == 0b1_0101),
        "a program should not run code from its stack",
    );
}

/// Programs from the disk: the loader handed over hello from
/// meridian/programs, and it runs in ring 3 from its ELF file, where its
/// test build checks every system call through the program runtime before
/// it greets; a file that is no ELF file does not start.
fn program_checks<W: Write>(report: &mut Report<'_, W>, info: &BootInfo, regions: &[MemoryRegion]) {
    use raw::user::{self, seen};
    let hello = raw::modules(info, regions).find(|(m, _)| m.name() == b"hello.mexec");
    let again = raw::modules(info, regions).find(|(m, _)| m.name() == b"hello_again.mexec");
    report.check(
        "kernel.programs.read",
        raw::modules(info, regions).count() == usize::try_from(info.modules.count).unwrap_or(0)
            && info.modules.count >= 2
            && hello.is_some_and(|(m, file)| {
                file.starts_with(b"\x7fELF") && m.size == file.len() as u64
            })
            && hello
                .zip(again)
                .is_some_and(|((m, file), (n, copy))| m.start != n.start && file == copy),
        "the loader should hand over hello and its copy from meridian/programs, each whole",
    );
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let before = free();
    seen::forget();
    let started = hello.and_then(|(_, file)| user::start_file(0, file, 0).ok());
    report.check(
        "kernel.programs.hello",
        ended_with(started, 0)
            && seen::logged(b"Hello from ring 3, in Rust")
            && seen::ended(b"user: hello ended with 0\n"),
        "hello should run from its file, find every call working, greet, and have its end logged",
    );
    let every = [
        abi::LOG,
        abi::YIELD,
        abi::SLEEP,
        abi::TICKS,
        abi::CHANNEL,
        abi::SEND,
        abi::RECEIVE,
        abi::CLOSE,
        abi::MAP,
        abi::UNMAP,
        abi::SPAWN,
        abi::WAIT,
        abi::TOKEN,
        abi::OPEN,
        abi::READ,
        abi::INFO,
        abi::LIST,
        abi::WRITE,
        abi::KEY,
        abi::TERMINATE,
        abi::THREAD,
        abi::END_THREAD,
        abi::EXIT,
    ];
    report.check(
        "kernel.programs.every_call",
        every.iter().all(|&number| seen::calls(number) > 0),
        "hello's checks and the children they spawn should reach the kernel with every call the runtime makes",
    );
    report.check(
        "kernel.programs.cleaned",
        before.is_some() && free() == before,
        "hello's image, stack and heap should all go back once it ended",
    );
    report.check(
        "kernel.programs.not_meridian",
        user::start_file(0, &plain_elf(), 0)
            == Err(user::Error::Refused(kernel::program::Refusal::NotMeridian)),
        "an ELF file without Meridian's notes is no .mexec file and should not start",
    );
    let mut text = [b' '; 64];
    text[..9].copy_from_slice(b"#!/bin/sh");
    low_memory_checks(report, hello.map(|(_, file)| file));
    report.check(
        "kernel.programs.not_elf",
        user::start_file(0, &text, 0) == Err(user::Error::Elf(elf::Error::NotElf))
            && user::start_file(0, b"\x7fELF", 0) == Err(user::Error::Elf(elf::Error::Truncated)),
        "a file that is no ELF file should not start",
    );
}

/// The smallest ELF file the kernel would otherwise run: a header, one
/// loadable segment over the whole file, and a `ud2` at its entry, but no
/// Meridian notes.
fn plain_elf() -> [u8; 128] {
    let mut file = [0u8; 128];
    file[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    let fields: [(usize, &[u8]); 13] = [
        (16, &3u16.to_le_bytes()),
        (18, &62u16.to_le_bytes()),
        (20, &1u32.to_le_bytes()),
        (24, &120u64.to_le_bytes()),
        (32, &64u64.to_le_bytes()),
        (52, &64u16.to_le_bytes()),
        (54, &56u16.to_le_bytes()),
        (56, &1u16.to_le_bytes()),
        (64, &1u32.to_le_bytes()),
        (68, &5u32.to_le_bytes()),
        (96, &128u64.to_le_bytes()),
        (104, &128u64.to_le_bytes()),
        (120, &[0x0f, 0x0b]),
    ];
    for (at, bytes) in fields {
        file[at..at + bytes.len()].copy_from_slice(bytes);
    }
    file
}

/// Takes every free frame out of the allocator, each holding the address
/// of the one taken before, and returns the last one taken.
fn hold_frames() -> u64 {
    let mut last = 0;
    while let Some(frame) = raw::memory::FRAMES
        .lock()
        .as_mut()
        .and_then(frames::Frames::alloc)
    {
        raw::memory::write_word(HHDM_BASE + frame, last);
        last = frame;
    }
    last
}

/// Gives back the last frame [`hold_frames`] took, and returns the one
/// before it.
fn release_frame(last: u64) -> u64 {
    let before = raw::memory::read_word(HHDM_BASE + last);
    if let Some(frames) = raw::memory::FRAMES.lock().as_mut() {
        let _ = frames.free(last);
    }
    before
}

/// A program that cannot have the memory it needs does not start, and
/// whatever it got so far goes back: hello is started with one free frame
/// after another more, until it starts.
fn low_memory_checks<W: Write>(report: &mut Report<'_, W>, file: Option<&[u8]>) {
    use raw::user::{self, Error};
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map_or(0, frames::Frames::free_frames)
    };
    let _ = writeln!(
        report.out,
        "kernel: hello once more, short of memory, so its own checks fail"
    );
    let before = free();
    let mut held = hold_frames();
    let mut refused = 0;
    let mut tidy = true;
    let mut started = None;
    while started.is_none() && tidy && held != 0 {
        let left = free();
        match file.map(|f| user::start_file(0, f, 0)) {
            Some(Err(Error::NoMemory)) => {
                refused += 1;
                tidy = free() == left;
                held = release_frame(held);
            }
            Some(Ok(slot)) => started = Some(slot),
            _ => tidy = false,
        }
    }
    if started.is_some() {
        let _ = ended_with(started, 0);
    }
    while held != 0 {
        held = release_frame(held);
    }
    report.check(
        "kernel.programs.low_memory",
        refused > 10 && tidy && started.is_some_and(|s| !raw::thread::alive(s)) && free() == before,
        "a program short of memory should not start and should leave every frame it took",
    );
}

/// Memory a program gives back is gone at once: reading it right after
/// faults, even where the processor still remembered the page.
fn given_back_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::user::{probe, seen};
    report.check(
        "kernel.user.spawn_names",
        ended_with(ran(&probe::SPAWN_LONGEST, 0), abi::BAD_ADDRESS)
            && ended_with(ran(&probe::SPAWN_TOO_LONG, 0), abi::TOO_LONG),
        "a name or path of 512 bytes may be spawned and is read from the program's memory; one of 513 is too long",
    );
    let stale = ran(&probe::STALE, 0);
    report.check(
        "kernel.user.given_back",
        ended_by(stale, 14)
            && stale.is_some_and(|s| seen::error_of(s) == 0b100)
            && seen::fault_address() == kernel::program::HEAP_START,
        "a page given back should be gone for the program at once",
    );
}

/// What a program leaves behind when it calls the kernel is put right: the
/// null stack segment the processor loads, and the AC flag, which would
/// switch SMAP off.
fn entry_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::user::{probe, seen};
    let flagged = ran(&probe::SET_AC, 0);
    let exited = flagged.is_some_and(|s| seen::exit_of(s) == 0);
    report.check(
        "kernel.user.stack_segment",
        exited && seen::STACK_SEGMENT.load(Ordering::Acquire) == u64::from(x86::tables::KERNEL_DATA),
        "a system call from ring 3 should run on the kernel's own stack segment, not the null one the processor leaves",
    );
    report.check(
        "kernel.user.access_flag",
        exited && !seen::USER_ACCESS.load(Ordering::Acquire),
        "a program that sets AC should not take it into the kernel, where it would switch SMAP off",
    );
}

/// Programs with threads: none starts in the kernel's half or on a stack
/// above the program's, a program whose first thread left ends with its
/// last one's code, and an exit ends a thread that never calls the kernel,
/// so that the address space comes back whole.
fn program_thread_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::user::{probe, seen};
    report.check(
        "kernel.threads.refused",
        ended_with(ran(&probe::THREAD_NOWHERE, 0), abi::BAD_ADDRESS)
            && ended_with(ran(&probe::THREAD_NO_STACK, 0), abi::BAD_ADDRESS),
        "a thread should not start in the kernel's half or with its stack above the program's",
    );
    let logged = |line: &[u8]| {
        (0..100).any(|_| {
            raw::thread::sleep(1);
            seen::ended(line)
        })
    };
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let before = free();
    let two = ran(&probe::TWO_THREADS, 0);
    report.check(
        "kernel.threads.last",
        two.is_some() && logged(b"user: the program ended with 5\n"),
        "a program whose first thread left should end with its last thread's code",
    );
    let all = ran(&probe::EXIT_ALL, 0);
    let back = (0..100).any(|_| {
        raw::thread::sleep(1);
        free() == before
    });
    report.check(
        "kernel.threads.all",
        all.is_some() && seen::ended(b"user: the program ended with 9\n") && back,
        "an exit should end a thread that never calls the kernel too, so the address space comes back whole",
    );
    seen::forget();
    let stale = raw::user::start(0, &probe::STALE_THREADS, 0).ok();
    let ended = (0..300).any(|_| {
        raw::thread::sleep(1);
        stale.is_some_and(|s| !raw::thread::alive(s))
    });
    report.check(
        "kernel.threads.shootdown",
        ended
            && !seen::ended(b"user: the program ended with 55\n")
            && !seen::ended(b"user: the program ended with 77\n")
            && seen::fault_address() == kernel::program::HEAP_START,
        "a page one thread gives back should be gone at once for its other thread, which reads it on another core",
    );
    shootdown_checks(report);
    shootdown_wait_checks(report);
}

/// Whether the thread that keeps an address space busy on another core
/// goes on, and the core it last found itself on.
static BUSY_GOES_ON: AtomicBool = AtomicBool::new(false);
static BUSY_ON: AtomicU64 = AtomicU64::new(u64::MAX);

extern "C" fn keep_busy(_: u64) {
    while BUSY_GOES_ON.load(Ordering::Acquire) {
        BUSY_ON.store(raw::interrupts::this_core() as u64, Ordering::Release);
        core::hint::spin_loop();
    }
}

/// Whether the thread that keeps interrupts off on its core goes on doing
/// so, whether it does yet, and whether the thread that gives a page back
/// meanwhile is done.
static HOLD_OFF: AtomicBool = AtomicBool::new(false);
static HOLDING: AtomicBool = AtomicBool::new(false);
static GIVEN_BACK: AtomicBool = AtomicBool::new(false);

extern "C" fn hold_interrupts(_: u64) {
    x86::raw::without_interrupts(|| {
        HOLDING.store(true, Ordering::Release);
        while HOLD_OFF.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
    });
}

extern "C" fn give_back_page(root: u64) {
    raw::memory::unmap_pages(root, kernel::program::HEAP_START, 1);
    GIVEN_BACK.store(true, Ordering::Release);
}

/// A shootdown waits for a core that keeps interrupts off until it can
/// answer, and holds the right to run all the while, so no other starts.
fn shootdown_wait_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::tlb::seen;
    let root = raw::memory::new_space().filter(|&root| {
        let flags = paging::Flags::USER | paging::Flags::WRITABLE;
        raw::memory::map_pages(root, kernel::program::HEAP_START, 1, flags).is_ok()
    });
    let me = raw::interrupts::this_core();
    // The page goes from this core, whose timer lets the check look in
    // while the shootdown waits for the other core, which holds interrupts
    // off.
    let cores = usize::try_from(raw::smp::running().0).unwrap_or(0) + 1;
    let holder_core = (me + 1) % cores;
    let soon = |done: &dyn Fn() -> bool| {
        (0..100).any(|_| {
            raw::thread::sleep(1);
            done()
        })
    };
    HOLD_OFF.store(true, Ordering::Release);
    HOLDING.store(false, Ordering::Release);
    GIVEN_BACK.store(false, Ordering::Release);
    let holder = root.and_then(|root| {
        raw::thread::spawn_in(holder_core, root, hold_interrupts, 0, raw::thread::NORMAL).ok()
    });
    let holding = holder.is_some() && soon(&|| HOLDING.load(Ordering::Acquire));
    let giver = root
        .filter(|_| holding)
        .and_then(|root| raw::thread::spawn_on(me, give_back_page, root, raw::thread::NORMAL).ok());
    let waiting = giver.is_some() && soon(&|| seen::pending() != 0);
    let held = waiting && seen::busy() && !GIVEN_BACK.load(Ordering::Acquire);
    HOLD_OFF.store(false, Ordering::Release);
    let done = soon(&|| {
        GIVEN_BACK.load(Ordering::Acquire) && holder.is_some_and(|slot| !raw::thread::alive(slot))
    });
    if let (Some(root), true) = (root, done) {
        raw::memory::release_space(root);
    }
    report.check(
        "kernel.tlb.one_at_a_time",
        cores >= 2 && held && done && !seen::busy() && seen::pending() == 0,
        "a shootdown should wait for a core that keeps interrupts off, holding the right to run all the while, and let it go once answered",
    );
}

/// Pages that go while a thread runs in their address space on another
/// core: that core alone is asked to forget them, in batches of 64, and
/// forgets each of them before the call that took them returns.
fn shootdown_checks<W: Write>(report: &mut Report<'_, W>) {
    use raw::tlb::seen;
    const PAGES: u64 = 70;
    let at = kernel::program::HEAP_START;
    let flags = paging::Flags::USER | paging::Flags::WRITABLE;
    let root = raw::memory::new_space()
        .filter(|&root| raw::memory::map_pages(root, at, PAGES, flags).is_ok());
    let me = raw::interrupts::this_core();
    BUSY_ON.store(u64::MAX, Ordering::Release);
    BUSY_GOES_ON.store(true, Ordering::Release);
    let other = usize::from(me == 0);
    let busy = root.and_then(|root| {
        raw::thread::spawn_in(other, root, keep_busy, 0, raw::thread::NORMAL).ok()
    });
    let running = busy.is_some()
        && (0..100).any(|_| {
            raw::thread::sleep(1);
            BUSY_ON.load(Ordering::Acquire) != u64::MAX
        });
    let on = usize::try_from(BUSY_ON.load(Ordering::Acquire)).unwrap_or(0) % kernel::CORES;
    let before: alloc::vec::Vec<_> = (0..kernel::CORES).map(seen::of).collect();
    if let Some(root) = root {
        raw::memory::unmap_pages(root, at, PAGES);
    }
    let waiting = seen::pending();
    let after: alloc::vec::Vec<_> = (0..kernel::CORES).map(seen::of).collect();
    BUSY_GOES_ON.store(false, Ordering::Release);
    let ended = (0..100).any(|_| {
        raw::thread::sleep(1);
        busy.is_some_and(|slot| !raw::thread::alive(slot))
    });
    if let (Some(root), true) = (root, ended) {
        raw::memory::release_space(root);
    }
    let sum = (0..PAGES).fold(0u64, |sum, page| sum.wrapping_add(at + page * 4096));
    let (looked, forgot, summed) = (
        after[on].0 - before[on].0,
        after[on].1 - before[on].1,
        after[on].2.wrapping_sub(before[on].2),
    );
    let others_left_alone = (0..kernel::CORES)
        .filter(|&core| core != on)
        .all(|core| after[core] == before[core]);
    report.check(
        "kernel.tlb.shootdown",
        running
            && on != me
            && ended
            && (looked, forgot, summed) == (2, PAGES, sum)
            && others_left_alone
            && waiting == 0,
        "seventy pages that go while a thread runs in their address space on another core should be forgotten there, each once, in two shootdowns that reach no other core and are done before the unmap returns",
    );
}

/// Programs in ring 3, each in an address space of its own: the first
/// says hello through a system call from ring 3, a program's stack works,
/// it can sleep, give way and ask for the ticks, every way it can go wrong
/// ends only it, three can run at once with their own data at the same
/// address, and none leaves a frame behind.
fn user_checks<W: Write>(report: &mut Report<'_, W>) {
    use kernel::syscall::{BAD_ADDRESS, NO_SUCH_CALL, TOO_LONG};
    use raw::user::{self, CODE_AT, HELLO, probe, seen};
    let exits = |slot: Option<usize>, code: u64| slot.is_some_and(|s| seen::exit_of(s) == code);
    let hello = ran(&HELLO, 0);
    report.check(
        "kernel.user.hello",
        exits(hello, 0)
            && seen::logged(b"Hello from ring 3, through a system call")
            && seen::RING.load(Ordering::Acquire) == 3,
        "the first program should say hello from ring 3 through a system call, and end with 0",
    );
    report.check(
        "kernel.user.stack",
        exits(ran(&probe::STACK, 0), 42),
        "a program's stack should work in ring 3",
    );
    let before = raw::interrupts::ticks();
    report.check(
        "kernel.user.sleep",
        ran(&probe::NAP, 0).is_some_and(|s| seen::exit_of(s) >= before + 2),
        "a program should sleep through a system call and ask for the ticks",
    );
    user_fault_checks(report);
    report.check(
        "kernel.user.refused",
        exits(ran(&probe::BAD_LOG, 0), BAD_ADDRESS)
            && exits(ran(&probe::LONG_LOG, 0), TOO_LONG)
            && exits(ran(&probe::UNKNOWN, 0), NO_SUCH_CALL),
        "a system call should refuse the kernel's memory, a line too long and a number that is no call",
    );
    report.check(
        "kernel.user.empty_log",
        exits(ran(&probe::EMPTY_LOG, 0), 0),
        "a line of no bytes should be logged from anywhere",
    );
    report.check(
        "kernel.user.edge",
        exits(ran(&probe::EDGE_LOG, 0), 0) && seen::logged(b"lastword"),
        "a line that ends exactly at the end of its page should be read whole",
    );
    report.check(
        "kernel.user.yield",
        exits(ran(&probe::YIELDER, 0), 0),
        "a program should give way through a system call and come back",
    );
    report.check(
        "kernel.user.unmapped",
        exits(ran(&probe::UNMAPPED_LOG, 0), BAD_ADDRESS),
        "the log should refuse a user address where nothing is mapped",
    );
    // The spinner keeps the first core in ring 3 for 20 ticks without
    // giving way; the first thread still runs in between, so the timer
    // takes the processor from ring 3 and comes back on the right stack.
    let spinner = user::start(0, &probe::SPINNER, 0);
    let mut turns = 0;
    while spinner.is_ok_and(raw::thread::alive) && turns < 200 {
        raw::thread::sleep(1);
        turns += 1;
    }
    report.check(
        "kernel.user.preempted",
        spinner.is_ok_and(|s| !raw::thread::alive(s) && seen::exit_of(s) == 0)
            && (10..200).contains(&turns),
        "a program that never gives way should still lose the processor at every tick",
    );
    let other = raw::smp::others().next().unwrap_or(0);
    // Three programs keep their argument at the same address at once, two
    // on this core and one on another; each finds its own again.
    let keepers = [(0, 11), (0, 22), (other, 33)]
        .map(|(core, argument)| user::start(core, &probe::KEEP, argument));
    let all_ended = (0..100).any(|_| {
        raw::thread::sleep(1);
        keepers
            .iter()
            .all(|k| k.is_ok_and(|s| !raw::thread::alive(s)))
    });
    report.check(
        "kernel.user.apart",
        all_ended
            && keepers
                .iter()
                .zip([11, 22, 33])
                .all(|(k, code)| k.is_ok_and(|s| seen::exit_of(s) == code)),
        "programs running at once should each have the same address to themselves",
    );
    let free = || {
        raw::memory::FRAMES
            .lock()
            .as_ref()
            .map(frames::Frames::free_frames)
    };
    let before = free();
    let ran_once = ran(&probe::STACK, 0);
    report.check(
        "kernel.user.cleaned",
        exits(ran_once, 42) && free() == before && raw::memory::translate(CODE_AT).is_none(),
        "a program's address space should go back whole once it ended, and the kernel's never hold it",
    );
}

static SEEN: [AtomicU64; 8] = [const { AtomicU64::new(u64::MAX) }; 8];
static SLEPT: AtomicU64 = AtomicU64::new(0);

static CR4: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Keeps the CR4 of the core it runs on under its argument.
extern "C" fn note_cr4(slot: u64) {
    if let Some(cr4) = usize::try_from(slot).ok().and_then(|s| CR4.get(s)) {
        cr4.store(x86::raw::read_cr4(), Ordering::Release);
    }
}

/// Keeps the number of the core it runs on under its argument.
extern "C" fn which_core(slot: u64) {
    if let Some(seen) = usize::try_from(slot).ok().and_then(|s| SEEN.get(s)) {
        seen.store(raw::interrupts::this_core() as u64, Ordering::Release);
    }
}

/// Stops the timer of the core it runs on.
extern "C" fn timer_off(_: u64) {
    x86::raw::stop_apic_timer();
}

/// Starts the timer of the core it runs on again, every `period` counts.
extern "C" fn timer_on(period: u64) {
    x86::raw::start_apic_ticks(
        raw::interrupts::TIMER,
        u32::try_from(period).unwrap_or(u32::MAX),
    );
}

/// Sleeps three ticks and keeps how many passed.
extern "C" fn sleeper(_: u64) {
    let at = raw::interrupts::ticks();
    raw::thread::sleep(3);
    SLEPT.store(raw::interrupts::ticks() - at, Ordering::Release);
}

/// The cores each of two busy threads found itself on, a bit for each.
static RAN_ON: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Keeps the processor for 20 ticks without giving way, noting every core
/// it finds itself running on.
extern "C" fn busy_noting_cores(me: u64) {
    let until = raw::interrupts::ticks() + 20;
    while raw::interrupts::ticks() < until {
        if let Some(cores) = usize::try_from(me).ok().and_then(|me| RAN_ON.get(me)) {
            cores.fetch_or(1 << raw::interrupts::this_core(), Ordering::AcqRel);
        }
        core::hint::spin_loop();
    }
}

/// Threads on the other cores: each runs on the core it was started on, a
/// thread there that never gives way loses the processor at that core's
/// own tick, and a sleeper there wakes on time.
fn other_core_checks<W: Write>(report: &mut Report<'_, W>, tsc_hz: u64) {
    use raw::thread::{self, NORMAL};
    let mut others = raw::smp::others();
    let (first, last) = (others.next(), raw::smp::others().last());
    let started = raw::smp::others()
        .all(|core| thread::spawn_on(core, which_core, core as u64, NORMAL).is_ok());
    thread::sleep(3);
    report.check(
        "kernel.thread.everywhere",
        first.is_some()
            && started
            && raw::smp::others().all(|core| {
                SEEN.get(core)
                    .is_some_and(|s| s.load(Ordering::Acquire) == core as u64)
            }),
        "a thread started on another core should run on that core",
    );
    let bits = x86::Protections::detect().cr4_bits();
    let asked = raw::smp::others()
        .all(|core| thread::spawn_on(core, note_cr4, core as u64, NORMAL).is_ok());
    thread::sleep(3);
    report.check(
        "kernel.protect.everywhere",
        first.is_some()
            && asked
            && bits != 0
            && raw::smp::others().all(|core| {
                CR4.get(core)
                    .is_some_and(|c| c.load(Ordering::Acquire) & bits == bits)
            }),
        "every other core should turn SMEP and SMAP on as well",
    );
    let Some(first) = first else {
        return;
    };
    // With its timer stopped, the halted core can only wake for a thread
    // started there through the interrupt that comes with it.
    let period = x86::raw::read_apic(x86::apic::TIMER_INITIAL);
    let stopped = thread::spawn_on(first, timer_off, 0, 31).is_ok();
    thread::sleep(2);
    let woken = (0..3).all(|_| {
        SEEN[first].store(u64::MAX, Ordering::Release);
        thread::spawn_on(first, which_core, first as u64, NORMAL).is_ok()
            && x86::raw::wait_for(tsc_hz / 10, || {
                SEEN[first].load(Ordering::Acquire) == first as u64
            })
    });
    let restarted = thread::spawn_on(first, timer_on, period, 31).is_ok();
    thread::sleep(2);
    report.check(
        "kernel.thread.woken_elsewhere",
        stopped && woken && restarted,
        "another core whose timer is stopped should still run a thread started there, woken by an interrupt",
    );
    // Threads that end there free their numbers once that core switched
    // away from them, so more than there are can start one after another.
    // Each runs within microseconds, but a busy host once kept the other
    // core away for 57 ms, so each gets half a second.
    RAN.store(0, Ordering::Release);
    let freed = (0..300u32).all(|n| {
        thread::spawn_on(first, counting, 0, NORMAL).is_ok()
            && x86::raw::wait_for(tsc_hz / 2, || RAN.load(Ordering::Acquire) == n + 1)
    });
    report.check(
        "kernel.thread.freed_elsewhere",
        freed,
        "threads that end on another core should free their numbers for the next ones",
    );
    forget_the_trail();
    STOP.store(false, Ordering::Release);
    RAN.store(0, Ordering::Release);
    let spinner = thread::spawn_on(first, spinning, 0, NORMAL);
    let other = thread::spawn_on(first, noting, 77, NORMAL);
    thread::sleep(5);
    let (noted, count) = trail();
    STOP.store(true, Ordering::Release);
    thread::sleep(3);
    report.check(
        "kernel.thread.preempted_elsewhere",
        spinner.is_ok() && other.is_ok() && count == 1 && noted[0] == 77 && RAN.load(Ordering::Acquire) == 1,
        "on another core too, a thread that never gives way should lose the processor at that core's tick",
    );
    SLEPT.store(0, Ordering::Release);
    let slept = last.map(|core| thread::spawn_on(core, sleeper, 0, NORMAL));
    thread::sleep(8);
    report.check(
        "kernel.thread.slept_elsewhere",
        slept.is_some_and(|s| s.is_ok()) && (3..=5).contains(&SLEPT.load(Ordering::Acquire)),
        "a thread sleeping on another core should wake there on time",
    );
    moving_checks(report, first);
}

/// Of two threads that never give way, started on core `first` without
/// being kept there, one moves to a core that has nothing else to run,
/// and two started for that core stay.
fn moving_checks<W: Write>(report: &mut Report<'_, W>, first: usize) {
    use raw::thread::{self, NORMAL};
    for cores in &RAN_ON {
        cores.store(0, Ordering::Release);
    }
    let started =
        (0..2).all(|me| thread::spawn_in(first, 0, busy_noting_cores, me, NORMAL).is_ok());
    thread::sleep(30);
    let both = RAN_ON
        .iter()
        .all(|cores| cores.load(Ordering::Acquire) != 0);
    let elsewhere = RAN_ON
        .iter()
        .any(|cores| cores.load(Ordering::Acquire) & !(1 << first) != 0);
    report.check(
        "kernel.thread.moved",
        started && both && elsewhere,
        "of two threads that never give way on one core, one should move to a core with nothing to run",
    );
    for cores in &RAN_ON {
        cores.store(0, Ordering::Release);
    }
    let pinned = (0..2).all(|me| thread::spawn_on(first, busy_noting_cores, me, NORMAL).is_ok());
    thread::sleep(30);
    report.check(
        "kernel.thread.kept",
        pinned
            && RAN_ON
                .iter()
                .all(|cores| cores.load(Ordering::Acquire) == 1 << first),
        "two threads started for one core should stay there, though another has nothing to run",
    );
}

fn memory_checks<W: Write>(
    report: &mut Report<'_, W>,
    regions: &[MemoryRegion],
    free: Option<usize>,
) {
    use crate::raw::memory::{FRAMES, heap_free};
    use alloc::boxed::Box;
    use alloc::format;
    use alloc::vec::Vec;
    use frames::FRAME;

    // The bits, the heap and the heap's bits came out of the free frames.
    let words = frames::Frames::words_for(kernel::ram_top(regions)) as u64;
    let bits_frames = (words * 8).div_ceil(FRAME);
    let heap_frames = kernel::HEAP_BYTES as u64 / FRAME;
    let heap_bits_frames = (heap::Heap::words_for(kernel::HEAP_BYTES) as u64 * 8).div_ceil(FRAME);
    report.check(
        "kernel.frames.counted",
        free.map(|f| f as u64)
            == Some(kernel::usable_frames(regions) - bits_frames - heap_frames - heap_bits_frames),
        "every whole free frame above 1 MiB should be free, but for the allocator's own and the heap",
    );

    let before = frame_checks(report, regions);
    report.check(
        "kernel.lock.let_go",
        !FRAMES.is_locked(),
        "a lock should be free again once let go",
    );
    let empty = heap_free();
    let boxed = Box::new(0x1234_5678_u64);
    let numbers: Vec<u32> = (0..100_000).collect();
    let text = format!("{} frames, {}", before, *boxed);
    let sum: u64 = numbers.iter().map(|&n| u64::from(n)).sum();
    report.check(
        "kernel.heap.used",
        *boxed == 0x1234_5678
            && sum == 4_999_950_000
            && text.ends_with("frames, 305419896")
            && heap_free() < empty - 400_000,
        "Box, Vec and String should work on the kernel's heap",
    );
    drop((boxed, numbers, text));
    let mut huge: Vec<u8> = Vec::new();
    report.check(
        "kernel.heap.given_back",
        heap_free() == empty
            && huge.try_reserve(kernel::HEAP_BYTES + 1).is_err()
            && heap_free() == empty,
        "the heap should get everything back, and refuse more than it holds",
    );
}

/// Maps a fresh frame where nothing was, uses it through both addresses,
/// and takes it away again.
fn mapping_checks<W: Write>(report: &mut Report<'_, W>) {
    use crate::raw::memory::{
        FRAMES, fill_frame, map_page, map_stack, read_word, unmap_page, write_word,
    };
    use paging::{Error, Flags};

    // The 512 GiB from here are free: the direct map lies below them and the
    // stacks above.
    const WINDOW: u64 = 0xffff_fd80_0000_0000;
    report.check(
        "kernel.paging.kernel_half",
        raw::memory::kernel_half_complete() && raw::memory::kernel_root() == x86::raw::read_cr3() & !0xfff,
        "every slot of the kernel's half should have a table, so every address space shares all of it",
    );
    let flags = Flags::WRITABLE | Flags::NO_EXECUTE;
    let missing = |c: Option<Caught>| {
        c.is_some_and(|c| c.vector == 14 && c.error == 0 && c.address == WINDOW)
    };

    let before = expected::read_at(WINDOW);
    let mut frames = FRAMES.lock();
    let frame = frames.as_mut().and_then(frames::Frames::alloc);
    drop(frames);
    let Some(frame) = frame else {
        report.check(
            "kernel.paging.mapped",
            false,
            "a frame should be free for the window",
        );
        return;
    };
    let mapped = map_page(WINDOW, frame, flags);
    fill_frame(frame, 0x0bad_cafe);
    let seen = read_word(WINDOW) == 0x0bad_cafe && read_word(WINDOW + 4088) == 0x0bad_cafe;
    write_word(WINDOW + 8, 0xfeed_f00d);
    report.check(
        "kernel.paging.mapped",
        missing(before)
            && mapped.is_ok()
            && seen
            && expected::read_at(WINDOW).is_none()
            && read_word(HHDM_BASE + frame + 8) == 0xfeed_f00d,
        "a frame mapped where nothing was should show through both its addresses",
    );
    let free = || FRAMES.lock().as_ref().map(frames::Frames::free_frames);
    let free_before = free();
    report.check(
        "kernel.paging.twice",
        map_page(WINDOW, frame, flags) == Err(Error::AlreadyMapped(WINDOW))
            && map_stack(WINDOW + 2 * 4096, 2) == Err(Error::AlreadyMapped(WINDOW))
            && free() == free_before
            && expected::read_at(WINDOW + 4096).is_some(),
        "mapping a page that is mapped already should be refused, and a stack that cannot be whole leave nothing behind",
    );
    // The page is read right before each unmap, so the processor surely
    // remembers it and has to be told to forget it.
    read_word(WINDOW);
    let unmapped = unmap_page(WINDOW);
    let after = expected::read_at(WINDOW);
    let forgotten = (0..8).all(|_| {
        map_page(WINDOW, frame, flags).is_ok()
            && read_word(WINDOW) == 0x0bad_cafe
            && unmap_page(WINDOW) == Ok(frame)
            && missing(expected::read_at(WINDOW))
    });
    let again = unmap_page(WINDOW);
    let freed = FRAMES
        .lock()
        .as_mut()
        .is_some_and(|f| f.free(frame).is_ok());
    report.check(
        "kernel.paging.unmapped",
        unmapped == Ok(frame)
            && missing(after)
            && forgotten
            && again == Err(Error::NotMapped(WINDOW))
            && freed,
        "an unmapped page should fault at once, give its frame back, and not unmap twice",
    );
}

/// Hands out frames, writes them, gives them back, and returns how many
/// were free before. The lock on the frames is let go on return.
fn frame_checks<W: Write>(report: &mut Report<'_, W>, regions: &[MemoryRegion]) -> usize {
    use crate::raw::memory::{FRAMES, fill_frame, frame_holds};
    use frames::FRAME;

    report.check(
        "kernel.frames.once",
        crate::raw::memory::take(regions).is_none(),
        "the frame allocator should be built once, so its bits have one owner",
    );
    let mut frames = FRAMES.lock();
    report.check(
        "kernel.lock.held",
        FRAMES.is_locked(),
        "a lock should show as held while someone holds it",
    );
    let Some(frames) = frames.as_mut() else {
        report.check(
            "kernel.frames.handed_out",
            false,
            "the frame allocator should be there",
        );
        return 0;
    };
    let before = frames.free_frames();
    let mut got = [0u64; 64];
    for (i, slot) in got.iter_mut().enumerate() {
        if let Some(a) = frames.alloc() {
            *slot = a;
            fill_frame(a, 0x5eed_0000 + i as u64);
        }
    }
    let distinct = got.iter().enumerate().all(|(i, a)| !got[..i].contains(a));
    report.check(
        "kernel.frames.handed_out",
        distinct
            && got.iter().all(|&a| {
                a >= frames::LOW && kernel::covered(regions, RegionKind::USABLE, a, FRAME)
            })
            && got
                .iter()
                .enumerate()
                .all(|(i, &a)| frame_holds(a, 0x5eed_0000 + i as u64)),
        "frames should be free memory above 1 MiB, each once, and keep what is written to them",
    );
    let given_back = got.iter().all(|&a| frames.free(a).is_ok());
    let run = frames.alloc_run(512, 512);
    let run_back = run.is_some_and(|r| (0..512).all(|i| frames.free(r + i * FRAME).is_ok()));
    report.check(
        "kernel.frames.given_back",
        given_back
            && run.is_some_and(|r| r.is_multiple_of(2 << 20))
            && run_back
            && frames.free_frames() == before
            && frames.free(got[0]).is_err(),
        "frames should come back, a 2 MiB run too, and not twice",
    );

    before
}

fn screen_checks<W: Write>(
    report: &mut Report<'_, W>,
    info: &BootInfo,
    screen: Option<&Canvas<'_>>,
) {
    report.check(
        "kernel.screen",
        screen.is_some_and(|c| {
            c.width() == info.framebuffer.width as usize
                && c.pixel(0, 0) == Some(NIGHT)
                && c.pixel(c.width() - 1, c.height() - 1) == Some(NIGHT)
                && c.pixel(60, 116) == Some(TANGERINE)
        }),
        "the status screen should fill the framebuffer the loader passed on",
    );
    report.check(
        "kernel.screen.title",
        screen.is_some_and(|c| {
            (60..260)
                .flat_map(|x| (60..100).map(move |y| (x, y)))
                .filter(|&(x, y)| c.pixel(x, y).is_some_and(|p| p != NIGHT))
                .count()
                > 400
        }),
        "the title should be drawn in the top left",
    );
    report.check(
        "kernel.screen.rows",
        screen.is_some_and(|c| {
            // Cores and uptime are rows 2 and 3, from baseline 186, 32 apart.
            [186 + 64, 186 + 96].iter().all(|&baseline| {
                (196..400)
                    .flat_map(|x| (baseline - 14..baseline).map(move |y| (x, y)))
                    .any(|(x, y)| c.pixel(x, y).is_some_and(|p| p != NIGHT))
            })
        }),
        "the cores and the uptime should be on the screen",
    );
    let bar_top = 56 + 130 + 5 * 32 + 8 + 1;
    report.check(
        "kernel.screen.bar",
        screen.is_some_and(|c| {
            c.pixel(57, bar_top) == Some(GRASS) && c.pixel(56 + 723, bar_top) == Some(NIGHT)
        }),
        "the memory bar should start with free memory and end where it should",
    );
    report.check(
        "kernel.screen.once",
        raw::screen(&info.framebuffer).is_none(),
        "the framebuffer should be handed out only once",
    );
}
