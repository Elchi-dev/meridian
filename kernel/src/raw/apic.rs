//! The local APIC: in x2APIC mode where the processor has one, and in
//! xAPIC mode, through its page mapped uncached, where it has not.

use paging::Flags;

/// Turns this processor's local APIC on and returns whether it is on. With
/// an x2APIC its registers are model specific registers; without one they
/// are a page of memory, which the first call maps uncached at
/// [`kernel::APIC_PAGE`], and every core's APIC answers there.
pub fn enable() -> bool {
    if x86::raw::enable_x2apic() {
        return true;
    }
    let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::NO_EXECUTE;
    let mapped = x86::raw::xapic_registers().is_some()
        || x86::raw::apic_base()
            .is_some_and(|base| super::memory::map_page(kernel::APIC_PAGE, base, flags).is_ok());
    // SAFETY: APIC_PAGE maps the page apic_base names, writable and
    // uncached, and nothing unmaps it.
    mapped && unsafe { x86::raw::enable_xapic(kernel::APIC_PAGE) }
}
