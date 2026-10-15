//! FAT32 (Microsoft's FAT specification, fatgen103), read: the geometry
//! the boot sector gives, the chains of clusters files lie in, directories
//! with their long names, and files found by path. The caller reads the
//! volume's sectors; sectors are counted from the volume's first.

#![cfg_attr(not(test), no_std)]

/// Why something on a volume could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The boot sector is no FAT boot sector, or one this crate does not
    /// read: sectors other than 512 to 4096 bytes, or a cluster size that
    /// is no power of two.
    NotFat,
    /// It is FAT, but FAT12 or FAT16 by its count of clusters.
    NotFat32,
    /// A sector could not be read.
    Read,
    /// A chain of clusters leaves the volume, loops, or ends too soon.
    BadChain,
    /// No entry has the name.
    NotFound,
    /// A name on the way is a file, not a directory.
    NotADirectory,
}

/// The geometry of a FAT32 volume, from its boot sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub sector_size: u32,
    pub sectors_per_cluster: u32,
    reserved: u32,
    fats: u32,
    fat_sectors: u32,
    pub root: u32,
    /// How many clusters the data region holds, the first of them number 2.
    pub clusters: u32,
}

fn u16_at(b: &[u8], at: usize) -> u32 {
    u32::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Where a chain ends: every entry from here on marks its last cluster.
const END: u32 = 0x0fff_fff8;

impl Volume {
    /// The volume whose boot sector is `boot`.
    ///
    /// # Errors
    ///
    /// When it is no FAT32 boot sector this crate reads.
    pub fn read(boot: &[u8; 512]) -> Result<Volume, Error> {
        let sector_size = u16_at(boot, 11);
        let sectors_per_cluster = u32::from(boot[13]);
        let reserved = u16_at(boot, 14);
        let fats = u32::from(boot[16]);
        let fat16_sectors = u16_at(boot, 22);
        let total16 = u16_at(boot, 19);
        let fat_sectors = if fat16_sectors == 0 {
            u32_at(boot, 36)
        } else {
            fat16_sectors
        };
        let total = if total16 == 0 {
            u32_at(boot, 32)
        } else {
            total16
        };
        let root_entries = u16_at(boot, 17);
        let sane = boot[510..512] == [0x55, 0xaa]
            && (512..=4096).contains(&sector_size)
            && sector_size.is_power_of_two()
            && sectors_per_cluster.is_power_of_two()
            && reserved > 0
            && fats > 0
            && fat_sectors > 0;
        if !sane {
            return Err(Error::NotFat);
        }
        // FAT12 and FAT16 have a root directory of a fixed size and give
        // their FAT's size in 16 bits; FAT32 has neither.
        if root_entries != 0 || fat16_sectors != 0 {
            return Err(Error::NotFat32);
        }
        let data = reserved + fats * fat_sectors;
        let clusters = total.checked_sub(data).ok_or(Error::NotFat)? / sectors_per_cluster;
        if clusters < 65525 {
            return Err(Error::NotFat32);
        }
        Ok(Volume {
            sector_size,
            sectors_per_cluster,
            reserved,
            fats,
            fat_sectors,
            root: u32_at(boot, 44),
            clusters,
        })
    }

    /// The first sector of cluster `cluster`, or none for a number that is
    /// no cluster of the data region.
    #[must_use]
    pub fn first_sector(&self, cluster: u32) -> Option<u64> {
        let index = cluster.checked_sub(2).filter(|&i| i < self.clusters)?;
        let data = u64::from(self.reserved) + u64::from(self.fats) * u64::from(self.fat_sectors);
        Some(data + u64::from(index) * u64::from(self.sectors_per_cluster))
    }

    /// The sector of the first FAT that holds cluster `cluster`'s entry,
    /// and where in it the entry lies.
    #[must_use]
    pub fn entry_of(&self, cluster: u32) -> (u64, usize) {
        let at = u64::from(cluster) * 4;
        let size = u64::from(self.sector_size);
        let offset = usize::try_from(at % size).unwrap_or(0);
        (u64::from(self.reserved) + at / size, offset)
    }

    /// How many bytes a cluster holds.
    #[must_use]
    pub fn cluster_bytes(&self) -> usize {
        (self.sector_size * self.sectors_per_cluster) as usize
    }
}

/// What a directory entry says about a file or directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its first cluster, 0 for an empty file.
    pub first: u32,
    pub size: u32,
    pub directory: bool,
}

impl Entry {
    fn read(e: &[u8]) -> Entry {
        Entry {
            first: u16_at(e, 20) << 16 | u16_at(e, 26),
            size: u32_at(e, 28),
            directory: e[11] & 0x10 != 0,
        }
    }
}

/// The checksum of a short name that ties its long name entries to it.
#[must_use]
pub fn checksum(short: &[u8]) -> u8 {
    short[..11]
        .iter()
        .fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}

/// Where in a long name entry its 13 UTF-16 units lie.
const UNITS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

/// The long name collected from the entries before a short one.
struct LongName {
    units: [u16; 260],
    checksum: u8,
    /// The number the next entry has to have, 0 once the name is whole,
    /// or none when what came is no name.
    next: Option<u8>,
}

impl LongName {
    fn new() -> LongName {
        LongName {
            units: [0; 260],
            checksum: 0,
            next: None,
        }
    }

    /// Takes one long name entry. They come last part first, numbered down
    /// to 1, all with the same checksum.
    fn add(&mut self, e: &[u8]) {
        let number = e[0] & 0x1f;
        if e[0] & 0x40 != 0 {
            self.units = [0; 260];
            self.checksum = e[13];
            self.next = Some(number);
        }
        if number == 0 || number > 20 || self.next != Some(number) || e[13] != self.checksum {
            self.next = None;
            return;
        }
        let at = (usize::from(number) - 1) * 13;
        for (i, &offset) in UNITS.iter().enumerate() {
            self.units[at + i] = u16::from_le_bytes([e[offset], e[offset + 1]]);
        }
        self.next = Some(number - 1);
    }

    /// The name, when it is whole and belongs to the short entry `short`.
    fn of(&self, short: &[u8]) -> Option<&[u16]> {
        if self.next != Some(0) || self.checksum != checksum(short) {
            return None;
        }
        let len = self
            .units
            .iter()
            .position(|&u| u == 0 || u == 0xffff)
            .unwrap_or(self.units.len());
        Some(&self.units[..len])
    }
}

/// Whether `units` spell `name`, ASCII letters in either case.
fn same(units: impl Iterator<Item = u16>, name: &str) -> bool {
    let fold = |u: u16| {
        if (u16::from(b'a')..=u16::from(b'z')).contains(&u) {
            u - 32
        } else {
            u
        }
    };
    units.map(fold).eq(name.encode_utf16().map(fold))
}

/// The units a short name reads as: its base, and a period and its
/// extension when it has one.
fn short_name(e: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let base = e[..8].iter().rposition(|&b| b != b' ').map_or(0, |p| p + 1);
    let ext = e[8..11]
        .iter()
        .rposition(|&b| b != b' ')
        .map_or(0, |p| p + 1);
    let dot = (ext > 0).then_some(b'.');
    e[..base]
        .iter()
        .copied()
        .chain(dot)
        .chain(e[8..8 + ext].iter().copied())
        .map(u16::from)
}

/// A FAT32 volume, read through `read`, which reads the volume's sector it
/// is given into the room it is given and says whether it could.
pub struct Fat<R> {
    pub volume: Volume,
    read: R,
}

impl<R: FnMut(u64, &mut [u8]) -> bool> Fat<R> {
    /// The volume `read` reads, from its boot sector.
    ///
    /// # Errors
    ///
    /// When the boot sector cannot be read or is no FAT32 one.
    pub fn open(mut read: R) -> Result<Fat<R>, Error> {
        let mut boot = [0u8; 512];
        if !read(0, &mut boot) {
            return Err(Error::Read);
        }
        Ok(Fat {
            volume: Volume::read(&boot)?,
            read,
        })
    }

    /// The cluster after `cluster` in its chain, or none at its end.
    ///
    /// # Errors
    ///
    /// When the FAT cannot be read, or names a cluster the volume does not
    /// have.
    pub fn next(&mut self, cluster: u32) -> Result<Option<u32>, Error> {
        let (sector, offset) = self.volume.entry_of(cluster);
        let mut room = [0u8; 4096];
        let size = self.volume.sector_size as usize;
        if !(self.read)(sector, &mut room[..size]) {
            return Err(Error::Read);
        }
        let next = u32_at(&room, offset) & 0x0fff_ffff;
        if next >= END {
            return Ok(None);
        }
        self.volume
            .first_sector(next)
            .map(|_| Some(next))
            .ok_or(Error::BadChain)
    }

    /// Hands every sector of the chain from `first` on to `each`, in order,
    /// until it answers false or the chain ends.
    fn walk(&mut self, first: u32, mut each: impl FnMut(&[u8]) -> bool) -> Result<(), Error> {
        let size = self.volume.sector_size as usize;
        let mut room = [0u8; 4096];
        let mut cluster = first;
        for _ in 0..self.volume.clusters {
            let start = self.volume.first_sector(cluster).ok_or(Error::BadChain)?;
            for sector in start..start + u64::from(self.volume.sectors_per_cluster) {
                if !(self.read)(sector, &mut room[..size]) {
                    return Err(Error::Read);
                }
                if !each(&room[..size]) {
                    return Ok(());
                }
            }
            match self.next(cluster)? {
                Some(next) => cluster = next,
                None => return Ok(()),
            }
        }
        Err(Error::BadChain)
    }

    /// The entry called `name` in the directory whose first cluster is
    /// `directory`: by its long name when it has one, else by its short.
    ///
    /// # Errors
    ///
    /// When no entry has the name, or the directory cannot be read.
    pub fn lookup(&mut self, directory: u32, name: &str) -> Result<Entry, Error> {
        let mut long = LongName::new();
        let mut found = None;
        self.walk(directory, |sector| {
            for e in sector.as_chunks::<32>().0 {
                match e[0] {
                    0 => return false,
                    0xe5 => long.next = None,
                    _ if e[11] & 0x3f == 0x0f => long.add(e),
                    _ if e[11] & 0x08 != 0 => long.next = None,
                    _ => {
                        let matches = match long.of(e) {
                            Some(units) => same(units.iter().copied(), name),
                            None => same(short_name(e), name),
                        };
                        long.next = None;
                        if matches {
                            found = Some(Entry::read(e));
                            return false;
                        }
                    }
                }
            }
            true
        })?;
        found.ok_or(Error::NotFound)
    }

    /// The entry at `path`, its names split by `/`, from the root
    /// directory on.
    ///
    /// # Errors
    ///
    /// When a name on the way is not there or is a file, or a directory
    /// cannot be read.
    pub fn find(&mut self, path: &str) -> Result<Entry, Error> {
        let mut entry = Entry {
            first: self.volume.root,
            size: 0,
            directory: true,
        };
        for name in path.split('/').filter(|n| !n.is_empty()) {
            if !entry.directory {
                return Err(Error::NotADirectory);
            }
            entry = self.lookup(entry.first, name)?;
        }
        Ok(entry)
    }

    /// Reads the file `entry` stands for into `into`, as much of it as
    /// fits, and returns how many bytes that is. Read whole, its chain has
    /// to end right after the last cluster its size needs, so a chain that
    /// loops back is caught instead of read twice.
    ///
    /// # Errors
    ///
    /// When its chain ends before its size does, goes on past it, or
    /// cannot be read.
    pub fn read_file(&mut self, entry: &Entry, into: &mut [u8]) -> Result<usize, Error> {
        let want = into.len().min(entry.size as usize);
        if want == 0 {
            return Ok(0);
        }
        let size = self.volume.sector_size as usize;
        let mut room = [0u8; 4096];
        let mut done = 0;
        let mut cluster = entry.first;
        loop {
            let start = self.volume.first_sector(cluster).ok_or(Error::BadChain)?;
            for sector in start..start + u64::from(self.volume.sectors_per_cluster) {
                if !(self.read)(sector, &mut room[..size]) {
                    return Err(Error::Read);
                }
                let n = size.min(want - done);
                into[done..done + n].copy_from_slice(&room[..n]);
                done += n;
                if done == want {
                    break;
                }
            }
            if done == want {
                break;
            }
            cluster = self.next(cluster)?.ok_or(Error::BadChain)?;
        }
        if want == entry.size as usize && self.next(cluster)?.is_some() {
            return Err(Error::BadChain);
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The sizes of the test volume: as small as FAT32 allows with one
    /// sector per cluster, as fatgen103 lays it out.
    const TOTAL: u32 = 70_000;
    const FAT_SECTORS: u32 = 545;

    /// A FAT32 volume in memory, where only the sectors written hold bytes
    /// and every other reads as zeros.
    struct Disk {
        sectors: BTreeMap<u64, [u8; 512]>,
        next: u32,
        per_cluster: u32,
    }

    impl Disk {
        fn new() -> Disk {
            Disk::of(1, TOTAL, FAT_SECTORS)
        }

        /// A volume of `total` sectors, `per_cluster` to a cluster, with two
        /// FATs of `fat_sectors` each.
        fn of(per_cluster: u8, total: u32, fat_sectors: u32) -> Disk {
            let mut boot = [0u8; 512];
            boot[..11].copy_from_slice(b"\xeb\x58\x90MSWIN4.1");
            boot[11..13].copy_from_slice(&512u16.to_le_bytes());
            boot[13] = per_cluster;
            boot[14..16].copy_from_slice(&32u16.to_le_bytes());
            boot[16] = 2;
            boot[21] = 0xf8;
            boot[32..36].copy_from_slice(&total.to_le_bytes());
            boot[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
            boot[44..48].copy_from_slice(&2u32.to_le_bytes());
            boot[510..512].copy_from_slice(&[0x55, 0xaa]);
            let mut disk = Disk {
                sectors: BTreeMap::new(),
                next: 3,
                per_cluster: u32::from(per_cluster),
            };
            disk.sectors.insert(0, boot);
            disk.link(&[2]);
            disk
        }

        fn sector(&mut self, at: u64) -> &mut [u8; 512] {
            self.sectors.entry(at).or_insert([0; 512])
        }

        fn set_fat(&mut self, cluster: u32, value: u32) {
            let at = u64::from(cluster) * 4;
            let offset = (at % 512) as usize;
            self.sector(32 + at / 512)[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }

        /// Chains `clusters` in this order and ends the chain.
        fn link(&mut self, clusters: &[u32]) {
            for pair in clusters.windows(2) {
                self.set_fat(pair[0], pair[1]);
            }
            self.set_fat(*clusters.last().unwrap(), 0x0fff_ffff);
        }

        fn take(&mut self, n: usize) -> Vec<u32> {
            let n = u32::try_from(n).unwrap();
            let clusters = (self.next..self.next + n).collect();
            self.next += n;
            clusters
        }

        /// Writes `bytes` across `clusters`, in order.
        fn fill(&mut self, clusters: &[u32], bytes: &[u8]) {
            let volume = Volume::read(&self.sectors[&0]).unwrap();
            let per_cluster = self.per_cluster as usize;
            for (&c, part) in clusters.iter().zip(bytes.chunks(512 * per_cluster)) {
                let first = volume.first_sector(c).unwrap();
                for (s, chunk) in (first..).zip(part.chunks(512)) {
                    self.sector(s)[..chunk.len()].copy_from_slice(chunk);
                }
            }
        }

        fn file(&mut self, bytes: &[u8]) -> u32 {
            if bytes.is_empty() {
                return 0;
            }
            let clusters = self.take(bytes.len().div_ceil(512 * self.per_cluster as usize));
            self.link(&clusters);
            self.fill(&clusters, bytes);
            clusters[0]
        }

        fn reader(&self) -> impl FnMut(u64, &mut [u8]) -> bool + '_ {
            |lba, into| {
                into.copy_from_slice(
                    &self.sectors.get(&lba).copied().unwrap_or([0; 512])[..into.len()],
                );
                true
            }
        }
    }

    fn short(name: &[u8; 11], attr: u8, first: u32, size: u32) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name);
        e[11] = attr;
        e[20..22].copy_from_slice(&first.to_le_bytes()[2..4]);
        e[26..28].copy_from_slice(&first.to_le_bytes()[0..2]);
        e[28..32].copy_from_slice(&size.to_le_bytes());
        e
    }

    /// The long name entries for `name`, last part first, as they lie
    /// before the short entry `short`.
    fn long(name: &str, short: &[u8; 11]) -> Vec<[u8; 32]> {
        let mut units: Vec<u16> = name.encode_utf16().collect();
        let parts = units.len().div_ceil(13);
        if !units.len().is_multiple_of(13) {
            units.push(0);
        }
        units.resize(parts * 13, 0xffff);
        (1..=parts)
            .rev()
            .map(|n| {
                let mut e = [0u8; 32];
                e[0] = u8::try_from(n).unwrap() | if n == parts { 0x40 } else { 0 };
                e[11] = 0x0f;
                e[13] = checksum(short);
                for (i, &offset) in UNITS.iter().enumerate() {
                    e[offset..offset + 2].copy_from_slice(&units[(n - 1) * 13 + i].to_le_bytes());
                }
                e
            })
            .collect()
    }

    /// Writes a directory's entries into fresh clusters, chained, and
    /// returns the first.
    fn directory(disk: &mut Disk, at: Option<u32>, entries: &[[u8; 32]]) -> u32 {
        let bytes: Vec<u8> = entries.iter().flatten().copied().collect();
        let clusters = match at {
            Some(first) if bytes.len() <= 512 => vec![first],
            Some(first) => {
                let mut more = vec![first];
                more.extend(disk.take(bytes.len().div_ceil(512) - 1));
                more
            }
            None => disk.take(bytes.len().div_ceil(512).max(1)),
        };
        disk.link(&clusters);
        disk.fill(&clusters, &bytes);
        clusters[0]
    }

    /// The test volume: files with short and long names, one over several
    /// clusters out of order, a directory of its own, and a root directory
    /// longer than a cluster.
    fn volume() -> (Disk, Vec<u8>) {
        let mut disk = Disk::new();
        let settings = disk.file(b"boot {\n    timeout = 2500ms\n}\n");
        let meridian = directory(&mut disk, None, &{
            let mut e = long("boot.meri", b"BOOT    MER");
            e.push(short(b"BOOT    MER", 0x20, settings, 29));
            e
        });
        let big: Vec<u8> = (0..1500u32).map(|i| (i % 251) as u8).collect();
        let scattered = [disk.next + 2, disk.next, disk.next + 1];
        disk.next += 3;
        disk.link(&scattered);
        disk.fill(&scattered, &big);
        let readme = disk.file(b"read me");
        let mut root = vec![short(b"MERIDIAN   ", 0x08, 0, 0)];
        root.extend(long("meridian", b"MERIDIAN   "));
        root.push(short(b"MERIDIAN   ", 0x10, meridian, 0));
        let mut deleted = short(b"OLD     TXT", 0x20, readme, 7);
        deleted[0] = 0xe5;
        root.push(deleted);
        root.push(short(b"README  TXT", 0x20, readme, 7));
        root.push(short(b"EMPTY      ", 0x20, 0, 0));
        for i in 0..20u8 {
            root.push(short(
                &[
                    b'F',
                    b'I',
                    b'L',
                    b'L',
                    b'0' + i / 10,
                    b'0' + i % 10,
                    b' ',
                    b' ',
                    b' ',
                    b' ',
                    b' ',
                ],
                0x20,
                readme,
                7,
            ));
        }
        let name = format!("long-name-{}", "x".repeat(244));
        root.extend(long(&name, b"LONG-N~1   "));
        root.push(short(b"LONG-N~1   ", 0x20, scattered[0], 1500));
        directory(&mut disk, Some(2), &root);
        (disk, big)
    }

    #[test]
    fn the_boot_sector_gives_the_geometry() {
        let (disk, _) = volume();
        let v = Volume::read(&disk.sectors[&0]).unwrap();
        assert_eq!((v.sector_size, v.sectors_per_cluster, v.root), (512, 1, 2));
        assert_eq!(v.clusters, TOTAL - 1122);
        assert_eq!(
            v.first_sector(2),
            Some(1122),
            "the data region after both FATs"
        );
        assert_eq!(v.first_sector(3), Some(1123));
        assert_eq!(v.first_sector(1), None);
        assert_eq!(
            v.first_sector(v.clusters + 1),
            Some(u64::from(TOTAL) - 1),
            "the last cluster"
        );
        assert_eq!(v.first_sector(v.clusters + 2), None, "past it");
        assert_eq!(v.entry_of(2), (32, 8));
        assert_eq!(v.entry_of(127), (32, 508));
        assert_eq!(v.entry_of(128), (33, 0));
        assert_eq!(v.cluster_bytes(), 512);
    }

    fn boot_with(change: impl FnOnce(&mut [u8; 512])) -> [u8; 512] {
        let mut boot = Disk::new().sectors[&0];
        change(&mut boot);
        boot
    }

    #[test]
    fn only_fat32_boot_sectors_are_read() {
        assert_eq!(
            Volume::read(&boot_with(|b| b[511] = 0)),
            Err(Error::NotFat),
            "no signature"
        );
        assert_eq!(
            Volume::read(&boot_with(
                |b| b[11..13].copy_from_slice(&768u16.to_le_bytes())
            )),
            Err(Error::NotFat)
        );
        assert_eq!(
            Volume::read(&boot_with(
                |b| b[11..13].copy_from_slice(&256u16.to_le_bytes())
            )),
            Err(Error::NotFat)
        );
        assert_eq!(
            Volume::read(&boot_with(|b| b[13] = 3)),
            Err(Error::NotFat),
            "three sectors a cluster"
        );
        assert_eq!(
            Volume::read(&boot_with(|b| b[14] = 0)),
            Err(Error::NotFat),
            "no reserved sector"
        );
        assert_eq!(
            Volume::read(&boot_with(|b| b[16] = 0)),
            Err(Error::NotFat),
            "no FAT"
        );
        assert_eq!(
            Volume::read(&boot_with(|b| b[36..40].fill(0))),
            Err(Error::NotFat),
            "an empty FAT"
        );
        assert_eq!(
            Volume::read(&boot_with(
                |b| b[32..36].copy_from_slice(&1000u32.to_le_bytes())
            )),
            Err(Error::NotFat),
            "smaller than its FATs"
        );
        assert_eq!(
            Volume::read(&boot_with(
                |b| b[32..36].copy_from_slice(&66_000u32.to_le_bytes())
            )),
            Err(Error::NotFat32),
            "few clusters: FAT16"
        );
        assert_eq!(
            Volume::read(&boot_with(|b| b[17] = 16)),
            Err(Error::NotFat32),
            "a FAT16 root directory"
        );
        assert!(
            Volume::read(&boot_with(|b| b[13] = 2)).is_err(),
            "two sectors a cluster: too few clusters"
        );
        let v = Volume::read(&boot_with(|b| {
            b[11..13].copy_from_slice(&4096u16.to_le_bytes());
            b[32..36].copy_from_slice(&(66_700 + 32 + 2 * 545u32).to_le_bytes());
        }))
        .unwrap();
        assert_eq!((v.sector_size, v.cluster_bytes()), (4096, 4096));
        assert_eq!(v.entry_of(1024), (33, 0), "1024 entries a 4 KiB sector");
    }

    #[test]
    fn files_are_found_by_their_names_in_either_case() {
        let (disk, _) = volume();
        let mut fat = Fat::open(disk.reader()).unwrap();
        let readme = fat.find("README.TXT").unwrap();
        assert_eq!((readme.size, readme.directory), (7, false));
        assert_eq!(fat.find("readme.txt"), Ok(readme));
        assert_eq!(fat.find("/ReadMe.Txt"), Ok(readme));
        let meridian = fat.find("meridian").unwrap();
        assert!(meridian.directory);
        let settings = fat.find("meridian/boot.meri").unwrap();
        assert_eq!(settings.size, 29);
        assert_eq!(fat.find("MERIDIAN/BOOT.MERI"), Ok(settings));
        assert_eq!(
            fat.find("//meridian//boot.meri"),
            Ok(settings),
            "empty names are no names"
        );
        assert_eq!(
            fat.find("fill19").map(|e| e.size),
            Ok(7),
            "past the first cluster of the root"
        );
        assert_eq!(fat.find("").map(|e| e.first), Ok(2), "the root itself");
    }

    #[test]
    fn what_is_not_there_is_not_found() {
        let (disk, _) = volume();
        let mut fat = Fat::open(disk.reader()).unwrap();
        assert_eq!(fat.find("nothing"), Err(Error::NotFound));
        assert_eq!(fat.find("OLD.TXT"), Err(Error::NotFound), "a deleted entry");
        assert_eq!(fat.find("MERIDIAN.TXT"), Err(Error::NotFound));
        assert_eq!(fat.find("README.TXT/inside"), Err(Error::NotADirectory));
        assert_eq!(fat.find("meridian/nothing"), Err(Error::NotFound));
        assert_eq!(
            fat.find("README"),
            Err(Error::NotFound),
            "the extension is part of the name"
        );
    }

    #[test]
    fn a_long_name_of_twenty_entries_is_read_whole() {
        let (disk, big) = volume();
        let mut fat = Fat::open(disk.reader()).unwrap();
        let name = format!("long-name-{}", "x".repeat(244));
        let entry = fat.find(&name).unwrap();
        assert_eq!(entry.size, 1500);
        assert_eq!(fat.find(&name.to_uppercase()), Ok(entry));
        assert_eq!(
            fat.find("LONG-N~1"),
            Err(Error::NotFound),
            "a long name hides its short one"
        );
        let mut room = vec![0u8; 2000];
        assert_eq!(fat.read_file(&entry, &mut room), Ok(1500));
        assert_eq!(
            &room[..1500],
            &big[..],
            "three clusters out of order, followed through the FAT"
        );
    }

    #[test]
    fn files_are_read_as_far_as_they_go_or_there_is_room() {
        let (disk, big) = volume();
        let mut fat = Fat::open(disk.reader()).unwrap();
        let settings = fat.find("meridian/boot.meri").unwrap();
        let mut room = [0u8; 64];
        assert_eq!(fat.read_file(&settings, &mut room), Ok(29));
        assert_eq!(
            &room[..29],
            b"boot {\n    timeout = 2500ms\n}\n"[..29].as_ref()
        );
        let entry = fat.find(&format!("long-name-{}", "x".repeat(244))).unwrap();
        let mut small = [0u8; 700];
        assert_eq!(
            fat.read_file(&entry, &mut small),
            Ok(700),
            "as much as fits"
        );
        assert_eq!(&small[..], &big[..700]);
        let empty = fat.find("EMPTY").unwrap();
        assert_eq!(fat.read_file(&empty, &mut room), Ok(0));
    }

    #[test]
    fn broken_chains_are_refused() {
        let (mut disk, _) = volume();
        let readme = Fat::open(disk.reader())
            .unwrap()
            .find("README.TXT")
            .unwrap();
        let too_big = Entry {
            size: 2000,
            ..readme
        };
        assert_eq!(
            Fat::open(disk.reader())
                .unwrap()
                .read_file(&too_big, &mut [0u8; 4096]),
            Err(Error::BadChain),
            "the chain ends before the size"
        );
        disk.set_fat(readme.first, readme.first);
        assert_eq!(
            Fat::open(disk.reader())
                .unwrap()
                .read_file(&too_big, &mut [0u8; 4096]),
            Err(Error::BadChain),
            "a loop"
        );
        disk.set_fat(readme.first, TOTAL);
        assert_eq!(
            Fat::open(disk.reader()).unwrap().next(readme.first),
            Err(Error::BadChain),
            "a cluster past the volume"
        );
        disk.set_fat(readme.first, 0x1fff_fff8);
        assert_eq!(
            Fat::open(disk.reader()).unwrap().next(readme.first),
            Ok(None),
            "the top four bits are not part of an entry"
        );
        let outside = Entry {
            first: 1,
            size: 10,
            directory: false,
        };
        assert_eq!(
            Fat::open(disk.reader())
                .unwrap()
                .read_file(&outside, &mut [0u8; 16]),
            Err(Error::BadChain)
        );
    }

    #[test]
    fn a_volume_that_cannot_be_read_says_so() {
        assert!(matches!(
            Fat::open(|_, _: &mut [u8]| false),
            Err(Error::Read)
        ));
        let (disk, _) = volume();
        let mut boot_only = |lba, into: &mut [u8]| {
            lba == 0 && {
                into.copy_from_slice(&disk.sectors[&0][..into.len()]);
                true
            }
        };
        let mut fat = Fat::open(&mut boot_only).unwrap();
        assert_eq!(fat.find("README.TXT"), Err(Error::Read));
        assert_eq!(fat.next(2), Err(Error::Read));
    }

    #[test]
    fn long_names_that_do_not_belong_fall_back_to_the_short_one() {
        let mut disk = Disk::new();
        let file = disk.file(b"x");
        let mut wrong_sum = long("lower.txt", b"LOWER   TXT");
        wrong_sum[0][13] ^= 1;
        let mut skipped = long("a much longer name.txt", b"AMUCHL~1TXT");
        skipped.remove(0);
        let mut root = wrong_sum;
        root.push(short(b"LOWER   TXT", 0x20, file, 1));
        root.extend(skipped);
        root.push(short(b"AMUCHL~1TXT", 0x20, file, 1));
        directory(&mut disk, Some(2), &root);
        let mut fat = Fat::open(disk.reader()).unwrap();
        assert!(
            fat.find("LOWER.TXT").is_ok(),
            "a checksum that does not match"
        );
        assert_eq!(
            fat.find("lower.txt").map(|e| e.size),
            Ok(1),
            "by its short name, in any case"
        );
        assert_eq!(
            fat.find("a much longer name.txt"),
            Err(Error::NotFound),
            "a part missing"
        );
        assert!(fat.find("AMUCHL~1.TXT").is_ok());
    }

    #[test]
    fn the_cluster_count_decides_fat32_at_65525() {
        let data = 32 + 2 * FAT_SECTORS;
        let exact = boot_with(|b| b[32..36].copy_from_slice(&(data + 65_525).to_le_bytes()));
        assert_eq!(Volume::read(&exact).map(|v| v.clusters), Ok(65_525));
        let one_less = boot_with(|b| b[32..36].copy_from_slice(&(data + 65_524).to_le_bytes()));
        assert_eq!(Volume::read(&one_less), Err(Error::NotFat32));
    }

    /// A volume of 8 sectors a cluster, with a file over three clusters at
    /// cluster 66000, past what 16 bits number.
    fn wide() -> (Disk, Vec<u8>) {
        let mut disk = Disk::of(8, 32 + 2 * 520 + 66_100 * 8, 520);
        let bytes: Vec<u8> = (0..10_000u32).map(|i| (i % 253) as u8).collect();
        let clusters = [66_000, 66_001, 66_002];
        disk.link(&clusters);
        disk.fill(&clusters, &bytes);
        let mut root = long("wide file.bin", b"WIDEFI~1BIN");
        root.push(short(b"WIDEFI~1BIN", 0x20, 66_000, 10_000));
        directory(&mut disk, Some(2), &root);
        (disk, bytes)
    }

    #[test]
    fn clusters_of_several_sectors_and_past_16_bits_are_read() {
        let (disk, bytes) = wide();
        let mut fat = Fat::open(disk.reader()).unwrap();
        let v = fat.volume;
        assert_eq!(
            (v.sectors_per_cluster, v.cluster_bytes(), v.clusters),
            (8, 4096, 66_100)
        );
        assert_eq!(
            v.first_sector(3),
            Some(32 + 1040 + 8),
            "8 sectors after the first cluster"
        );
        assert_eq!(
            v.entry_of(66_000),
            (32 + 515, 320),
            "the high half of the number"
        );
        let entry = fat.find("wide file.bin").unwrap();
        assert_eq!(entry.first, 66_000);
        let mut room = vec![0u8; 12_000];
        assert_eq!(fat.read_file(&entry, &mut room), Ok(10_000));
        assert_eq!(
            &room[..10_000],
            &bytes[..],
            "every sector of every cluster, not the first alone"
        );
    }

    /// A root directory holding `entries` and a file of one byte they may
    /// point at, and the file's first cluster.
    fn root_of(entries: impl FnOnce(u32) -> Vec<[u8; 32]>) -> Disk {
        let mut disk = Disk::new();
        let file = disk.file(b"x");
        let root = entries(file);
        directory(&mut disk, Some(2), &root);
        disk
    }

    #[test]
    fn parts_of_a_long_name_have_to_agree_and_come_in_order() {
        let name = "a name in two parts.txt";
        let short_name = b"ANAMEI~1TXT";
        let found = |change: &dyn Fn(&mut Vec<[u8; 32]>)| {
            let disk = root_of(|file| {
                let mut e = long(name, short_name);
                change(&mut e);
                e.push(short(short_name, 0x20, file, 1));
                e
            });
            Fat::open(disk.reader()).unwrap().find(name).is_ok()
        };
        assert!(found(&|_| {}), "as written");
        assert!(!found(&|e| e[1][13] ^= 1), "a part with another checksum");
        assert!(!found(&|e| e[1][0] = 0), "a part numbered 0");
        assert!(!found(&|e| e.swap(0, 1)), "parts out of order");
        assert!(
            !found(&|e| {
                e.pop();
            }),
            "the last part missing"
        );
        assert!(
            !found(&|e| e[0][0] = 0x40 | 0x15),
            "a part past the 20 a name can have"
        );
    }

    #[test]
    fn a_directory_ends_at_its_first_empty_entry() {
        let disk = root_of(|file| {
            let mut entries: Vec<[u8; 32]> = (0..14u8)
                .map(|i| {
                    short(
                        &[
                            b'F',
                            b'I',
                            b'L',
                            b'L',
                            b'0' + i / 10,
                            b'0' + i % 10,
                            b' ',
                            b' ',
                            b' ',
                            b' ',
                            b' ',
                        ],
                        0x20,
                        file,
                        1,
                    )
                })
                .collect();
            entries.push(short(b"TWICE   TXT", 0x20, file, 1));
            entries.push([0u8; 32]);
            entries.push(short(b"AFTER   TXT", 0x20, file, 1));
            entries
        });
        let mut fat = Fat::open(disk.reader()).unwrap();
        assert!(fat.find("FILL13").is_ok());
        assert_eq!(
            fat.find("AFTER.TXT"),
            Err(Error::NotFound),
            "nothing after the end counts, not even in the next cluster"
        );
    }

    #[test]
    fn of_two_entries_with_one_name_the_first_is_taken() {
        let disk = root_of(|file| {
            let mut entries: Vec<[u8; 32]> = (0..15u8)
                .map(|i| {
                    short(
                        &[
                            b'F',
                            b'I',
                            b'L',
                            b'L',
                            b'0' + i / 10,
                            b'0' + i % 10,
                            b' ',
                            b' ',
                            b' ',
                            b' ',
                            b' ',
                        ],
                        0x20,
                        file,
                        1,
                    )
                })
                .collect();
            entries.push(short(b"TWICE   TXT", 0x20, file, 1));
            entries.push(short(b"TWICE   TXT", 0x20, file, 9));
            entries
        });
        let mut fat = Fat::open(disk.reader()).unwrap();
        assert_eq!(
            fat.find("TWICE.TXT").map(|e| e.size),
            Ok(1),
            "the second, in the next cluster, is not read"
        );
    }

    #[test]
    fn a_long_name_for_another_short_one_is_not_its_name() {
        let disk = root_of(|file| {
            let mut entries = long("different.txt", b"DIFFER~1TXT");
            entries.push(short(b"LOWER   TXT", 0x20, file, 1));
            entries
        });
        let mut fat = Fat::open(disk.reader()).unwrap();
        assert_eq!(
            fat.find("different.txt"),
            Err(Error::NotFound),
            "its checksum is another's"
        );
        assert!(fat.find("LOWER.TXT").is_ok());
    }

    #[test]
    fn short_name_checksums_are_fatgen103s() {
        assert_eq!(checksum(b"MERIDIAN   "), 0x4b);
        assert_eq!(checksum(b"LONG-N~1   "), 0xb3);
        assert_eq!(checksum(b"BOOT    MER"), 0x6d);
    }
}
