//! The instructions themselves. A function is safe when no argument can
//! make it touch memory; the others say what their caller promises.

use core::arch::asm;
use core::fmt;

use crate::{Features, cr0, efer, serial};

/// COM1, set up for 115200 baud when opened. Only COM1 can be opened, so
/// no caller can point the port writes anywhere else.
pub struct Com1 {
    _only_com1: (),
}

impl Com1 {
    #[must_use]
    pub fn open() -> Com1 {
        if let Some(sequence) = serial::init_sequence(serial::BASE_BAUD) {
            for (offset, value) in sequence {
                outb(serial::COM1 + offset, value);
            }
        }
        Com1 { _only_com1: () }
    }

    fn send(byte: u8) {
        for _ in 0..serial::MAX_POLLS {
            if inb(serial::COM1 + serial::LINE_STATUS) & serial::TRANSMIT_EMPTY != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        outb(serial::COM1, byte);
    }
}

impl fmt::Write for Com1 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in serial::wire_bytes(s) {
            Self::send(byte);
        }
        Ok(())
    }
}

fn outb(port: u16, value: u8) {
    // SAFETY: only called with the COM1 registers. Writing them sets up or
    // feeds the UART and touches no memory.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: only called with the COM1 line status register, which reads
    // without side effects.
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Stops this processor for good.
pub fn halt() -> ! {
    loop {
        // SAFETY: disabling interrupts and halting has no effect on memory.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Ends QEMU through its isa-debug-exit device on port 0xf4. QEMU exits
/// with `(value << 1) | 1`: 33 for a pass, 35 for a failure.
#[cfg(feature = "qemu-test")]
pub fn qemu_exit(passed: bool) -> ! {
    let value: u32 = if passed { 0x10 } else { 0x11 };
    // SAFETY: test images only run in QEMU, where sextant attaches the
    // isa-debug-exit device to this port.
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4_u16, in("eax") value, options(nomem, nostack, preserves_flags));
    }
    halt()
}

/// EAX, EBX, ECX and EDX of CPUID `leaf`, with sub-leaf 0.
#[must_use]
pub fn cpuid(leaf: u32) -> [u32; 4] {
    let r = core::arch::x86_64::__cpuid_count(leaf, 0);
    [r.eax, r.ebx, r.ecx, r.edx]
}

/// # Safety
///
/// `msr` must exist on this processor; reading one that does not faults.
unsafe fn read_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: the caller promises the register exists.
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    u64::from(high) << 32 | u64::from(low)
}

/// # Safety
///
/// `msr` must exist, and the new value must not change how memory or
/// running code behaves under anyone's feet.
unsafe fn write_msr(msr: u32, value: u64) {
    let bytes = value.to_le_bytes();
    let low = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let high = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    // SAFETY: the caller promises the write is harmless.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high, options(nostack, preserves_flags));
    }
}

/// The extended feature enable register, which every `x86_64` processor has.
#[must_use]
pub fn efer() -> u64 {
    // SAFETY: EFER exists wherever long mode does, and reading it changes
    // nothing.
    unsafe { read_msr(efer::MSR) }
}

/// Lets page table entries forbid running code, when the processor can.
/// Returns whether it is on.
#[must_use]
pub fn enable_no_execute() -> bool {
    if !Features::detect().no_execute {
        return false;
    }
    // SAFETY: before this, bit 63 of an entry was reserved and would have
    // faulted, so no table in use sets it, and turning it into the
    // no-execute bit changes no mapping that is already there.
    unsafe { write_msr(efer::MSR, efer() | efer::NO_EXECUTE_ENABLE) };
    true
}

#[must_use]
pub fn read_cr0() -> u64 {
    let value: u64;
    // SAFETY: reading CR0 changes nothing.
    unsafe { asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Makes read-only pages read-only for kernel code too. Writes that used
/// to go through now fault, which can stop a program but never corrupt
/// memory.
pub fn enable_write_protect() {
    let value = read_cr0() | cr0::WRITE_PROTECT;
    // SAFETY: setting WP only adds faults; paging and caching stay as
    // they are.
    unsafe { asm!("mov cr0, {}", in(reg) value, options(nostack, preserves_flags)) };
}

/// Turns write protection off again. Only the QEMU checks do this, so they
/// can see [`enable_write_protect`] turn it on even where the firmware had
/// it on already.
#[cfg(feature = "qemu-test")]
pub fn clear_write_protect() {
    let value = read_cr0() & !cr0::WRITE_PROTECT;
    // SAFETY: clearing WP only removes faults; paging stays as it is.
    unsafe { asm!("mov cr0, {}", in(reg) value, options(nostack, preserves_flags)) };
}

/// Whether this processor takes maskable interrupts right now.
#[must_use]
pub fn interrupts_enabled() -> bool {
    let flags: u64;
    // SAFETY: pushes RFLAGS and pops it into a register, leaving the stack
    // as it was.
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags)) };
    flags & 1 << 9 != 0
}

/// Runs `f` with maskable interrupts off, and turns them back on after it
/// only if they were on before, so calls can nest.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let were_on = interrupts_enabled();
    // SAFETY: clearing the interrupt flag touches no memory.
    unsafe { asm!("cli", options(nomem, nostack)) };
    let result = f();
    if were_on {
        // SAFETY: interrupts were on when this started, so whoever handles
        // them was ready then and still is.
        unsafe { asm!("sti", options(nomem, nostack)) };
    }
    result
}
