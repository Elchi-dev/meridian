//! What the kernel learns about the machine from its ACPI tables, and the
//! speed of its time stamp counter.

use acpi::{Memory, PM_TIMER_HZ};

use crate::clock::{ticks_between, tsc_hz};

/// What the tables say, as far as the kernel uses it yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    /// Processor cores that are enabled or can be brought online.
    pub cores: Option<u32>,
    /// The power management timer's port and how many bits it counts in.
    pub pm_timer: Option<(u16, u32)>,
}

/// Reads the MADT and the FADT through the root pointer at `rsdp`. What
/// cannot be read stays `None`.
pub fn read(memory: &impl Memory, rsdp: u64) -> Machine {
    let Ok(xsdt) = acpi::xsdt_address(memory, rsdp) else {
        return Machine::default();
    };
    let cores = acpi::find(memory, xsdt, b"APIC")
        .and_then(|at| acpi::table(memory, at, b"APIC"))
        .and_then(acpi::processors)
        .ok()
        .map(|p| p.usable);
    let pm_timer = acpi::find(memory, xsdt, b"FACP")
        .and_then(|at| acpi::table(memory, at, b"FACP"))
        .ok()
        .and_then(|fadt| Some((acpi::pm_timer_port(fadt)?, acpi::pm_timer_bits(fadt))));
    Machine { cores, pm_timer }
}

/// The most times the timer is read while waiting for 10 ms to pass; a
/// timer that does not move gives up instead of hanging the kernel.
pub const PATIENCE: u32 = 2_000_000;

/// Measures the time stamp counter against the power management timer
/// for 10 ms, and returns its frequency. `pm` reads the timer, which
/// counts in `bits`, and `tsc` reads the counter.
pub fn calibrate(
    mut pm: impl FnMut() -> u32,
    bits: u32,
    mut tsc: impl FnMut() -> u64,
) -> Option<u64> {
    let wanted = u32::try_from(PM_TIMER_HZ / 100).unwrap_or(u32::MAX);
    let (pm_start, tsc_start) = (pm(), tsc());
    for _ in 0..PATIENCE {
        let elapsed = ticks_between(pm_start, pm(), bits);
        if elapsed >= wanted {
            return tsc_hz(tsc().wrapping_sub(tsc_start), elapsed);
        }
        core::hint::spin_loop();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Physical memory from `base` on, holding `bytes`.
    struct Ram {
        base: u64,
        bytes: Vec<u8>,
    }

    impl Memory for Ram {
        fn bytes(&self, at: u64, len: usize) -> Option<&[u8]> {
            let start = usize::try_from(at.checked_sub(self.base)?).ok()?;
            self.bytes.get(start..start.checked_add(len)?)
        }
    }

    fn fix(bytes: &mut [u8], at: usize) {
        bytes[at] = 0;
        let sum = bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        bytes[at] = sum.wrapping_neg();
    }

    fn sdt(signature: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; 36];
        t[..4].copy_from_slice(&signature);
        t[4..8].copy_from_slice(&u32::try_from(36 + body.len()).unwrap().to_le_bytes());
        t.extend_from_slice(body);
        fix(&mut t, 9);
        t
    }

    /// A machine's tables from 0x1000 on: the root pointer, the XSDT and
    /// then each table, in order.
    fn tables(listed: &[Vec<u8>]) -> Ram {
        let base = 0x1000u64;
        let mut at = base + 36 + 36 + 8 * listed.len() as u64;
        let mut list = Vec::new();
        for t in listed {
            list.extend(at.to_le_bytes());
            at += t.len() as u64;
        }
        let mut rsdp = vec![0u8; 36];
        rsdp[..8].copy_from_slice(b"RSD PTR ");
        rsdp[15] = 2;
        rsdp[20..24].copy_from_slice(&36u32.to_le_bytes());
        rsdp[24..32].copy_from_slice(&(base + 36).to_le_bytes());
        fix(&mut rsdp[..20], 8);
        fix(&mut rsdp, 32);
        let mut bytes = rsdp;
        bytes.extend(sdt(*b"XSDT", &list));
        for t in listed {
            bytes.extend(t);
        }
        Ram { base, bytes }
    }

    fn madt(cores: u8) -> Vec<u8> {
        let mut body = vec![0, 0, 0xe0, 0xfe, 1, 0, 0, 0];
        for id in 0..cores {
            body.extend([0, 8, id, id, 1, 0, 0, 0]);
        }
        sdt(*b"APIC", &body)
    }

    fn fadt(port: u32, flags: u32) -> Vec<u8> {
        let mut body = vec![0u8; 240];
        body[40..44].copy_from_slice(&port.to_le_bytes());
        body[76..80].copy_from_slice(&flags.to_le_bytes());
        sdt(*b"FACP", &body)
    }

    #[test]
    fn cores_and_timer_come_from_the_tables() {
        let ram = tables(&[fadt(0x608, 1 << 8), madt(4)]);
        assert_eq!(
            read(&ram, 0x1000),
            Machine {
                cores: Some(4),
                pm_timer: Some((0x608, 32)),
            }
        );
    }

    #[test]
    fn a_missing_table_leaves_only_its_part_out() {
        let ram = tables(&[madt(2)]);
        assert_eq!(
            read(&ram, 0x1000),
            Machine {
                cores: Some(2),
                pm_timer: None
            }
        );
        let ram = tables(&[fadt(0x408, 0)]);
        assert_eq!(
            read(&ram, 0x1000),
            Machine {
                cores: None,
                pm_timer: Some((0x408, 24))
            }
        );
        let ram = tables(&[fadt(0, 0)]);
        assert_eq!(read(&ram, 0x1000).pm_timer, None, "an FADT without a timer");
    }

    #[test]
    fn no_root_pointer_means_nothing_is_known() {
        let ram = tables(&[madt(2)]);
        assert_eq!(read(&ram, 0x9000), Machine::default());
        let mut broken = tables(&[madt(2)]);
        broken.bytes[0] = b'X';
        assert_eq!(read(&broken, 0x1000), Machine::default());
    }

    /// When the tests start measuring: the machine has been on for a while.
    const START: u64 = 7_000_000_000;

    /// A machine whose time moves `step` nanoseconds each time the timer is
    /// read; reading the counter takes no time. Its time stamp counter runs
    /// at `hz`; its timer is at `pm_start` when the test starts and wraps
    /// after `bits`.
    struct Clocks {
        ns: Cell<u64>,
        step: u64,
        hz: u64,
        pm_start: u64,
        bits: u32,
    }

    impl Clocks {
        fn new(hz: u64, pm_start: u64, bits: u32) -> Self {
            Clocks {
                ns: Cell::new(START),
                step: 1_000,
                hz,
                pm_start,
                bits,
            }
        }

        fn pm(&self) -> u32 {
            self.ns.set(self.ns.get() + self.step);
            let ticks = self.pm_start + (self.ns.get() - START) * PM_TIMER_HZ / 1_000_000_000;
            u32::try_from(ticks & ((1 << self.bits) - 1)).unwrap()
        }

        fn tsc(&self) -> u64 {
            let cycles = u128::from(self.ns.get()) * u128::from(self.hz) / 1_000_000_000;
            u64::try_from(cycles).unwrap()
        }
    }

    fn calibrated(clocks: &Clocks) -> Option<u64> {
        calibrate(|| clocks.pm(), clocks.bits, || clocks.tsc())
    }

    #[test]
    fn calibration_measures_10_ms() {
        let clocks = Clocks::new(2_980_000_000, 100, 24);
        let hz = calibrated(&clocks).unwrap();
        assert!((2_977_000_000..2_983_000_000).contains(&hz), "{hz}");
        let took = clocks.ns.get() - START;
        assert!((10_000_000..10_010_000).contains(&took), "took {took} ns");
    }

    #[test]
    fn calibration_counts_the_ticks_that_passed() {
        // Reads 300 µs apart overshoot 10 ms by 2%, which the rate has to
        // take into account.
        let clocks = Clocks {
            step: 300_000,
            ..Clocks::new(2_000_000_000, 0, 24)
        };
        let hz = calibrated(&clocks).unwrap();
        assert!((1_999_800_000..2_000_200_000).contains(&hz), "{hz}");
        assert_eq!(
            clocks.ns.get() - START,
            35 * 300_000,
            "the first read, then 34 more until 10 ms passed"
        );
    }

    #[test]
    fn calibration_survives_the_timer_wrapping() {
        let clocks = Clocks::new(3_500_000_000, 0xff_ff00, 24);
        let hz = calibrated(&clocks).unwrap();
        assert!((3_496_000_000..3_504_000_000).contains(&hz), "{hz}");
        let clocks = Clocks::new(1_000_000_000, 0xffff_ff00, 32);
        let hz = calibrated(&clocks).unwrap();
        assert!((999_000_000..1_001_000_000).contains(&hz), "{hz}");
    }

    #[test]
    fn a_timer_that_stands_still_gives_up() {
        let reads = Cell::new(0u32);
        let tsc = || 1u64;
        assert_eq!(
            calibrate(
                || {
                    reads.set(reads.get() + 1);
                    42
                },
                24,
                tsc
            ),
            None
        );
        assert_eq!(reads.get(), PATIENCE + 1);
    }

    #[test]
    fn calibration_stops_on_the_tick_that_makes_10_ms() {
        // A timer that moves one tick a read reaches 10 ms on exactly the
        // read that brings the count to it.
        let reads = Cell::new(0u32);
        let pm = || {
            reads.set(reads.get() + 1);
            reads.get()
        };
        let wanted = u32::try_from(PM_TIMER_HZ / 100).unwrap();
        let hz = calibrate(pm, 32, || u64::from(reads.get()) * 1000);
        assert_eq!(reads.get(), 1 + wanted, "the first read, then one per tick");
        assert_eq!(hz, Some(1000 * PM_TIMER_HZ));
    }
}
