//! GUID partition tables written, for a disk of 512-byte sectors: the
//! protective MBR, the header with its entry array at the start of the
//! disk, and their copies at its end, all as UEFI 2.10 chapter 5 asks.

use crate::{Guid, Partition, crc32_continue};

/// How many entries the array has, and how big each is, as every common
/// tool writes them: 32 sectors in all.
const ENTRIES: u32 = 128;
const ENTRY_SIZE: u32 = 128;
const ARRAY_SECTORS: u64 = 32;

/// The sector partitions are best aligned to, at 1 MiB.
pub const ALIGN: u64 = 2048;

/// Why a table was not written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The disk is too small to hold the table and its copy.
    Small,
    /// More partitions were asked for than the array has entries.
    TooMany,
    /// Partition `n`, counted from 0, has no kind, or does not lie in
    /// the usable sectors, or ends before it starts.
    Outside(usize),
    /// Partitions `a` and `b` share sectors.
    Overlap(usize, usize),
    /// A name has more than the 36 UTF-16 units an entry holds.
    Name,
    /// The sector given could not be written.
    Write(u64),
}

impl Partition {
    /// A partition of `kind` called `name`, with the GUID `unique`, from
    /// sector `first` to `last` with both in it.
    ///
    /// # Errors
    ///
    /// When the name takes more than 36 UTF-16 units.
    pub fn new(
        kind: Guid,
        unique: Guid,
        first: u64,
        last: u64,
        name: &str,
    ) -> Result<Partition, Error> {
        let mut units = [0u16; 36];
        let mut room = units.iter_mut();
        for unit in name.encode_utf16() {
            *room.next().ok_or(Error::Name)? = unit;
        }
        Ok(Partition {
            kind,
            unique,
            first,
            last,
            attributes: 0,
            name: units,
        })
    }

    /// The 128 bytes of its entry.
    fn entry(&self) -> [u8; 128] {
        let mut e = [0u8; 128];
        e[..16].copy_from_slice(&self.kind.0);
        e[16..32].copy_from_slice(&self.unique.0);
        e[32..40].copy_from_slice(&self.first.to_le_bytes());
        e[40..48].copy_from_slice(&self.last.to_le_bytes());
        e[48..56].copy_from_slice(&self.attributes.to_le_bytes());
        for (i, unit) in self.name.iter().enumerate() {
            e[56 + 2 * i..58 + 2 * i].copy_from_slice(&unit.to_le_bytes());
        }
        e
    }
}

/// The first and the last sector partitions may use on a disk of `total`
/// sectors, or none when it is too small to hold the table twice.
#[must_use]
pub fn usable(total: u64) -> Option<(u64, u64)> {
    let first = 2 + ARRAY_SECTORS;
    let last = total.checked_sub(first)?;
    (first <= last).then_some((first, last))
}

/// `sector`, or the first sector after it on a boundary of [`ALIGN`].
#[must_use]
pub fn aligned(sector: u64) -> u64 {
    sector.div_ceil(ALIGN) * ALIGN
}

/// Writes a table that lists `partitions` on a disk of `total` sectors
/// with the GUID `disk`, through `write`, which puts 512 bytes at the
/// sector it is given and says whether it could. Each partition takes
/// the entry of its place in the slice. The entries go before the
/// headers that name them, the copies first and the protective MBR last,
/// so a write cut off halfway leaves no header that names entries not yet
/// there.
///
/// # Errors
///
/// When the disk is too small, a partition is outside its usable sectors
/// or shares some with another, there are more than 128, or a sector
/// could not be written.
pub fn write(
    total: u64,
    disk: Guid,
    partitions: &[Partition],
    mut write: impl FnMut(u64, &[u8]) -> bool,
) -> Result<(), Error> {
    let (first, last) = usable(total).ok_or(Error::Small)?;
    if partitions.len() > ENTRIES as usize {
        return Err(Error::TooMany);
    }
    for (n, p) in partitions.iter().enumerate() {
        if p.kind.is_zero() || p.first < first || p.last > last || p.last < p.first {
            return Err(Error::Outside(n));
        }
        let earlier = partitions[..n]
            .iter()
            .position(|o| o.first <= p.last && p.first <= o.last);
        if let Some(o) = earlier {
            return Err(Error::Overlap(o, n));
        }
    }
    let mut put = |at: u64, bytes: &[u8]| {
        if write(at, bytes) {
            Ok(())
        } else {
            Err(Error::Write(at))
        }
    };
    let copy = total - 1 - ARRAY_SECTORS;
    let mut crc = u32::MAX;
    let mut fours = partitions.chunks(4);
    for sector in 0..ARRAY_SECTORS {
        let mut bytes = [0u8; 512];
        let rooms = bytes.as_chunks_mut::<128>().0.iter_mut();
        for (room, p) in rooms.zip(fours.next().unwrap_or_default()) {
            *room = p.entry();
        }
        crc = crc32_continue(crc, &bytes);
        put(copy + sector, &bytes)?;
        put(2 + sector, &bytes)?;
    }
    let header = |current: u64, backup: u64, entries_at: u64| {
        let mut h = [0u8; 512];
        h[..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&current.to_le_bytes());
        h[32..40].copy_from_slice(&backup.to_le_bytes());
        h[40..48].copy_from_slice(&first.to_le_bytes());
        h[48..56].copy_from_slice(&last.to_le_bytes());
        h[56..72].copy_from_slice(&disk.0);
        h[72..80].copy_from_slice(&entries_at.to_le_bytes());
        h[80..84].copy_from_slice(&ENTRIES.to_le_bytes());
        h[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        h[88..92].copy_from_slice(&(!crc).to_le_bytes());
        let own = !crc32_continue(u32::MAX, &h[..92]);
        h[16..20].copy_from_slice(&own.to_le_bytes());
        h
    };
    put(total - 1, &header(total - 1, 1, copy))?;
    put(1, &header(1, total - 1, 2))?;
    put(0, &protective_mbr(total))
}

/// The MBR that tells tools from before GPT that the whole disk is taken,
/// by one partition of type 0xEE from sector 1 to the end, or as far as
/// 32 bits reach.
#[must_use]
pub fn protective_mbr(total: u64) -> [u8; 512] {
    let mut m = [0u8; 512];
    let last = total.saturating_sub(1);
    let p = &mut m[446..462];
    p[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    p[4] = 0xee;
    p[5..8].copy_from_slice(&chs(last));
    p[8..12].copy_from_slice(&1u32.to_le_bytes());
    p[12..16].copy_from_slice(&u32::try_from(last).unwrap_or(u32::MAX).to_le_bytes());
    m[510..512].copy_from_slice(&[0x55, 0xaa]);
    m
}

/// Where `sector` is for a disk of 255 heads and 63 sectors to the track,
/// as an MBR holds it, or 0xFFFFFF when that is past cylinder 1023.
fn chs(sector: u64) -> [u8; 3] {
    let track = sector / 63;
    let (cylinder, head, inside) = (track / 255, track % 255, sector % 63 + 1);
    if cylinder > 1023 {
        return [0xff; 3];
    }
    // Each fits its byte but the cylinder, whose two high bits go above
    // the sector's six.
    let byte = |n: u64| n.to_le_bytes()[0];
    [
        byte(head),
        byte(inside) | byte(cylinder >> 8 << 6),
        byte(cylinder),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{sextant_backup, sextant_entries, sextant_header};
    use crate::{EFI_SYSTEM, Header, MERIDIAN_SYSTEM, crc32};
    use std::collections::BTreeMap;
    use std::process::Command;

    /// The sectors a table was written into, as they were written last,
    /// and the order they came in.
    fn written(
        total: u64,
        disk: Guid,
        partitions: &[Partition],
    ) -> (BTreeMap<u64, Vec<u8>>, Vec<u64>) {
        let (mut sectors, mut order) = (BTreeMap::new(), Vec::new());
        write(total, disk, partitions, |at, bytes: &[u8]| {
            assert_eq!(bytes.len(), 512);
            sectors.insert(at, bytes.to_vec());
            order.push(at);
            true
        })
        .unwrap();
        (sectors, order)
    }

    fn run_of(sectors: &BTreeMap<u64, Vec<u8>>, from: u64) -> Vec<u8> {
        (from..from + 32)
            .flat_map(|s| sectors[&s].clone())
            .collect()
    }

    fn guid(bytes: &[u8]) -> Guid {
        Guid(bytes.try_into().unwrap())
    }

    /// The partition of the image sextant wrote.
    fn sextant_partition() -> Partition {
        let entry = sextant_entries();
        Partition::new(
            EFI_SYSTEM,
            guid(&entry[16..32]),
            2048,
            133_119,
            "EFI system partition",
        )
        .unwrap()
    }

    #[test]
    fn the_table_sextant_wrote_is_written_byte_for_byte() {
        let disk = guid(&sextant_header()[56..72]);
        let (sectors, _) = written(135_168, disk, &[sextant_partition()]);
        assert_eq!(sectors[&1], sextant_header());
        assert_eq!(run_of(&sectors, 2), sextant_entries());
        assert_eq!(sectors[&135_167], sextant_backup());
        assert_eq!(
            run_of(&sectors, 135_135),
            sextant_entries(),
            "the entries' copy"
        );
        assert_eq!(sectors.len(), 1 + 1 + 32 + 32 + 1, "and nothing else");
    }

    #[test]
    fn the_copies_come_first_and_the_mbr_last() {
        let disk = guid(&sextant_header()[56..72]);
        let (_, order) = written(135_168, disk, &[sextant_partition()]);
        assert_eq!(order.first(), Some(&135_135));
        let header_copy = order.iter().position(|&s| s == 135_167).unwrap();
        let header = order.iter().position(|&s| s == 1).unwrap();
        assert!(header_copy < header && header == order.len() - 2);
        assert_eq!(order.last(), Some(&0));
    }

    /// Runs `tool` with `args`, the path of a file holding `image` in
    /// place of `IMAGE`, and answers whether it was content and what it
    /// printed, all on one line; none when the tool is missing.
    fn run(tool: &str, image: &[u8], args: &[&str]) -> Option<(bool, String)> {
        if Command::new(tool).arg("--version").output().is_err() {
            assert!(
                std::env::var("SEXTANT_REQUIRE_TOOLS").as_deref() != Ok("1"),
                "{tool} is required but missing"
            );
            return None;
        }
        let path = std::env::temp_dir().join(format!(
            "meridian-gpt-{}-{tool}-{}.img",
            std::process::id(),
            args.join("")
        ));
        std::fs::write(&path, image).unwrap();
        let args = args.iter().map(|&a| {
            if a == "IMAGE" {
                path.as_os_str()
            } else {
                a.as_ref()
            }
        });
        let out = Command::new(tool).args(args).output().unwrap();
        let _ = std::fs::remove_file(&path);
        let said = [out.stdout, out.stderr].concat();
        let said = String::from_utf8_lossy(&said)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        Some((out.status.success(), said))
    }

    /// An 8 MiB disk of junk, with a table of an EFI system partition that
    /// has to stay and a Meridian system partition after it.
    fn two_partitions() -> (Vec<u8>, Guid, [Partition; 2]) {
        let disk = Guid::new(0x1234_5678, 0x9abc, 0xdef0, [1, 2, 3, 4, 5, 6, 7, 8]);
        let mut esp = Partition::new(
            EFI_SYSTEM,
            Guid::new(0xaaaa_0001, 2, 3, [4; 8]),
            ALIGN,
            5 * ALIGN - 1,
            "EFI system partition",
        )
        .unwrap();
        esp.attributes = 1;
        let (_, last) = usable(16_384).unwrap();
        let system = Partition::new(
            MERIDIAN_SYSTEM,
            Guid::new(0xbbbb_0001, 2, 3, [5; 8]),
            aligned(esp.last),
            last,
            "Meridian",
        )
        .unwrap();
        let mut image = vec![0xa5u8; 16_384 * 512];
        write(16_384, disk, &[esp, system], |at, bytes: &[u8]| {
            let at = usize::try_from(at).unwrap() * 512;
            image[at..at + 512].copy_from_slice(bytes);
            true
        })
        .unwrap();
        (image, disk, [esp, system])
    }

    #[test]
    fn sfdisk_reads_the_table_written() {
        let (image, disk, [esp, system]) = two_partitions();
        let Some((read, dump)) = run("sfdisk", &image, &["--dump", "IMAGE"]) else {
            return;
        };
        assert!(read, "{dump}");
        let wanted = [
            "label: gpt".to_string(),
            format!("label-id: {disk}"),
            "first-lba: 34 last-lba: 16350".to_string(),
            format!(
                "start= 2048, size= 8192, type={}, uuid={}, name=\"EFI system partition\", attrs=\"RequiredPartition\"",
                esp.kind, esp.unique
            ),
            format!(
                "start= 10240, size= 6111, type={}, uuid={}, name=\"Meridian\"",
                system.kind, system.unique
            ),
        ];
        for want in wanted {
            assert!(dump.contains(&want), "{want} is missing in {dump}");
        }
        let (sound, verified) = run("sfdisk", &image, &["--verify", "IMAGE"]).unwrap();
        assert!(
            sound && verified.contains("No errors detected"),
            "{verified}"
        );
    }

    #[test]
    fn fdisk_finds_the_protective_mbr_whole() {
        let (image, ..) = two_partitions();
        let Some((listed, said)) = run("fdisk", &image, &["-l", "IMAGE"]) else {
            return;
        };
        assert!(listed && said.contains("Disklabel type: gpt"), "{said}");
        let lower = said.to_lowercase();
        assert!(
            !lower.contains("mismatch") && !lower.contains("corrupt"),
            "{said}"
        );
    }

    #[test]
    fn the_table_reads_back_as_written() {
        let (image, disk, partitions) = two_partitions();
        for (at, other) in [(1, 16_383), (16_383, 1)] {
            let start = at * 512;
            let h = Header::read(&image[start..start + 512]).unwrap();
            assert_eq!((h.current, h.backup, h.disk), (at as u64, other, disk));
            let array = usize::try_from(h.entries_at).unwrap() * 512;
            let found: Vec<_> = h.partitions(&image[array..]).unwrap().collect();
            assert_eq!(found, [(1, partitions[0]), (2, partitions[1])]);
        }
        assert_eq!(&image[446..462], &protective_mbr(16_384)[446..462]);
        assert_eq!(
            image[34 * 512],
            0xa5,
            "the partitions' sectors are left alone"
        );
    }

    #[test]
    fn a_disk_past_2_tib_has_its_mbr_capped() {
        let total = 1 << 33;
        let (sectors, _) = written(total, Guid([9; 16]), &[]);
        let m = &sectors[&0];
        assert_eq!(
            &m[446..462],
            &[
                0, 0, 2, 0, 0xee, 0xff, 0xff, 0xff, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff
            ]
        );
        assert_eq!((m[510], m[511]), (0x55, 0xaa));
        let copy = Header::read(&sectors[&(total - 1)]).unwrap();
        assert_eq!(
            (copy.current, copy.entries_at, copy.last_usable),
            (total - 1, total - 33, total - 34)
        );
        assert_eq!(
            copy.partitions(&run_of(&sectors, total - 33))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn the_mbr_names_the_last_sector_as_cylinder_head_and_sector() {
        assert_eq!(chs(16_383), [5, 4, 1], "cylinder 1, head 5, sector 4");
        let at = |c: u64, h: u64, s: u64| (c * 255 + h) * 63 + s - 1;
        assert_eq!(
            chs(at(300, 7, 10)),
            [7, 0x0a | 0x40, 44],
            "the cylinder's high bits"
        );
        assert_eq!(
            chs(at(1023, 254, 63)),
            [254, 0xff, 0xff],
            "the last one that fits"
        );
        assert_eq!(chs(at(1023, 254, 63) + 1), [0xff; 3]);
        assert_eq!(
            &protective_mbr(16_384)[446..462],
            &[0, 0, 2, 0, 0xee, 5, 4, 1, 1, 0, 0, 0, 0xff, 0x3f, 0, 0]
        );
    }

    #[test]
    fn a_table_that_cannot_be_is_refused() {
        let disk = Guid([3; 16]);
        let at = |first, last| Partition::new(EFI_SYSTEM, Guid([1; 16]), first, last, "x").unwrap();
        let refused = |total, ps: &[Partition]| write(total, disk, ps, |_, _: &[u8]| true);
        assert_eq!(refused(67, &[]), Err(Error::Small));
        assert_eq!(
            usable(68),
            Some((34, 34)),
            "the smallest disk, of one usable sector"
        );
        assert_eq!(refused(68, &[at(34, 34)]), Ok(()));
        assert_eq!(refused(68, &[at(34, 35)]), Err(Error::Outside(0)));
        assert_eq!(
            refused(100, &[at(40, 50), at(33, 34)]),
            Err(Error::Outside(1))
        );
        assert_eq!(refused(100, &[at(50, 40)]), Err(Error::Outside(0)));
        let mut none = at(40, 50);
        none.kind = Guid([0; 16]);
        assert_eq!(
            refused(100, &[none]),
            Err(Error::Outside(0)),
            "an entry with no kind is unused"
        );
        assert_eq!(
            refused(100, &[at(34, 40), at(41, 50), at(50, 60)]),
            Err(Error::Overlap(1, 2))
        );
        assert_eq!(
            refused(100, &[at(45, 50), at(34, 45)]),
            Err(Error::Overlap(0, 1))
        );
        assert_eq!(
            refused(100, &[at(45, 50), at(40, 60)]),
            Err(Error::Overlap(0, 1))
        );
        let many: Vec<_> = (0..129).map(|n| at(34 + n, 34 + n)).collect();
        assert_eq!(refused(1000, &many[..128]), Ok(()), "every entry used");
        assert_eq!(refused(1000, &many), Err(Error::TooMany));
        let broken = write(100, disk, &[], |at, _: &[u8]| at != 67);
        assert_eq!(broken, Err(Error::Write(67)), "the first copy sector");
    }

    #[test]
    fn names_take_36_utf16_units_at_most() {
        let named = |name: &str| Partition::new(EFI_SYSTEM, Guid([1; 16]), 40, 50, name);
        let long = "n".repeat(36);
        assert_eq!(named(&long).unwrap().name().collect::<String>(), long);
        assert_eq!(named(&"n".repeat(37)), Err(Error::Name));
        let wide = "\u{1f600}".repeat(18);
        assert_eq!(
            named(&wide).unwrap().name().collect::<String>(),
            wide,
            "two units each"
        );
        assert_eq!(named(&format!("{wide}n")), Err(Error::Name));
        assert_eq!(
            named("Meridian").unwrap().entry()[56..74],
            *b"M\0e\0r\0i\0d\0i\0a\0n\0\0\0"
        );
    }

    #[test]
    fn partitions_start_on_a_mib_when_aligned() {
        assert_eq!(
            [aligned(0), aligned(1), aligned(2048), aligned(2049)],
            [0, 2048, 2048, 4096]
        );
    }

    #[test]
    fn a_crc_taken_in_parts_is_the_same() {
        let mut parts = b"123456789".chunks(4);
        let crc = parts
            .try_fold(u32::MAX, |c, p| Some(crc32_continue(c, p)))
            .unwrap();
        assert_eq!(
            !crc,
            crc32(b"123456789"),
            "a CRC taken in parts is the same"
        );
    }
}
