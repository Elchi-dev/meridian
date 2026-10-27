//! The way into the kernel and the ways out of it: the entry point the
//! loader jumps to, the descriptor tables and exceptions, the screen, and
//! what happens when the kernel has to stop.

pub mod ahci;
pub mod apic;
pub mod console;
pub mod disk;
pub mod fpu;
pub mod hpet;
pub mod interrupts;
pub mod ioapic;
pub mod ipc;
pub mod memory;
pub mod nvme;
pub mod pci;
pub mod ps2;
pub mod serial;
pub mod smp;
pub mod thread;
pub mod timer;
pub mod tlb;
pub mod user;
pub mod vfs;
pub mod virtio;

#[cfg(not(test))]
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bootinfo::{BootInfo, Framebuffer, HHDM_BASE, MemoryRegion, Module, PixelFormat};
#[cfg(not(test))]
use canvas::Scratch;
use canvas::{Canvas, Format};
#[cfg(not(test))]
use kernel::{Text, panic::Details};
#[cfg(not(test))]
use qr::Code;
#[cfg(not(test))]
use x86::raw::Com1;

#[unsafe(no_mangle)]
extern "sysv64" fn kernel_start(info: *const BootInfo) -> ! {
    let mut serial = serial::open();
    // SAFETY: the loader passes the boot information it built, mapped in
    // the direct map, and does not run again to change it.
    let Some(info) = (unsafe { info.as_ref() }) else {
        let _ = writeln!(serial, "kernel: the loader passed no boot information");
        stop()
    };
    if let Err(mismatch) = info.check() {
        let _ = writeln!(
            serial,
            "kernel: the boot information is not the kind this kernel reads: {mismatch:?}"
        );
        stop()
    }
    let regions = match usize::try_from(info.memory.count) {
        // SAFETY: boot information that passed its check points at count
        // regions in the direct map.
        Ok(count) => unsafe {
            core::slice::from_raw_parts(info.memory.regions as *const MemoryRegion, count)
        },
        Err(_) => &[],
    };
    save_screen(&info.framebuffer);
    interrupts::install();
    x86::raw::protect(x86::Protections::detect());
    fpu::start(x86::fpu::Saving::detect());
    let _ = x86::raw::set_cpu_number(0);
    crate::run(info, regions, &mut serial)
}

/// Physical memory as the ACPI tables are read from it: through the direct
/// map, and only where the memory map says there is something to read.
pub struct DirectMap<'a>(pub &'a [MemoryRegion]);

impl acpi::Memory for DirectMap<'_> {
    fn bytes(&self, at: u64, len: usize) -> Option<&[u8]> {
        if !kernel::readable(self.0, at, u64::try_from(len).ok()?) {
            return None;
        }
        // SAFETY: readable keeps the bytes inside one region of the map
        // and below the end of the direct map, and nothing in the kernel
        // writes the firmware's tables.
        Some(unsafe { core::slice::from_raw_parts((HHDM_BASE + at) as *const u8, len) })
    }
}

/// Reads the ACPI power management timer at `port`, which must be the
/// port the FADT names for it.
pub fn pm_timer(port: u16) -> u32 {
    // SAFETY: the timer only counts, so reading it changes nothing.
    unsafe { x86::raw::read_port_u32(port) }
}

/// `len` bytes of physical memory, read through the direct map. Only the
/// test image reads memory this way, and only memory the boot information
/// names.
#[cfg(feature = "qemu-test")]
pub fn physical(address: u64, len: usize) -> &'static [u8] {
    // SAFETY: the loader maps all physical memory at HHDM_BASE, and the
    // checks only read memory the boot information names.
    unsafe { core::slice::from_raw_parts((HHDM_BASE + address) as *const u8, len) }
}

/// `len` bytes at a virtual address the kernel is mapped at.
#[cfg(feature = "qemu-test")]
pub fn mapped(address: u64, len: usize) -> &'static [u8] {
    // SAFETY: only called with the kernel's own image range.
    unsafe { core::slice::from_raw_parts(address as *const u8, len) }
}

/// Whether the framebuffer was handed out already.
static SCREEN_TAKEN: AtomicBool = AtomicBool::new(false);

/// The framebuffer the loader handed over, kept for the panic screen: its
/// base, its size, width and height, stride and pixel format.
static SAVED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn save_screen(fb: &Framebuffer) {
    let fields = [
        fb.base,
        fb.size,
        u64::from(fb.width) << 32 | u64::from(fb.height),
        u64::from(fb.stride) << 32 | u64::from(fb.format.0),
    ];
    for (slot, value) in SAVED.iter().zip(fields) {
        slot.store(value, Ordering::Release);
    }
}

fn saved_screen() -> Framebuffer {
    let [base, size, dims, layout] = SAVED.each_ref().map(|s| s.load(Ordering::Acquire));
    let high = |v: u64| u32::try_from(v >> 32).unwrap_or(0);
    let low = |v: u64| u32::try_from(v & 0xffff_ffff).unwrap_or(0);
    Framebuffer {
        base,
        size,
        width: high(dims),
        height: low(dims),
        stride: high(layout),
        format: PixelFormat(low(layout)),
    }
}

/// The pixel order and how many pixels `fb` spans, when it can be drawn on.
fn layout(fb: &Framebuffer) -> Option<(Format, usize)> {
    let format = match fb.format {
        PixelFormat::RGB => Format::Rgb,
        PixelFormat::BGR => Format::Bgr,
        _ => return None,
    };
    let (width, height, stride) = (fb.width as usize, fb.height as usize, fb.stride as usize);
    let len = height
        .checked_sub(1)?
        .checked_mul(stride)?
        .checked_add(width)?;
    (fb.base != 0 && len.checked_mul(4)? as u64 <= fb.size).then_some((format, len))
}

/// The framebuffer as a canvas, reached through the direct map. It is
/// handed out once, so there is never a second way to write it.
pub fn screen(fb: &Framebuffer) -> Option<Canvas<'static>> {
    let (format, len) = layout(fb)?;
    if SCREEN_TAKEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    // SAFETY: the loader maps the whole framebuffer in the direct map, the
    // size check in layout keeps the slice inside it, and SCREEN_TAKEN
    // makes this the only slice over it.
    let pixels = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + fb.base) as *mut u32, len) };
    Canvas::new(
        pixels,
        fb.width as usize,
        fb.height as usize,
        fb.stride as usize,
        format,
    )
}

/// The room the loader set aside for drawing off the screen, as a canvas
/// in the framebuffer's pixel order, or `None` when there is none.
pub fn back_buffer(info: &BootInfo, regions: &[MemoryRegion]) -> Option<Canvas<'static>> {
    let (format, _) = layout(&info.framebuffer)?;
    let len = kernel::back_buffer_pixels(info, regions)?;
    if BACK_TAKEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    // SAFETY: the loader allocated these pages for this alone and they are
    // the kernel's in the memory map, which back_buffer_pixels checked for
    // all len pixels; the direct map covers all memory, and BACK_TAKEN
    // makes this the only slice over them.
    let pixels =
        unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + info.back_buffer) as *mut u32, len) };
    let (width, height) = (
        info.framebuffer.width as usize,
        info.framebuffer.height as usize,
    );
    Canvas::new(pixels, width, height, width, format)
}

static BACK_TAKEN: AtomicBool = AtomicBool::new(false);

/// A file the loader read, with its bytes.
type File = (Module, &'static [u8]);

/// The files the loader read for the kernel, by name, kept for programs to
/// spawn.
static KEPT: memory::Spin<[Option<File>; bootinfo::Modules::MAX]> =
    memory::Spin::new([None; bootinfo::Modules::MAX]);

/// Keeps the files the loader read, for [`module`] to find.
pub fn keep_modules(info: &BootInfo, regions: &[MemoryRegion]) {
    let mut kept = KEPT.lock();
    for (slot, module) in kept.iter_mut().zip(modules(info, regions)) {
        *slot = Some(module);
    }
}

/// The bytes of the file the loader read whose name is `name` in
/// capitals, as the tree of files compares names.
pub fn module_in_capitals(name: &str) -> Option<&'static [u8]> {
    let name = name.to_uppercase();
    KEPT.lock()
        .iter()
        .flatten()
        .find(|(m, _)| core::str::from_utf8(m.name()).is_ok_and(|n| n.to_uppercase() == name))
        .map(|&(_, bytes)| bytes)
}

/// The bytes of the file the loader read under `name`.
pub fn module(name: &[u8]) -> Option<&'static [u8]> {
    KEPT.lock()
        .iter()
        .flatten()
        .find(|(m, _)| m.name() == name)
        .map(|&(_, bytes)| bytes)
}

/// The files the loader read for the kernel, each with its bytes, leaving
/// out any that do not lie in the kernel's own memory.
pub fn modules<'a>(
    info: &BootInfo,
    regions: &'a [MemoryRegion],
) -> impl Iterator<Item = (Module, &'static [u8])> + 'a {
    let (at, count) = kernel::program::module_list(info, regions).unwrap_or((0, 0));
    (0..count).filter_map(move |i| {
        let entry = HHDM_BASE + at + (i * size_of::<Module>()) as u64;
        // SAFETY: module_list found the whole list in the kernel's own
        // memory, which the direct map covers and nothing writes.
        let module = Module::from_bytes(unsafe { &*(entry as *const [u8; size_of::<Module>()]) });
        let len = usize::try_from(module.size).ok()?;
        kernel::program::module_kept(&module, regions).then(|| {
            // SAFETY: the same, for the bytes module_kept checked.
            let bytes = unsafe {
                core::slice::from_raw_parts((HHDM_BASE + module.start) as *const u8, len)
            };
            (module, bytes)
        })
    })
}

/// The framebuffer for the panic screen, even when it was handed out.
#[cfg(not(test))]
fn panic_screen() -> Option<Canvas<'static>> {
    let fb = saved_screen();
    let (format, len) = layout(&fb)?;
    // SAFETY: the panic handler calls this once. Whatever held the screen
    // before is further down this stack and never runs again, so its slice
    // is never used after this one is made.
    let pixels = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + fb.base) as *mut u32, len) };
    Canvas::new(
        pixels,
        fb.width as usize,
        fb.height as usize,
        fb.stride as usize,
        format,
    )
}

/// Stops for good: halts on a real machine, ends a QEMU test as failed.
pub fn stop() -> ! {
    #[cfg(feature = "qemu-test")]
    x86::raw::qemu_exit(false);
    #[cfg(not(feature = "qemu-test"))]
    x86::raw::halt()
}

/// Set by the first panic, so a panic while drawing the panic screen
/// stops at once instead of drawing again, and the console draws no more.
static PANICKING: AtomicBool = AtomicBool::new(false);

/// Whether a crash took the screen, which nothing else may draw on then.
pub fn panicking() -> bool {
    PANICKING.load(Ordering::Acquire)
}

/// Working space for the panic screen. The stack may be nearly used up
/// when a panic comes, so it is not taken from there.
#[cfg(not(test))]
struct PanicSpace(UnsafeCell<(Code, Scratch)>);

// SAFETY: only the first panic touches the space, and PANICKING makes
// sure there is only one first panic.
#[cfg(not(test))]
unsafe impl Sync for PanicSpace {}

#[cfg(not(test))]
static PANIC_SPACE: PanicSpace = PanicSpace(UnsafeCell::new((Code::new(), Scratch::new())));

#[cfg(not(test))]
#[panic_handler]
fn on_panic(info: &core::panic::PanicInfo) -> ! {
    let mut message = Text::<256>::new();
    let _ = write!(message, "{}", info.message());
    let mut location = Text::<128>::new();
    if let Some(at) = info.location() {
        let _ = write!(location, "{}:{}:{}", at.file(), at.line(), at.column());
    }
    crash(message.as_str(), location.as_str())
}

/// Shows a processor exception on the panic screen, with the instruction
/// it came from.
#[cfg(not(test))]
fn crash_from_exception(vector: u64, error: u64, address: u64, rip: u64) -> ! {
    let mut message = Text::<256>::new();
    let _ = kernel::exceptions::describe(vector, error, address, &mut message);
    let mut location = Text::<128>::new();
    let _ = write!(location, "the instruction at {rip:#x}");
    crash(message.as_str(), location.as_str())
}

#[cfg(test)]
fn crash_from_exception(_: u64, _: u64, _: u64, _: u64) -> ! {
    unreachable!("exceptions only come on the real machine")
}

/// Writes the crash to the serial port, draws the panic screen once and
/// stops.
#[cfg(not(test))]
fn crash(message: &str, location: &str) -> ! {
    let mut serial = Com1::open();
    let _ = writeln!(serial, "kernel: stopped: {message} ({location})");
    if PANICKING.swap(true, Ordering::AcqRel) {
        stop()
    }
    let details = Details { message, location };
    let mut screen = panic_screen();
    if let Some(canvas) = screen.as_mut() {
        // SAFETY: PANICKING lets only the first crash get here, so this is
        // the only reference to the space there ever is.
        let (code, scratch) = unsafe { &mut *PANIC_SPACE.0.get() };
        if let Err(e) = kernel::panic::draw(canvas, &details, code, scratch) {
            let _ = writeln!(serial, "kernel: the panic screen could not be drawn: {e:?}");
        }
    }
    after_panic(screen.as_ref(), &details, &mut serial)
}

/// Lets the test image crash a second time, to check a crash that comes
/// from the processor after one that came from a panic. The first crash's
/// working space is never used again, as its code never resumes.
#[cfg(all(not(test), feature = "qemu-test"))]
pub fn allow_another_crash() {
    PANICKING.store(false, Ordering::Release);
}

#[cfg(all(not(test), feature = "qemu-test"))]
fn after_panic(screen: Option<&Canvas<'_>>, details: &Details<'_>, serial: &mut Com1) -> ! {
    crate::qemu_test::after_panic(screen, details, serial)
}

#[cfg(all(not(test), not(feature = "qemu-test")))]
fn after_panic(_: Option<&Canvas<'_>>, _: &Details<'_>, _: &mut Com1) -> ! {
    x86::raw::halt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Framebuffer {
        Framebuffer {
            base: 0x8000_0000,
            size: 1280 * 800 * 4,
            width: 1280,
            height: 800,
            stride: 1280,
            format: PixelFormat::BGR,
        }
    }

    #[test]
    fn the_saved_screen_comes_back_whole() {
        let fb = Framebuffer {
            base: 0x40_0000_1000,
            size: 0x7f_ffff,
            width: 0xffff_fff1,
            height: 0xffff_fff2,
            stride: 0xffff_fff3,
            format: PixelFormat(0xffff_fff4),
        };
        save_screen(&fb);
        assert_eq!(saved_screen(), fb);
        save_screen(&screen());
        assert_eq!(saved_screen(), screen());
    }

    #[test]
    fn only_screens_that_add_up_are_drawn_on() {
        assert_eq!(layout(&screen()), Some((Format::Bgr, 1280 * 800)));
        let rgb = Framebuffer {
            format: PixelFormat::RGB,
            ..screen()
        };
        assert_eq!(layout(&rgb), Some((Format::Rgb, 1280 * 800)));
        let padded = Framebuffer {
            stride: 1344,
            size: (1344 * 799 + 1280) * 4,
            ..screen()
        };
        assert_eq!(
            layout(&padded),
            Some((Format::Bgr, 1344 * 799 + 1280)),
            "the last row may be short"
        );
        let refused = [
            Framebuffer {
                base: 0,
                ..screen()
            },
            Framebuffer {
                size: 1280 * 800 * 4 - 1,
                ..screen()
            },
            Framebuffer {
                format: PixelFormat::NONE,
                ..screen()
            },
            Framebuffer {
                format: PixelFormat(3),
                ..screen()
            },
            Framebuffer {
                height: 0,
                ..screen()
            },
            Framebuffer {
                stride: u32::MAX,
                height: u32::MAX,
                size: u64::MAX,
                ..screen()
            },
        ];
        for fb in refused {
            assert_eq!(layout(&fb), None, "{fb:?}");
        }
    }
}
