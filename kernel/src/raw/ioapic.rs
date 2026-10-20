//! The first I/O APIC, which turns the interrupts of devices into
//! interrupts of a processor, through registers mapped uncached at
//! [`kernel::IO_APIC_PAGE`]: one select register naming the register the
//! window after it reads and writes.

use core::sync::atomic::{AtomicU64, Ordering};

use x86::ioapic;

use super::memory::{self, Spin};

/// Where the select register is, once [`start`] mapped the page; 0 before.
static SELECT: AtomicU64 = AtomicU64::new(0);

/// Held from selecting a register to reading or writing it.
static WINDOW: Spin<()> = Spin::new(());

fn read(register: u32) -> u32 {
    let _held = WINDOW.lock();
    let select = SELECT.load(Ordering::Acquire);
    // SAFETY: start mapped the I/O APIC's page uncached and stored where
    // its select register lies, and the window is 16 bytes after it.
    unsafe {
        core::ptr::write_volatile(select as *mut u32, register);
        core::ptr::read_volatile((select + ioapic::WINDOW) as *const u32)
    }
}

fn write(register: u32, value: u32) {
    let _held = WINDOW.lock();
    let select = SELECT.load(Ordering::Acquire);
    // SAFETY: as in read, for the registers the system may write.
    unsafe {
        core::ptr::write_volatile(select as *mut u32, register);
        core::ptr::write_volatile((select + ioapic::WINDOW) as *mut u32, value);
    }
}

/// Maps the I/O APIC whose registers lie at physical `address`, holds
/// every one of its inputs back, and returns how many it has; none when
/// its page cannot be mapped.
pub fn start(address: u64) -> Option<u32> {
    if !memory::map_registers(kernel::IO_APIC_PAGE, address & !0xfff, 1) {
        return None;
    }
    SELECT.store(kernel::IO_APIC_PAGE + (address & 0xfff), Ordering::Release);
    let inputs = ioapic::inputs(read(ioapic::VERSION));
    for input in 0..inputs {
        route(input, ioapic::entry(0, 0, false, false, true));
    }
    Some(inputs)
}

/// Sets the redirection entry of `input` to `entry`: its destination
/// first, so an entry that lets the interrupt through never sends it to
/// the processor the old one named.
pub fn route(input: u32, entry: u64) {
    let register = ioapic::redirection(input);
    write(register + 1, u32::try_from(entry >> 32).unwrap_or(0));
    write(register, u32::try_from(entry & 0xffff_ffff).unwrap_or(0));
}

/// The redirection entry of `input`. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn entry(input: u32) -> u64 {
    let register = ioapic::redirection(input);
    u64::from(read(register + 1)) << 32 | u64::from(read(register))
}
