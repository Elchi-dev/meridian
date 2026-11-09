//! The time of day, counted on from the moment the loader read it
//! (decision record 0028).

use bootinfo::Clock;

use super::memory::Spin;

/// That moment, and the counter's rate, once the kernel measured it.
static WALL: Spin<Option<(Clock, u64)>> = Spin::new(None);

/// Keeps the moment `clock` the loader handed over, to count on from at
/// `tsc_hz`; without a rate the time stays unknown.
pub fn keep(clock: Clock, tsc_hz: Option<u64>) {
    *WALL.lock() = tsc_hz.map(|hz| (clock, hz));
}

/// The whole seconds since 1970 began, in UTC; none while the time is not
/// known.
pub fn seconds() -> Option<u64> {
    now().map(|nanoseconds| nanoseconds / 1_000_000_000)
}

/// The nanoseconds since 1970 began, in UTC; none while the time is not
/// known.
pub fn now() -> Option<u64> {
    let wall = *WALL.lock();
    let (clock, hz) = wall?;
    kernel::clock::wall(&clock, hz, x86::raw::tsc())
}
