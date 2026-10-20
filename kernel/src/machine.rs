//! What the kernel learns about the machine from its ACPI tables, and the
//! speed of its time stamp counter.

use acpi::{Memory, PM_TIMER_HZ};
use core::fmt;

use crate::clock::{ticks_between, tsc_hz};

/// What the tables say, as far as the kernel uses it yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    /// Processor cores that are enabled or can be brought online.
    pub cores: Option<u32>,
    /// The power management timer's port and how many bits it counts in.
    pub pm_timer: Option<(u16, u32)>,
    /// The physical address of the HPET's registers.
    pub hpet: Option<u64>,
    /// Where the configuration space of PCI segment 0 lies.
    pub ecam: Option<acpi::Ecam>,
    /// The first I/O APIC, which takes the interrupts of devices.
    pub io_apic: Option<acpi::IoApic>,
    /// Where the keyboard's interrupt, ISA's first, arrives at it.
    pub keyboard: Option<acpi::Route>,
    /// Whether the FADT says there is an 8042, the PS/2 controller. Some
    /// firmware says no where there is one, so the kernel looks anyway.
    pub i8042: Option<bool>,
}

/// A clock the time stamp counter is measured against: how often it ticks
/// a second, and how many bits it counts in before it wraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    pub hz: u64,
    pub bits: u32,
}

/// The clocks the time stamp counter can be measured against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    AcpiTimer,
    Hpet,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Source::AcpiTimer => "the ACPI timer",
            Source::Hpet => "the HPET",
        })
    }
}

/// The time stamp counter's rate, and the clock it was measured against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measured {
    pub hz: u64,
    pub against: Source,
}

/// Reads the MADT and the FADT through the root pointer at `rsdp`. What
/// cannot be read stays `None`.
pub fn read(memory: &impl Memory, rsdp: u64) -> Machine {
    let Ok(xsdt) = acpi::xsdt_address(memory, rsdp) else {
        return Machine::default();
    };
    let madt = madt(memory, rsdp);
    let cores = madt
        .and_then(|t| acpi::processors(t).ok())
        .map(|p| p.usable);
    let io_apic = madt.and_then(|t| {
        let mut first = None;
        acpi::each_io_apic(t, |a| {
            first.get_or_insert(a);
        })
        .ok()?;
        first
    });
    let keyboard = madt.and_then(|t| acpi::isa_route(t, 1).ok());
    let fadt = acpi::find(memory, xsdt, b"FACP")
        .and_then(|at| acpi::table(memory, at, b"FACP"))
        .ok();
    let pm_timer =
        fadt.and_then(|fadt| Some((acpi::pm_timer_port(fadt)?, acpi::pm_timer_bits(fadt))));
    let i8042 = fadt.and_then(acpi::has_8042);
    let hpet = acpi::find(memory, xsdt, b"HPET")
        .and_then(|at| acpi::table(memory, at, b"HPET"))
        .ok()
        .and_then(acpi::hpet_address);
    let ecam = acpi::find(memory, xsdt, b"MCFG")
        .and_then(|at| acpi::table(memory, at, b"MCFG"))
        .ok()
        .and_then(|mcfg| acpi::ecam_regions(mcfg).find(|e| e.segment == 0));
    Machine {
        cores,
        pm_timer,
        hpet,
        ecam,
        io_apic,
        keyboard,
        i8042,
    }
}

/// The MADT, which lists the processors, when the ACPI tables have one.
pub fn madt(memory: &impl Memory, rsdp: u64) -> Option<&[u8]> {
    let xsdt = acpi::xsdt_address(memory, rsdp).ok()?;
    let at = acpi::find(memory, xsdt, b"APIC").ok()?;
    acpi::table(memory, at, b"APIC").ok()
}

/// The most times the timer is read while waiting for 10 ms to pass; a
/// timer that does not move gives up instead of hanging the kernel.
pub const PATIENCE: u32 = 2_000_000;

/// Measures the time stamp counter against the power management timer
/// for 10 ms, and returns its frequency. `pm` reads the timer, which
/// counts in `bits`, and `tsc` reads the counter.
pub fn calibrate(mut pm: impl FnMut() -> u32, bits: u32, tsc: impl FnMut() -> u64) -> Option<u64> {
    let reference = Reference {
        hz: PM_TIMER_HZ,
        bits,
    };
    calibrate_against(reference, || u64::from(pm()), tsc)
}

/// The time stamp counter's rate measured against the ACPI timer, when that
/// gave `acpi_timer`, or else against the HPET, which `hpet` measures.
pub fn measure(acpi_timer: Option<u64>, hpet: impl FnOnce() -> Option<u64>) -> Option<Measured> {
    match acpi_timer {
        Some(hz) => Some(Measured {
            hz,
            against: Source::AcpiTimer,
        }),
        None => hpet().map(|hz| Measured {
            hz,
            against: Source::Hpet,
        }),
    }
}

/// Measures the time stamp counter against `reference` for 10 ms, and
/// returns its frequency. `read` reads the reference clock and `tsc` the
/// counter.
pub fn calibrate_against(
    reference: Reference,
    mut read: impl FnMut() -> u64,
    mut tsc: impl FnMut() -> u64,
) -> Option<u64> {
    let wanted = reference.hz / 100;
    let (start, tsc_start) = (read(), tsc());
    for _ in 0..PATIENCE {
        let elapsed = ticks_between(start, read(), reference.bits);
        if elapsed >= wanted {
            return tsc_hz(tsc().wrapping_sub(tsc_start), elapsed, reference.hz);
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

    /// Where ISA puts its first interrupt when no override moves it.
    const ISA_FIRST: acpi::Route = acpi::Route {
        interrupt: 1,
        active_low: false,
        level: false,
    };

    #[test]
    fn the_io_apic_and_the_keyboards_line_come_from_the_madt() {
        let mut body = vec![0, 0, 0xe0, 0xfe, 1, 0, 0, 0];
        body.extend([0, 8, 0, 0, 1, 0, 0, 0]);
        body.extend([1, 12, 4, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0]);
        body.extend([1, 12, 5, 0, 0, 0x10, 0xc0, 0xfe, 24, 0, 0, 0]);
        body.extend([2, 10, 0, 1, 17, 0, 0, 0, 0x0f, 0]);
        let ram = tables(&[sdt(*b"APIC", &body)]);
        let m = read(&ram, 0x1000);
        assert_eq!(
            m.io_apic,
            Some(acpi::IoApic {
                id: 4,
                address: 0xfec0_0000,
                first: 0
            }),
            "the first of two"
        );
        assert_eq!(
            m.keyboard,
            Some(acpi::Route {
                interrupt: 17,
                active_low: true,
                level: true
            }),
            "moved by an override"
        );
        let mut broken = vec![0, 0, 0xe0, 0xfe, 1, 0, 0, 0];
        broken.extend([1, 8, 4, 0, 0, 0, 0xc0, 0xfe]);
        let ram = tables(&[sdt(*b"APIC", &broken)]);
        assert_eq!(read(&ram, 0x1000).io_apic, None, "an entry cut short");
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
                hpet: None,
                ecam: None,
                io_apic: None,
                keyboard: Some(ISA_FIRST),
                i8042: None,
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
                pm_timer: None,
                hpet: None,
                ecam: None,
                io_apic: None,
                keyboard: Some(ISA_FIRST),
                i8042: None,
            }
        );
        let ram = tables(&[fadt(0x408, 0)]);
        assert_eq!(
            read(&ram, 0x1000),
            Machine {
                cores: None,
                pm_timer: Some((0x408, 24)),
                hpet: None,
                ecam: None,
                io_apic: None,
                keyboard: None,
                i8042: None,
            }
        );
        let ram = tables(&[fadt(0, 0)]);
        assert_eq!(read(&ram, 0x1000).pm_timer, None, "an FADT without a timer");
    }

    #[test]
    fn the_fadt_says_whether_there_is_a_ps2_controller() {
        let with = |revision: u8, boot: u8| {
            let mut t = fadt(0x608, 0);
            t[8] = revision;
            t[109] = boot;
            fix(&mut t, 9);
            read(&tables(&[t]), 0x1000).i8042
        };
        assert_eq!(with(3, 0b10), Some(true), "as q35 has it");
        assert_eq!(with(3, 0), Some(false), "and as it has it with i8042=off");
        assert_eq!(with(1, 0b10), None, "too old a FADT to say");
        assert_eq!(read(&tables(&[madt(1)]), 0x1000).i8042, None, "no FADT");
    }

    fn hpet(address: u64) -> Vec<u8> {
        let mut body = vec![0u8; 20];
        body[8..16].copy_from_slice(&address.to_le_bytes());
        sdt(*b"HPET", &body)
    }

    fn mcfg(entries: &[(u64, u16, u8, u8)]) -> Vec<u8> {
        let mut body = vec![0u8; 8];
        for &(base, segment, first, last) in entries {
            body.extend_from_slice(&base.to_le_bytes());
            body.extend_from_slice(&segment.to_le_bytes());
            body.extend_from_slice(&[first, last, 0, 0, 0, 0]);
        }
        sdt(*b"MCFG", &body)
    }

    #[test]
    fn pci_comes_from_the_mcfg_of_segment_0() {
        let ram = tables(&[
            madt(2),
            mcfg(&[(0x1_0000_0000, 1, 0, 0xff), (0xb000_0000, 0, 0, 0xff)]),
        ]);
        assert_eq!(
            read(&ram, 0x1000).ecam,
            Some(acpi::Ecam {
                base: 0xb000_0000,
                segment: 0,
                first_bus: 0,
                last_bus: 0xff
            })
        );
        let other = tables(&[mcfg(&[(0x1_0000_0000, 1, 0, 0xff)])]);
        assert_eq!(read(&other, 0x1000).ecam, None, "only another segment");
        assert_eq!(read(&tables(&[madt(2)]), 0x1000).ecam, None, "no MCFG");
    }

    #[test]
    fn the_hpet_comes_from_its_table() {
        let ram = tables(&[madt(2), hpet(0xfed0_0000)]);
        assert_eq!(read(&ram, 0x1000).hpet, Some(0xfed0_0000));
        assert_eq!(read(&ram, 0x1000).cores, Some(2));
        let ram = tables(&[hpet(0)]);
        assert_eq!(
            read(&ram, 0x1000).hpet,
            None,
            "an HPET table without an address"
        );
        let mut broken = tables(&[hpet(0xfed0_0000)]);
        let at = broken.bytes.len() - 20;
        broken.bytes[at] ^= 1;
        assert_eq!(
            read(&broken, 0x1000).hpet,
            None,
            "a table whose checksum is wrong"
        );
    }

    #[test]
    fn the_acpi_timer_comes_first_and_the_hpet_after() {
        let unasked = || -> Option<u64> { panic!("the HPET is only asked without the ACPI timer") };
        assert_eq!(
            measure(Some(3_000_000_000), unasked),
            Some(Measured {
                hz: 3_000_000_000,
                against: Source::AcpiTimer
            })
        );
        assert_eq!(
            measure(None, || Some(3_549_000_000)),
            Some(Measured {
                hz: 3_549_000_000,
                against: Source::Hpet
            })
        );
        assert_eq!(measure(None, || None), None);
    }

    #[test]
    fn the_clocks_name_themselves() {
        assert_eq!(Source::AcpiTimer.to_string(), "the ACPI timer");
        assert_eq!(Source::Hpet.to_string(), "the HPET");
    }

    #[test]
    fn calibration_works_against_a_wide_fast_clock() {
        // A 100 MHz HPET that counts in 64 bits, read every microsecond,
        // near the top of its range, while the counter runs at 3.2 GHz.
        let now = Cell::new(0u64);
        let hpet = || {
            now.set(now.get() + 1_000);
            (u64::MAX - 50_000).wrapping_add(now.get() / 10)
        };
        let tsc = || now.get() * 32 / 10;
        let reference = Reference {
            hz: 100_000_000,
            bits: 64,
        };
        let hz = calibrate_against(reference, hpet, tsc).unwrap();
        assert!((3_199_000_000..3_201_000_000).contains(&hz), "{hz}");
        assert!(
            (10_000_000..10_002_000).contains(&now.get()),
            "took {} ns",
            now.get()
        );
        let still = Reference { hz: 100, bits: 64 };
        assert_eq!(
            calibrate_against(still, || 7, || 1),
            None,
            "a clock too slow to measure 10 ms with"
        );
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
