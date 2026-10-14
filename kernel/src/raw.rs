//! The way into the kernel and the ways out of it: the entry point the
//! loader jumps to, the screen, and what happens when the kernel has to
//! stop.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

use bootinfo::{BootInfo, Framebuffer, HHDM_BASE, MemoryRegion, PixelFormat};
use canvas::{Canvas, Format};
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

/// The framebuffer as a canvas, reached through the direct map. It is
/// handed out once, so there is never a second way to write it.
pub fn screen(fb: &Framebuffer) -> Option<Canvas<'static>> {
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
    if fb.base == 0
        || len.checked_mul(4)? as u64 > fb.size
        || SCREEN_TAKEN.swap(true, Ordering::AcqRel)
    {
        return None;
    }
    // SAFETY: the loader maps the whole framebuffer in the direct map, the
    // size check above keeps the slice inside it, and SCREEN_TAKEN makes
    // this the only slice over it.
    let pixels = unsafe { core::slice::from_raw_parts_mut((HHDM_BASE + fb.base) as *mut u32, len) };
    Canvas::new(pixels, width, height, stride, format)
}

/// Stops for good: halts on a real machine, ends a QEMU test as failed.
pub fn stop() -> ! {
    #[cfg(feature = "qemu-test")]
    x86::raw::qemu_exit(false);
    #[cfg(not(feature = "qemu-test"))]
    x86::raw::halt()
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let _ = writeln!(Com1::open(), "kernel: panic: {info}");
    stop()
}
