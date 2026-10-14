//! The way into the kernel and the ways out of it: the entry point the
//! loader jumps to, and what happens when the kernel has to stop.

use core::fmt::Write;

#[cfg(feature = "qemu-test")]
use bootinfo::HHDM_BASE;
use bootinfo::{BootInfo, MemoryRegion};
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
