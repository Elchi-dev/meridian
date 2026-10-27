//! The APIC timer, which wakes the processor a hundred times a second so
//! it can sleep in between.

use super::interrupts::{SPURIOUS, TIMER};

/// How many times a second the timer interrupts, as programs count.
pub const HZ: u64 = abi::TICKS_A_SECOND;

/// Turns the local APIC on, measures its timer against the time stamp
/// counter that runs at `tsc_hz` for a hundredth of a second, starts it at
/// [`HZ`] and lets interrupts in. Returns the timer's period in its own
/// counts, or `None` without an APIC or a known counter rate.
pub fn start(tsc_hz: Option<u64>) -> Option<u32> {
    let tsc_hz = tsc_hz?;
    if !super::apic::enable() {
        return None;
    }
    x86::raw::mask_legacy_pics();
    x86::raw::set_spurious_vector(SPURIOUS);
    x86::raw::start_apic_count();
    let begin = x86::raw::tsc();
    x86::raw::spin(tsc_hz / 100);
    let counted = u64::from(u32::MAX - x86::raw::apic_timer_count());
    let period = x86::apic::period(counted, x86::raw::tsc() - begin, tsc_hz, HZ)?;
    x86::raw::start_apic_ticks(TIMER, period);
    // SAFETY: install put handlers for the timer and the spurious vector
    // in the IDT, the legacy PICs are masked, and nothing else is routed
    // to this processor.
    unsafe { x86::raw::enable_interrupts() };
    // The other cores' timers run at the same rate, started by a short
    // thread on each, so their threads take turns and wake on time too.
    for core in super::smp::others() {
        let _ = super::thread::spawn_on(core, tick_here, u64::from(period), 31);
    }
    Some(period)
}

/// Starts the timer of the core it runs on, interrupting every `period`
/// of its counts.
extern "C" fn tick_here(period: u64) {
    x86::raw::start_apic_ticks(TIMER, u32::try_from(period).unwrap_or(u32::MAX));
}

/// Stops the timer and the interrupts it brings.
#[cfg(feature = "qemu-test")]
pub fn stop() {
    x86::raw::disable_interrupts();
    x86::raw::stop_apic_timer();
}
