//! The parts of the kernel that are plain computation, so they are tested
//! on the build machine. `main.rs` runs them on the real thing.

#![cfg_attr(not(test), no_std)]

use core::fmt::{self, Write};

use bootinfo::{MemoryRegion, RegionKind};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How many bytes of memory are of `kind`.
#[must_use]
pub fn bytes_of(regions: &[MemoryRegion], kind: RegionKind) -> u64 {
    regions
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.pages * MemoryRegion::PAGE_SIZE)
        .sum()
}

/// Whether the regions are sorted by address and none overlaps the next.
#[must_use]
pub fn is_tidy(regions: &[MemoryRegion]) -> bool {
    regions.windows(2).all(|w| w[0].end() <= w[1].start)
}

/// Whether one region of `kind` holds all of `len` bytes from `start`.
/// The loader merges neighbours of the same kind, so one is enough.
#[must_use]
pub fn covered(regions: &[MemoryRegion], kind: RegionKind, start: u64, len: u64) -> bool {
    regions
        .iter()
        .any(|r| r.kind == kind && r.start <= start && start.saturating_add(len) <= r.end())
}

/// A number of bytes, written in the largest binary unit it reaches, with
/// one decimal when that is not a whole number: `1.5 GiB`, `512 MiB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size(pub u64);

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
        let (mut whole, mut rest, mut unit) = (self.0, 0, 0);
        while whole >= 1024 && unit + 1 < UNITS.len() {
            rest = whole % 1024;
            whole /= 1024;
            unit += 1;
        }
        let tenth = rest * 10 / 1024;
        if tenth == 0 {
            write!(f, "{whole} {}", UNITS[unit])
        } else {
            write!(f, "{whole}.{tenth} {}", UNITS[unit])
        }
    }
}

/// The first lines the kernel prints.
///
/// # Errors
///
/// Fails when the writer does.
pub fn greet(out: &mut impl Write, regions: &[MemoryRegion]) -> fmt::Result {
    writeln!(out, "Meridian kernel {VERSION}")?;
    writeln!(
        out,
        "Memory: {} free, {} taken by the kernel, {} regions",
        Size(bytes_of(regions, RegionKind::USABLE)),
        Size(bytes_of(regions, RegionKind::KERNEL)),
        regions.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(start: u64, pages: u64, kind: RegionKind) -> MemoryRegion {
        MemoryRegion {
            start,
            pages,
            kind,
            reserved: 0,
        }
    }

    fn sample() -> Vec<MemoryRegion> {
        vec![
            region(0, 0x9f, RegionKind::USABLE),
            region(0x10_0000, 0x100, RegionKind::KERNEL),
            region(0x20_0000, 0x1_0000, RegionKind::USABLE),
            region(0x1020_0000, 4, RegionKind::ACPI_RECLAIMABLE),
            region(0xfee0_0000, 1, RegionKind::RESERVED),
        ]
    }

    #[test]
    fn counts_bytes_per_kind() {
        let r = sample();
        assert_eq!(bytes_of(&r, RegionKind::USABLE), (0x9f + 0x1_0000) * 4096);
        assert_eq!(bytes_of(&r, RegionKind::KERNEL), 0x100 * 4096);
        assert_eq!(bytes_of(&r, RegionKind::UNUSABLE), 0);
        assert_eq!(bytes_of(&[], RegionKind::USABLE), 0);
    }

    #[test]
    fn tidy_means_sorted_without_overlap() {
        let mut r = sample();
        assert!(is_tidy(&r));
        assert!(is_tidy(&[]));
        assert!(is_tidy(&r[..1]));

        let touching = [
            region(0, 1, RegionKind::USABLE),
            region(0x1000, 1, RegionKind::KERNEL),
        ];
        assert!(is_tidy(&touching));
        let overlapping = [
            region(0, 2, RegionKind::USABLE),
            region(0x1000, 1, RegionKind::KERNEL),
        ];
        assert!(!is_tidy(&overlapping));

        r.swap(1, 2);
        assert!(!is_tidy(&r));
    }

    #[test]
    fn coverage_needs_one_region_of_the_kind() {
        let r = sample();
        assert!(covered(&r, RegionKind::KERNEL, 0x10_0000, 0x10_0000));
        assert!(covered(&r, RegionKind::KERNEL, 0x18_0000, 0x1000));
        assert!(
            !covered(&r, RegionKind::KERNEL, 0x10_0000, 0x10_0001),
            "one byte past the end"
        );
        assert!(
            !covered(&r, RegionKind::KERNEL, 0xf_f000, 0x2000),
            "starts before the region"
        );
        assert!(
            !covered(&r, RegionKind::USABLE, 0x10_0000, 0x1000),
            "wrong kind"
        );
        assert!(covered(&r, RegionKind::RESERVED, 0xfee0_0000, 0));
        assert!(
            !covered(&r, RegionKind::USABLE, u64::MAX - 4, 16),
            "no wrap around"
        );
    }

    #[test]
    fn sizes_pick_the_largest_unit() {
        let cases = [
            (0, "0 bytes"),
            (1023, "1023 bytes"),
            (1024, "1 KiB"),
            (1536, "1.5 KiB"),
            (1024 * 1024 - 1, "1023.9 KiB"),
            (512 << 20, "512 MiB"),
            ((1 << 30) + (1 << 29), "1.5 GiB"),
            ((2 << 30) - 1, "1.9 GiB"),
            (3 << 40, "3 TiB"),
            (5000 << 40, "5000 TiB"),
            (u64::MAX, "16777215.9 TiB"),
        ];
        for (bytes, want) in cases {
            assert_eq!(Size(bytes).to_string(), want, "{bytes}");
        }
    }

    #[test]
    fn greeting() {
        let mut out = String::new();
        greet(&mut out, &sample()).unwrap();
        assert_eq!(
            out,
            format!(
                "Meridian kernel {VERSION}\nMemory: 256.6 MiB free, 1 MiB taken by the kernel, 5 regions\n"
            )
        );
    }

    #[test]
    fn greeting_stops_when_the_writer_fails() {
        struct Broken;
        impl Write for Broken {
            fn write_str(&mut self, _: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }
        assert!(greet(&mut Broken, &sample()).is_err());
    }
}
