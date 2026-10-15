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
    let Some(base) = x86::raw::apic_base() else {
        return false;
    };
    if x86::raw::xapic_registers().is_none() {
        let flags = Flags::WRITABLE | Flags::NO_CACHE | Flags::WRITE_THROUGH | Flags::NO_EXECUTE;
        if super::memory::map_page(kernel::APIC_PAGE, base, flags).is_err() {
            return false;
        }
    }
    // SAFETY: APIC_PAGE maps the page apic_base names, writable and
    // uncached, and nothing unmaps it.
    unsafe { x86::raw::enable_xapic(kernel::APIC_PAGE) }
}
