//! The floating point and vector registers: on on every core, the same way
//! on each, kept for each thread in the page above its stack while the
//! thread does not run, and as at power on for every thread that starts.
//! The kernel itself uses none of them, so they need keeping only when a
//! core goes from one thread to another.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use kernel::thread::registers_at;
use x86::fpu::{self, Saving};

/// How the first core chose to keep them: 0 for FXSAVE, or the parts XSAVE
/// keeps, which always include the x87.
static PARTS: AtomicU64 = AtomicU64::new(0);

/// How the registers are kept.
pub fn saving() -> Saving {
    match PARTS.load(Ordering::Acquire) {
        0 => Saving::Fx,
        on => Saving::X(on),
    }
}

/// Turns the registers on for this core, to be kept as `saving` says from
/// now on. The first core picks with [`Saving::detect`], and the others go
/// with what it picked, [`saving`].
pub fn start(saving: Saving) {
    let on = match saving {
        Saving::Fx => 0,
        Saving::X(on) => on,
    };
    PARTS.store(on, Ordering::Release);
    let cr0 = fpu::cr0_on(x86::raw::read_cr0());
    let cr4 = x86::raw::read_cr4() | saving.cr4_bits();
    // SAFETY: with EM and TS clear and NE set, x87 and SSE instructions run
    // and their errors come as exceptions, which have handlers. Every
    // x86-64 processor has FXSAVE and SSE, and OSXSAVE is set only where
    // CPUID says the processor has XSAVE.
    unsafe {
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
    if on != 0 {
        let (high, low) = halves(on);
        // SAFETY: OSXSAVE is on, and leaf 0xD of CPUID says the processor
        // has every part, the x87 among them, and AVX under AVX-512.
        unsafe {
            asm!("xsetbv", in("ecx") 0, in("eax") low, in("edx") high, options(nomem, nostack, preserves_flags));
        }
    }
}

/// `value` as EDX and EAX take it, the high half first.
fn halves(value: u64) -> (u32, u32) {
    let bytes = value.to_le_bytes();
    let (low, high) = bytes.split_at(4);
    let half = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    (half(high), half(low))
}

/// Keeps this core's registers for thread `from` and loads thread `to`'s.
/// Interrupts have to be off, and `to` has to have run before or been
/// given [`fresh`] registers.
pub fn switch(from: usize, to: usize) {
    let (from, to) = (registers_at(from), registers_at(to));
    // SAFETY: every thread's page is mapped before it first runs and stays
    // so, only the core running or switching to a thread touches it, and
    // it holds what an earlier save or `fresh` wrote, which the restore
    // takes. Both pages are aligned to 4096, more than either needs.
    unsafe {
        match saving() {
            Saving::Fx => asm!(
                "fxsave64 [{from}]",
                "fxrstor64 [{to}]",
                from = in(reg) from,
                to = in(reg) to,
                options(nostack, preserves_flags),
            ),
            Saving::X(_) => asm!(
                "xsave64 [{from}]",
                "xrstor64 [{to}]",
                from = in(reg) from,
                to = in(reg) to,
                in("eax") u32::MAX,
                in("edx") u32::MAX,
                options(nostack, preserves_flags),
            ),
        }
    }
}

/// Gives thread `slot` the registers a new thread starts with.
pub fn fresh(slot: usize) {
    let at = registers_at(slot) as *mut [u8; fpu::FRESH_BYTES];
    // SAFETY: the slot's page is mapped, and its thread is about to start,
    // so nothing running uses it.
    unsafe { at.write(fpu::fresh()) };
}

/// The registers as XSAVE or FXSAVE lays them out, with room for every part
/// Meridian keeps. Only the test image looks at them.
#[cfg(feature = "qemu-test")]
#[repr(C, align(64))]
pub struct Image(pub [u8; 4096]);

/// XCR0, the parts XSAVE keeps, where XSAVE is on. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn xcr0() -> Option<u64> {
    if x86::raw::read_cr4() & x86::cr4::OSXSAVE == 0 {
        return None;
    }
    let (low, high): (u32, u32);
    // SAFETY: OSXSAVE is on, so XGETBV runs, and XCR0 always exists.
    unsafe {
        asm!("xgetbv", in("ecx") 0, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    Some(x86::join(high, low))
}

/// Loads this core's registers from `image`: with XRSTOR of the parts in
/// `parts`, or with FXRSTOR for none. Only the test image does this.
#[cfg(feature = "qemu-test")]
pub fn load(image: &Image, parts: Option<u64>) {
    let at = image.0.as_ptr();
    // SAFETY: the image is aligned and large enough for either, and the
    // test image fills it with values either takes.
    unsafe {
        match parts {
            None => asm!("fxrstor64 [{}]", in(reg) at, options(nostack, readonly, preserves_flags)),
            Some(on) => {
                let (high, low) = halves(on);
                asm!(
                    "xrstor64 [{}]",
                    in(reg) at,
                    in("eax") low,
                    in("edx") high,
                    options(nostack, readonly, preserves_flags),
                );
            }
        }
    }
}

/// Keeps this core's registers in `image`, as [`load`] would take them
/// back. Only the test image does this.
#[cfg(feature = "qemu-test")]
pub fn dump(image: &mut Image, parts: Option<u64>) {
    let at = image.0.as_mut_ptr();
    // SAFETY: as for `load`; XSAVE and FXSAVE only write the image.
    unsafe {
        match parts {
            None => asm!("fxsave64 [{}]", in(reg) at, options(nostack, preserves_flags)),
            Some(on) => {
                let (high, low) = halves(on);
                asm!(
                    "xsave64 [{}]",
                    in(reg) at,
                    in("eax") low,
                    in("edx") high,
                    options(nostack, preserves_flags),
                );
            }
        }
    }
}

/// The x87 control word and MXCSR, read on their own. Only the test image
/// asks.
#[cfg(feature = "qemu-test")]
pub fn controls() -> (u16, u32) {
    let (mut control, mut mxcsr) = (0u16, 0u32);
    // SAFETY: both only store a register into the variable given.
    unsafe {
        asm!(
            "fnstcw [{}]",
            "stmxcsr [{}]",
            in(reg) &raw mut control,
            in(reg) &raw mut mxcsr,
            options(nostack, preserves_flags),
        );
    }
    (control, mxcsr)
}
