//! The HPET's main counter: a clock to measure the time stamp counter
//! against where the ACPI timer is missing.

use core::sync::atomic::{AtomicU64, Ordering};

use kernel::hpet;
use kernel::machine::Reference;
use paging::Flags;

/// Where the HPET's registers are reached once [`start`] mapped them; 0
/// before.
static REGISTERS: AtomicU64 = AtomicU64::new(0);

/// Maps the HPET's registers at physical `address` uncached at
/// [`kernel::HPET_PAGE`], lets its main counter run, and returns it as a
/// reference clock. `None` when the page cannot be mapped or the counter
/// is not one the specification allows.
pub fn start(address: u64) -> Option<Reference> {
    let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::NO_EXECUTE;
    super::memory::map_page(kernel::HPET_PAGE, address & !0xfff, flags).ok()?;
    REGISTERS.store(
        hpet::registers_at(kernel::HPET_PAGE, address),
        Ordering::Release,
    );
    let reference = hpet::counter(read(hpet::CAPABILITIES))?;
    write(
        hpet::CONFIGURATION,
        read(hpet::CONFIGURATION) | hpet::ENABLE,
    );
    Some(reference)
}

/// The main counter as a reference clock, read again from its
/// capabilities. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn reference() -> Option<Reference> {
    hpet::counter(read(hpet::CAPABILITIES))
}

/// Where the main counter stands, or 0 before [`start`].
pub fn count() -> u64 {
    read(hpet::COUNTER)
}

fn read(register: u64) -> u64 {
    let at = REGISTERS.load(Ordering::Acquire);
    if at == 0 {
        return 0;
    }
    // SAFETY: start mapped the HPET's registers at `at`, uncached, and
    // they take aligned 64-bit reads that change nothing.
    unsafe { core::ptr::read_volatile((at + register) as *const u64) }
}

fn write(register: u64, value: u64) {
    let at = REGISTERS.load(Ordering::Acquire);
    if at != 0 {
        // SAFETY: as for read; the only write lets the main counter run.
        unsafe { core::ptr::write_volatile((at + register) as *mut u64, value) };
    }
}
