//! The kernel built as a QEMU test image. The loader runs its own checks,
//! starts this kernel only when they all passed, and the kernel then checks
//! what it was handed and ends the run with `TESTS DONE`, in the format
//! sextant reads.

use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering};

use bootinfo::{BootInfo, HHDM_BASE, MemoryRegion, RegionKind, STACK_PAGES, STACK_TOP};
use canvas::Canvas;
use canvas::palette::{GRASS, INK, NIGHT, SUN, TANGERINE};
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
    screen: Option<&Canvas<'_>>,
    serial: &mut Com1,
) -> ! {
    let mut report = Report {
        out: serial,
        failed: 0,
    };
    report.check("kernel.entered", true, "");

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

    screen_checks(&mut report, info, screen);
    if screen.is_some() {
        // sextant keeps the picture and answers with a newline.
        // The same port, not a new one: opening it again would clear an
        // answer that came quickly.
        let _ = writeln!(report.out, "SCREEN status");
        let answer = report.out.receive(1 << 24);
        report.check(
            "kernel.screen.kept",
            answer == Some(b'\n'),
            "sextant should answer SCREEN with a newline on the serial port",
        );
    }

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
            answer == Some(b'\n'),
            "sextant should answer SCREEN with a newline on the serial port",
        );
    }
}

/// The kernel's own tables are loaded, and each exception a check asks for
/// reaches the handler with the right vector, error code and address.
fn exception_checks<W: Write>(report: &mut Report<'_, W>) {
    x86::qemu_test::tables(&mut |name, ok, why| report.check(name, ok, why));
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
    let bar_top = 56 + 130 + 3 * 32 + 8 + 1;
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
