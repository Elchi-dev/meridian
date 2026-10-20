//! Disks, the parts that need no driver: which controllers to drive,
//! their partition tables, read through whatever reads their sectors, and
//! what the kernel logs about them.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Display, Write};

use gpt::{Header, Partition};

/// The storage controllers the kernel drives, as the walk over PCI meets
/// them: the first AHCI controller and the first NVM Express one, each
/// with where it is and where its registers lie.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controllers {
    pub ahci: Option<(pci::Address, pci::Bar)>,
    pub nvme: Option<(pci::Address, pci::Bar)>,
    pub virtio: Option<(pci::Address, pci::Header)>,
}

impl Controllers {
    /// Takes note of the function at `at` when it is the first of its kind.
    pub fn saw(&mut self, at: pci::Address, h: &pci::Header) {
        if h.vendor == virtio::VENDOR
            && matches!(h.device, virtio::BLOCK | virtio::BLOCK_TRANSITIONAL)
        {
            self.virtio.get_or_insert((at, *h));
            return;
        }
        let (first, registers) = match (h.class, h.subclass, h.interface) {
            (0x01, 0x06, 0x01) => (&mut self.ahci, 5),
            (0x01, 0x08, 0x02) => (&mut self.nvme, 0),
            _ => return,
        };
        first.get_or_insert((at, h.bar(registers)));
    }
}

/// Writes the lines the kernel logs about what the disk `disk` holds,
/// read as [`Table::read`] reads: each partition its table lists, and how
/// the ones that hold FAT32 are divided. Nothing for a disk without one.
///
/// # Errors
///
/// When `out` fails.
pub fn describe(
    out: &mut impl Write,
    disk: &dyn Display,
    read: &mut impl FnMut(u64, &mut [u8]) -> bool,
) -> fmt::Result {
    let Some(table) = Table::read(read) else {
        return Ok(());
    };
    for (number, partition) in table.partitions() {
        out.write_str("kernel: ")?;
        crate::partition_line(out, disk, number, &partition)?;
        let mut boot = [0u8; 512];
        if read(partition.first, &mut boot)
            && let Ok(volume) = fat::Volume::read(&boot)
        {
            writeln!(
                out,
                "kernel: {disk}, partition {number} holds FAT32, {} clusters of {} bytes",
                volume.clusters,
                volume.cluster_bytes()
            )?;
        }
    }
    Ok(())
}

/// A disk's GUID partition table: the header in sector 1 and its entry
/// array, once both passed their checks.
pub struct Table {
    pub header: Header,
    entries: Vec<u8>,
}

impl Table {
    /// The table of the disk whose 512-byte sectors `read` reads into the
    /// room it is given, starting at the sector it is given, saying whether
    /// it could; none when the disk has no table that passes its checks.
    pub fn read(read: &mut impl FnMut(u64, &mut [u8]) -> bool) -> Option<Table> {
        let mut sector = [0u8; 512];
        if !read(1, &mut sector) {
            return None;
        }
        let header = Header::read(&sector).ok()?;
        let len = usize::try_from(header.entries_bytes().div_ceil(512) * 512).ok()?;
        let mut entries = vec![0u8; len];
        if !read(header.entries_at, &mut entries) {
            return None;
        }
        header.partitions(&entries).map(|_| ())?;
        Some(Table { header, entries })
    }

    /// The first partition the table lists of the type `kind`.
    #[must_use]
    pub fn first_of(&self, kind: gpt::Guid) -> Option<Partition> {
        self.partitions().map(|(_, p)| p).find(|p| p.kind == kind)
    }

    /// The partition the loader came from, which it names by its unique
    /// GUID `boot`; when it names none, all zeros, the first EFI system
    /// partition. None when the partition it names is on another disk.
    #[must_use]
    pub fn boot_volume(&self, boot: [u8; 16]) -> Option<Partition> {
        if boot == [0; 16] {
            return self.first_of(gpt::EFI_SYSTEM);
        }
        self.partitions()
            .map(|(_, p)| p)
            .find(|p| p.unique.0 == boot)
    }

    /// The partitions the table lists, each with its number, counted from
    /// 1 as entries are, unused ones left out.
    pub fn partitions(&self) -> impl Iterator<Item = (usize, Partition)> + '_ {
        self.header.partitions(&self.entries).into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disk of `sectors` sectors with a GPT whose array holds `entries`,
    /// each a type, a first and a last sector, in the entries given.
    fn disk(sectors: u64, entries: &[(usize, [u8; 16], u64, u64)]) -> Vec<u8> {
        let mut image = vec![0u8; usize::try_from(sectors * 512).unwrap()];
        let array = &mut image[1024..1024 + 128 * 128];
        for &(slot, kind, first, last) in entries {
            let e = &mut array[slot * 128..(slot + 1) * 128];
            e[..16].copy_from_slice(&kind);
            e[32..40].copy_from_slice(&first.to_le_bytes());
            e[40..48].copy_from_slice(&last.to_le_bytes());
            e[56..58].copy_from_slice(&[b'P', 0]);
        }
        let crc = gpt::crc32(&image[1024..1024 + 128 * 128]);
        let h = &mut image[512..604];
        h[..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&1u64.to_le_bytes());
        h[32..40].copy_from_slice(&(sectors - 1).to_le_bytes());
        h[40..48].copy_from_slice(&34u64.to_le_bytes());
        h[48..56].copy_from_slice(&(sectors - 34).to_le_bytes());
        h[72..80].copy_from_slice(&2u64.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&crc.to_le_bytes());
        let crc = gpt::crc32(&image[512..604]);
        image[528..532].copy_from_slice(&crc.to_le_bytes());
        image
    }

    fn reader(image: &[u8]) -> impl FnMut(u64, &mut [u8]) -> bool + '_ {
        |lba, into| {
            let at = usize::try_from(lba * 512).unwrap();
            image.get(at..at + into.len()).is_some_and(|bytes| {
                into.copy_from_slice(bytes);
                true
            })
        }
    }

    #[test]
    fn a_disk_lists_its_partitions_by_number() {
        let image = disk(
            4096,
            &[
                (0, gpt::EFI_SYSTEM.0, 2048, 3071),
                (2, gpt::MERIDIAN_SYSTEM.0, 3072, 4000),
            ],
        );
        let table = Table::read(&mut reader(&image)).unwrap();
        assert_eq!((table.header.current, table.header.backup), (1, 4095));
        let parts: Vec<(usize, Partition)> = table.partitions().collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(
            (parts[0].0, parts[0].1.kind, parts[0].1.first),
            (1, gpt::EFI_SYSTEM, 2048)
        );
        assert_eq!(
            (parts[1].0, parts[1].1.kind, parts[1].1.last),
            (3, gpt::MERIDIAN_SYSTEM, 4000)
        );
        assert_eq!(parts[1].1.name().collect::<String>(), "P");
    }

    #[test]
    fn each_partition_is_logged_by_its_name_size_and_type() {
        let image = disk(
            300_000,
            &[
                (0, gpt::EFI_SYSTEM.0, 2048, 133_119),
                (4, [0x11; 16], 133_120, 133_120),
            ],
        );
        let table = Table::read(&mut reader(&image)).unwrap();
        let lines: Vec<String> = table
            .partitions()
            .map(|(number, p)| {
                let mut out = String::new();
                crate::partition_line(&mut out, &"AHCI port 3", number, &p).unwrap();
                out
            })
            .collect();
        assert_eq!(
            lines,
            [
                "AHCI port 3, partition 1: P, 64 MiB from sector 2048, type C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n",
                "AHCI port 3, partition 5: P, 0 MiB from sector 133120, type 11111111-1111-1111-1111-111111111111\n",
            ]
        );
    }

    #[test]
    fn the_first_partition_of_a_type_is_found_by_it() {
        let image = disk(
            8192,
            &[
                (0, gpt::MERIDIAN_SYSTEM.0, 2048, 3071),
                (1, gpt::EFI_SYSTEM.0, 3072, 4095),
                (2, gpt::EFI_SYSTEM.0, 4096, 5119),
            ],
        );
        let table = Table::read(&mut reader(&image)).unwrap();
        assert_eq!(table.first_of(gpt::EFI_SYSTEM).map(|p| p.first), Some(3072));
        assert_eq!(
            table.first_of(gpt::MERIDIAN_SYSTEM).map(|p| p.first),
            Some(2048)
        );
        assert_eq!(table.first_of(gpt::Guid([0x11; 16])), None);
    }

    #[test]
    fn the_boot_volume_is_the_one_the_loader_names() {
        let mut image = disk(
            8192,
            &[
                (0, gpt::EFI_SYSTEM.0, 2048, 3071),
                (1, gpt::EFI_SYSTEM.0, 3072, 4095),
            ],
        );
        let unique = |slot: usize, id: u8| {
            let at = 1024 + slot * 128 + 16;
            (at, [id; 16])
        };
        for (at, id) in [unique(0, 0xa1), unique(1, 0xb2)] {
            image[at..at + 16].copy_from_slice(&id);
        }
        let crc = gpt::crc32(&image[1024..1024 + 128 * 128]);
        image[512 + 88..512 + 92].copy_from_slice(&crc.to_le_bytes());
        image[512 + 16..512 + 20].fill(0);
        let crc = gpt::crc32(&image[512..604]);
        image[528..532].copy_from_slice(&crc.to_le_bytes());
        let table = Table::read(&mut reader(&image)).unwrap();
        assert_eq!(
            table.boot_volume([0xb2; 16]).map(|p| p.first),
            Some(3072),
            "the second one, as named"
        );
        assert_eq!(table.boot_volume([0xa1; 16]).map(|p| p.first), Some(2048));
        assert_eq!(
            table.boot_volume([0; 16]).map(|p| p.first),
            Some(2048),
            "no word: the first"
        );
        assert_eq!(table.boot_volume([0xc3; 16]), None, "one on another disk");
    }

    #[test]
    fn a_disk_without_a_table_that_holds_has_none() {
        let image = disk(4096, &[(0, gpt::EFI_SYSTEM.0, 2048, 3071)]);
        assert!(
            Table::read(&mut |_, _| false).is_none(),
            "nothing could be read"
        );
        let mut no_entries = reader(&image);
        let mut header_only = |lba, into: &mut [u8]| lba == 1 && no_entries(lba, into);
        assert!(
            Table::read(&mut header_only).is_none(),
            "the entries could not be read"
        );
        let mut broken = image.clone();
        broken[1024 + 40] ^= 1;
        assert!(
            Table::read(&mut reader(&broken)).is_none(),
            "an entry changed behind its CRC"
        );
        let mut blank = image.clone();
        blank[512..520].fill(0);
        assert!(Table::read(&mut reader(&blank)).is_none(), "no header");
        assert!(
            Table::read(&mut reader(&image[..1024])).is_none(),
            "a disk too small for its array"
        );
    }

    /// The first 64 bytes of the configuration space of a function of a
    /// class, subclass and interface, with `bars`, as QEMU's storage
    /// controllers have them.
    fn function((class, subclass, interface): (u8, u8, u8), bars: [u32; 6]) -> pci::Header {
        let mut bytes = [0u8; 64];
        bytes[9..12].copy_from_slice(&[interface, subclass, class]);
        for (i, bar) in bars.iter().enumerate() {
            bytes[16 + 4 * i..20 + 4 * i].copy_from_slice(&bar.to_le_bytes());
        }
        pci::Header::read(&bytes).unwrap()
    }

    fn at(bus: u8, device: u8, function: u8) -> pci::Address {
        pci::Address {
            bus,
            device,
            function,
        }
    }

    #[test]
    fn the_first_controller_of_each_kind_is_driven() {
        let ahci = function((0x01, 0x06, 0x01), [0, 0, 0, 0, 0x10, 0xfebd_5000]);
        let nvme = function((0x01, 0x08, 0x02), [0xfebd_0004, 0x1, 0, 0, 0, 0]);
        let bridge = function((0x06, 0x04, 0x00), [0; 6]);
        let ide = function((0x01, 0x01, 0x80), [0xc001, 0, 0, 0, 0, 0xfebd_6000]);
        let mut found = Controllers::default();
        found.saw(at(0, 0, 0), &bridge);
        found.saw(at(0, 1, 0), &ide);
        assert_eq!(
            found,
            Controllers::default(),
            "neither is a controller the kernel drives"
        );
        found.saw(at(0, 3, 0), &nvme);
        found.saw(at(0, 0x1f, 2), &ahci);
        let first = found;
        found.saw(at(1, 0, 0), &nvme);
        found.saw(at(2, 0, 0), &ahci);
        assert_eq!(found, first, "later ones are left alone");
        let memory = |address| pci::Bar::Memory {
            address,
            prefetchable: false,
        };
        assert_eq!(found.ahci, Some((at(0, 0x1f, 2), memory(0xfebd_5000))));
        assert_eq!(found.nvme, Some((at(0, 3, 0), memory(0x1_febd_0000))));
    }

    #[test]
    fn the_first_virtio_block_device_is_driven() {
        let virtio = |device: u16| {
            let mut h = function((0x01, 0x00, 0x00), [0; 6]);
            h.vendor = virtio::VENDOR;
            h.device = device;
            h
        };
        let rng = virtio(0x1044);
        let mut found = Controllers::default();
        found.saw(at(1, 0, 0), &rng);
        assert_eq!(
            found,
            Controllers::default(),
            "a random number generator is no disk"
        );
        let mut other = virtio(virtio::BLOCK);
        other.vendor = 0x8086;
        found.saw(at(0, 2, 0), &other);
        assert_eq!(found.virtio, None, "nor is another vendor's device 0x1042");
        found.saw(at(0, 4, 0), &virtio(virtio::BLOCK));
        found.saw(at(0, 5, 0), &virtio(virtio::BLOCK_TRANSITIONAL));
        assert_eq!(found.virtio, Some((at(0, 4, 0), virtio(virtio::BLOCK))));
        let mut older = Controllers::default();
        older.saw(at(0, 5, 0), &virtio(virtio::BLOCK_TRANSITIONAL));
        assert_eq!(older.virtio.map(|(a, _)| a), Some(at(0, 5, 0)));
        assert_eq!((older.ahci, older.nvme), (None, None));
    }

    /// A FAT32 boot sector for a volume of `total` sectors of 512 bytes,
    /// `per_cluster` to a cluster, with two FATs of `fat` sectors each.
    fn fat32(total: u32, per_cluster: u8, fat: u32) -> [u8; 512] {
        let mut boot = [0u8; 512];
        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = per_cluster;
        boot[14..16].copy_from_slice(&32u16.to_le_bytes());
        boot[16] = 2;
        boot[32..36].copy_from_slice(&total.to_le_bytes());
        boot[36..40].copy_from_slice(&fat.to_le_bytes());
        boot[44..48].copy_from_slice(&2u32.to_le_bytes());
        boot[510..512].copy_from_slice(&[0x55, 0xaa]);
        boot
    }

    #[test]
    fn a_disk_is_logged_with_its_partitions_and_their_volumes() {
        let mut image = disk(
            140_000,
            &[
                (0, gpt::EFI_SYSTEM.0, 2048, 133_119),
                (1, gpt::MERIDIAN_SYSTEM.0, 133_120, 139_000),
            ],
        );
        image[2048 * 512..2049 * 512].copy_from_slice(&fat32(131_072, 1, 1009));
        let mut out = String::new();
        describe(&mut out, &"AHCI port 0", &mut reader(&image)).unwrap();
        assert_eq!(
            out,
            "kernel: AHCI port 0, partition 1: P, 64 MiB from sector 2048, type C12A7328-F81F-11D2-BA4B-00A0C93EC93B\n\
             kernel: AHCI port 0, partition 1 holds FAT32, 129022 clusters of 512 bytes\n\
             kernel: AHCI port 0, partition 2: P, 2 MiB from sector 133120, type B0DD29AB-2286-43EF-9809-E345D01A1B29\n"
        );
        let mut unreadable = reader(&image);
        let mut no_volumes = |lba, into: &mut [u8]| lba < 2048 && unreadable(lba, into);
        let mut out = String::new();
        describe(&mut out, &"AHCI port 1", &mut no_volumes).unwrap();
        assert_eq!(
            out.lines().count(),
            2,
            "no FAT32 line for a boot sector it could not read:\n{out}"
        );
        let mut out = String::new();
        describe(&mut out, &"AHCI port 1", &mut |_, _| false).unwrap();
        assert_eq!(out, "", "a disk without a table");
    }
}
