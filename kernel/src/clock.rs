//! Time without interrupts: the processor's time stamp counter, measured
//! against the ACPI power management timer, which ticks at a known rate.

use core::fmt::{self, Write};

use acpi::PM_TIMER_HZ;

/// Ticks of a counter `bits` wide from `earlier` to `later`, across at
/// most one wraparound.
#[must_use]
pub fn ticks_between(earlier: u32, later: u32, bits: u32) -> u32 {
    let mask = if bits >= 32 {
        u32::MAX
    } else {
        (1 << bits) - 1
    };
    later.wrapping_sub(earlier) & mask
}

/// The time stamp counter's frequency, from `tsc` of its ticks counted
/// while the power management timer counted `pm` of its own.
#[must_use]
pub fn tsc_hz(tsc: u64, pm: u32) -> Option<u64> {
    if pm == 0 {
        return None;
    }
    u64::try_from(u128::from(tsc) * u128::from(PM_TIMER_HZ) / u128::from(pm)).ok()
}

/// The uptime to draw again, when the second it shows has passed.
#[must_use]
pub fn to_redraw(shown: u64, now: u64) -> Option<u64> {
    (now != shown).then_some(now)
}

/// Writes a time span the way the status screen shows how long the
/// machine has been up: the two largest units, such as `1 h 5 min`.
///
/// # Errors
///
/// Fails when the writer does.
pub fn span(seconds: u64, out: &mut impl Write) -> fmt::Result {
    let (days, hours, minutes, secs) = (
        seconds / 86_400,
        seconds / 3_600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    match (days, hours, minutes) {
        (0, 0, 0) => write!(out, "{secs} s"),
        (0, 0, _) => write!(out, "{minutes} min {secs} s"),
        (0, _, _) => write!(out, "{hours} h {minutes} min"),
        _ => write!(out, "{days} d {hours} h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_wrap_around_their_width() {
        assert_eq!(ticks_between(100, 350, 24), 250);
        assert_eq!(
            ticks_between(0xff_fff0, 0x10, 24),
            0x20,
            "across the 24 bit wrap"
        );
        assert_eq!(
            ticks_between(0xffff_fff0, 0x10, 32),
            0x20,
            "across the 32 bit wrap"
        );
        assert_eq!(
            ticks_between(0x1ff_fff0, 0x10, 24),
            0x20,
            "bits above the width are ignored"
        );
        assert_eq!(ticks_between(5, 5, 24), 0);
    }

    #[test]
    fn the_counter_frequency_scales_by_the_timer() {
        // 10 ms of the timer while the counter ran 30 million ticks: 3 GHz.
        assert_eq!(tsc_hz(30_000_000, 35_795), Some(3_000_037_714));
        assert_eq!(
            tsc_hz(PM_TIMER_HZ, u32::try_from(PM_TIMER_HZ).unwrap()),
            Some(PM_TIMER_HZ)
        );
        assert_eq!(tsc_hz(1_000, 0), None, "no time passed");
        assert_eq!(tsc_hz(u64::MAX, 1), None, "too fast to be true");
    }

    fn text(seconds: u64) -> String {
        let mut out = String::new();
        span(seconds, &mut out).unwrap();
        out
    }

    #[test]
    fn spans_show_their_two_largest_units() {
        assert_eq!(text(0), "0 s");
        assert_eq!(text(59), "59 s");
        assert_eq!(text(60), "1 min 0 s");
        assert_eq!(text(3_599), "59 min 59 s");
        assert_eq!(text(3_600), "1 h 0 min");
        assert_eq!(text(3_600 + 5 * 60 + 9), "1 h 5 min");
        assert_eq!(text(86_399), "23 h 59 min");
        assert_eq!(text(86_400), "1 d 0 h");
        assert_eq!(text(3 * 86_400 + 7 * 3_600 + 59), "3 d 7 h");
        assert_eq!(
            text(u64::MAX),
            format!("{} d {} h", u64::MAX / 86_400, u64::MAX / 3_600 % 24)
        );
    }

    #[test]
    fn the_uptime_is_drawn_again_when_its_second_passed() {
        assert_eq!(to_redraw(4, 4), None);
        assert_eq!(to_redraw(4, 5), Some(5));
        assert_eq!(to_redraw(0, u64::MAX), Some(u64::MAX));
    }
}
