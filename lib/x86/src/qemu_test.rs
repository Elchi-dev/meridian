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
