//! Turns the firmware's memory map into the regions the kernel gets.

use bootinfo::{KERNEL_MEMORY_TYPE, MemoryRegion, RegionKind};
use uefi::memory::{MemoryDescriptor, MemoryType};

/// What a firmware memory type becomes once boot services are gone. Memory
/// the loader used for itself is free again; what the kernel keeps was
/// allocated with [`KERNEL_MEMORY_TYPE`].
#[must_use]
pub fn kind_of(ty: MemoryType) -> RegionKind {
    match ty {
        MemoryType::CONVENTIONAL
        | MemoryType::BOOT_SERVICES_CODE
        | MemoryType::BOOT_SERVICES_DATA
        | MemoryType::LOADER_CODE
        | MemoryType::LOADER_DATA => RegionKind::USABLE,
        MemoryType::ACPI_RECLAIM => RegionKind::ACPI_RECLAIMABLE,
        MemoryType::ACPI_NVS => RegionKind::ACPI_NVS,
        MemoryType::UNUSABLE => RegionKind::UNUSABLE,
        MemoryType(KERNEL_MEMORY_TYPE) => RegionKind::KERNEL,
        _ => RegionKind::RESERVED,
    }
}

/// The output buffer cannot hold every region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooSmall {
    pub needed: usize,
}

/// Converts descriptors into regions sorted by address, with neighbours of
/// the same kind merged into one. Empty descriptors are skipped.
///
/// Firmware maps are not always tidy, and where two descriptors overlap the
/// safe reading wins: usable memory loses the overlap to any other kind, and
/// a usable region cut by one in its middle also loses its tail. Handing the
/// kernel reserved memory as free would be far worse than losing some.
///
/// # Errors
///
/// Returns how many regions would have been needed when `out` is too small.
pub fn convert(
    descriptors: impl Iterator<Item = MemoryDescriptor>,
    out: &mut [MemoryRegion],
) -> Result<usize, TooSmall> {
    let mut count = 0;
    let mut needed = 0;
    for d in descriptors.filter(|d| d.pages > 0) {
        needed += 1;
        if count == out.len() {
            continue;
        }
        let region = MemoryRegion {
            start: d.physical_start,
            pages: d.pages,
            kind: kind_of(d.ty),
            reserved: 0,
        };
        let mut at = count;
        while at > 0 && goes_after(region, out[at - 1]) {
            out[at] = out[at - 1];
            at -= 1;
        }
        out[at] = region;
        count += 1;
    }
    if needed > out.len() {
        return Err(TooSmall { needed });
    }
    Ok(tidy(&mut out[..count]))
}

/// Whether `b` sorts after `a`: by start, and at the same start usable
/// memory last, so the stricter kind comes first and keeps the overlap.
fn goes_after(a: MemoryRegion, b: MemoryRegion) -> bool {
    b.start > a.start || (b.start == a.start && b.kind == RegionKind::USABLE)
}

/// Resolves overlaps and merges neighbours in a sorted slice, in place, and
/// returns how many regions remain at its start. Each rule is written so it
/// changes nothing when two regions do not overlap.
fn tidy(regions: &mut [MemoryRegion]) -> usize {
    const PAGE: u64 = MemoryRegion::PAGE_SIZE;
    let mut kept = 0;
    for i in 0..regions.len() {
        let mut r = regions[i];
        if kept > 0 {
            let last = &mut regions[kept - 1];
            if r.kind == last.kind && r.start <= last.end() {
                last.pages = (r.end().max(last.end()) - last.start) / PAGE;
                continue;
            }
            if last.kind == RegionKind::USABLE {
                // Sorting put stricter kinds first at equal starts, so r
                // starts after last and last keeps at least a page.
                last.pages = last.pages.min((r.start - last.start) / PAGE);
            } else {
                let cut = r.pages.min(last.end().saturating_sub(r.start) / PAGE);
                r.start += cut * PAGE;
                r.pages -= cut;
                if r.pages == 0 {
                    continue;
                }
            }
        }
        regions[kept] = r;
        kept += 1;
    }
    kept
}

/// Where memory ends in the firmware's map: the end of the last descriptor
/// that is not reserved. Reserved ranges above it are devices, such as
/// PCI windows far beyond all memory, which the direct map leaves out.
#[must_use]
pub fn memory_top(descriptors: impl Iterator<Item = MemoryDescriptor>) -> u64 {
    descriptors
        .filter(|d| kind_of(d.ty) != RegionKind::RESERVED)
        .map(|d| d.end())
        .max()
        .unwrap_or(0)
}

/// The highest physical address any region reaches, which is how far the
/// direct map has to go.
#[must_use]
pub fn top(regions: &[MemoryRegion]) -> u64 {
    regions.iter().map(MemoryRegion::end).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_ends_before_the_device_windows() {
        let map = [
            desc(MemoryType::CONVENTIONAL, 0, 0xa0),
            desc(MemoryType::BOOT_SERVICES_DATA, 0x10_0000, 0x1_0000),
            desc(MemoryType::ACPI_NVS, 0x1ff7_8000, 0x88),
            desc(MemoryType::RESERVED, 0xe000_0000, 0x1_0000),
            desc(MemoryType::MMIO, 0xffc0_0000, 0x400),
            desc(MemoryType::RESERVED, 0xfd_0000_0000, 0x30_0000),
        ];
        assert_eq!(memory_top(map.into_iter()), 0x2000_0000);
        assert_eq!(memory_top([map[0]].into_iter()), 0xa_0000);
        assert_eq!(memory_top(map[3..].iter().copied()), 0, "devices only");
        assert_eq!(memory_top(core::iter::empty()), 0);
        let runtime = desc(MemoryType::RUNTIME_SERVICES_DATA, 0x3000_0000, 1);
        assert_eq!(
            memory_top([map[0], runtime].into_iter()),
            0xa_0000,
            "runtime memory is reserved too"
        );
    }

    fn desc(ty: MemoryType, start: u64, pages: u64) -> MemoryDescriptor {
        MemoryDescriptor {
            ty,
            physical_start: start,
            virtual_start: 0,
            pages,
            attribute: 0,
        }
    }

    fn region(kind: RegionKind, start: u64, pages: u64) -> MemoryRegion {
        MemoryRegion {
            start,
            pages,
            kind,
            reserved: 0,
        }
    }

    fn run(descs: &[MemoryDescriptor]) -> Vec<MemoryRegion> {
        let mut out = vec![region(RegionKind(0), 0, 0); descs.len()];
        let n = convert(descs.iter().copied(), &mut out).expect("enough room");
        out.truncate(n);
        out
    }

    const PAGE: u64 = 4096;

    #[test]
    fn every_firmware_type_has_a_kind() {
        let usable = [1, 2, 3, 4, 7];
        for t in 0..16u32 {
            let kind = kind_of(MemoryType(t));
            let want = match t {
                _ if usable.contains(&t) => RegionKind::USABLE,
                9 => RegionKind::ACPI_RECLAIMABLE,
                10 => RegionKind::ACPI_NVS,
                8 => RegionKind::UNUSABLE,
                _ => RegionKind::RESERVED,
            };
            assert_eq!(kind, want, "type {t}");
        }
        assert_eq!(kind_of(MemoryType(KERNEL_MEMORY_TYPE)), RegionKind::KERNEL);
        assert_eq!(
            kind_of(MemoryType(KERNEL_MEMORY_TYPE + 1)),
            RegionKind::RESERVED
        );
        assert_eq!(kind_of(MemoryType(u32::MAX)), RegionKind::RESERVED);
    }

    #[test]
    fn sorts_by_address() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 0x30_0000, 1),
            desc(MemoryType::ACPI_NVS, 0x10_0000, 1),
            desc(MemoryType::RESERVED, 0x20_0000, 1),
        ]);
        let starts: Vec<u64> = got.iter().map(|r| r.start).collect();
        assert_eq!(starts, [0x10_0000, 0x20_0000, 0x30_0000]);
        assert_eq!(got[0].kind, RegionKind::ACPI_NVS);
    }

    #[test]
    fn merges_neighbours_of_the_same_kind() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 0, 16),
            desc(MemoryType::BOOT_SERVICES_DATA, 16 * PAGE, 4),
            desc(MemoryType::LOADER_CODE, 20 * PAGE, 2),
            desc(MemoryType::ACPI_RECLAIM, 22 * PAGE, 1),
            desc(MemoryType::CONVENTIONAL, 23 * PAGE, 1),
        ]);
        assert_eq!(
            got,
            [
                region(RegionKind::USABLE, 0, 22),
                region(RegionKind::ACPI_RECLAIMABLE, 22 * PAGE, 1),
                region(RegionKind::USABLE, 23 * PAGE, 1),
            ]
        );
    }

    #[test]
    fn a_gap_keeps_regions_apart() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 0, 1),
            desc(MemoryType::CONVENTIONAL, 2 * PAGE, 1),
        ]);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn usable_memory_loses_overlaps_with_reserved_memory_before_it() {
        let got = run(&[
            desc(MemoryType::RESERVED, 0, 10),
            desc(MemoryType::CONVENTIONAL, 8 * PAGE, 10),
            desc(MemoryType::CONVENTIONAL, 2 * PAGE, 3),
        ]);
        assert_eq!(
            got,
            [
                region(RegionKind::RESERVED, 0, 10),
                region(RegionKind::USABLE, 10 * PAGE, 8),
            ]
        );
    }

    #[test]
    fn usable_memory_loses_overlaps_with_reserved_memory_inside_it() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 0, 16),
            desc(MemoryType::RESERVED, 4 * PAGE, 2),
        ]);
        assert_eq!(
            got,
            [
                region(RegionKind::USABLE, 0, 4),
                region(RegionKind::RESERVED, 4 * PAGE, 2),
            ]
        );
    }

    #[test]
    fn usable_memory_at_the_same_start_keeps_its_rest() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 4 * PAGE, 4),
            desc(MemoryType::ACPI_NVS, 4 * PAGE, 1),
        ]);
        assert_eq!(
            got,
            [
                region(RegionKind::ACPI_NVS, 4 * PAGE, 1),
                region(RegionKind::USABLE, 5 * PAGE, 3),
            ]
        );
    }

    #[test]
    fn two_strict_kinds_keep_the_first() {
        let got = run(&[
            desc(MemoryType::ACPI_NVS, 0, 4),
            desc(MemoryType::RESERVED, 2 * PAGE, 4),
        ]);
        assert_eq!(
            got,
            [
                region(RegionKind::ACPI_NVS, 0, 4),
                region(RegionKind::RESERVED, 4 * PAGE, 2),
            ]
        );
    }

    #[test]
    fn an_overlap_of_the_same_kind_merges() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 0, 4),
            desc(MemoryType::CONVENTIONAL, 2 * PAGE, 4),
        ]);
        assert_eq!(got, [region(RegionKind::USABLE, 0, 6)]);
        let inside = run(&[
            desc(MemoryType::RESERVED, 0, 8),
            desc(MemoryType::RESERVED, 2 * PAGE, 2),
        ]);
        assert_eq!(inside, [region(RegionKind::RESERVED, 0, 8)]);
    }

    #[test]
    fn at_the_same_start_the_stricter_kind_wins_in_any_order() {
        for descs in [
            [
                desc(MemoryType::ACPI_NVS, PAGE, 2),
                desc(MemoryType::CONVENTIONAL, PAGE, 2),
            ],
            [
                desc(MemoryType::CONVENTIONAL, PAGE, 2),
                desc(MemoryType::ACPI_NVS, PAGE, 2),
            ],
        ] {
            assert_eq!(run(&descs), [region(RegionKind::ACPI_NVS, PAGE, 2)]);
        }
    }

    #[test]
    fn empty_descriptors_are_skipped() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 0, 0),
            desc(MemoryType::ACPI_NVS, PAGE, 1),
        ]);
        assert_eq!(got, [region(RegionKind::ACPI_NVS, PAGE, 1)]);
    }

    #[test]
    fn kernel_memory_stays_apart_from_usable_memory() {
        let got = run(&[
            desc(MemoryType::CONVENTIONAL, 0, 4),
            desc(MemoryType(KERNEL_MEMORY_TYPE), 4 * PAGE, 4),
            desc(MemoryType::CONVENTIONAL, 8 * PAGE, 4),
        ]);
        assert_eq!(
            got.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [RegionKind::USABLE, RegionKind::KERNEL, RegionKind::USABLE]
        );
    }

    #[test]
    fn a_full_buffer_says_how_much_was_needed() {
        let descs = [
            desc(MemoryType::CONVENTIONAL, 0, 1),
            desc(MemoryType::ACPI_NVS, PAGE, 1),
            desc(MemoryType::RESERVED, 2 * PAGE, 1),
            desc(MemoryType::CONVENTIONAL, 3 * PAGE, 0),
        ];
        let mut out = vec![region(RegionKind(0), 0, 0); 2];
        assert_eq!(
            convert(descs.iter().copied(), &mut out),
            Err(TooSmall { needed: 3 })
        );
        let mut exact = vec![region(RegionKind(0), 0, 0); 3];
        assert_eq!(convert(descs.iter().copied(), &mut exact), Ok(3));
        assert_eq!(convert(core::iter::empty(), &mut []), Ok(0));
    }

    #[test]
    fn top_is_the_highest_end() {
        let regions = [
            region(RegionKind::USABLE, 0, 4),
            region(RegionKind::RESERVED, 0xfee0_0000, 1),
            region(RegionKind::USABLE, 0x1_0000_0000, 0x10),
        ];
        assert_eq!(top(&regions), 0x1_0001_0000);
        assert_eq!(top(&regions[..2]), 0xfee0_1000);
        assert_eq!(top(&[]), 0);
    }

    // The memory map OVMF hands a 256MiB QEMU machine has about a hundred
    // descriptors, many of them neighbours of the same kind once boot
    // services are gone. This is its shape, shortened.
    #[test]
    fn a_firmware_shaped_map() {
        let got = run(&[
            desc(MemoryType::BOOT_SERVICES_CODE, 0, 1),
            desc(MemoryType::CONVENTIONAL, PAGE, 159),
            desc(MemoryType::CONVENTIONAL, 0x10_0000, 0x700),
            desc(MemoryType::ACPI_NVS, 0x80_0000, 8),
            desc(MemoryType::CONVENTIONAL, 0x80_8000, 3),
            desc(MemoryType::ACPI_NVS, 0x80_b000, 1),
            desc(MemoryType::LOADER_DATA, 0x80_c000, 0x10),
            desc(MemoryType(KERNEL_MEMORY_TYPE), 0x81_c000, 0x40),
            desc(MemoryType::BOOT_SERVICES_DATA, 0x85_c000, 0x7000),
            desc(MemoryType::RUNTIME_SERVICES_DATA, 0x785_c000, 0x20),
            desc(MemoryType::ACPI_RECLAIM, 0x787_c000, 4),
            desc(MemoryType::MMIO, 0xffc0_0000, 0x400),
        ]);
        let kinds: Vec<_> = got.iter().map(|r| (r.start, r.kind)).collect();
        assert_eq!(
            kinds,
            [
                (0, RegionKind::USABLE),
                (0x10_0000, RegionKind::USABLE),
                (0x80_0000, RegionKind::ACPI_NVS),
                (0x80_8000, RegionKind::USABLE),
                (0x80_b000, RegionKind::ACPI_NVS),
                (0x80_c000, RegionKind::USABLE),
                (0x81_c000, RegionKind::KERNEL),
                (0x85_c000, RegionKind::USABLE),
                (0x785_c000, RegionKind::RESERVED),
                (0x787_c000, RegionKind::ACPI_RECLAIMABLE),
                (0xffc0_0000, RegionKind::RESERVED),
            ]
        );
        assert_eq!(got[0].pages, 160);
        assert_eq!(top(&got), 0x1_0000_0000);
    }
}

#[cfg(test)]
mod from_firmware_bytes {
    use super::*;
    use uefi::memory::descriptors;

    // OVMF reports 48-byte descriptors, 8 more than the specification's
    // layout. This runs raw bytes in that shape through the descriptor
    // reader and the conversion, as the loader does.
    #[test]
    fn a_raw_map_with_large_descriptors() {
        let entries: [(u32, u64, u64); 4] = [
            (7, 0x10_0000, 0x100),
            (4, 0x20_0000, 0x200),
            (KERNEL_MEMORY_TYPE, 0x40_0000, 0x10),
            (6, 0x41_0000, 0x10),
        ];
        let mut raw = Vec::new();
        for (ty, start, pages) in entries {
            let mut d = [0u8; 48];
            d[0..4].copy_from_slice(&ty.to_le_bytes());
            d[8..16].copy_from_slice(&start.to_le_bytes());
            d[24..32].copy_from_slice(&pages.to_le_bytes());
            d[40..48].copy_from_slice(&[0xab; 8]);
            raw.extend_from_slice(&d);
        }
        let mut out = [MemoryRegion {
            start: 0,
            pages: 0,
            kind: RegionKind(0),
            reserved: 0,
        }; 8];
        let n = convert(descriptors(&raw, 48), &mut out).unwrap();
        let got: Vec<_> = out[..n]
            .iter()
            .map(|r| (r.start, r.pages, r.kind))
            .collect();
        assert_eq!(
            got,
            [
                (0x10_0000, 0x300, RegionKind::USABLE),
                (0x40_0000, 0x10, RegionKind::KERNEL),
                (0x41_0000, 0x10, RegionKind::RESERVED),
            ]
        );
        assert_eq!(top(&out[..n]), 0x42_0000);
    }
}
