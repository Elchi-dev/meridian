//! The instructions themselves. A function is safe when no argument can
//! make it touch memory; the others say what their caller promises.

use core::arch::asm;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::tables::{self, Gate, Pointer};
use crate::{Features, apic, cr0, efer, serial};

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
                outb(serial::port(offset), value);
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
            if serial::has_data(inb(serial::port(serial::LINE_STATUS))) {
                return Some(inb(serial::port(0)));
            }
            core::hint::spin_loop();
        }
        None
    }

    fn send(byte: u8) {
        for _ in 0..serial::MAX_POLLS {
            if serial::can_send(inb(serial::port(serial::LINE_STATUS))) {
                break;
            }
            core::hint::spin_loop();
        }
        outb(serial::port(0), byte);
    }

    /// What the line control register holds, which says how the port is
    /// set up.
    #[must_use]
    pub fn line_control(&self) -> u8 {
        inb(serial::port(serial::LINE_CONTROL))
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
    crate::join(high, low)
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
    let supported = Features::detect().no_execute;
    if supported {
        // SAFETY: before this, bit 63 of an entry was reserved and would
        // have faulted, so no table in use sets it, and turning it into the
        // no-execute bit changes no mapping that is already there.
        unsafe { write_msr(efer::MSR, efer() | efer::NO_EXECUTE_ENABLE) };
    }
    supported
}

/// Drops what this processor remembers about the page at `virt`, so a
/// changed mapping takes effect.
pub fn invalidate_page(virt: u64) {
    // SAFETY: invlpg only empties a TLB entry; the next access reads the
    // page tables again.
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

/// The physical address of the page table root, with its flags.
#[must_use]
pub fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: reading CR3 changes nothing.
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
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
    let selector = task_register();
    if selector == 0 {
        return None;
    }
    let entries = gdt_entries();
    let at = usize::from(selector / 8);
    let (low, high) = (*entries.get(at)?, *entries.get(at + 1)?);
    if !tables::is_task_state(low) {
        return None;
    }
    let base = (low >> 16 & 0xff_ffff) | (low >> 56 & 0xff) << 24 | high << 32;
    // SAFETY: the processor uses the task state at this address for as
    // long as the task register names it, so it is mapped and stays.
    unsafe { (base as *const tables::TaskState).as_ref() }
}

/// The processor's time stamp counter, which counts at a fixed rate on
/// every processor Meridian runs on.
#[must_use]
pub fn tsc() -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: rdtsc only reads the counter.
    unsafe {
        asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    crate::join(high, low)
}

/// Gives this processor a number, which RDTSCP and RDPID hand back, so code
/// can tell which processor it runs on. Returns whether the processor has
/// a place for it.
#[must_use]
pub fn set_cpu_number(number: u32) -> bool {
    let supported = crate::tsc_aux::supported(cpuid(0x8000_0000)[0], cpuid(0x8000_0001)[3]);
    if supported {
        // SAFETY: the register exists where RDTSCP does, and it only holds
        // a number for software to read.
        unsafe { write_msr(crate::tsc_aux::MSR, u64::from(number)) };
    }
    supported
}

/// The number [`set_cpu_number`] gave this processor, if it has a place
/// for one.
#[must_use]
pub fn cpu_number() -> Option<u32> {
    if !crate::tsc_aux::supported(cpuid(0x8000_0000)[0], cpuid(0x8000_0001)[3]) {
        return None;
    }
    // SAFETY: the register exists where RDTSCP does, and reading it
    // changes nothing.
    let value = unsafe { read_msr(crate::tsc_aux::MSR) };
    u32::try_from(value & 0xffff_ffff).ok()
}

/// Reads 32 bits from an I/O port.
///
/// # Safety
///
/// Reading the port must have no effect beyond giving its value, as for
/// a counter such as the ACPI power management timer.
#[must_use]
pub unsafe fn read_port_u32(port: u16) -> u32 {
    let value: u32;
    // SAFETY: the caller promises reading has no side effects.
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Whether [`enable_x2apic`] turned the x2APIC on, so its registers exist.
static X2APIC_ON: AtomicBool = AtomicBool::new(false);

/// Turns this processor's local APIC on in x2APIC mode, where its
/// registers are model specific registers. Returns false, changing
/// nothing, on a processor without an x2APIC.
pub fn enable_x2apic() -> bool {
    if apic::supported(cpuid(1)[2]) {
        // SAFETY: IA32_APIC_BASE exists wherever an x2APIC does. The APIC
        // is turned on first and then switched to x2APIC mode, the way the
        // SDM allows; its base address stays, so no memory changes
        // meaning, and nothing is routed to it before its vectors are set.
        unsafe {
            for value in apic::steps_to_x2apic(read_msr(apic::BASE_MSR)) {
                write_msr(apic::BASE_MSR, value);
            }
        }
        X2APIC_ON.store(true, Ordering::Release);
    }
    x2apic_enabled()
}

/// Whether the local APIC is on in x2APIC mode.
#[must_use]
pub fn x2apic_enabled() -> bool {
    let both = apic::ENABLE | apic::X2APIC;
    // SAFETY: the register exists wherever an x2APIC does, and reading it
    // changes nothing.
    apic::supported(cpuid(1)[2]) && unsafe { read_msr(apic::BASE_MSR) } & both == both
}

/// Writes an x2APIC register once the x2APIC is on, and does nothing
/// before.
fn write_apic(register: u32, value: u64) {
    if X2APIC_ON.load(Ordering::Acquire) {
        // SAFETY: the x2APIC is on, so its registers exist; callers here
        // only set up this processor's own timer and interrupts.
        unsafe { write_msr(register, value) };
    }
}

/// Lets the APIC deliver interrupts, sending those it gives up on to
/// `vector`.
pub fn set_spurious_vector(vector: u8) {
    write_apic(apic::SPURIOUS, apic::spurious(vector));
}

/// Starts the APIC timer counting down from the largest count at the
/// APIC's own rate, without an interrupt, to measure that rate.
pub fn start_apic_count() {
    write_apic(apic::TIMER_DIVIDE, apic::DIVIDE_BY_1);
    write_apic(apic::LVT_TIMER, apic::MASKED);
    write_apic(apic::TIMER_INITIAL, u64::from(u32::MAX));
}

/// Starts the APIC timer raising `vector` every `period` counts.
pub fn start_apic_ticks(vector: u8, period: u32) {
    write_apic(apic::TIMER_DIVIDE, apic::DIVIDE_BY_1);
    write_apic(apic::LVT_TIMER, apic::periodic(vector));
    write_apic(apic::TIMER_INITIAL, u64::from(period));
}

/// Stops the APIC timer.
pub fn stop_apic_timer() {
    write_apic(apic::LVT_TIMER, apic::MASKED);
    write_apic(apic::TIMER_INITIAL, 0);
}

/// The x2APIC register `register`, or 0 before the x2APIC is on. Only the
/// QEMU checks read the timer's entry and the spurious register back.
#[cfg(feature = "qemu-test")]
#[must_use]
pub fn read_apic(register: u32) -> u64 {
    if !X2APIC_ON.load(Ordering::Acquire) {
        return 0;
    }
    // SAFETY: the x2APIC is on, so its registers exist, and the checks
    // only read ones whose reading changes nothing.
    unsafe { read_msr(register) }
}

/// Waits until `done` says so or the time stamp counter has moved on by
/// `cycles`, and returns whether `done` said so.
pub fn wait_for(cycles: u64, done: impl Fn() -> bool) -> bool {
    let begin = tsc();
    loop {
        if done() {
            return true;
        }
        if tsc() - begin >= cycles {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Waits until the time stamp counter has moved on by `cycles`.
pub fn spin(cycles: u64) {
    let begin = tsc();
    while tsc() - begin < cycles {
        core::hint::spin_loop();
    }
}

/// Where the APIC timer's count stands, or 0 before the x2APIC is on.
#[must_use]
pub fn apic_timer_count() -> u32 {
    if !X2APIC_ON.load(Ordering::Acquire) {
        return 0;
    }
    // SAFETY: the x2APIC is on, so the register exists; reading it
    // changes nothing.
    let count = unsafe { read_msr(apic::TIMER_CURRENT) };
    u32::try_from(count & 0xffff_ffff).unwrap_or(0)
}

/// This processor's APIC id, once the x2APIC is on.
#[must_use]
pub fn apic_id() -> Option<u32> {
    if !X2APIC_ON.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: the x2APIC is on, so the register exists; reading it
    // changes nothing.
    u32::try_from(unsafe { read_msr(apic::ID) }).ok()
}

/// Sends INIT to the processor whose APIC id is `id`, which stops it.
///
/// # Safety
///
/// `id` must name another processor that runs nothing the system needs,
/// as one that has waited since the firmware started this one does.
pub unsafe fn send_init(id: u32) {
    write_apic(apic::COMMAND, apic::init(id));
}

/// Starts the processor whose APIC id is `id`, after [`send_init`], in
/// real mode at the start of `page`.
///
/// # Safety
///
/// As for [`send_init`], and `page` must hold code that brings that
/// processor somewhere safe.
pub unsafe fn send_startup(id: u32, page: u8) {
    write_apic(apic::COMMAND, apic::startup(id, page));
}

/// Sends interrupt `vector` to the processor whose APIC id is `id`.
///
/// # Safety
///
/// That processor's interrupt descriptor table must have a handler for
/// `vector`.
pub unsafe fn send_interrupt(id: u32, vector: u8) {
    write_apic(apic::COMMAND, apic::fixed(id, vector));
}

/// Tells the APIC the interrupt it delivered last is handled.
pub fn end_of_interrupt() {
    write_apic(apic::END_OF_INTERRUPT, 0);
}

/// Masks every line of the two legacy 8259 interrupt controllers, so only
/// the APIC delivers interrupts.
pub fn mask_legacy_pics() {
    // SAFETY: writing the interrupt mask registers of the 8259s only stops
    // them from raising interrupts; no memory is involved.
    unsafe {
        asm!("out dx, al", in("dx") 0xa1_u16, in("al") 0xff_u8, options(nomem, nostack, preserves_flags));
        asm!("out dx, al", in("dx") 0x21_u16, in("al") 0xff_u8, options(nomem, nostack, preserves_flags));
    }
}

/// Lets this processor take maskable interrupts.
///
/// # Safety
///
/// Every vector the APIC can deliver must lead to a handler in the loaded
/// interrupt descriptor table.
pub unsafe fn enable_interrupts() {
    // SAFETY: the caller promises every vector has its handler.
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// Lets this processor take interrupts and sleeps until one comes. One
/// that arrives between the two still wakes it, since `sti` holds them
/// back for one more instruction.
///
/// # Safety
///
/// As for [`enable_interrupts`].
pub unsafe fn enable_interrupts_and_wait() {
    // SAFETY: the caller promises every vector has its handler; halting
    // touches no memory.
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
}

/// Stops this processor taking maskable interrupts.
pub fn disable_interrupts() {
    // SAFETY: clearing the interrupt flag touches no memory.
    unsafe { asm!("cli", options(nomem, nostack)) };
}

/// Sleeps until the next interrupt. With interrupts off it returns at
/// once, since nothing could wake it.
pub fn wait_for_interrupt() {
    if interrupts_enabled() {
        // SAFETY: halting until an interrupt, with interrupts on, touches
        // no memory.
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}

/// The address the last page fault was about.
#[must_use]
pub fn cr2() -> u64 {
    let value: u64;
    // SAFETY: reading CR2 changes nothing.
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}
