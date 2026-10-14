//! The way into the kernel and the ways out of it: the entry point the
//! loader jumps to, the descriptor tables and exceptions, the screen, and
//! what happens when the kernel has to stop.

pub mod interrupts;

#[cfg(not(test))]
use core::cell::UnsafeCell;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bootinfo::{BootInfo, Framebuffer, HHDM_BASE, MemoryRegion, PixelFormat};
#[cfg(not(test))]
use canvas::Scratch;
use canvas::{Canvas, Format};
#[cfg(not(test))]
use kernel::{Text, panic::Details};
#[cfg(not(test))]
use qr::Code;
use x86::raw::Com1;

#[unsafe(no_mangle)]
extern "sysv64" fn kernel_start(info: *const BootInfo) -> ! {
    let mut serial = Com1::open();
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
    crate::run(info, regions, &mut serial)
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
/// stops at once instead of drawing again.
#[cfg(not(test))]
static PANICKING: AtomicBool = AtomicBool::new(false);
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
