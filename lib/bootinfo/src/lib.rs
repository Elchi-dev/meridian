//! What the Meridian bootloader hands the kernel. Both sides are built from
//! this crate, and the magic, version and size at the start let the kernel
//! refuse a loader it was not built with instead of misreading its data.
//!
//! Addresses are physical unless their name or comment says virtual. The
//! kernel reaches physical memory through the direct map at [`HHDM_BASE`].

#![cfg_attr(not(test), no_std)]

use core::mem::size_of;

pub const MAGIC: u64 = u64::from_le_bytes(*b"MERIDIAN");
pub const VERSION: u32 = 3;

/// Where the direct map of all physical memory starts: the first address of
/// the higher half. Physical address `p` is at virtual `HHDM_BASE + p`.
pub const HHDM_BASE: u64 = 0xffff_8000_0000_0000;

/// The top of the kernel's first stack. The page below the stack stays
/// unmapped, so running off its end faults instead of overwriting memory.
pub const STACK_TOP: u64 = 0xffff_ff00_0000_0000;
pub const STACK_PAGES: u64 = 16;

/// The UEFI memory type the loader uses for everything the kernel keeps:
/// its image, page tables, stack and boot information. UEFI leaves types
/// from `0x8000_0000` up to the operating system.
pub const KERNEL_MEMORY_TYPE: u32 = 0x8000_0000;

/// The boot information itself. The loader passes its virtual address,
/// inside the direct map, in `rdi`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u32,
    pub size: u32,
    pub memory: MemoryMap,
    pub hhdm: u64,
    pub kernel: KernelImage,
    pub stack: Stack,
    /// The ACPI root system description pointer, or 0 when the firmware
    /// has none.
    pub rsdp: u64,
    /// The UEFI system table, for the runtime services that outlive the
    /// loader: the clock, firmware variables and resetting the machine.
    pub uefi_system_table: u64,
    pub framebuffer: Framebuffer,
    /// Room for one frame of the framebuffer's width and height, in rows
    /// `width` pixels apart, which the kernel draws into before it copies
    /// the frame to the screen; 0 without a framebuffer. Its pages are the
    /// kernel's in the memory map.
    pub back_buffer: u64,
    pub modules: Modules,
}

/// The files the loader read for the kernel from `meridian/programs/` on
/// the EFI partition, at most [`Modules::MAX`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Modules {
    /// Virtual address of the first [`Module`], inside the direct map.
    pub list: u64,
    pub count: u64,
}

impl Modules {
    pub const MAX: usize = 16;
}

/// One file the loader read. Its pages are the kernel's in the memory map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Module {
    pub start: u64,
    /// Its length in bytes.
    pub size: u64,
    /// Its file name, padded with zero bytes.
    pub name: [u8; Module::NAME],
}

impl Module {
    /// The longest file name a module keeps.
    pub const NAME: usize = 32;

    /// A module for the file `name`, unless the name is empty, longer than
    /// [`Module::NAME`] or holds a zero byte.
    #[must_use]
    pub fn new(start: u64, size: u64, name: &[u8]) -> Option<Module> {
        if name.is_empty() || name.len() > Module::NAME || name.contains(&0) {
            return None;
        }
        let mut padded = [0; Module::NAME];
        padded[..name.len()].copy_from_slice(name);
        Some(Module {
            start,
            size,
            name: padded,
        })
    }

    /// Its file name without the padding.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        let len = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(Module::NAME);
        &self.name[..len]
    }

    /// The module the loader wrote as `bytes`.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; size_of::<Module>()]) -> Module {
        let word = |at: usize| {
            let mut w = [0; 8];
            w.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(w)
        };
        let mut name = [0; Module::NAME];
        name.copy_from_slice(&bytes[16..]);
        Module {
            start: word(0),
            size: word(8),
            name,
        }
    }

    /// The module as it lies in memory.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; size_of::<Module>()] {
        let mut out = [0u8; size_of::<Module>()];
        out[0..8].copy_from_slice(&self.start.to_le_bytes());
        out[8..16].copy_from_slice(&self.size.to_le_bytes());
        out[16..].copy_from_slice(&self.name);
        out
    }
}

/// The screen as the firmware left it, still showing what the loader drew.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Framebuffer {
    /// Physical address of the top left pixel, or 0 without a screen.
    pub base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Pixels from the start of one row to the start of the next.
    pub stride: u32,
    pub format: PixelFormat,
}

/// How the four bytes of a pixel are ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PixelFormat(pub u32);

impl PixelFormat {
    pub const NONE: PixelFormat = PixelFormat(0);
    /// Red, green, blue, unused.
    pub const RGB: PixelFormat = PixelFormat(1);
    /// Blue, green, red, unused.
    pub const BGR: PixelFormat = PixelFormat(2);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct MemoryMap {
    /// Virtual address of the first region, inside the direct map.
    pub regions: u64,
    pub count: u64,
}

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

    /// The region as it lies in memory.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; size_of::<MemoryRegion>()] {
        let mut out = [0u8; size_of::<MemoryRegion>()];
        out[0..8].copy_from_slice(&self.start.to_le_bytes());
        out[8..16].copy_from_slice(&self.pages.to_le_bytes());
        out[16..20].copy_from_slice(&self.kind.0.to_le_bytes());
        out[20..24].copy_from_slice(&self.reserved.to_le_bytes());
        out
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

/// Where the kernel image is: one physically contiguous block mapped at
/// `virtual_base`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct KernelImage {
    pub physical: u64,
    pub virtual_base: u64,
    pub size: u64,
}

/// The stack the kernel starts on, as virtual addresses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Stack {
    pub bottom: u64,
    pub top: u64,
}

/// Why a kernel refuses the boot information it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mismatch {
    Magic(u64),
    Version(u32),
    Size(u32),
}

impl BootInfo {
    #[must_use]
    pub fn new() -> BootInfo {
        BootInfo {
            magic: MAGIC,
            version: VERSION,
            size: u32::try_from(size_of::<BootInfo>()).unwrap_or(u32::MAX),
            memory: MemoryMap::default(),
            hhdm: HHDM_BASE,
            kernel: KernelImage::default(),
            stack: Stack::default(),
            rsdp: 0,
            uefi_system_table: 0,
            framebuffer: Framebuffer::default(),
            back_buffer: 0,
            modules: Modules::default(),
        }
    }

    /// The structure as it lies in memory, for a loader that only has the
    /// pages it goes into as bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; size_of::<BootInfo>()] {
        let mut out = [0u8; size_of::<BootInfo>()];
        out[0..8].copy_from_slice(&self.magic.to_le_bytes());
        out[8..12].copy_from_slice(&self.version.to_le_bytes());
        out[12..16].copy_from_slice(&self.size.to_le_bytes());
        let words = [
            self.memory.regions,
            self.memory.count,
            self.hhdm,
            self.kernel.physical,
            self.kernel.virtual_base,
            self.kernel.size,
            self.stack.bottom,
            self.stack.top,
            self.rsdp,
            self.uefi_system_table,
        ];
        for (chunk, word) in out[16..].chunks_mut(8).zip(words) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        let fb = &self.framebuffer;
        out[96..104].copy_from_slice(&fb.base.to_le_bytes());
        out[104..112].copy_from_slice(&fb.size.to_le_bytes());
        let fields = [fb.width, fb.height, fb.stride, fb.format.0];
        for (chunk, field) in out[112..128].chunks_mut(4).zip(fields) {
            chunk.copy_from_slice(&field.to_le_bytes());
        }
        out[128..136].copy_from_slice(&self.back_buffer.to_le_bytes());
        out[136..144].copy_from_slice(&self.modules.list.to_le_bytes());
        out[144..152].copy_from_slice(&self.modules.count.to_le_bytes());
        out
    }

    /// Checks that the loader and the kernel agree on this structure.
    ///
    /// # Errors
    ///
    /// Names the first field that does not match.
    pub fn check(&self) -> Result<(), Mismatch> {
        if self.magic != MAGIC {
            return Err(Mismatch::Magic(self.magic));
        }
        if self.version != VERSION {
            return Err(Mismatch::Version(self.version));
        }
        if self.size as usize != size_of::<BootInfo>() {
            return Err(Mismatch::Size(self.size));
        }
        Ok(())
    }
}

impl Default for BootInfo {
    fn default() -> Self {
        BootInfo::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn a_new_structure_passes_its_own_check() {
        let b = BootInfo::new();
        assert_eq!(b.check(), Ok(()));
        assert_eq!(BootInfo::default(), b);
        assert_eq!(b.size as usize, size_of::<BootInfo>());
        assert_eq!(
            (
                b.rsdp,
                b.memory.count,
                b.kernel.size,
                b.stack.top,
                b.uefi_system_table,
                b.back_buffer,
                b.modules.count
            ),
            (0, 0, 0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn the_magic_is_checked_first() {
        let mut b = BootInfo::new();
        b.magic = 0;
        b.version = 9;
        b.size = 1;
        assert_eq!(b.check(), Err(Mismatch::Magic(0)));
        b.magic = MAGIC;
        assert_eq!(b.check(), Err(Mismatch::Version(9)));
        b.version = VERSION;
        assert_eq!(b.check(), Err(Mismatch::Size(1)));
    }

    #[test]
    fn each_mismatch_is_named() {
        let mut b = BootInfo::new();
        b.magic = 0x1234;
        assert_eq!(b.check(), Err(Mismatch::Magic(0x1234)));
        let mut b = BootInfo::new();
        b.version = 1;
        assert_eq!(
            b.check(),
            Err(Mismatch::Version(1)),
            "a loader from before the back buffer"
        );
        let mut b = BootInfo::new();
        b.version = 2;
        assert_eq!(
            b.check(),
            Err(Mismatch::Version(2)),
            "a loader from before modules"
        );
        let mut b = BootInfo::new();
        b.size -= 8;
        assert_eq!(b.check(), Err(Mismatch::Size(b.size)));
        b.size += 16;
        assert_eq!(b.check(), Err(Mismatch::Size(b.size)));
    }

    #[test]
    fn the_magic_spells_meridian() {
        assert_eq!(&MAGIC.to_le_bytes(), b"MERIDIAN");
        assert_eq!(VERSION, 3);
    }

    #[test]
    fn layout_is_fixed() {
        let offsets = [
            (offset_of!(BootInfo, magic), 0),
            (offset_of!(BootInfo, version), 8),
            (offset_of!(BootInfo, size), 12),
            (offset_of!(BootInfo, memory), 16),
            (offset_of!(BootInfo, hhdm), 32),
            (offset_of!(BootInfo, kernel), 40),
            (offset_of!(BootInfo, stack), 64),
            (offset_of!(BootInfo, rsdp), 80),
            (offset_of!(BootInfo, uefi_system_table), 88),
            (offset_of!(BootInfo, framebuffer), 96),
            (offset_of!(BootInfo, back_buffer), 128),
            (offset_of!(BootInfo, modules), 136),
        ];
        for (got, want) in offsets {
            assert_eq!(got, want);
        }
        assert_eq!(size_of::<BootInfo>(), 152);
        assert_eq!(offset_of!(Modules, count), 8);
        assert_eq!(size_of::<Module>(), 48);
        assert_eq!(offset_of!(Module, size), 8);
        assert_eq!(offset_of!(Module, name), 16);
        assert!(
            Modules::MAX * size_of::<Module>() <= 4096,
            "the list fits a page"
        );
        assert_eq!(size_of::<Framebuffer>(), 32);
        assert_eq!(offset_of!(Framebuffer, width), 16);
        assert_eq!(offset_of!(Framebuffer, stride), 24);
        assert_eq!(offset_of!(Framebuffer, format), 28);
        assert_eq!(offset_of!(MemoryMap, count), 8);
        assert_eq!(offset_of!(KernelImage, virtual_base), 8);
        assert_eq!(offset_of!(KernelImage, size), 16);
        assert_eq!(offset_of!(Stack, top), 8);
    }

    fn u64_at(b: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn bytes_follow_the_layout() {
        let info = BootInfo {
            memory: MemoryMap {
                regions: 0x1111,
                count: 0x2222,
            },
            hhdm: 0x3333,
            kernel: KernelImage {
                physical: 0x4444,
                virtual_base: 0x5555,
                size: 0x6666,
            },
            stack: Stack {
                bottom: 0x7777,
                top: 0x8888,
            },
            rsdp: 0x9999,
            uefi_system_table: 0xaaaa,
            framebuffer: Framebuffer {
                base: 0xbbbb,
                size: 0xcccc,
                width: 0xdd,
                height: 0xee,
                stride: 0xff,
                format: PixelFormat::BGR,
            },
            back_buffer: 0x1234_5000,
            modules: Modules {
                list: 0xffff_8000_0012_3000,
                count: 3,
            },
            ..BootInfo::new()
        };
        let b = info.to_bytes();
        assert_eq!(&b[0..8], b"MERIDIAN");
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), VERSION);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 152);
        let fields = [
            (
                offset_of!(BootInfo, memory) + offset_of!(MemoryMap, regions),
                0x1111,
            ),
            (
                offset_of!(BootInfo, memory) + offset_of!(MemoryMap, count),
                0x2222,
            ),
            (offset_of!(BootInfo, hhdm), 0x3333),
            (
                offset_of!(BootInfo, kernel) + offset_of!(KernelImage, physical),
                0x4444,
            ),
            (
                offset_of!(BootInfo, kernel) + offset_of!(KernelImage, virtual_base),
                0x5555,
            ),
            (
                offset_of!(BootInfo, kernel) + offset_of!(KernelImage, size),
                0x6666,
            ),
            (
                offset_of!(BootInfo, stack) + offset_of!(Stack, bottom),
                0x7777,
            ),
            (offset_of!(BootInfo, stack) + offset_of!(Stack, top), 0x8888),
            (offset_of!(BootInfo, rsdp), 0x9999),
            (offset_of!(BootInfo, uefi_system_table), 0xaaaa),
        ];
        for (at, want) in fields {
            assert_eq!(u64_at(&b, at), want, "offset {at}");
        }
        let fb = offset_of!(BootInfo, framebuffer);
        assert_eq!(u64_at(&b, fb + offset_of!(Framebuffer, base)), 0xbbbb);
        assert_eq!(u64_at(&b, fb + offset_of!(Framebuffer, size)), 0xcccc);
        let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(fb + offset_of!(Framebuffer, width)), 0xdd);
        assert_eq!(u32_at(fb + offset_of!(Framebuffer, height)), 0xee);
        assert_eq!(u32_at(fb + offset_of!(Framebuffer, stride)), 0xff);
        assert_eq!(u32_at(fb + offset_of!(Framebuffer, format)), 2);
        assert_eq!(u64_at(&b, offset_of!(BootInfo, back_buffer)), 0x1234_5000);
        let modules = offset_of!(BootInfo, modules);
        assert_eq!(
            u64_at(&b, modules + offset_of!(Modules, list)),
            0xffff_8000_0012_3000
        );
        assert_eq!(u64_at(&b, modules + offset_of!(Modules, count)), 3);
    }

    #[test]
    fn a_module_keeps_its_name_padded() {
        let m = Module::new(0x20_0000, 0x1234, b"hello").unwrap();
        assert_eq!(m.name(), b"hello");
        assert_eq!(&m.name[..6], b"hello\0");
        assert!(m.name[5..].iter().all(|&b| b == 0));
        let longest = [b'a'; Module::NAME];
        let full = Module::new(0, 1, &longest).unwrap();
        assert_eq!(full.name(), &longest, "no room left for a zero");
        assert_eq!(Module::new(0, 1, &[b'a'; Module::NAME + 1]), None);
        assert_eq!(Module::new(0, 1, b""), None);
        assert_eq!(
            Module::new(0, 1, b"he\0llo"),
            None,
            "the name would end early"
        );
    }

    #[test]
    fn modules_read_back_as_the_loader_wrote_them() {
        let modules = [
            Module::new(0x20_0000, 0x1234, b"hello").unwrap(),
            Module::new(0x0123_4567_89ab_c000, 1, &[b'z'; Module::NAME]).unwrap(),
            Module::new(0x30_0000, 0x10_0000, b"init").unwrap(),
        ];
        let size = size_of::<Module>();
        let mut page = vec![0u8; 4096];
        for (i, m) in modules.iter().enumerate() {
            page[i * size..(i + 1) * size].copy_from_slice(&m.to_bytes());
        }
        for (i, m) in modules.iter().enumerate() {
            let back = Module::from_bytes(page[i * size..(i + 1) * size].try_into().unwrap());
            assert_eq!(back, *m, "module {i}");
            assert_eq!(back.name(), m.name());
        }
        let mut garbled = modules[0].to_bytes();
        garbled[16 + 6] = b'x';
        assert_eq!(
            Module::from_bytes(&garbled).name(),
            b"hello",
            "the name ends at its first zero"
        );
    }

    #[test]
    fn names_of_every_length_and_byte_come_back_whole() {
        for len in 1..=Module::NAME {
            let name: Vec<u8> = (1..=255u8).cycle().skip(len * 7).take(len).collect();
            let m = Module::new(0x1000 * len as u64, len as u64, &name).unwrap();
            let back = Module::from_bytes(&m.to_bytes());
            assert_eq!(back.name(), name.as_slice(), "{len} bytes");
            assert_eq!((back.start, back.size), (0x1000 * len as u64, len as u64));
        }
        for byte in 1..=255u8 {
            let m = Module::new(0, 0, &[byte; 3]).unwrap();
            assert_eq!(Module::from_bytes(&m.to_bytes()).name(), &[byte; 3]);
        }
    }

    #[test]
    fn module_bytes_follow_the_layout() {
        let m = Module::new(0x0123_4567_89ab_c000, 0x4_2000, b"init").unwrap();
        let b = m.to_bytes();
        assert_eq!(u64_at(&b, offset_of!(Module, start)), 0x0123_4567_89ab_c000);
        assert_eq!(u64_at(&b, offset_of!(Module, size)), 0x4_2000);
        assert_eq!(&b[16..20], b"init");
        assert!(b[20..].iter().all(|&b| b == 0));
        assert_eq!(b.len(), size_of::<Module>());
    }

    #[test]
    fn region_bytes_follow_the_layout() {
        let r = MemoryRegion {
            start: 0x0123_4567_89ab_c000,
            pages: 0x42,
            kind: RegionKind::ACPI_NVS,
            reserved: 0xdead_beef,
        };
        let b = r.to_bytes();
        assert_eq!(u64_at(&b, offset_of!(MemoryRegion, start)), r.start);
        assert_eq!(u64_at(&b, offset_of!(MemoryRegion, pages)), 0x42);
        assert_eq!(&b[16..20], &4u32.to_le_bytes());
        assert_eq!(&b[20..24], &0xdead_beef_u32.to_le_bytes());
    }

    #[test]
    fn pixel_formats_are_numbered_from_none() {
        assert_eq!(PixelFormat::default(), PixelFormat::NONE);
        assert_eq!(
            [PixelFormat::NONE.0, PixelFormat::RGB.0, PixelFormat::BGR.0],
            [0, 1, 2]
        );
        assert_eq!(size_of::<PixelFormat>(), 4);
    }

    /// A fresh structure is the header, the direct map's base and zeros:
    /// the loader fills in everything else, and a field it forgets reads
    /// as absent rather than as garbage.
    #[test]
    fn a_new_structure_is_zero_past_the_header() {
        let b = BootInfo::new().to_bytes();
        assert_eq!(&b[..8], b"MERIDIAN");
        let hhdm = offset_of!(BootInfo, hhdm);
        assert_eq!(u64_at(&b, hhdm), HHDM_BASE);
        for (i, &byte) in b.iter().enumerate().skip(16) {
            if !(hhdm..hhdm + 8).contains(&i) {
                assert_eq!(byte, 0, "byte {i}");
            }
        }
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

    #[test]
    fn the_direct_map_starts_the_higher_half() {
        assert_eq!(HHDM_BASE, 0xffff_8000_0000_0000);
        assert_eq!(HHDM_BASE >> 47, 0x1_ffff);
        assert_eq!(BootInfo::new().hhdm, HHDM_BASE);
    }

    #[test]
    fn the_stack_sits_alone_in_the_higher_half() {
        assert_eq!(STACK_TOP % 4096, 0);
        assert_eq!(
            STACK_TOP >> 47,
            0x1_ffff,
            "canonical and in the higher half"
        );
        assert_eq!(
            (STACK_TOP - 1) >> 39 & 0x1ff,
            509,
            "a level 4 slot of its own"
        );
        assert_eq!(STACK_PAGES, 16);
    }
}
