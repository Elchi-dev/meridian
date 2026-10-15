//! GUID partition tables (UEFI 2.10, chapter 5), read: the header with
//! every check the specification asks for, and the partitions its array
//! lists. Reading the sectors is the caller's.

#![cfg_attr(not(test), no_std)]

/// A GUID in its on-disk byte order: the first three fields little
/// endian, the last two as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// The GUID written as `a-b-c-d`, with `d` the last eight bytes.
    #[must_use]
    pub const fn new(a: u32, b: u16, c: u16, d: [u8; 8]) -> Guid {
        let a = a.to_le_bytes();
        let b = b.to_le_bytes();
        let c = c.to_le_bytes();
        Guid([
            a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5],
            d[6], d[7],
        ])
    }

    /// Whether it is all zeros, which marks an unused entry.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.0 == [0; 16]
    }
}

impl core::fmt::Display for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let g = &self.0;
        write!(
            f,
            "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-",
            g[3], g[2], g[1], g[0], g[5], g[4], g[7], g[6], g[8], g[9]
        )?;
        g[10..].iter().try_for_each(|b| write!(f, "{b:02X}"))
    }
}

/// The partition firmware starts loaders from.
pub const EFI_SYSTEM: Guid = Guid::new(
    0xc12a_7328,
    0xf81f,
    0x11d2,
    [0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b],
);

/// The partition that holds Meridian's own file system.
pub const MERIDIAN_SYSTEM: Guid = Guid::new(
    0xb0dd_29ab,
    0x2286,
    0x43ef,
    [0x98, 0x09, 0xe3, 0x45, 0xd0, 0x1a, 0x1b, 0x29],
);

/// The CRC-32 the header and the entry array are checked with, the one
/// of IEEE 802.3, zlib and PNG.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |crc, &byte| {
        (0..8).fold(crc ^ u32::from(byte), |c, _| {
            if c & 1 == 1 {
                c >> 1 ^ 0xedb8_8320
            } else {
                c >> 1
            }
        })
    })
}

/// Why a sector holds no header this crate trusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// It does not start with `EFI PART`.
    NotGpt,
    /// It is of a revision other than 1.0.
    Revision(u32),
    /// Its size is below 92 bytes or past its sector.
    Size(u32),
    /// Its CRC-32 does not match.
    Checksum,
    /// Its entries are not 128 bytes or a larger power of two, or the
    /// first usable sector comes after the last.
    Layout,
}

/// What a GPT header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The sector this header is in, and the one its copy is in.
    pub current: u64,
    pub backup: u64,
    pub first_usable: u64,
    pub last_usable: u64,
    pub disk: Guid,
    /// The first sector of the entry array, how many entries it has and
    /// how big each is.
    pub entries_at: u64,
    pub entries: u32,
    pub entry_size: u32,
    entries_crc: u32,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | u64::from(u32_at(b, at + 4)) << 32
}

impl Header {
    /// The header in `sector`, checked.
    ///
    /// # Errors
    ///
    /// When it is no GPT header, of another revision, of a size it cannot
    /// have, with a CRC-32 that does not match, or of a layout that makes
    /// no sense.
    pub fn read(sector: &[u8]) -> Result<Header, Error> {
        if sector.len() < 92 || sector[..8] != *b"EFI PART" {
            return Err(Error::NotGpt);
        }
        let revision = u32_at(sector, 8);
        if revision != 0x0001_0000 {
            return Err(Error::Revision(revision));
        }
        let size = u32_at(sector, 12);
        let len = usize::try_from(size).unwrap_or(usize::MAX);
        if len < 92 || len > sector.len() {
            return Err(Error::Size(size));
        }
        let mut copy = [0u8; 512];
        let checked = copy.get_mut(..len).ok_or(Error::Size(size))?;
        checked.copy_from_slice(&sector[..len]);
        checked[16..20].fill(0);
        if crc32(checked) != u32_at(sector, 16) {
            return Err(Error::Checksum);
        }
        let mut disk = [0u8; 16];
        disk.copy_from_slice(&sector[56..72]);
        let header = Header {
            current: u64_at(sector, 24),
            backup: u64_at(sector, 32),
            first_usable: u64_at(sector, 40),
            last_usable: u64_at(sector, 48),
            disk: Guid(disk),
            entries_at: u64_at(sector, 72),
            entries: u32_at(sector, 80),
            entry_size: u32_at(sector, 84),
            entries_crc: u32_at(sector, 88),
        };
        let entry_size_fits = header.entry_size >= 128 && header.entry_size.is_power_of_two();
        if !entry_size_fits || header.first_usable > header.last_usable {
            return Err(Error::Layout);
        }
        Ok(header)
    }

    /// How many bytes the entry array takes.
    #[must_use]
    pub fn entries_bytes(&self) -> u64 {
        u64::from(self.entries) * u64::from(self.entry_size)
    }

    /// The partitions in `array`, the entry array the header names, when
    /// its CRC-32 matches; the unused entries are left out.
    pub fn partitions<'a>(&self, array: &'a [u8]) -> Option<impl Iterator<Item = Partition> + 'a> {
        let len = usize::try_from(self.entries_bytes()).ok()?;
        let bytes = array.get(..len)?;
        if crc32(bytes) != self.entries_crc {
            return None;
        }
        let size = usize::try_from(self.entry_size).ok()?;
        Some(bytes.chunks_exact(size).filter_map(Partition::read))
    }
}

/// One partition the entry array lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub kind: Guid,
    pub unique: Guid,
    /// Its first and its last sector.
    pub first: u64,
    pub last: u64,
    pub attributes: u64,
    name: [u16; 36],
}

impl Partition {
    /// The partition in an entry of at least 128 bytes, or none for an
    /// unused one.
    fn read(entry: &[u8]) -> Option<Partition> {
        let mut kind = [0u8; 16];
        kind.copy_from_slice(&entry[..16]);
        let kind = Guid(kind);
        if kind.is_zero() {
            return None;
        }
        let mut unique = [0u8; 16];
        unique.copy_from_slice(&entry[16..32]);
        let mut name = [0u16; 36];
        for (i, unit) in name.iter_mut().enumerate() {
            *unit = u16::from_le_bytes([entry[56 + 2 * i], entry[57 + 2 * i]]);
        }
        Some(Partition {
            kind,
            unique: Guid(unique),
            first: u64_at(entry, 32),
            last: u64_at(entry, 40),
            attributes: u64_at(entry, 48),
            name,
        })
    }

    /// Its name, up to the first zero.
    pub fn name(&self) -> impl Iterator<Item = char> + '_ {
        let units = self.name.iter().copied().take_while(|&u| u != 0);
        char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// How many sectors it takes; none when its last comes before its
    /// first.
    #[must_use]
    pub fn sectors(&self) -> u64 {
        self.last.checked_sub(self.first).map_or(0, |n| n + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// LBA 1 of a test image sextant wrote, 66 MiB with one EFI system
    /// partition.
    fn sextant_header() -> Vec<u8> {
        let mut sector = hex(concat!(
            "4546492050415254000001005c00000048bb5d44000000000100000000000000",
            "ff0f0200000000002200000000000000de0f0200000000009fa99c6e55fc504b",
            "862e3801e99ee352020000000000000080000000800000002d5f91d5",
        ));
        sector.resize(512, 0);
        sector
    }

    /// The entry array of the same image: its first entry, then 127 unused.
    fn sextant_entries() -> Vec<u8> {
        let mut array = hex(concat!(
            "28732ac11ff8d211ba4b00a0c93ec93b281150d06f09494a832e5bd507f44a38",
            "0008000000000000ff07020000000000000000000000000045004600490020",
            "00730079007300740065006d00200070006100720074006900740069006f006e",
            "00000000000000000000000000000000000000000000000000000000000000",
            "0000",
        ));
        array.resize(128 * 128, 0);
        array
    }

    /// The last sector of the same image: the header's copy.
    fn sextant_backup() -> Vec<u8> {
        let mut sector = hex(concat!(
            "4546492050415254000001005c00000011b2acb000000000ff0f020000000000",
            "01000000000000002200000000000000de0f0200000000009fa99c6e55fc504b",
            "862e3801e99ee352df0f02000000000080000000800000002d5f91d5",
        ));
        sector.resize(512, 0);
        sector
    }

    #[test]
    fn the_copy_at_the_end_points_back_at_the_first() {
        let first = Header::read(&sextant_header()).unwrap();
        let copy = Header::read(&sextant_backup()).unwrap();
        assert_eq!(
            (copy.current, copy.backup),
            (first.backup, first.current),
            "each names the other"
        );
        assert_eq!(copy.entries_at, 135_135, "its entries right before it");
        assert_eq!(
            copy.entries_at + 32,
            copy.current,
            "32 sectors of 128 entries"
        );
        assert_eq!(
            (copy.first_usable, copy.last_usable, copy.disk),
            (first.first_usable, first.last_usable, first.disk)
        );
        assert_eq!(
            copy.partitions(&sextant_entries()).unwrap().count(),
            1,
            "the same entries"
        );
    }

    #[test]
    fn a_disk_past_2_tib_has_sectors_past_32_bits() {
        let big = changed(|s| {
            s[32..40].copy_from_slice(&0x1_2345_6789u64.to_le_bytes());
            s[48..56].copy_from_slice(&0x1_2345_6768u64.to_le_bytes());
        });
        let h = Header::read(&big).unwrap();
        assert_eq!((h.backup, h.last_usable), (0x1_2345_6789, 0x1_2345_6768));
        let mut entry = sextant_entries()[..128].to_vec();
        entry[44..48].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(Partition::read(&entry).unwrap().last, 0x1_0002_07ff);
    }

    #[test]
    fn a_header_may_fill_its_sector_exactly() {
        let short = &sextant_header()[..92];
        assert!(
            Header::read(short).is_ok(),
            "92 bytes, the header and nothing more"
        );
        let mut entry = sextant_entries()[..128].to_vec();
        let p = Partition::read(&entry).unwrap();
        assert_eq!(p.sectors(), 131_072);
        entry[40..48].copy_from_slice(&2048u64.to_le_bytes());
        assert_eq!(
            Partition::read(&entry).unwrap().sectors(),
            1,
            "first and last the same"
        );
        entry[40..48].copy_from_slice(&2047u64.to_le_bytes());
        assert_eq!(
            Partition::read(&entry).unwrap().sectors(),
            0,
            "ending before it starts"
        );
    }

    #[test]
    fn crc32_is_the_one_of_zlib() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926, "the check value");
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xe8b7_be43);
        assert_eq!(crc32(&[0; 16384]), 0xab54_d286);
    }

    #[test]
    fn the_header_sextant_wrote_reads_back() {
        let h = Header::read(&sextant_header()).unwrap();
        assert_eq!(
            (h.current, h.backup),
            (1, 135_167),
            "at the start and the end of 66 MiB"
        );
        assert_eq!((h.first_usable, h.last_usable), (34, 135_134));
        assert_eq!((h.entries_at, h.entries, h.entry_size), (2, 128, 128));
        assert_eq!(h.entries_bytes(), 16384);
        assert_eq!(h.disk.to_string(), "6E9CA99F-FC55-4B50-862E-3801E99EE352");
    }

    #[test]
    fn its_partition_is_the_efi_system_partition() {
        let h = Header::read(&sextant_header()).unwrap();
        let parts: Vec<Partition> = h.partitions(&sextant_entries()).unwrap().collect();
        assert_eq!(parts.len(), 1, "the unused entries are left out");
        let esp = parts[0];
        assert_eq!(esp.kind, EFI_SYSTEM);
        assert_eq!(
            (esp.first, esp.last, esp.sectors()),
            (2048, 133_119, 131_072),
            "64 MiB from 1 MiB on"
        );
        assert_eq!(esp.name().collect::<String>(), "EFI system partition");
        assert_eq!(esp.attributes, 0);
        assert_eq!(
            esp.unique.to_string(),
            "D0501128-096F-4A49-832E-5BD507F44A38"
        );
    }

    #[test]
    fn guids_read_as_they_are_written() {
        assert_eq!(
            EFI_SYSTEM.to_string(),
            "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
        );
        assert_eq!(
            MERIDIAN_SYSTEM.to_string(),
            "B0DD29AB-2286-43EF-9809-E345D01A1B29"
        );
        assert_eq!(
            &EFI_SYSTEM.0[..4],
            &[0x28, 0x73, 0x2a, 0xc1],
            "the first field little endian"
        );
        assert_eq!(
            &EFI_SYSTEM.0[8..],
            &[0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b],
            "the last as written"
        );
        assert!(Guid([0; 16]).is_zero() && !EFI_SYSTEM.is_zero());
    }

    /// The header with `change` made and its CRC-32 put right again.
    fn changed(change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut sector = sextant_header();
        change(&mut sector);
        sector[16..20].fill(0);
        let crc = crc32(&sector[..92]);
        sector[16..20].copy_from_slice(&crc.to_le_bytes());
        sector
    }

    #[test]
    fn a_header_has_to_be_one() {
        let mut other = sextant_header();
        other[0] = b'X';
        assert_eq!(Header::read(&other), Err(Error::NotGpt));
        assert_eq!(
            Header::read(&sextant_header()[..91]),
            Err(Error::NotGpt),
            "too short"
        );
        let revision = changed(|s| s[8..12].copy_from_slice(&0x0002_0000u32.to_le_bytes()));
        assert_eq!(Header::read(&revision), Err(Error::Revision(0x0002_0000)));
        let small = changed(|s| s[12..16].copy_from_slice(&91u32.to_le_bytes()));
        assert_eq!(Header::read(&small), Err(Error::Size(91)));
        let large = changed(|s| s[12..16].copy_from_slice(&513u32.to_le_bytes()));
        assert_eq!(
            Header::read(&large),
            Err(Error::Size(513)),
            "past its sector"
        );
        let mut flipped = sextant_header();
        flipped[40] ^= 1;
        assert_eq!(
            Header::read(&flipped),
            Err(Error::Checksum),
            "a bit changed, the CRC not"
        );
        let mut crc = sextant_header();
        crc[16] ^= 1;
        assert_eq!(Header::read(&crc), Err(Error::Checksum));
    }

    #[test]
    fn a_header_has_to_make_sense() {
        let small_entries = changed(|s| s[84..88].copy_from_slice(&64u32.to_le_bytes()));
        assert_eq!(Header::read(&small_entries), Err(Error::Layout));
        let odd_entries = changed(|s| s[84..88].copy_from_slice(&192u32.to_le_bytes()));
        assert_eq!(
            Header::read(&odd_entries),
            Err(Error::Layout),
            "not a power of two"
        );
        let large_entries = changed(|s| s[84..88].copy_from_slice(&256u32.to_le_bytes()));
        assert!(Header::read(&large_entries).is_ok(), "256 bytes is allowed");
        let backwards = changed(|s| s[40..48].copy_from_slice(&200_000u64.to_le_bytes()));
        assert_eq!(Header::read(&backwards), Err(Error::Layout));
        let one_sector = changed(|s| s[40..48].copy_from_slice(&135_134u64.to_le_bytes()));
        assert!(
            Header::read(&one_sector).is_ok(),
            "first and last usable the same"
        );
    }

    #[test]
    fn a_larger_header_counts_its_zeros_in_the_crc() {
        let longer = changed(|s| s[12..16].copy_from_slice(&96u32.to_le_bytes()));
        assert_eq!(
            Header::read(&longer),
            Err(Error::Checksum),
            "the CRC above covered 92 bytes"
        );
        let mut right = longer.clone();
        right[16..20].fill(0);
        let crc = crc32(&right[..96]);
        right[16..20].copy_from_slice(&crc.to_le_bytes());
        assert!(Header::read(&right).is_ok());
    }

    #[test]
    fn the_entry_array_has_to_match_its_crc() {
        let h = Header::read(&sextant_header()).unwrap();
        let mut entries = sextant_entries();
        entries[40] ^= 1;
        assert!(h.partitions(&entries).is_none(), "a changed entry");
        assert!(
            h.partitions(&sextant_entries()[..16383]).is_none(),
            "an array cut short"
        );
        let mut longer = sextant_entries();
        longer.extend_from_slice(&[0xff; 512]);
        assert_eq!(
            h.partitions(&longer).unwrap().count(),
            1,
            "what follows the array is not read"
        );
    }

    #[test]
    fn entries_larger_than_128_bytes_are_read_by_their_size() {
        let mut array = vec![0u8; 4 * 256];
        let first = &sextant_entries()[..128];
        array[256..384].copy_from_slice(first);
        array[768..896].copy_from_slice(first);
        array[768 + 32] = 0x10;
        let header = changed(|s| {
            s[80..84].copy_from_slice(&4u32.to_le_bytes());
            s[84..88].copy_from_slice(&256u32.to_le_bytes());
            s[88..92].copy_from_slice(&crc32(&array).to_le_bytes());
        });
        let h = Header::read(&header).unwrap();
        let parts: Vec<Partition> = h.partitions(&array).unwrap().collect();
        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].first, parts[1].first), (2048, 2064));
    }

    #[test]
    fn names_end_at_their_first_zero_and_survive_bad_units() {
        let mut entry = sextant_entries()[..128].to_vec();
        entry[56..60].copy_from_slice(&[0x00, 0xd8, 0x41, 0x00]);
        let p = Partition::read(&entry).unwrap();
        assert_eq!(
            p.name().next(),
            Some(char::REPLACEMENT_CHARACTER),
            "a lone surrogate"
        );
        assert_eq!(p.name().nth(1), Some('A'));
        let mut full = entry.clone();
        for unit in full[56..128].chunks_mut(2) {
            unit.copy_from_slice(&[b'x', 0]);
        }
        assert_eq!(
            Partition::read(&full).unwrap().name().count(),
            36,
            "36 units, no zero"
        );
        assert_eq!(Partition::read(&[0u8; 128]), None, "an unused entry");
    }
}
