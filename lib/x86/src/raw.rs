//! The instructions themselves. A function is safe when no argument can
//! make it touch memory; the others say what their caller promises.

use core::arch::asm;
use core::fmt;

use crate::tables::{self, Gate, Pointer};
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

    /// The next byte that arrived, waiting through at most `polls` reads
    /// of the line status. A board without COM1 reads back 0xff, which
    /// looks like data, so a 0xff byte counts as nothing.
    #[must_use]
    pub fn receive(&mut self, polls: u32) -> Option<u8> {
        for _ in 0..polls {
            let status = inb(serial::COM1 + serial::LINE_STATUS);
            if status != 0xff && status & serial::DATA_READY != 0 {
                return Some(inb(serial::COM1));
            }
            core::hint::spin_loop();
        }
        None
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
    // SAFETY: only called with COM1's line status register, which reads
    // without side effects, and its receive register, which only takes
    // the byte that arrived.
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

/// Loads `table` as the global descriptor table and reloads every segment
/// register from it: code from [`tables::KERNEL_CODE`], the data segments
/// from [`tables::KERNEL_DATA`], FS and GS with the null selector.
///
/// # Safety
///
/// `table` must stay where it is for good and hold the kernel's code and
/// data descriptors at those selectors, like [`tables::gdt`] makes.
pub unsafe fn load_gdt(table: &'static [u64]) {
    let pointer = Pointer::to(table);
    // SAFETY: the caller promises the table is permanent and has the
    // selectors used here. The far return pops the new CS and the address
    // of the next instruction.
    unsafe {
        asm!(
            "lgdt [{pointer}]",
            "push {code}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov ss, {data:x}",
            "mov fs, {null:x}",
            "mov gs, {null:x}",
            pointer = in(reg) &raw const pointer,
            code = const tables::KERNEL_CODE as u64,
            tmp = lateout(reg) _,
            data = in(reg) u32::from(tables::KERNEL_DATA),
            null = in(reg) 0u32,
            options(preserves_flags),
        );
    }
}

/// Loads the task register with the task state segment's selector.
///
/// # Safety
///
/// The current global descriptor table must hold an available task state
/// segment at `selector` whose memory stays where it is.
pub unsafe fn load_task_register(selector: u16) {
    // SAFETY: the caller promises the descriptor is there and valid.
    unsafe { asm!("ltr {:x}", in(reg) selector, options(nostack, preserves_flags)) };
}

/// Loads `table` as the interrupt descriptor table.
///
/// # Safety
///
/// `table` must stay where it is for good, and every present gate must
/// lead to a handler that can take that interrupt.
pub unsafe fn load_idt(table: &'static [Gate; 256]) {
    let pointer = Pointer::to(table);
    // SAFETY: the caller promises the table and its handlers are sound.
    unsafe {
        asm!("lidt [{}]", in(reg) &raw const pointer, options(readonly, nostack, preserves_flags));
    }
}

/// Where the global descriptor table is and how long it is.
#[must_use]
pub fn gdt_register() -> Pointer {
    let mut pointer = Pointer::default();
    // SAFETY: sgdt writes ten bytes into the pointer and nothing else.
    unsafe { asm!("sgdt [{}]", in(reg) &raw mut pointer, options(nostack, preserves_flags)) };
    pointer
}

/// Where the interrupt descriptor table is and how long it is.
#[must_use]
pub fn idt_register() -> Pointer {
    let mut pointer = Pointer::default();
    // SAFETY: sidt writes ten bytes into the pointer and nothing else.
    unsafe { asm!("sidt [{}]", in(reg) &raw mut pointer, options(nostack, preserves_flags)) };
    pointer
}

/// The code and stack segment selectors in use.
#[must_use]
pub fn segments() -> (u16, u16) {
    let (cs, ss): (u16, u16);
    // SAFETY: reading segment registers changes nothing.
    unsafe {
        asm!("mov {:x}, cs", "mov {:x}, ss", out(reg) cs, out(reg) ss, options(nomem, nostack, preserves_flags));
    }
    (cs, ss)
}

/// Loads a copy of the firmware's interrupt descriptor table and then the
/// firmware's own again, with interrupts off all along, and returns what
/// `sidt` showed in between. Only the QEMU checks do this.
#[cfg(feature = "qemu-test")]
#[must_use]
pub fn swap_idt_and_back() -> Pointer {
    struct Copy(core::cell::UnsafeCell<[Gate; 256]>);
    // SAFETY: only this function touches the copy, with interrupts off.
    unsafe impl Sync for Copy {}
    static COPY: Copy = Copy(core::cell::UnsafeCell::new([Gate::MISSING; 256]));

    without_interrupts(|| {
        let firmware = idt_register();
        let count = ((usize::from(firmware.limit) + 1) / 16).min(256);
        // SAFETY: the firmware's table is mapped where IDTR says and holds
        // count gates; the copy is only used here.
        let copy = unsafe {
            let table = &mut *COPY.0.get();
            core::ptr::copy_nonoverlapping(firmware.base as *const Gate, table.as_mut_ptr(), count);
            &*table
        };
        // SAFETY: the copy holds the firmware's own gates, and interrupts
        // stay off until the firmware's table is back.
        unsafe { load_idt(copy) };
        let during = idt_register();
        // SAFETY: puts back exactly the table the firmware had loaded.
        unsafe {
            asm!("lidt [{}]", in(reg) &raw const firmware, options(readonly, nostack, preserves_flags));
        }
        during
    })
}

/// The selector in the task register.
#[must_use]
pub fn task_register() -> u16 {
    let selector: u16;
    // SAFETY: str only reads the task register.
    unsafe { asm!("str {:x}", out(reg) selector, options(nomem, nostack, preserves_flags)) };
    selector
}

/// The entries of the global descriptor table in use, as the processor
/// sees them.
#[must_use]
pub fn gdt_entries() -> &'static [u64] {
    let pointer = gdt_register();
    let count = (usize::from(pointer.limit) + 1) / 8;
    // SAFETY: the processor reads the table at this place for as long as
    // it is loaded, so it is mapped, and every table this crate loads is
    // 'static. Reading it changes nothing.
    unsafe { core::slice::from_raw_parts(pointer.base as *const u64, count) }
}

/// The gates of the interrupt descriptor table in use.
#[must_use]
pub fn idt_entries() -> &'static [Gate] {
    let pointer = idt_register();
    let count = (usize::from(pointer.limit) + 1) / size_of::<Gate>();
    // SAFETY: the processor reads the table at this place for as long as
    // it is loaded, and every table this crate loads is 'static.
    unsafe { core::slice::from_raw_parts(pointer.base as *const Gate, count) }
}

/// The task state segment the task register names, found through its
/// descriptor in the global descriptor table.
#[must_use]
pub fn task_state() -> Option<&'static tables::TaskState> {
    let entries = gdt_entries();
    let at = usize::from(task_register() / 8);
    let (low, high) = (*entries.get(at)?, *entries.get(at + 1)?);
    let base = (low >> 16 & 0xff_ffff) | (low >> 56 & 0xff) << 24 | high << 32;
    // SAFETY: the processor uses the task state at this address for as
    // long as the task register names it, so it is mapped and stays.
    unsafe { (base as *const tables::TaskState).as_ref() }
}

/// The address the last page fault was about.
#[must_use]
pub fn cr2() -> u64 {
    let value: u64;
    // SAFETY: reading CR2 changes nothing.
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}
