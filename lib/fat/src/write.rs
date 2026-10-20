//! Writing FAT32: clusters taken and given back, in every copy of the
//! FAT, with the free count and the next free cluster `FSInfo` keeps,
//! files written into their chains, which grow as they need, and entries
//! made with their long names and removed.

use crate::{Entry, Error, Fat, Place, name, u32_at};

/// The most entries one name takes: 20 for its long name and the short.
const MOST: usize = 21;

/// When entries are made, as FAT dates and times count: 1 January 1980,
/// the first day they can say, until there is a clock to ask.
const DATE: u16 = 0x21;

/// What a writer puts in the entry of a chain's last cluster.
const LAST: u32 = 0x0fff_ffff;

/// `FSInfo`'s signatures at its start, at 484 and at its end, and where it
/// keeps the free count and the next free cluster; a count of all ones is
/// none known.
const LEAD: u32 = 0x4161_5252;
const MIDDLE: u32 = 0x6141_7272;
const TRAIL: u32 = 0xaa55_0000;
const FREE: usize = 488;
const NEXT: usize = 492;
const UNKNOWN: u32 = u32::MAX;

/// A volume `Fat` reads, which `write` writes: it writes the volume's
/// sector it is given from the bytes it is given and says whether it could.
pub struct Writer<R, W> {
    pub fat: Fat<R>,
    write: W,
}

impl<R: FnMut(u64, &mut [u8]) -> bool, W: FnMut(u64, &[u8]) -> bool> Writer<R, W> {
    #[must_use]
    pub fn new(fat: Fat<R>, write: W) -> Writer<R, W> {
        Writer { fat, write }
    }

    fn put(&mut self, at: u64, from: &[u8]) -> Result<(), Error> {
        if (self.write)(at, from) {
            Ok(())
        } else {
            Err(Error::Write)
        }
    }

    /// The entry of cluster `cluster` in the first FAT, its top four bits
    /// left out.
    fn entry(&mut self, cluster: u32) -> Result<u32, Error> {
        let (sector, offset) = self.fat.volume.entry_of(cluster);
        let size = self.fat.volume.sector_size as usize;
        let mut room = [0u8; 4096];
        self.fat.sector(sector, &mut room[..size])?;
        Ok(u32_at(&room, offset) & LAST)
    }

    /// Sets the entry of cluster `cluster` to `value` in every FAT, keeping
    /// the four top bits, which are not the entry's.
    fn set(&mut self, cluster: u32, value: u32) -> Result<(), Error> {
        let v = self.fat.volume;
        let (sector, offset) = v.entry_of(cluster);
        let size = v.sector_size as usize;
        let mut room = [0u8; 4096];
        self.fat.sector(sector, &mut room[..size])?;
        let kept = u32_at(&room, offset) & !LAST;
        room[offset..offset + 4].copy_from_slice(&(kept | value & LAST).to_le_bytes());
        for copy in 0..u64::from(v.fats) {
            self.put(sector + copy * u64::from(v.fat_sectors), &room[..size])?;
        }
        Ok(())
    }

    /// `FSInfo`'s sector, when the volume has one with its signatures.
    fn fsinfo(&mut self, room: &mut [u8; 4096]) -> Result<Option<u64>, Error> {
        let v = self.fat.volume;
        let size = v.sector_size as usize;
        if v.fsinfo == 0 || size < 512 {
            return Ok(None);
        }
        let at = u64::from(v.fsinfo);
        self.fat.sector(at, &mut room[..size])?;
        let signed =
            u32_at(room, 0) == LEAD && u32_at(room, 484) == MIDDLE && u32_at(room, 508) == TRAIL;
        Ok(signed.then_some(at))
    }

    /// Tells `FSInfo` that `taken` clusters more are in use, fewer when it is
    /// below 0, and that `next` is free, when it says anything.
    fn count(&mut self, taken: i64, next: Option<u32>) -> Result<(), Error> {
        let mut room = [0u8; 4096];
        let Some(at) = self.fsinfo(&mut room)? else {
            return Ok(());
        };
        let free = u32_at(&room, FREE);
        if free != UNKNOWN {
            let counted = i64::from(free) - taken;
            let counted = u32::try_from(counted.clamp(0, i64::from(self.fat.volume.clusters)))
                .unwrap_or(UNKNOWN);
            room[FREE..FREE + 4].copy_from_slice(&counted.to_le_bytes());
        }
        if let Some(next) = next {
            room[NEXT..NEXT + 4].copy_from_slice(&next.to_le_bytes());
        }
        let size = self.fat.volume.sector_size as usize;
        self.put(at, &room[..size])
    }

    /// Where looking for a free cluster starts: where `FSInfo` says one is,
    /// or the first cluster.
    fn hint(&mut self) -> Result<u32, Error> {
        let mut room = [0u8; 4096];
        let last = self.fat.volume.clusters + 1;
        Ok(match self.fsinfo(&mut room)? {
            Some(_) => Some(u32_at(&room, NEXT)).filter(|n| (2..=last).contains(n)),
            None => None,
        }
        .unwrap_or(2))
    }

    /// Takes a free cluster as the last of a chain of its own and answers
    /// it: the first free one from where `FSInfo` says to look on, wrapping
    /// around to the first cluster.
    ///
    /// # Errors
    ///
    /// When no cluster is free, or the volume cannot be read or written.
    pub fn take(&mut self) -> Result<u32, Error> {
        let clusters = self.fat.volume.clusters;
        let start = self.hint()?;
        for i in 0..clusters {
            let cluster = 2 + (start - 2 + i) % clusters;
            if self.entry(cluster)? == 0 {
                self.set(cluster, LAST)?;
                let next = 2 + (cluster - 1) % clusters;
                self.count(1, Some(next))?;
                return Ok(cluster);
            }
        }
        Err(Error::Full)
    }

    /// Takes a free cluster as [`Writer::take`] does, and zeroes it.
    ///
    /// # Errors
    ///
    /// As for [`Writer::take`].
    pub fn take_zeroed(&mut self) -> Result<u32, Error> {
        let cluster = self.take()?;
        let v = self.fat.volume;
        let first = v.first_sector(cluster).ok_or(Error::BadChain)?;
        let zeros = [0u8; 4096];
        for sector in first..first + u64::from(v.sectors_per_cluster) {
            self.put(sector, &zeros[..v.sector_size as usize])?;
        }
        Ok(cluster)
    }

    /// Writes `entry`'s first cluster and size into its short entry at
    /// `place`, leaving the rest of the entry as it is.
    ///
    /// # Errors
    ///
    /// When the sector cannot be read or written.
    pub fn save(&mut self, place: Place, entry: &Entry) -> Result<(), Error> {
        let size = self.fat.volume.sector_size as usize;
        let mut room = [0u8; 4096];
        self.fat.sector(place.sector, &mut room[..size])?;
        let e = &mut room[place.index * 32..place.index * 32 + 32];
        let first = entry.first.to_le_bytes();
        e[20..22].copy_from_slice(&first[2..4]);
        e[26..28].copy_from_slice(&first[..2]);
        e[28..32].copy_from_slice(&entry.size.to_le_bytes());
        self.put(place.sector, &room[..size])
    }

    /// Writes `from` into the file `entry` stands for from its byte
    /// `offset` on, taking clusters for it as it grows, and answers how many
    /// bytes that is: all of them. Bytes between its end and `offset` read
    /// as zeros after. `entry` holds its new first cluster and size, which
    /// [`Writer::save`] puts in its directory.
    ///
    /// # Errors
    ///
    /// When the file would grow past what FAT32 sizes hold, no cluster is
    /// left, its chain is broken, or the volume cannot be read or written.
    pub fn write_at(
        &mut self,
        entry: &mut Entry,
        offset: u64,
        from: &[u8],
    ) -> Result<usize, Error> {
        let end = offset
            .checked_add(from.len() as u64)
            .ok_or(Error::TooLarge)?;
        let end = u32::try_from(end).map_err(|_| Error::TooLarge)?;
        if from.is_empty() {
            return Ok(0);
        }
        let start = u32::try_from(offset).map_err(|_| Error::TooLarge)?;
        let zeros = [0u8; 4096];
        while entry.size < start {
            let n = (start - entry.size).min(4096);
            self.write_at(entry, u64::from(entry.size), &zeros[..n as usize])?;
        }
        if entry.first == 0 {
            entry.first = self.take_zeroed()?;
        }
        let bytes = u32::try_from(self.fat.volume.cluster_bytes()).map_err(|_| Error::TooLarge)?;
        let (first, last) = (start / bytes, (end - 1) / bytes);
        let mut cluster = entry.first;
        for index in 0..=last {
            if index >= first {
                let from_at = (index * bytes).max(start);
                let to = (index + 1).checked_mul(bytes).map_or(end, |t| t.min(end));
                let part = &from[(from_at - start) as usize..(to - start) as usize];
                self.write_in(cluster, (from_at - index * bytes) as usize, part)?;
            }
            if index < last {
                cluster = if let Some(next) = self.fat.next(cluster)? {
                    next
                } else {
                    let grown = self.take_zeroed()?;
                    self.set(cluster, grown)?;
                    grown
                };
            }
        }
        entry.size = entry.size.max(end);
        Ok(from.len())
    }

    /// Writes `from` into cluster `cluster` from its byte `at` on, reading
    /// first each sector it writes only part of.
    fn write_in(&mut self, cluster: u32, at: usize, from: &[u8]) -> Result<(), Error> {
        let v = self.fat.volume;
        let size = v.sector_size as usize;
        let first = v.first_sector(cluster).ok_or(Error::BadChain)?;
        let mut room = [0u8; 4096];
        let mut done = 0;
        while done < from.len() {
            let byte = at + done;
            let sector = first + (byte / size) as u64;
            let skip = byte % size;
            let n = (size - skip).min(from.len() - done);
            if n < size {
                self.fat.sector(sector, &mut room[..size])?;
            }
            room[skip..skip + n].copy_from_slice(&from[done..done + n]);
            self.put(sector, &room[..size])?;
            done += n;
        }
        Ok(())
    }

    /// Makes an entry called `name` in the directory whose first cluster is
    /// `directory`, for an empty file or, when `directory_too`, a directory
    /// that holds its `.` and `..`, and answers it with where its short
    /// entry lies. The directory grows when it has no room for it.
    ///
    /// # Errors
    ///
    /// When an entry has the name already, the name is empty or too long,
    /// no cluster is left, or the volume cannot be read or written.
    pub fn create(
        &mut self,
        directory: u32,
        name: &str,
        directory_too: bool,
    ) -> Result<(Entry, Place), Error> {
        match self.fat.lookup(directory, name) {
            Ok(_) => return Err(Error::Exists),
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        let mut scan = Ok(());
        let (short, needs_long) = name::short_for(name, |candidate| {
            let found = self.taken(directory, candidate);
            let taken = found.unwrap_or(true);
            scan = scan.and(found.map(|_| ()));
            taken
        })
        .ok_or(Error::Exists)?;
        scan?;
        let mut entries = [[0u8; 32]; MOST];
        let mut longs = [[0u8; 32]; 20];
        let count = if needs_long {
            name::long_entries(name, &short, &mut longs).ok_or(Error::BadName)?
        } else {
            0
        };
        entries[..count].copy_from_slice(&longs[..count]);
        let first = if directory_too {
            self.take_zeroed()?
        } else {
            0
        };
        let attributes = if directory_too { 0x10 } else { 0x20 };
        entries[count] = short_entry(&short, attributes, first);
        let mut places = [Place {
            sector: 0,
            index: 0,
        }; MOST];
        self.free_run(directory, count + 1, &mut places)?;
        for (place, entry) in places.iter().zip(&entries[..=count]) {
            self.put_entry(*place, entry)?;
        }
        if directory_too {
            let parent = if directory == self.fat.volume.root {
                0
            } else {
                directory
            };
            let sector = self.fat.volume.first_sector(first).ok_or(Error::BadChain)?;
            self.put_entry(
                Place { sector, index: 0 },
                &short_entry(b".          ", 0x10, first),
            )?;
            self.put_entry(
                Place { sector, index: 1 },
                &short_entry(b"..         ", 0x10, parent),
            )?;
        }
        let entry = Entry {
            first,
            size: 0,
            directory: directory_too,
        };
        Ok((entry, places[count]))
    }

    /// Whether a short entry of the directory at `directory` has the name
    /// `short`.
    fn taken(&mut self, directory: u32, short: &[u8; 11]) -> Result<bool, Error> {
        let mut found = false;
        self.fat.walk(directory, |_, sector| {
            for e in sector.as_chunks::<32>().0 {
                if e[0] == 0 {
                    return false;
                }
                found |= e[0] != 0xe5 && e[11] & 0x3f != 0x0f && e[..11] == short[..];
            }
            !found
        })?;
        Ok(found)
    }

    /// Finds `need` free entries one after another in the directory at
    /// `directory`, growing it by a zeroed cluster while it has none, and
    /// puts where they lie into `places`.
    fn free_run(
        &mut self,
        directory: u32,
        need: usize,
        places: &mut [Place; MOST],
    ) -> Result<(), Error> {
        for _ in 0..=MOST {
            let mut run = 0;
            self.fat.walk(directory, |sector, bytes| {
                for (index, e) in bytes.as_chunks::<32>().0.iter().enumerate() {
                    if e[0] == 0 || e[0] == 0xe5 {
                        places[run] = Place { sector, index };
                        run += 1;
                        if run == need {
                            return false;
                        }
                    } else {
                        run = 0;
                    }
                }
                true
            })?;
            if run == need {
                return Ok(());
            }
            let mut last = directory;
            while let Some(next) = self.fat.next(last)? {
                last = next;
            }
            let grown = self.take_zeroed()?;
            self.set(last, grown)?;
        }
        Err(Error::Full)
    }

    /// Writes the 32 bytes of `entry` at `place`.
    fn put_entry(&mut self, place: Place, entry: &[u8; 32]) -> Result<(), Error> {
        let size = self.fat.volume.sector_size as usize;
        let mut room = [0u8; 4096];
        self.fat.sector(place.sector, &mut room[..size])?;
        room[place.index * 32..place.index * 32 + 32].copy_from_slice(entry);
        self.put(place.sector, &room[..size])
    }

    /// Removes the entry called `name` from the directory whose first
    /// cluster is `directory`, its long name with it, and gives its
    /// clusters back. A directory has to hold nothing.
    ///
    /// # Errors
    ///
    /// When no entry has the name, it is a directory that holds entries,
    /// its chain is broken, or the volume cannot be read or written.
    pub fn remove(&mut self, directory: u32, name: &str) -> Result<(), Error> {
        let (entry, place) = self.fat.lookup_at(directory, name)?;
        if entry.directory {
            let mut holds = false;
            self.fat.entries(entry.first, |_, _| {
                holds = true;
                false
            })?;
            if holds {
                return Err(Error::NotEmpty);
            }
        }
        let mut longs = [Place {
            sector: 0,
            index: 0,
        }; 20];
        let mut count = 0;
        let mut found = false;
        self.fat.walk(directory, |sector, bytes| {
            for (index, e) in bytes.as_chunks::<32>().0.iter().enumerate() {
                let here = Place { sector, index };
                if here == place {
                    found = true;
                    return false;
                }
                if e[0] != 0xe5 && e[11] & 0x3f == 0x0f && count < 20 {
                    longs[count] = here;
                    count += 1;
                } else {
                    count = 0;
                }
            }
            true
        })?;
        for at in longs[..count]
            .iter()
            .chain(core::iter::once(&place))
            .filter(|_| found)
        {
            let size = self.fat.volume.sector_size as usize;
            let mut room = [0u8; 4096];
            self.fat.sector(at.sector, &mut room[..size])?;
            room[at.index * 32] = 0xe5;
            self.put(at.sector, &room[..size])?;
        }
        if entry.first != 0 {
            self.give_back(entry.first)?;
        }
        Ok(())
    }

    /// Gives back every cluster of the chain from `first` on, and answers
    /// how many.
    ///
    /// # Errors
    ///
    /// When the chain leaves the volume or loops, or the volume cannot be
    /// read or written.
    pub fn give_back(&mut self, first: u32) -> Result<u32, Error> {
        let mut cluster = Some(first);
        let mut given = 0;
        while let Some(c) = cluster {
            if given == self.fat.volume.clusters || self.fat.volume.first_sector(c).is_none() {
                return Err(Error::BadChain);
            }
            cluster = self.fat.next(c)?;
            self.set(c, 0)?;
            given += 1;
        }
        self.count(-i64::from(given), None)?;
        Ok(given)
    }
}

/// A short entry called `short` with `attributes`, whose first cluster is
/// `first`, empty, made on [`DATE`].
pub(crate) fn short_entry(short: &[u8; 11], attributes: u8, first: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(short);
    e[11] = attributes;
    let first = first.to_le_bytes();
    e[20..22].copy_from_slice(&first[2..4]);
    e[26..28].copy_from_slice(&first[..2]);
    for at in [16, 18, 24] {
        e[at..at + 2].copy_from_slice(&DATE.to_le_bytes());
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::process::Command;
    use std::rc::Rc;

    fn require_tool(name: &str) -> bool {
        if Command::new(name).arg("--help").output().is_ok() {
            return true;
        }
        assert!(
            std::env::var("SEXTANT_REQUIRE_TOOLS").as_deref() != Ok("1"),
            "{name} is required but missing"
        );
        false
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("meridian-fat-{}-{name}.img", std::process::id()))
    }

    /// A FAT32 volume as mkfs.fat makes it: 40 MiB in sectors of 512
    /// bytes, one to a cluster, which gives 80628 clusters.
    fn formatted(name: &str) -> Option<Vec<u8>> {
        if !require_tool("mkfs.fat") {
            return None;
        }
        let path = scratch(name);
        let _ = std::fs::remove_file(&path);
        let made = Command::new("mkfs.fat")
            .args([
                "-F", "32", "-S", "512", "-s", "1", "-n", "TEST", "-i", "12345678", "-C",
            ])
            .arg(&path)
            .arg("40960")
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let image = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        Some(image)
    }

    /// Whether fsck.fat finds the volume `image` sound.
    fn sound(image: &[u8], name: &str) -> bool {
        if !require_tool("fsck.fat") {
            return true;
        }
        let path = scratch(name);
        std::fs::write(&path, image).unwrap();
        let checked = Command::new("fsck.fat")
            .arg("-n")
            .arg(&path)
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        checked.status.success()
    }

    type Image = Rc<RefCell<Vec<u8>>>;
    type Reads = Box<dyn FnMut(u64, &mut [u8]) -> bool>;
    type Writes = Box<dyn FnMut(u64, &[u8]) -> bool>;

    fn writer(image: &Image) -> Writer<Reads, Writes> {
        let (from, to) = (Rc::clone(image), Rc::clone(image));
        let reads: Reads = Box::new(move |at, into: &mut [u8]| {
            let start = usize::try_from(at).unwrap() * 512;
            into.copy_from_slice(&from.borrow()[start..start + into.len()]);
            true
        });
        let writes: Writes = Box::new(move |at, bytes: &[u8]| {
            let start = usize::try_from(at).unwrap() * 512;
            to.borrow_mut()[start..start + bytes.len()].copy_from_slice(bytes);
            true
        });
        Writer::new(Fat::open(reads).unwrap(), writes)
    }

    /// Cluster `cluster`'s entry in FAT `copy` of mkfs.fat's layout.
    fn entry_in(image: &[u8], copy: usize, cluster: u32) -> u32 {
        u32_at(image, (32 + copy * 630) * 512 + 4 * cluster as usize)
    }

    fn fsinfo(image: &[u8]) -> (u32, u32) {
        (u32_at(image, 512 + FREE), u32_at(image, 512 + NEXT))
    }

    /// A volume mkfs.fat made, with `files` copied onto its root by mcopy.
    fn holding(name: &str, files: &[(&str, &[u8])]) -> Option<Vec<u8>> {
        let image = formatted(name)?;
        if !require_tool("mcopy") {
            return None;
        }
        let path = scratch(name);
        std::fs::write(&path, &image).unwrap();
        for (file, bytes) in files {
            let source = scratch(&format!("{name}-{file}"));
            std::fs::write(&source, bytes).unwrap();
            let copied = Command::new("mcopy")
                .arg("-i")
                .arg(&path)
                .arg(&source)
                .arg(format!("::{file}"))
                .output()
                .unwrap();
            assert!(copied.status.success(), "{copied:?}");
            let _ = std::fs::remove_file(&source);
        }
        let image = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        Some(image)
    }

    /// The file `file` on the volume `image`, as mtype reads it.
    fn typed(image: &[u8], name: &str, file: &str) -> Vec<u8> {
        let path = scratch(name);
        std::fs::write(&path, image).unwrap();
        let out = Command::new("mtype")
            .arg("-i")
            .arg(&path)
            .arg(format!("::{file}"))
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(out.status.success(), "{out:?}");
        out.stdout
    }

    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| u8::try_from(i % 251).unwrap() ^ seed)
            .collect()
    }

    #[test]
    fn a_file_written_inside_keeps_the_rest() {
        let old = pattern(1000, 7);
        let Some(image) = holding("inside", &[("a.txt", &old)]) else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (mut entry, place) = w.fat.lookup_at(root, "a.txt").unwrap();
        let before = image.borrow()[entry_bytes(place)].to_vec();
        assert_eq!(w.write_at(&mut entry, 510, b"across a sector"), Ok(15));
        assert_eq!(entry.size, 1000, "no longer than it was");
        w.save(place, &entry).unwrap();
        let mut want = old.clone();
        want[510..525].copy_from_slice(b"across a sector");
        let now = image.borrow().clone();
        assert_eq!(typed(&now, "inside", "a.txt"), want);
        assert_eq!(now[entry_bytes(place)], before[..], "the entry as it was");
        assert!(sound(&now, "inside"));
    }

    /// Where the short entry at `place` lies in the image.
    fn entry_bytes(place: Place) -> core::ops::Range<usize> {
        let at = usize::try_from(place.sector).unwrap() * 512 + place.index * 32;
        at..at + 32
    }

    #[test]
    fn a_file_that_grows_takes_clusters_for_it() {
        let old = pattern(1000, 3);
        let Some(image) = holding("grow", &[("b.bin", &old)]) else {
            return;
        };
        let free = fsinfo(&image).0;
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (mut entry, place) = w.fat.lookup_at(root, "b.bin").unwrap();
        let more = pattern(3000, 9);
        assert_eq!(w.write_at(&mut entry, 1000, &more), Ok(3000));
        assert_eq!(entry.size, 4000);
        w.save(place, &entry).unwrap();
        let now = image.borrow().clone();
        let mut want = old;
        want.extend_from_slice(&more);
        assert_eq!(typed(&now, "grow", "b.bin"), want);
        assert_eq!(
            fsinfo(&now).0,
            free - 6,
            "two clusters held a kilobyte, eight hold four"
        );
        assert!(sound(&now, "grow"));
        let mut read = vec![0u8; 4000];
        assert_eq!(w.fat.read_file(&entry, &mut read), Ok(4000));
        assert_eq!(read, want, "as the reader reads it too");
    }

    #[test]
    fn what_lies_between_a_file_and_a_write_past_its_end_reads_as_zeros() {
        let Some(mut image) = holding("gap", &[("c.bin", b"short")]) else {
            return;
        };
        let root_place = {
            let probe = Rc::new(RefCell::new(image.clone()));
            let mut w = writer(&probe);
            let root = w.fat.volume.root;
            let (entry, _) = w.fat.lookup_at(root, "c.bin").unwrap();
            w.fat.volume.first_sector(entry.first).unwrap()
        };
        let sector = usize::try_from(root_place).unwrap() * 512;
        image[sector + 5..sector + 512].fill(0xaa);
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (mut entry, place) = w.fat.lookup_at(root, "c.bin").unwrap();
        assert_eq!(w.write_at(&mut entry, 5000, b"end"), Ok(3));
        w.save(place, &entry).unwrap();
        let now = image.borrow().clone();
        let mut want = b"short".to_vec();
        want.resize(5000, 0);
        want.extend_from_slice(b"end");
        assert_eq!(
            typed(&now, "gap", "c.bin"),
            want,
            "not what lay past its old end"
        );
        assert!(sound(&now, "gap"));
    }

    #[test]
    fn an_empty_file_gets_its_first_cluster() {
        let Some(image) = holding("empty", &[("d.txt", b"")]) else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (mut entry, place) = w.fat.lookup_at(root, "d.txt").unwrap();
        assert_eq!((entry.first, entry.size), (0, 0));
        assert_eq!(w.write_at(&mut entry, 0, &[]), Ok(0), "nothing written");
        assert_eq!(entry.first, 0, "and nothing taken");
        assert_eq!(w.write_at(&mut entry, 0, b"now"), Ok(3));
        assert!(entry.first >= 3);
        w.save(place, &entry).unwrap();
        let now = image.borrow().clone();
        assert_eq!(typed(&now, "empty", "d.txt"), b"now");
        assert!(sound(&now, "empty"));
    }

    #[test]
    fn no_file_grows_past_what_fat32_sizes_hold() {
        let Some(image) = holding("large", &[("e.txt", b"e")]) else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let before = image.borrow().clone();
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (mut entry, _) = w.fat.lookup_at(root, "e.txt").unwrap();
        assert_eq!(
            w.write_at(&mut entry, u64::from(u32::MAX), b"x"),
            Err(Error::TooLarge)
        );
        assert_eq!(w.write_at(&mut entry, u64::MAX, b"x"), Err(Error::TooLarge));
        assert_eq!(w.write_at(&mut entry, 1 << 32, &[]), Err(Error::TooLarge));
        assert_eq!(*image.borrow(), before, "and nothing was written");
    }

    /// What mdir lists for `directory` on the volume `image`, bare names
    /// only.
    fn listed(image: &[u8], name: &str, directory: &str) -> Vec<String> {
        let path = scratch(name);
        std::fs::write(&path, image).unwrap();
        let out = Command::new("mdir")
            .args(["-b", "-i"])
            .arg(&path)
            .arg(format!("::{directory}"))
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| {
                let l = l.trim_end_matches('/');
                l.rsplit('/').next().unwrap_or(l).to_string()
            })
            .collect()
    }

    #[test]
    fn a_file_made_with_a_long_name_holds_what_was_written() {
        let Some(image) = formatted("create") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (mut entry, place) = w.create(root, "services.meri", false).unwrap();
        let text = b"service shell {\n    program = shell\n}\n";
        w.write_at(&mut entry, 0, text).unwrap();
        w.save(place, &entry).unwrap();
        let now = image.borrow().clone();
        assert_eq!(typed(&now, "create", "services.meri"), text);
        assert_eq!(listed(&now, "create", "/"), ["services.meri"]);
        assert_eq!(w.fat.find("services.meri").map(|e| e.size), Ok(38));
        assert_eq!(
            w.create(root, "SERVICES.MERI", false),
            Err(Error::Exists),
            "names in any case are one"
        );
        assert!(sound(&now, "create"));
    }

    #[test]
    fn a_directory_made_holds_a_file_made_in_it() {
        let Some(image) = formatted("directory") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (folder, _) = w.create(root, "Programs", true).unwrap();
        assert!(folder.directory && folder.first >= 3);
        let (inner, _) = w.create(folder.first, "inner", true).unwrap();
        let (mut file, place) = w.create(inner.first, "hello.mexec", false).unwrap();
        w.write_at(&mut file, 0, &pattern(2000, 1)).unwrap();
        w.save(place, &file).unwrap();
        let now = image.borrow().clone();
        assert_eq!(
            typed(&now, "directory", "Programs/inner/hello.mexec"),
            pattern(2000, 1)
        );
        assert_eq!(listed(&now, "directory", "/Programs"), ["inner"]);
        assert!(
            sound(&now, "directory"),
            "with their . and .. as they should be"
        );
        let found = w.fat.find("programs/INNER/Hello.mexec").unwrap();
        assert_eq!((found.size, found.directory), (2000, false));
    }

    #[test]
    fn a_full_directory_grows_for_more_entries() {
        let Some(image) = formatted("many") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let names: Vec<String> = (0..40).map(|i| format!("a file called {i}.txt")).collect();
        for name in &names {
            w.create(root, name, false).unwrap();
        }
        let now = image.borrow().clone();
        let mut want = names.clone();
        want.sort();
        let mut got = listed(&now, "many", "/");
        got.sort();
        assert_eq!(
            got, want,
            "forty names of three entries each, past ten clusters' worth"
        );
        assert!(sound(&now, "many"));
    }

    #[test]
    fn short_names_do_not_meet() {
        let Some(image) = formatted("short") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (_, first) = w.create(root, "readme.txt", false).unwrap();
        let (_, other) = w.create(root, "readme.text", false).unwrap();
        assert_eq!(
            w.create(root, "README.TXT", false),
            Err(Error::Exists),
            "README.TXT is readme.txt"
        );
        let bytes = image.borrow().clone();
        let short = |p: Place| bytes[entry_bytes(p)][..11].to_vec();
        assert_eq!(short(first), b"README~1TXT");
        assert_eq!(short(other), b"README~1TEX");
        let (_, second) = w.create(root, "readme!.txt", false).unwrap();
        assert_eq!(image.borrow()[entry_bytes(second)][..11], *b"README~2TXT");
        assert!(sound(&image.borrow(), "short"));
        assert_eq!(w.create(root, &"y".repeat(256), false), Err(Error::BadName));
    }

    #[test]
    fn a_removed_file_gives_its_clusters_back() {
        let Some(image) = holding(
            "remove",
            &[
                ("keep.txt", b"kept"),
                ("long file name.bin", &pattern(5000, 2)),
            ],
        ) else {
            return;
        };
        let free = fsinfo(&image).0;
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        assert_eq!(w.remove(root, "long file name.bin"), Ok(()));
        let now = image.borrow().clone();
        assert_eq!(listed(&now, "remove", "/"), ["keep.txt"]);
        assert_eq!(fsinfo(&now).0, free + 10, "its ten clusters back");
        assert_eq!(typed(&now, "remove", "keep.txt"), b"kept");
        assert!(sound(&now, "remove"), "no long name left over");
        assert_eq!(w.remove(root, "long file name.bin"), Err(Error::NotFound));
    }

    #[test]
    fn only_an_empty_directory_is_removed() {
        let Some(image) = formatted("rmdir") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let root = w.fat.volume.root;
        let (folder, _) = w.create(root, "folder", true).unwrap();
        w.create(folder.first, "inside", false).unwrap();
        assert_eq!(w.remove(root, "folder"), Err(Error::NotEmpty));
        assert_eq!(w.remove(folder.first, "inside"), Ok(()));
        assert_eq!(w.remove(root, "folder"), Ok(()));
        let now = image.borrow().clone();
        assert_eq!(listed(&now, "rmdir", "/"), Vec::<String>::new());
        assert_eq!(fsinfo(&now).0, 80_627, "every cluster back");
        assert!(sound(&now, "rmdir"));
    }

    #[test]
    fn a_cluster_taken_ends_a_chain_of_its_own_in_both_fats() {
        let Some(image) = formatted("take") else {
            return;
        };
        assert_eq!(fsinfo(&image), (80_627, 2), "as mkfs.fat leaves it");
        let image = Rc::new(RefCell::new(image));
        let before = image.borrow().clone();
        let mut w = writer(&image);
        let (a, b) = (w.take().unwrap(), w.take().unwrap());
        assert_eq!((a, b), (3, 4), "past the root directory's");
        {
            let now = image.borrow();
            for copy in 0..2 {
                assert_eq!(entry_in(&now, copy, a), LAST);
                assert_eq!(entry_in(&now, copy, b), LAST);
            }
            assert_eq!(fsinfo(&now), (80_625, 5));
        }
        assert_eq!((w.give_back(a), w.give_back(b)), (Ok(1), Ok(1)));
        let after = image.borrow().clone();
        assert_eq!(after[..512], before[..512]);
        assert_eq!(after[1024..], before[1024..], "both FATs as they were");
        assert_eq!(fsinfo(&after), (80_627, 5));
        assert!(sound(&after, "take"));
    }

    #[test]
    fn a_chain_is_given_back_whole() {
        let Some(image) = formatted("chain") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let chain = [w.take().unwrap(), w.take().unwrap(), w.take().unwrap()];
        w.set(chain[0], chain[2]).unwrap();
        w.set(chain[2], chain[1]).unwrap();
        assert_eq!(w.give_back(chain[0]), Ok(3));
        let now = image.borrow();
        assert!(
            chain
                .iter()
                .all(|&c| entry_in(&now, 0, c) == 0 && entry_in(&now, 1, c) == 0)
        );
        assert_eq!(fsinfo(&now).0, 80_627);
        assert!(sound(&now, "chain"));
    }

    #[test]
    fn a_broken_chain_is_not_given_back() {
        let Some(image) = formatted("broken") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        let (a, b) = (w.take().unwrap(), w.take().unwrap());
        w.set(a, b).unwrap();
        w.set(b, a).unwrap();
        assert_eq!(w.give_back(a), Err(Error::BadChain), "a loop");
        let c = w.take().unwrap();
        w.set(c, 90_000).unwrap();
        assert_eq!(w.give_back(c), Err(Error::BadChain), "off the volume");
        assert_eq!(
            w.give_back(1),
            Err(Error::BadChain),
            "no cluster of the data region"
        );
    }

    #[test]
    fn looking_for_a_free_cluster_wraps_around() {
        let Some(mut image) = formatted("wrap") else {
            return;
        };
        let last = 80_629u32;
        image[512 + NEXT..512 + NEXT + 4].copy_from_slice(&last.to_le_bytes());
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        assert_eq!(w.take(), Ok(last), "where FSInfo says");
        assert_eq!(fsinfo(&image.borrow()).1, 2, "and on from the first");
        assert_eq!(w.take(), Ok(3));
        let mut odd = image.borrow().clone();
        odd[512 + NEXT..512 + NEXT + 4].copy_from_slice(&90_000u32.to_le_bytes());
        *image.borrow_mut() = odd;
        assert_eq!(
            writer(&image).take(),
            Ok(4),
            "a hint off the volume is no hint"
        );
    }

    #[test]
    fn a_full_volume_has_nothing_to_take() {
        let Some(mut image) = formatted("full") else {
            return;
        };
        for copy in 0..2 {
            let start = (32 + copy * 630) * 512 + 12;
            let end = start + 4 * 80_627;
            image[start..end].fill(0xff);
        }
        let image = Rc::new(RefCell::new(image));
        let before = image.borrow().clone();
        assert_eq!(writer(&image).take(), Err(Error::Full));
        assert_eq!(*image.borrow(), before, "and leaves it as it was");
    }

    #[test]
    fn without_fsinfo_nothing_is_counted() {
        let Some(mut image) = formatted("bare") else {
            return;
        };
        image[48..50].copy_from_slice(&0u16.to_le_bytes());
        let image = Rc::new(RefCell::new(image));
        let mut w = writer(&image);
        assert_eq!(w.take(), Ok(3));
        assert_eq!(fsinfo(&image.borrow()), (80_627, 2), "FSInfo is left alone");
        let mut unsigned = image.borrow().clone();
        unsigned[48] = 1;
        unsigned[512 + 484] = 0;
        *image.borrow_mut() = unsigned;
        let mut w = writer(&image);
        assert_eq!(w.take(), Ok(4), "nor is one without its signature trusted");
        assert_eq!(fsinfo(&image.borrow()), (80_627, 2));
        let mut unknown = image.borrow().clone();
        unknown[512 + 484..512 + 488].copy_from_slice(&MIDDLE.to_le_bytes());
        unknown[512 + FREE..512 + FREE + 4].copy_from_slice(&UNKNOWN.to_le_bytes());
        *image.borrow_mut() = unknown;
        assert_eq!(writer(&image).take(), Ok(5));
        assert_eq!(
            fsinfo(&image.borrow()),
            (UNKNOWN, 6),
            "a count not known stays so"
        );
    }

    #[test]
    fn a_volume_that_takes_no_writes_says_so() {
        let Some(image) = formatted("refused") else {
            return;
        };
        let image = Rc::new(RefCell::new(image));
        let from = Rc::clone(&image);
        let fat = Fat::open(move |at, into: &mut [u8]| {
            let start = usize::try_from(at).unwrap() * 512;
            into.copy_from_slice(&from.borrow()[start..start + into.len()]);
            true
        })
        .unwrap();
        let mut w = Writer::new(fat, |_, _: &[u8]| false);
        assert_eq!(w.take(), Err(Error::Write));
        assert_eq!((LEAD, TRAIL, LAST), (0x4161_5252, 0xaa55_0000, 0x0fff_ffff));
    }
}
