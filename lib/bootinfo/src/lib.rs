//! What the Meridian bootloader hands the kernel. Both sides are built from
//! this crate. It starts with the memory map; the rest of the handover
//! joins it as the loader learns to start the kernel.

#![cfg_attr(not(test), no_std)]

/// The UEFI memory type the loader uses for everything the kernel keeps:
/// its image, page tables, stack and boot information. UEFI leaves types
/// from `0x8000_0000` up to the operating system.
pub const KERNEL_MEMORY_TYPE: u32 = 0x8000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct MemoryRegion {
    pub start: u64,
    pub pages: u64,
    pub kind: RegionKind,
    pub reserved: u32,
}

impl MemoryRegion {
    pub const PAGE_SIZE: u64 = 4096;

    #[must_use]
    pub fn end(&self) -> u64 {
        self.start + self.pages * Self::PAGE_SIZE
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RegionKind(pub u32);

impl RegionKind {
    /// Free for the kernel to use.
    pub const USABLE: RegionKind = RegionKind(1);
    /// Owned by the kernel from the start: its image, page tables, stack
    /// and the boot information itself.
    pub const KERNEL: RegionKind = RegionKind(2);
    /// ACPI tables; usable once the kernel has read them.
    pub const ACPI_RECLAIMABLE: RegionKind = RegionKind(3);
    /// Must be kept across sleep states.
    pub const ACPI_NVS: RegionKind = RegionKind(4);
    /// Firmware runtime services and anything else that is not RAM to use.
    pub const RESERVED: RegionKind = RegionKind(5);
    /// Memory the firmware found broken.
    pub const UNUSABLE: RegionKind = RegionKind(6);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn region_end() {
        let r = MemoryRegion {
            start: 0x10_0000,
            pages: 3,
            kind: RegionKind::USABLE,
            reserved: 0,
        };
        assert_eq!(r.end(), 0x10_3000);
        let empty = MemoryRegion { pages: 0, ..r };
        assert_eq!(empty.end(), empty.start);
    }

    #[test]
    fn region_layout_is_fixed() {
        assert_eq!(size_of::<MemoryRegion>(), 24);
        assert_eq!(offset_of!(MemoryRegion, pages), 8);
        assert_eq!(offset_of!(MemoryRegion, kind), 16);
        assert_eq!(offset_of!(MemoryRegion, reserved), 20);
        assert_eq!(size_of::<RegionKind>(), 4);
    }

    #[test]
    fn kinds_are_numbered_from_one() {
        let kinds = [
            RegionKind::USABLE,
            RegionKind::KERNEL,
            RegionKind::ACPI_RECLAIMABLE,
            RegionKind::ACPI_NVS,
            RegionKind::RESERVED,
            RegionKind::UNUSABLE,
        ];
        for (i, k) in kinds.iter().enumerate() {
            assert_eq!(k.0 as usize, i + 1);
        }
        assert!(RegionKind::USABLE < RegionKind::UNUSABLE);
    }

    #[test]
    fn kernel_memory_is_in_the_range_uefi_leaves_to_the_os() {
        assert_eq!(KERNEL_MEMORY_TYPE, 0x8000_0000);
        assert_eq!(KERNEL_MEMORY_TYPE >> 31, 1);
    }
}
