//! The High Precision Event Timer's registers, as the IA-PC HPET
//! specification 1.0a lays them out, and what they say about its main
//! counter.

use crate::machine::Reference;

/// The general capabilities and id register.
pub const CAPABILITIES: u64 = 0x000;
/// The general configuration register.
pub const CONFIGURATION: u64 = 0x010;
/// The main counter.
pub const COUNTER: u64 = 0x0f0;
/// The configuration bit that lets the main counter run.
pub const ENABLE: u64 = 1;

/// The longest period between two ticks the specification allows: 100 ns,
/// in femtoseconds.
const LONGEST_PERIOD: u64 = 100_000_000;

/// The main counter as a reference clock, from the capabilities register:
/// its period in femtoseconds in the upper half, whether it counts in 64
/// bits in bit 13, and its revision, never 0, in the lowest byte. `None`
/// for one the specification does not allow.
#[must_use]
pub fn counter(capabilities: u64) -> Option<Reference> {
    let (period, revision) = (capabilities >> 32, capabilities & 0xff);
    if revision == 0 || period == 0 || period > LONGEST_PERIOD {
        return None;
    }
    let bits = if capabilities & 1 << 13 == 0 { 32 } else { 64 };
    Some(Reference {
        hz: 1_000_000_000_000_000 / period,
        bits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(period: u64, wide: bool, revision: u64) -> u64 {
        period << 32 | 0x8086 << 16 | u64::from(wide) << 13 | 2 << 8 | revision
    }

    #[test]
    fn the_counter_runs_at_its_period() {
        // QEMU's HPET ticks every 10 ns in 64 bits.
        assert_eq!(
            counter(capabilities(10_000_000, true, 1)),
            Some(Reference {
                hz: 100_000_000,
                bits: 64
            })
        );
        // Intel's chipsets tick at 14.318 MHz, some only in 32 bits.
        assert_eq!(
            counter(capabilities(69_841_279, false, 1)),
            Some(Reference {
                hz: 14_318_179,
                bits: 32
            })
        );
        assert_eq!(
            counter(capabilities(LONGEST_PERIOD, true, 0xff)),
            Some(Reference {
                hz: 10_000_000,
                bits: 64
            }),
            "the slowest the specification allows"
        );
    }

    #[test]
    fn counters_the_specification_does_not_allow_are_refused() {
        assert_eq!(
            counter(capabilities(10_000_000, true, 0)),
            None,
            "revision 0"
        );
        assert_eq!(counter(capabilities(0, true, 1)), None, "no period");
        assert_eq!(
            counter(capabilities(LONGEST_PERIOD + 1, true, 1)),
            None,
            "too slow"
        );
        assert_eq!(counter(0), None);
        assert_eq!(counter(u64::MAX), None);
    }

    #[test]
    fn the_registers_are_where_the_specification_puts_them() {
        assert_eq!(
            (CAPABILITIES, CONFIGURATION, COUNTER, ENABLE),
            (0, 0x10, 0xf0, 1)
        );
    }
}
