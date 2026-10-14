//! The kernel built as a QEMU test image. The loader runs its own checks,
//! starts this kernel only when they all passed, and the kernel then checks
//! what it was handed and ends the run with `TESTS DONE`, in the format
//! sextant reads.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use bootinfo::{BootInfo, HHDM_BASE, MemoryRegion, RegionKind, STACK_PAGES, STACK_TOP};
use canvas::Canvas;
use canvas::palette::{GRASS, INK, NIGHT, SUN, TANGERINE};
use kernel::display::Display;
use kernel::machine::Machine;
use kernel::panic::Details;
use kernel::{bytes_of, covered, is_tidy};
use x86::raw::Com1;

use crate::raw::interrupts::expected::{self, Caught};
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
    display: Option<&Display<'_>>,
    free: Option<usize>,
    serial: &mut Com1,
) -> ! {
    let mut report = Report {
        out: serial,
        failed: 0,
    };
    report.check("kernel.entered", true, "");
    let screen = display.map(Display::screen);

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
            && mapped(info.kernel.virtual_base, 4096) == physical(info.kernel.physical, 4096),
        "the kernel's virtual and physical first page should be the same memory",
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
    handover_screen_checks(&mut report, info, regions, display);
    if screen.is_some() {
        // sextant keeps the picture and answers with a carriage return.
        // The same port, not a new one: opening it again would clear an
        // answer that came quickly.
        let _ = writeln!(report.out, "SCREEN status");
        let answer = report.out.receive(1 << 24);
        report.check(
            "kernel.screen.kept",
            answer == Some(b'\r'),
            "sextant should answer SCREEN with a carriage return on the serial port",
        );
    }

    timer_checks(&mut report, tsc_hz);
    exception_checks(&mut report);

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
            report.check(
                "kernel.double_fault",
                details.message.starts_with("double fault: ") && details.location.starts_with("the instruction at 0xffffffff8"),
                "running out of stack should end on the panic screen as a double fault in the kernel",
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

/// The kernel's own tables are loaded, and each exception a check asks for
/// reaches the handler with the right vector, error code and address.
fn exception_checks<W: Write>(report: &mut Report<'_, W>) {
    use crate::raw::interrupts::{SPURIOUS, TIMER};
    x86::qemu_test::tables(
        &[TIMER, SPURIOUS],
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
    let agrees = pm_timer.zip(tsc_hz).is_some_and(|((port, bits), hz)| {
        let tenth = u32::try_from(acpi::PM_TIMER_HZ / 10).unwrap_or(u32::MAX);
        let (pm_start, tsc_start) = (raw::pm_timer(port), x86::raw::tsc());
        while kernel::clock::ticks_between(pm_start, raw::pm_timer(port), bits) < tenth {
            core::hint::spin_loop();
        }
        let ms = (x86::raw::tsc() - tsc_start) / (hz / 1000);
        (90..=110).contains(&ms)
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
}

/// The other cores: all running on their own task states when there is an
/// x2APIC to start them with, and none otherwise.
fn smp_checks<W: Write>(report: &mut Report<'_, W>, machine: &Machine, tsc_hz: Option<u64>) {
    if !x86::apic::supported(x86::raw::cpuid(1)[2]) {
        report.check(
            "kernel.smp.alone",
            raw::smp::running() == (0, 0) && x86::raw::gdt_entries().get(5) == Some(&0),
            "without an x2APIC no other core should be started or given a task state",
        );
        return;
    }
    report.check(
        "kernel.smp.running",
        machine
            .cores
            .is_some_and(|n| n > 1 && raw::smp::running() == (n - 1, n - 1)),
        "every other core the MADT lists should have come into the kernel, on its own stack and the kernel's tables",
    );
    let entries = x86::raw::gdt_entries();
    let others = machine.cores.map_or(0, |n| n.saturating_sub(1) as usize);
    report.check(
        "kernel.smp.task_states",
        (1..=others).all(|i| entries.get(3 + 2 * i).is_some_and(|e| e >> 40 & 0xf == 0xb))
            && entries.get(3 + 2 * (others + 1)).is_some_and(|&e| e == 0),
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
    report.check(
        "kernel.smp.quick",
        tsc_hz.is_some_and(|hz| {
            raw::smp::took() < hz / 1000 * 15 * u64::from(machine.cores.unwrap_or(1))
        }),
        "starting the other cores should not wait out the time a core that never starts gets",
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
fn timer_checks<W: Write>(report: &mut Report<'_, W>, tsc_hz: Option<u64>) {
    use crate::raw::interrupts::ticks;
    use crate::raw::timer;

    use x86::apic;
    use x86::raw::read_apic;

    report.check(
        "x86.apic.off_before",
        APIC_OFF_BEFORE.load(Ordering::Acquire),
        "OVMF leaves the APIC in xAPIC mode, and nothing reaches its x2APIC registers before",
    );
    x86::qemu_test::apic(&mut |name, ok, why| report.check(name, ok, why));
    if !x86::apic::supported(x86::raw::cpuid(1)[2]) {
        report.check(
            "kernel.timer.none",
            timer::start(tsc_hz).is_none() && !x86::raw::interrupts_enabled(),
            "without an x2APIC the kernel should keep spinning, with interrupts off",
        );
        return;
    }
    let tenth = tsc_hz.unwrap_or(0) / 10;
    let period = timer::start(tsc_hz);
    report.check(
        "kernel.timer.x2apic",
        period.is_some()
            && x86::raw::x2apic_enabled()
            && x86::raw::interrupts_enabled()
            && read_apic(apic::SPURIOUS) & 0x1ff == apic::spurious(crate::raw::interrupts::SPURIOUS)
            && read_apic(apic::LVT_TIMER) & 0x3_00ff == apic::periodic(crate::raw::interrupts::TIMER),
        "the x2APIC should be on with its spurious vector, its timer periodic on its vector, and interrupts let in",
    );
    let first = ticks();
    x86::raw::spin(tenth);
    let counted = ticks() - first;
    report.check(
        "kernel.timer.ticks",
        (5..=20).contains(&counted),
        "the timer should interrupt about ten times in a tenth of a second",
    );
    let before = ticks();
    x86::raw::wait_for_interrupt();
    report.check(
        "kernel.timer.wakes",
        ticks() > before,
        "a halted processor should wake at the next tick",
    );
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
    let unmapped = unmap_page(WINDOW);
    let after = expected::read_at(WINDOW);
    let again = unmap_page(WINDOW);
    let freed = FRAMES
        .lock()
        .as_mut()
        .is_some_and(|f| f.free(frame).is_ok());
    report.check(
        "kernel.paging.unmapped",
        unmapped == Ok(frame) && missing(after) && again == Err(Error::NotMapped(WINDOW)) && freed,
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
