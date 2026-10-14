//! The memory map the firmware hands over (UEFI 2.10, section 7.2).

/// Page size the firmware counts memory in.
pub const PAGE_SIZE: u64 = 4096;

/// `EFI_MEMORY_TYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct MemoryType(pub u32);

impl MemoryType {
    pub const RESERVED: MemoryType = MemoryType(0);
    pub const LOADER_CODE: MemoryType = MemoryType(1);
    pub const LOADER_DATA: MemoryType = MemoryType(2);
    pub const BOOT_SERVICES_CODE: MemoryType = MemoryType(3);
    pub const BOOT_SERVICES_DATA: MemoryType = MemoryType(4);
    pub const RUNTIME_SERVICES_CODE: MemoryType = MemoryType(5);
    pub const RUNTIME_SERVICES_DATA: MemoryType = MemoryType(6);
    pub const CONVENTIONAL: MemoryType = MemoryType(7);
    pub const UNUSABLE: MemoryType = MemoryType(8);
    pub const ACPI_RECLAIM: MemoryType = MemoryType(9);
    pub const ACPI_NVS: MemoryType = MemoryType(10);
    pub const MMIO: MemoryType = MemoryType(11);
    pub const MMIO_PORT_SPACE: MemoryType = MemoryType(12);
    pub const PAL_CODE: MemoryType = MemoryType(13);
    pub const PERSISTENT: MemoryType = MemoryType(14);
    pub const UNACCEPTED: MemoryType = MemoryType(15);

    /// Whether the kernel may use this memory once boot services are gone.
    /// Boot services memory is free again after `ExitBootServices`.
    #[must_use]
    pub fn is_free_after_boot(self) -> bool {
        matches!(
            self,
            MemoryType::CONVENTIONAL
                | MemoryType::BOOT_SERVICES_CODE
                | MemoryType::BOOT_SERVICES_DATA
        )
    }
}

/// One `EFI_MEMORY_DESCRIPTOR`. Firmware may use a larger descriptor size
/// than this struct, so maps are read with [`descriptors`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryDescriptor {
    pub ty: MemoryType,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub pages: u64,
    pub attribute: u64,
}

/// Size of the descriptor in the specification. Firmware reports its own
/// size, which may be larger, never smaller.
pub const DESCRIPTOR_SIZE: usize = 40;

impl MemoryDescriptor {
    #[must_use]
    pub fn end(&self) -> u64 {
        self.physical_start + self.pages * PAGE_SIZE
    }

    fn read(b: &[u8]) -> MemoryDescriptor {
        let u64_at = |at: usize| {
            let mut v = [0u8; 8];
            v.copy_from_slice(&b[at..at + 8]);
            u64::from_le_bytes(v)
        };
        MemoryDescriptor {
            ty: MemoryType(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            physical_start: u64_at(8),
            virtual_start: u64_at(16),
            pages: u64_at(24),
            attribute: u64_at(32),
        }
    }
}

/// Reads the descriptors of a memory map buffer whose entries are
/// `descriptor_size` bytes apart, as `GetMemoryMap` reports. A trailing
/// piece shorter than a descriptor is ignored, and so is everything when the
/// descriptor size is smaller than the specification's layout.
pub fn descriptors(
    map: &[u8],
    descriptor_size: usize,
) -> impl Iterator<Item = MemoryDescriptor> + '_ {
    let usable: &[u8] = if descriptor_size >= DESCRIPTOR_SIZE {
        map
    } else {
        &[]
    };
    usable
        .chunks(descriptor_size.max(1))
        .filter(|c| c.len() >= DESCRIPTOR_SIZE)
        .map(MemoryDescriptor::read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ty: u32, start: u64, pages: u64, size: usize) -> Vec<u8> {
        let mut b = vec![0u8; size];
        b[0..4].copy_from_slice(&ty.to_le_bytes());
        b[4..8].copy_from_slice(&[0xee; 4]);
        b[8..16].copy_from_slice(&start.to_le_bytes());
        b[16..24].copy_from_slice(&(start + 0x1000_0000).to_le_bytes());
        b[24..32].copy_from_slice(&pages.to_le_bytes());
        b[32..40].copy_from_slice(&0x0000_0000_0000_000f_u64.to_le_bytes());
        for x in &mut b[40..] {
            *x = 0xcc;
        }
        b
    }

    #[test]
    fn reads_each_field_little_endian() {
        let map = entry(7, 0x0010_0000, 0x7ff0, 40);
        let d: Vec<_> = descriptors(&map, 40).collect();
        assert_eq!(
            d,
            [MemoryDescriptor {
                ty: MemoryType::CONVENTIONAL,
                physical_start: 0x0010_0000,
                virtual_start: 0x1010_0000,
                pages: 0x7ff0,
                attribute: 0xf,
            }]
        );
    }

    #[test]
    fn honours_a_larger_descriptor_size() {
        let mut map = entry(1, 0x1000, 2, 48);
        map.extend(entry(4, 0x8000, 3, 48));
        map.extend(entry(7, 0x10_0000, 4, 48));
        let d: Vec<_> = descriptors(&map, 48).collect();
        assert_eq!(d.len(), 3);
        assert_eq!(
            (d[0].ty, d[0].physical_start, d[0].pages),
            (MemoryType::LOADER_CODE, 0x1000, 2)
        );
        assert_eq!(
            (d[1].ty, d[1].physical_start, d[1].pages),
            (MemoryType::BOOT_SERVICES_DATA, 0x8000, 3)
        );
        assert_eq!(
            (d[2].ty, d[2].physical_start, d[2].pages),
            (MemoryType::CONVENTIONAL, 0x10_0000, 4)
        );
    }

    #[test]
    fn ignores_a_short_tail_and_a_size_below_the_layout() {
        let mut map = entry(7, 0x1000, 1, 40);
        map.extend_from_slice(&[1, 2, 3]);
        assert_eq!(descriptors(&map, 40).count(), 1);
        assert_eq!(descriptors(&map, 39).count(), 0);
        assert_eq!(descriptors(&map, 0).count(), 0);
        assert_eq!(descriptors(&[], 40).count(), 0);
        assert_eq!(descriptors(&map[..39], 40).count(), 0);
    }

    #[test]
    fn keeps_the_runtime_attribute_bit() {
        let mut map = entry(5, 0xff00_0000, 16, 40);
        map[32..40].copy_from_slice(&0x8000_0000_0000_000f_u64.to_le_bytes());
        let d = descriptors(&map, 40).next().unwrap();
        assert_eq!(d.attribute >> 63, 1);
        assert_eq!(d.ty, MemoryType::RUNTIME_SERVICES_CODE);
        assert_eq!(d.end(), 0xff01_0000);
    }

    #[test]
    fn end_is_start_plus_pages() {
        let d = MemoryDescriptor {
            ty: MemoryType::CONVENTIONAL,
            physical_start: 0x1000,
            virtual_start: 0,
            pages: 3,
            attribute: 0,
        };
        assert_eq!(d.end(), 0x4000);
    }

    #[test]
    fn free_after_boot() {
        let free = [
            MemoryType::CONVENTIONAL,
            MemoryType::BOOT_SERVICES_CODE,
            MemoryType::BOOT_SERVICES_DATA,
        ];
        for t in 0..16 {
            let ty = MemoryType(t);
            assert_eq!(ty.is_free_after_boot(), free.contains(&ty), "type {t}");
        }
    }

    #[test]
    fn type_values_match_the_specification() {
        let expected = [
            MemoryType::RESERVED,
            MemoryType::LOADER_CODE,
            MemoryType::LOADER_DATA,
            MemoryType::BOOT_SERVICES_CODE,
            MemoryType::BOOT_SERVICES_DATA,
            MemoryType::RUNTIME_SERVICES_CODE,
            MemoryType::RUNTIME_SERVICES_DATA,
            MemoryType::CONVENTIONAL,
            MemoryType::UNUSABLE,
            MemoryType::ACPI_RECLAIM,
            MemoryType::ACPI_NVS,
            MemoryType::MMIO,
            MemoryType::MMIO_PORT_SPACE,
            MemoryType::PAL_CODE,
            MemoryType::PERSISTENT,
            MemoryType::UNACCEPTED,
        ];
        for (i, t) in expected.iter().enumerate() {
            assert_eq!(t.0 as usize, i);
        }
        assert_eq!(PAGE_SIZE, 4096);
    }
}
