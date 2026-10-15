//! Checks of the instructions against a real processor, run from the
//! loader's test image in QEMU while the firmware is still in charge.

use crate::raw;
use crate::{Features, cr0, efer};

/// Runs every check and hands each result to `check`: its name, whether it
/// passed, and what a failure means.
pub fn run(check: &mut impl FnMut(&str, bool, &str)) {
    let vendor = crate::vendor();
    check(
        "x86.cpuid.vendor",
        raw::cpuid(0)[0] >= 1 && vendor.as_str().len() == 12,
        "CPUID 0 should name the vendor in twelve printable bytes",
    );
    check(
        "x86.cpuid.brand",
        crate::brand().as_str().len() > 4,
        "QEMU's processors carry a brand name",
    );
    check(
        "x86.cpuid.extended",
        raw::cpuid(0x8000_0000)[0] >= 0x8000_0001,
        "a 64-bit processor has the extended leaves",
    );

    registers(check);

    let (first, second) = (raw::tsc(), raw::tsc());
    check(
        "x86.tsc",
        second > first && first > 0,
        "the time stamp counter should have counted since power on, and go on",
    );

    let e = raw::efer();
    check(
        "x86.efer.long_mode",
        e & 1 << 8 != 0 && e & 1 << 10 != 0,
        "EFER should say long mode is enabled and active",
    );
    let c = raw::read_cr0();
    check(
        "x86.cr0.paging",
        c & 1 << 31 != 0 && c & 1 != 0,
        "CR0 should have protection and paging on",
    );
    check(
        "x86.interrupts",
        raw::interrupts_enabled(),
        "UEFI runs applications with interrupts on",
    );

    let (inside, nested, after_nested) = raw::without_interrupts(|| {
        let inside = raw::interrupts_enabled();
        let nested = raw::without_interrupts(raw::interrupts_enabled);
        (inside, nested, raw::interrupts_enabled())
    });
    check(
        "x86.without_interrupts",
        !inside && !nested && raw::interrupts_enabled(),
        "interrupts should be off inside and on again after",
    );
    check(
        "x86.without_interrupts.nested",
        !after_nested,
        "an inner call should leave interrupts off for the outer one",
    );
    check(
        "x86.without_interrupts.result",
        raw::without_interrupts(|| 0x5eed) == 0x5eed,
        "the closure's result should come back",
    );

    let features = Features::detect();
    check(
        "x86.features",
        features.no_execute && features.pages_1g,
        "QEMU's max processor has no-execute pages and 1 GiB pages",
    );
    let before = raw::efer();
    let on = raw::enable_no_execute() && raw::enable_no_execute();
    let after = raw::efer();
    check(
        "x86.no_execute",
        on && after & efer::NO_EXECUTE_ENABLE != 0,
        "turning on no-execute pages should show in EFER",
    );
    check(
        "x86.no_execute.alone",
        before | efer::NO_EXECUTE_ENABLE == after,
        "turning on no-execute pages should leave the other EFER bits alone",
    );

    let before = raw::read_cr0();
    raw::clear_write_protect();
    let cleared = raw::read_cr0();
    raw::enable_write_protect();
    let after = raw::read_cr0();
    check(
        "x86.write_protect",
        cleared & cr0::WRITE_PROTECT == 0 && after & cr0::WRITE_PROTECT != 0,
        "turning write protection off and on should show in CR0",
    );
    check(
        "x86.write_protect.alone",
        before | cr0::WRITE_PROTECT == after,
        "turning on write protection should leave the other CR0 bits alone",
    );
}

/// Checks of the kernel's descriptor tables once it has loaded them, with
/// gates for `interrupts` past the exceptions and room for the task states
/// of `cores`. They run from the kernel's test image, since the firmware
/// needs its own tables while it runs.
pub fn tables(interrupts: &[u8], cores: usize, check: &mut impl FnMut(&str, bool, &str)) {
    use crate::tables::{KERNEL_CODE, KERNEL_DATA, SEGMENTS, TASK_STATE};

    let segments = SEGMENTS.len();
    let gdt = raw::gdt_register();
    check(
        "x86.gdt",
        usize::from(gdt.limit) + 1 == (segments + 2 * cores) * 8 && { gdt.base }
            >= 0xffff_8000_0000_0000,
        "the kernel's GDT, its segments and a task state for each core, should be loaded from the higher half",
    );
    let entries = raw::gdt_entries();
    let tss = entries.get(segments..segments + 2).map_or(0, |e| {
        (e[0] >> 16 & 0xff_ffff) | (e[0] >> 56 & 0xff) << 24 | e[1] << 32
    });
    let want = crate::tables::gdt(tss);
    check(
        "x86.gdt.entries",
        entries.len() == segments + 2 * cores
            && entries[0] == 0
            && (1..segments).all(|i| entries[i] & !(1 << 40) == want[i])
            && entries[segments] & !(1 << 41) == want[segments]
            && entries[segments + 1] == want[segments + 1],
        "the loaded GDT should hold the null, kernel and user code and data, and task state entries",
    );
    check(
        "x86.segments",
        raw::segments() == (KERNEL_CODE, KERNEL_DATA),
        "code and stack should use the kernel's own selectors",
    );
    check(
        "x86.task_register",
        raw::task_register() == TASK_STATE,
        "the task register should hold the task state segment's selector",
    );
    // The processor marks code and data descriptors accessed, bit 40, when
    // it loads them, and the task state descriptor busy, bit 41, so the
    // comparison above leaves those bits out.
    check(
        "x86.task_state.busy",
        entries.get(segments).is_some_and(|e| e >> 40 & 0xf == 0xb),
        "loading the task register should mark its descriptor busy",
    );
    let idt = raw::idt_register();
    check(
        "x86.idt",
        { idt.limit } == 4095 && { idt.base } >= 0xffff_8000_0000_0000,
        "the kernel's 256 gate IDT should be loaded",
    );
    let gates = raw::idt_entries();
    let kernel = |handler: u64| handler >= 0xffff_ffff_8000_0000;
    check(
        "x86.idt.exceptions",
        gates.len() == 256
            && gates[..32]
                .iter()
                .all(|g| g.is_present() && kernel(g.handler())),
        "every exception should have a gate into the kernel",
    );
    check(
        "x86.idt.fault_stacks",
        gates
            .iter()
            .enumerate()
            .all(|(v, g)| g.stack() == u8::from(matches!(v, 2 | 8 | 18))),
        "NMIs, double faults and machine checks should switch to the fault stack, nothing else",
    );
    let mut handlers: [u64; 32] =
        core::array::from_fn(|v| gates.get(v).map_or(0, crate::tables::Gate::handler));
    handlers.sort_unstable();
    check(
        "x86.idt.distinct",
        handlers.windows(2).all(|w| w[0] != w[1]),
        "every exception should have a stub of its own, so the vector is known",
    );
    check(
        "x86.idt.nothing_else",
        gates.len() == 256
            && gates[32..]
                .iter()
                .zip(32u8..=255)
                .all(|(g, v)| g.is_present() == interrupts.contains(&v)),
        "past the exceptions, only the interrupts the kernel handles should have gates",
    );
    let stacks = raw::task_state().map(|t| t.interrupt_stacks);
    check(
        "x86.task_state.stack",
        stacks.is_some_and(|s| s[0] != 0 && s[0] % 16 == 0 && kernel(s[0]) && s[1..] == [0; 6]),
        "the task state should hold one fault stack, aligned and in the kernel",
    );
}

/// Whether the APIC is as OVMF leaves it: in xAPIC mode, with nothing
/// reaching its x2APIC registers yet and interrupts off. The kernel's test
/// image asks before it starts the other cores, which turns it on.
#[must_use]
pub fn apic_is_off() -> bool {
    !raw::x2apic_enabled()
        && raw::apic_timer_count() == 0
        && raw::read_apic(crate::apic::SPURIOUS) == 0
        && !raw::interrupts_enabled()
}

/// The local APIC and its timer, from the kernel's test image with
/// interrupts off, once the kernel has turned the APIC on and before it
/// starts the timer itself: spinning, the APIC's mode, version and id, its
/// timer counting down without an interrupt, set up for one, and stopped,
/// and the spurious vector.
pub fn apic(check: &mut impl FnMut(&str, bool, &str)) {
    use crate::apic::{LVT_TIMER, MASKED, SPURIOUS, TIMER_INITIAL, periodic, spurious};
    use raw::{apic_timer_count, read_apic};

    let begin = raw::tsc();
    raw::spin(1_000_000);
    check(
        "x86.apic.spin",
        raw::tsc() - begin >= 1_000_000,
        "spinning should last at least as many counter cycles as asked",
    );
    let polls = core::cell::Cell::new(0u32);
    let soon = raw::wait_for(u64::MAX, || {
        polls.set(polls.get() + 1);
        polls.get() == 5
    });
    let begin = raw::tsc();
    let never = raw::wait_for(1_000_000, || false);
    let waited = raw::tsc() - begin;
    check(
        "x86.wait_for",
        soon && polls.get() == 5 && !never && waited >= 1_000_000 && raw::wait_for(0, || true),
        "waiting should end when the condition holds, or give up once the time is up",
    );
    // The kernel has turned the APIC on: in x2APIC mode where the
    // processor has one, in xAPIC mode through a mapped page otherwise.
    let x2apic = crate::apic::supported(raw::cpuid(1)[2]);
    check(
        "x86.apic.mode",
        if x2apic {
            raw::enable_x2apic() && raw::x2apic_enabled() && raw::xapic_registers().is_none()
        } else {
            !raw::enable_x2apic() && !raw::x2apic_enabled() && raw::xapic_registers().is_some()
        },
        "the APIC should be in x2APIC mode where the processor has one, and in xAPIC mode otherwise",
    );
    let version = read_apic(crate::apic::VERSION);
    check(
        "x86.apic.version",
        (0x10..=0x15).contains(&(version & 0xff)) && version >> 16 & 0xff >= 3,
        "an integrated APIC should give its version and at least four timer and interrupt entries",
    );
    identity(check, x2apic);
    let cr3 = raw::read_cr3();
    check(
        "x86.cr3",
        cr3 != 0 && cr3.is_multiple_of(4096) && cr3 >> 52 == 0,
        "the page table root should be a page address, with no flags and no context id",
    );
    raw::start_apic_count();
    let first = apic_timer_count();
    raw::spin(100_000);
    let later = apic_timer_count();
    check(
        "x86.apic.count",
        first < u32::MAX && later < first && read_apic(LVT_TIMER) & 0x3_00ff == MASKED,
        "the timer should count down from the top, held back from interrupting",
    );
    // The longest period, so it cannot end and leave an interrupt for a
    // vector nobody handles waiting until interrupts are let in.
    raw::start_apic_ticks(0x40, u32::MAX);
    check(
        "x86.apic.ticks",
        read_apic(LVT_TIMER) & 0x3_00ff == periodic(0x40)
            && read_apic(TIMER_INITIAL) == u64::from(u32::MAX),
        "the timer should be periodic on its vector, with the period asked for",
    );
    raw::stop_apic_timer();
    raw::spin(100_000);
    check(
        "x86.apic.stopped",
        apic_timer_count() == 0
            && read_apic(TIMER_INITIAL) == 0
            && read_apic(LVT_TIMER) & MASKED != 0,
        "a stopped timer should count nothing and interrupt nothing",
    );
    raw::set_spurious_vector(0xef);
    let set = read_apic(SPURIOUS) & 0x1ff;
    raw::end_of_interrupt();
    check(
        "x86.apic.spurious",
        set == spurious(0xef),
        "the spurious vector should be set, with the APIC turned on",
    );
}

/// The APIC's id, read through its register directly and through
/// [`raw::apic_id`], against the one CPUID reports.
fn identity(check: &mut impl FnMut(&str, bool, &str), x2apic: bool) {
    use raw::read_apic;
    // Read directly, the id register holds the id whole in x2APIC mode and
    // in its top byte in xAPIC mode, so a wrong place in the page shows.
    let register = read_apic(crate::apic::ID);
    check(
        "x86.apic.id_register",
        if x2apic {
            raw::apic_id().is_some_and(|id| u64::from(id) == register)
        } else {
            raw::apic_id().is_some_and(|id| register == u64::from(id) << 24)
        },
        "the id register should hold the id where the APIC's mode puts it",
    );
    // CPUID leaf 0xB gives the x2APIC id in EDX, leaf 1 the low 8 bits
    // of it, the whole xAPIC id, in the top byte of EBX.
    let leaf_b = (raw::cpuid(0)[0] >= 0xb).then(|| raw::cpuid(0xb)[3]);
    let low = raw::cpuid(1)[1] >> 24;
    let id = raw::apic_id();
    check(
        "x86.apic.id",
        id.is_some_and(|id| {
            if x2apic {
                id & 0xff == low && leaf_b.is_none_or(|b| b == id)
            } else {
                id == low
            }
        }),
        "the APIC id should be the one CPUID reports for this processor",
    );
}

/// Under the firmware: the segments OVMF runs with, its tables, and an IDT
/// loaded and put back with interrupts off.
pub fn firmware_tables(check: &mut impl FnMut(&str, bool, &str)) {
    let (cs, ss) = raw::segments();
    check(
        "x86.firmware.segments",
        cs != 0 && ss != 0 && cs % 8 == 0 && ss % 8 == 0 && cs != ss,
        "OVMF runs on its own code and stack selectors",
    );
    let gdt = raw::gdt_register();
    check(
        "x86.firmware.gdt",
        { gdt.base } != 0
            && ({ gdt.limit } + 1) % 8 == 0
            && raw::gdt_entries().len() > usize::from(cs / 8),
        "the firmware's GDT should be there and hold its code selector",
    );
    let entries = raw::gdt_entries();
    let code = entries.get(usize::from(cs / 8)).copied().unwrap_or(0);
    check(
        "x86.firmware.code_descriptor",
        code >> 47 & 1 == 1 && code >> 43 & 1 == 1 && code >> 53 & 1 == 1,
        "the descriptor CS names should be present 64-bit code",
    );
    let data = entries.get(usize::from(ss / 8)).copied().unwrap_or(0);
    check(
        "x86.firmware.data_descriptor",
        data >> 47 & 1 == 1 && data >> 43 & 1 == 0 && data >> 41 & 1 == 1,
        "the descriptor SS names should be present, writable data",
    );
    let gates = raw::idt_entries();
    check(
        "x86.firmware.idt",
        gates.len() == 256 && gates[..20].iter().filter(|g| g.is_present()).count() >= 18,
        "OVMF's IDT should have its 256 gates, with the exceptions handled",
    );
    let (tr, state) = (raw::task_register(), raw::task_state());
    check(
        "x86.firmware.task_state",
        (tr == 0) == state.is_none(),
        "a task state should be found exactly when the task register names one",
    );
    let first: [crate::tables::Gate; 32] = core::array::from_fn(|i| gates[i]);
    let before = raw::idt_register();
    let during = raw::swap_idt_and_back();
    check(
        "x86.load_idt",
        { during.base } != { before.base }
            && { during.limit } == 4095
            && raw::idt_register() == before,
        "loading an IDT should show in sidt, and the firmware's should come back",
    );
    check(
        "x86.load_idt.untouched",
        raw::idt_entries()[..32] == first,
        "the firmware's gates should be just as they were",
    );
}

/// Registers that QEMU's machine answers the same as a real one: COM1's
/// line control, and the processor number in `IA32_TSC_AUX`.
fn registers(check: &mut impl FnMut(&str, bool, &str)) {
    check(
        "x86.serial.line_control",
        raw::Com1::open().line_control() == crate::serial::EIGHT_N_ONE,
        "COM1 should be set to 8 data bits, no parity and one stop bit once opened",
    );
    let before = raw::cpu_number();
    let set = raw::set_cpu_number(0x5a5a_0001);
    let read = raw::cpu_number();
    let restored = raw::set_cpu_number(before.unwrap_or(0));
    check(
        "x86.cpu_number",
        set && restored && read == Some(0x5a5a_0001),
        "QEMU's max processor has RDTSCP, so it should keep a number of its own in IA32_TSC_AUX",
    );
}
