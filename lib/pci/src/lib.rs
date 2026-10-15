//! PCI, the parts that need no hardware: where a function's configuration
//! space lies in an ECAM region, and what its first 64 bytes say: who made
//! the device, what kind of device it is, and where its registers are.
//! The kernel reads the bytes; this crate tells what they mean.

#![cfg_attr(not(test), no_std)]

/// One function of one device on one bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl Address {
    /// How far from the start of an ECAM region whose first bus is
    /// `first_bus` the function's 4 KiB of configuration space begin; none
    /// for a bus below it, a device past 31 or a function past 7.
    #[must_use]
    pub fn ecam_offset(self, first_bus: u8) -> Option<u64> {
        let bus = self.bus.checked_sub(first_bus)?;
        (self.device < 32 && self.function < 8).then(|| {
            u64::from(bus) << 20 | u64::from(self.device) << 15 | u64::from(self.function) << 12
        })
    }
}

/// The parts of a function's configuration header Meridian looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub vendor: u16,
    pub device: u16,
    pub revision: u8,
    pub interface: u8,
    pub subclass: u8,
    pub class: u8,
    /// 0 for a device, 1 for a bridge to another bus.
    pub layout: u8,
    /// Whether the device has functions past 0.
    pub multifunction: bool,
    /// The base address registers of a device; a bridge has two.
    pub bars: [u32; 6],
}

/// The vendor id configuration space reads as where no function is.
pub const NOBODY: u16 = 0xffff;

impl Header {
    /// The header in the first 64 bytes of a function's configuration
    /// space, or none when no function is there.
    #[must_use]
    pub fn read(bytes: &[u8; 64]) -> Option<Header> {
        let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let vendor = word(0);
        if vendor == NOBODY {
            return None;
        }
        let mut bars = [0u32; 6];
        for (i, bar) in bars.iter_mut().enumerate() {
            let at = 16 + 4 * i;
            *bar = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        }
        Some(Header {
            vendor,
            device: word(2),
            revision: bytes[8],
            interface: bytes[9],
            subclass: bytes[10],
            class: bytes[11],
            layout: bytes[14] & 0x7f,
            multifunction: bytes[14] & 0x80 != 0,
            bars,
        })
    }

    /// What its base address register `index` holds, the next one taken as
    /// the high half of a 64-bit address. A bridge has only the first two.
    #[must_use]
    pub fn bar(&self, index: usize) -> Bar {
        let count = if self.layout == 0 { 6 } else { 2 };
        if index >= count {
            return Bar::None;
        }
        let low = self.bars[index];
        if low & 1 == 1 {
            return Bar::Io(low & !0x3);
        }
        let prefetchable = low & 0x8 != 0;
        match low >> 1 & 0x3 {
            0 => Bar::Memory {
                address: u64::from(low & !0xf),
                prefetchable,
            },
            2 if index + 1 < count => Bar::Memory {
                address: u64::from(self.bars[index + 1]) << 32 | u64::from(low & !0xf),
                prefetchable,
            },
            _ => Bar::None,
        }
    }

    /// What kind of device it is, in a few words.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        kind(self.class, self.subclass, self.interface)
    }
}

/// Where a base address register points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    /// It is unused, or not of a kind Meridian reads.
    None,
    /// I/O ports from this one on.
    Io(u32),
    /// Memory from this address on.
    Memory { address: u64, prefetchable: bool },
}

/// What kind of device a class, subclass and programming interface stand
/// for, in a few words, for the ones Meridian meets on PCs; the class
/// alone for the rest.
#[must_use]
pub fn kind(class: u8, subclass: u8, interface: u8) -> &'static str {
    match (class, subclass, interface) {
        (0x01, 0x06, 0x01) => "SATA controller (AHCI)",
        (0x01, 0x08, 0x02) => "NVMe controller",
        (0x01, 0x00, _) => "SCSI controller",
        (0x01, 0x01, _) => "IDE controller",
        (0x01, _, _) => "storage controller",
        (0x02, 0x00, _) => "Ethernet controller",
        (0x02, _, _) => "network controller",
        (0x03, 0x00, _) => "VGA display",
        (0x03, _, _) => "display controller",
        (0x04, 0x03, _) => "audio device",
        (0x04, _, _) => "multimedia device",
        (0x06, 0x00, _) => "host bridge",
        (0x06, 0x01, _) => "ISA bridge",
        (0x06, 0x04, _) => "PCI bridge",
        (0x06, _, _) => "bridge",
        (0x0c, 0x03, 0x30) => "USB controller (xHCI)",
        (0x0c, 0x03, _) => "USB controller",
        (0x0c, 0x05, _) => "SMBus controller",
        (0x0c, _, _) => "serial bus controller",
        _ => "device",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first 64 bytes of a function's configuration space.
    fn space(vendor: u16, device: u16, class: [u8; 4], layout: u8, bars: [u32; 6]) -> [u8; 64] {
        let mut b = [0u8; 64];
        b[0..2].copy_from_slice(&vendor.to_le_bytes());
        b[2..4].copy_from_slice(&device.to_le_bytes());
        b[8..12].copy_from_slice(&class);
        b[14] = layout;
        for (i, bar) in bars.iter().enumerate() {
            b[16 + 4 * i..20 + 4 * i].copy_from_slice(&bar.to_le_bytes());
        }
        b
    }

    fn at(bus: u8, device: u8, function: u8) -> Address {
        Address {
            bus,
            device,
            function,
        }
    }

    #[test]
    fn each_function_has_4_kib_of_its_own() {
        assert_eq!(at(0, 0, 0).ecam_offset(0), Some(0));
        assert_eq!(at(0, 0, 1).ecam_offset(0), Some(0x1000));
        assert_eq!(at(0, 1, 0).ecam_offset(0), Some(0x8000));
        assert_eq!(at(0, 0x1f, 2).ecam_offset(0), Some(0xfa000), "q35's AHCI");
        assert_eq!(at(1, 0, 0).ecam_offset(0), Some(0x10_0000));
        assert_eq!(
            at(0xff, 31, 7).ecam_offset(0),
            Some(0xfff_f000),
            "the last of 256 MiB"
        );
        assert_eq!(
            at(0x81, 0, 0).ecam_offset(0x80),
            Some(0x10_0000),
            "counted from its first bus"
        );
    }

    #[test]
    fn functions_outside_a_region_have_no_place_in_it() {
        assert_eq!(
            at(0x7f, 0, 0).ecam_offset(0x80),
            None,
            "below its first bus"
        );
        assert_eq!(at(0, 32, 0).ecam_offset(0), None);
        assert_eq!(at(0, 0, 8).ecam_offset(0), None);
        assert_eq!(at(0, 31, 7).ecam_offset(0), Some(0xff000));
    }

    #[test]
    fn a_header_says_who_made_the_device_and_what_it_is() {
        let ahci = space(
            0x8086,
            0x2922,
            [0x02, 0x01, 0x06, 0x01],
            0,
            [0, 0, 0, 0, 0xc041, 0xfebd_5000],
        );
        let h = Header::read(&ahci).unwrap();
        assert_eq!((h.vendor, h.device, h.revision), (0x8086, 0x2922, 2));
        assert_eq!((h.class, h.subclass, h.interface), (0x01, 0x06, 0x01));
        assert_eq!((h.layout, h.multifunction), (0, false));
        assert_eq!(h.kind(), "SATA controller (AHCI)");
        assert_eq!(h.bar(4), Bar::Io(0xc040));
        assert_eq!(
            h.bar(5),
            Bar::Memory {
                address: 0xfebd_5000,
                prefetchable: false
            }
        );
        assert_eq!(
            h.bar(0),
            Bar::Memory {
                address: 0,
                prefetchable: false
            }
        );
    }

    #[test]
    fn where_no_function_is_configuration_space_reads_all_ones() {
        assert_eq!(Header::read(&[0xff; 64]), None);
        let mut nobody = [0u8; 64];
        nobody[0..2].copy_from_slice(&NOBODY.to_le_bytes());
        assert_eq!(Header::read(&nobody), None, "the vendor alone says so");
    }

    #[test]
    fn a_device_with_more_functions_says_so() {
        let mut bridge = space(0x8086, 0x2918, [0x02, 0x00, 0x01, 0x06], 0x80, [0; 6]);
        let h = Header::read(&bridge).unwrap();
        assert!(h.multifunction);
        assert_eq!(h.layout, 0, "the top bit is not part of the layout");
        assert_eq!(h.kind(), "ISA bridge");
        bridge[14] = 0x81;
        assert_eq!(Header::read(&bridge).unwrap().layout, 1);
    }

    #[test]
    fn a_64_bit_bar_takes_the_next_one_as_its_high_half() {
        let nvme = space(
            0x1b36,
            0x0010,
            [0x02, 0x02, 0x08, 0x01],
            0,
            [0xfe00_000c, 0x1, 0, 0, 0, 0],
        );
        let h = Header::read(&nvme).unwrap();
        assert_eq!(h.kind(), "NVMe controller");
        assert_eq!(
            h.bar(0),
            Bar::Memory {
                address: 0x1_fe00_0000,
                prefetchable: true
            }
        );
        let last = space(1, 2, [0; 4], 0, [0, 0, 0, 0, 0, 0xfe00_0004]);
        assert_eq!(
            Header::read(&last).unwrap().bar(5),
            Bar::None,
            "the last one has no high half"
        );
        let reserved = space(1, 2, [0; 4], 0, [0x6, 0, 0, 0, 0, 0]);
        assert_eq!(
            Header::read(&reserved).unwrap().bar(0),
            Bar::None,
            "type 1 is reserved"
        );
    }

    #[test]
    fn a_bridge_has_two_bars() {
        let bridge = space(
            0x1b36,
            0x000c,
            [0, 0, 0x04, 0x06],
            1,
            [0xfe80_0000, 0, 0x0100_0100, 0, 0, 0],
        );
        let h = Header::read(&bridge).unwrap();
        assert_eq!(h.kind(), "PCI bridge");
        assert_eq!(
            h.bar(0),
            Bar::Memory {
                address: 0xfe80_0000,
                prefetchable: false
            }
        );
        assert_eq!(h.bar(2), Bar::None, "the bus numbers live there");
        assert_eq!(h.bar(6), Bar::None);
        let device = Header::read(&space(1, 2, [0; 4], 0, [0; 6])).unwrap();
        assert_eq!(device.bar(6), Bar::None, "there is no seventh");
    }

    #[test]
    fn kinds_have_names_and_the_rest_says_device() {
        let named = [
            ((0x01, 0x06, 0x01), "SATA controller (AHCI)"),
            ((0x01, 0x08, 0x02), "NVMe controller"),
            ((0x01, 0x00, 0x00), "SCSI controller"),
            ((0x01, 0x01, 0x80), "IDE controller"),
            ((0x01, 0x06, 0x00), "storage controller"),
            ((0x02, 0x00, 0x00), "Ethernet controller"),
            ((0x02, 0x80, 0x00), "network controller"),
            ((0x03, 0x00, 0x00), "VGA display"),
            ((0x03, 0x80, 0x00), "display controller"),
            ((0x04, 0x03, 0x00), "audio device"),
            ((0x04, 0x01, 0x00), "multimedia device"),
            ((0x06, 0x00, 0x00), "host bridge"),
            ((0x06, 0x01, 0x00), "ISA bridge"),
            ((0x06, 0x04, 0x01), "PCI bridge"),
            ((0x06, 0x80, 0x00), "bridge"),
            ((0x0c, 0x03, 0x30), "USB controller (xHCI)"),
            ((0x0c, 0x03, 0x20), "USB controller"),
            ((0x0c, 0x05, 0x00), "SMBus controller"),
            ((0x0c, 0x80, 0x00), "serial bus controller"),
            ((0x08, 0x80, 0x00), "device"),
        ];
        for ((class, subclass, interface), name) in named {
            assert_eq!(
                kind(class, subclass, interface),
                name,
                "{class:#x} {subclass:#x} {interface:#x}"
            );
        }
    }
}
