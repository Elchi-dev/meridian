//! A FAT32 volume in the tree of files, the parts that need no processor:
//! what the names of a path lead to on it, and files and directories
//! opened, read, written, made and removed as `kernel::vfs` lets a token.

use abi::file;
use fat::write::Writer;
use fat::{Entry, Fat, Place};
use security::{Token, rights};

use crate::vfs::{self, Disposition, NoDisposition, Refusal};

/// Why a file could not be opened, read, written, made or removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The path is none, or names nothing that can be made or removed.
    BadPath,
    /// No file has the path.
    NotFound,
    /// It is a directory, which has no bytes to read or write.
    NotAFile,
    /// The token may not do it, or the volume is mounted read only.
    Refused(Refusal),
    /// It is a file, which has no entries to list.
    NotADirectory,
    /// The room given is too small for what was asked.
    TooSmall,
    /// Every file that can be open is.
    NoRoom,
    /// Its volume could not be read.
    Read,
    /// What to do by whether the file is there is none of the ways.
    BadDisposition,
    /// A file or directory has the path already.
    Exists,
    /// No room is left on the volume.
    DiskFull,
    /// The directory holds entries.
    NotEmpty,
    /// The file is open.
    InUse,
    /// Its volume could not be written.
    Unwritable,
    /// A directory would move into itself, or into a directory inside it.
    Inside,
    /// The two paths of a move are on different volumes.
    NotSameVolume,
}

impl From<fat::Error> for Error {
    fn from(e: fat::Error) -> Error {
        match e {
            fat::Error::NotFound | fat::Error::NotADirectory => Error::NotFound,
            fat::Error::Write => Error::Unwritable,
            fat::Error::Full | fat::Error::TooLarge => Error::DiskFull,
            fat::Error::Exists => Error::Exists,
            fat::Error::NotEmpty => Error::NotEmpty,
            fat::Error::BadName => Error::BadPath,
            fat::Error::Inside => Error::Inside,
            fat::Error::NotFat | fat::Error::NotFat32 | fat::Error::Read | fat::Error::BadChain => {
                Error::Read
            }
        }
    }
}

/// A file opened on a volume: what its directory entry says, where that
/// lies, none for the volume's root, and whether it may be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opened {
    pub entry: Entry,
    pub place: Option<Place>,
    pub writes: bool,
}

/// A FAT32 volume read through `R` and written through `W`, or mounted
/// read only, which then writes nothing whatever `W` would do.
pub struct Volume<R, W> {
    fs: Writer<R, W>,
    pub read_only: bool,
}

impl<R: FnMut(u64, &mut [u8]) -> bool, W: FnMut(u64, &[u8]) -> bool> Volume<R, W> {
    #[must_use]
    pub fn new(fat: Fat<R>, write: W, read_only: bool) -> Volume<R, W> {
        Volume {
            fs: Writer::new(fat, write),
            read_only,
        }
    }

    /// The entry `names` lead to from the root, and where it lies.
    fn walk(&mut self, names: &[&str]) -> Result<(Entry, Option<Place>), Error> {
        let mut entry = Entry {
            first: self.fs.fat.volume.root,
            size: 0,
            directory: true,
        };
        let mut place = None;
        for name in names {
            if !entry.directory {
                return Err(Error::NotFound);
            }
            let (found, at) = self.fs.fat.lookup_at(entry.first, name)?;
            entry = found;
            place = Some(at);
        }
        Ok((entry, place))
    }

    /// The first cluster of the directory all of `names` but the last lead
    /// to, and the last: the name of what is made or removed in it.
    fn parent<'p>(&mut self, names: &[&'p str]) -> Result<(u32, &'p str), Error> {
        let (name, above) = names.split_last().ok_or(Error::BadPath)?;
        let (directory, _) = self.walk(above)?;
        if !directory.directory {
            return Err(Error::NotFound);
        }
        Ok((directory.first, name))
    }

    /// Opens the file `names` lead to for the rights `wanted`, as `token`
    /// may, doing what `how` says by whether it is there, as OPEN takes
    /// it. Making or emptying a file takes the right to write it.
    ///
    /// # Errors
    ///
    /// When `how` is none of the ways or does nothing with the file as it
    /// is, the token may not open it so, the volume is mounted read only
    /// and it would be written, a directory would be emptied, or the volume
    /// cannot be read or written.
    pub fn open(
        &mut self,
        names: &[&str],
        token: &Token,
        wanted: u32,
        how: u64,
    ) -> Result<Opened, Error> {
        let found = match self.walk(names) {
            Ok(found) => Some(found),
            Err(Error::NotFound) => None,
            Err(e) => return Err(e),
        };
        let disposition = disposed(how, found.is_some())?;
        vfs::may_open(
            token,
            &vfs::plain(),
            rights(disposition, wanted),
            self.read_only,
        )
        .map_err(Error::Refused)?;
        let (entry, place) = match (disposition, found) {
            (Disposition::Open, Some(found)) => found,
            (Disposition::Empty, Some((mut entry, Some(place)))) if !entry.directory => {
                if entry.first != 0 {
                    self.fs.give_back(entry.first)?;
                }
                entry.first = 0;
                entry.size = 0;
                self.fs.save(place, &entry)?;
                (entry, Some(place))
            }
            (Disposition::Create, _) => {
                let (directory, name) = self.parent(names)?;
                let (entry, place) = self.fs.create(directory, name, false)?;
                (entry, Some(place))
            }
            _ => return Err(Error::NotAFile),
        };
        Ok(Opened {
            entry,
            place,
            writes: wanted & file::WRITE != 0,
        })
    }

    /// Reads the file `opened` from its byte `offset` on into `into`, and
    /// returns how many bytes it read.
    ///
    /// # Errors
    ///
    /// When it is a directory, or the volume cannot be read.
    pub fn read(&mut self, opened: &Opened, offset: u64, into: &mut [u8]) -> Result<usize, Error> {
        if opened.entry.directory {
            return Err(Error::NotAFile);
        }
        self.fs
            .fat
            .read_at(&opened.entry, offset, into)
            .map_err(|_| Error::Read)
    }

    /// Writes `from` into the file `opened` from its byte `offset` on,
    /// which may lie past its end, and returns what its directory entry
    /// says now. Bytes between its old end and `offset` read as zeros.
    ///
    /// # Errors
    ///
    /// When it was not opened to write, it is a directory, no room is left,
    /// or the volume cannot be read or written.
    pub fn write(&mut self, opened: &Opened, offset: u64, from: &[u8]) -> Result<Entry, Error> {
        if !opened.writes {
            return Err(Error::Refused(Refusal::Denied(file::WRITE)));
        }
        let (false, Some(place)) = (opened.entry.directory, opened.place) else {
            return Err(Error::NotAFile);
        };
        let mut entry = opened.entry;
        self.fs.write_at(&mut entry, offset, from)?;
        self.fs.save(place, &entry)?;
        Ok(entry)
    }

    /// Writes the name of entry `index` of the directory `opened`, counted
    /// from 0, into `into` in UTF-8, and returns its length and whether
    /// that entry is a directory itself.
    ///
    /// # Errors
    ///
    /// When it is a file, it has no entry `index`, the name does not fit,
    /// or the volume cannot be read.
    pub fn list(
        &mut self,
        opened: &Opened,
        index: u64,
        into: &mut [u8],
    ) -> Result<(usize, bool), Error> {
        if !opened.entry.directory {
            return Err(Error::NotADirectory);
        }
        let mut found = None;
        let mut at = 0;
        self.fs
            .fat
            .entries(opened.entry.first, |units, entry| {
                if at == index {
                    found = Some((vfs::name_into(units, into), entry.directory));
                    return false;
                }
                at += 1;
                true
            })
            .map_err(|_| Error::Read)?;
        let (len, directory) = found.ok_or(Error::NotFound)?;
        Ok((len.ok_or(Error::TooSmall)?, directory))
    }

    /// Removes the file or the empty directory `names` lead to, as `token`
    /// may, unless `open` says it is open, by where its entry lies.
    ///
    /// # Errors
    ///
    /// When `names` lead to the root, nothing is there, the token may not
    /// remove it, the volume is mounted read only, it is open, it is a
    /// directory that holds entries, or the volume cannot be read or
    /// written.
    pub fn remove(
        &mut self,
        names: &[&str],
        token: &Token,
        open: impl Fn(Place) -> bool,
    ) -> Result<(), Error> {
        vfs::may_open(token, &vfs::plain(), rights::DELETE, self.read_only)
            .map_err(Error::Refused)?;
        let (directory, name) = self.parent(names)?;
        let (_, place) = self.walk(names)?;
        if place.is_some_and(open) {
            return Err(Error::InUse);
        }
        Ok(self.fs.remove(directory, name)?)
    }

    /// Moves the file or directory `from` leads to where `to` leads on the
    /// volume, as `token` may, unless `open` says it is open, by where its
    /// entry lies.
    ///
    /// # Errors
    ///
    /// When either leads to the root, nothing is at `from`, the directory
    /// `to` would be in is not there, something else is at `to`, the token
    /// may not remove what is at `from`, the volume is mounted read only, it
    /// is open, a directory would go inside itself, no room is left, or the
    /// volume cannot be read or written.
    pub fn rename(
        &mut self,
        from: &[&str],
        to: &[&str],
        token: &Token,
        open: impl Fn(Place) -> bool,
    ) -> Result<(), Error> {
        vfs::may_open(token, &vfs::plain(), rights::DELETE, self.read_only)
            .map_err(Error::Refused)?;
        let (source, name) = self.parent(from)?;
        let (target, new) = self.parent(to)?;
        let (_, place) = self.walk(from)?;
        if place.is_some_and(open) {
            return Err(Error::InUse);
        }
        self.fs.rename(source, name, target, new)?;
        Ok(())
    }

    /// Makes a directory where `names` lead, as `token` may.
    ///
    /// # Errors
    ///
    /// When `names` lead to the root, the directory it would be in is not
    /// there, the token may not write there, the volume is mounted read
    /// only, something is there already, no room is left, or the volume
    /// cannot be read or written.
    pub fn make_directory(&mut self, names: &[&str], token: &Token) -> Result<(), Error> {
        vfs::may_open(token, &vfs::plain(), file::WRITE, self.read_only).map_err(Error::Refused)?;
        let (directory, name) = self.parent(names)?;
        self.fs.create(directory, name, true)?;
        Ok(())
    }
}

/// What to do with a file by whether it `exists`, as `how` says.
fn disposed(how: u64, exists: bool) -> Result<Disposition, Error> {
    vfs::disposition(how, exists).map_err(|e| match e {
        NoDisposition::Unknown => Error::BadDisposition,
        NoDisposition::Missing => Error::NotFound,
        NoDisposition::Exists => Error::Exists,
    })
}

/// The rights opening a file as `disposition` says takes, besides those
/// `wanted`: making or emptying it writes it.
fn rights(disposition: Disposition, wanted: u32) -> u32 {
    match disposition {
        Disposition::Open => wanted,
        Disposition::Create | Disposition::Empty => wanted | file::WRITE,
    }
}

/// Whether `token` may open a directory of the tree itself for `wanted`,
/// as `how` says: as one on a volume mounted read only, since the tree
/// above the volumes lies on none of them and nothing writes it.
///
/// # Errors
///
/// As [`Volume::open`] answers for a directory on a volume read only.
pub fn open_tree(token: &Token, wanted: u32, how: u64) -> Result<(), Error> {
    let disposition = disposed(how, true)?;
    vfs::may_open(token, &vfs::plain(), rights(disposition, wanted), true).map_err(Error::Refused)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use abi::file::{
        CREATE_ALWAYS, CREATE_NEW, OPEN_ALWAYS, OPEN_EXISTING, READ, TRUNCATE_EXISTING, WRITE,
    };
    use security::{Integrity, Privileges, Restriction};

    use super::*;

    type Image = Rc<RefCell<Vec<u8>>>;
    type Reads = Box<dyn FnMut(u64, &mut [u8]) -> bool>;
    type Writes = Box<dyn FnMut(u64, &[u8]) -> bool>;

    fn reads(image: &Image) -> Reads {
        let image = Rc::clone(image);
        Box::new(move |at, into: &mut [u8]| {
            let start = usize::try_from(at).unwrap() * 512;
            let image = image.borrow();
            let bytes = image.get(start..start + into.len());
            bytes.inspect(|b| into.copy_from_slice(b)).is_some()
        })
    }

    /// Writes into `image` while `works` holds true.
    fn writes(image: &Image, works: &Rc<Cell<bool>>) -> Writes {
        let (image, works) = (Rc::clone(image), Rc::clone(works));
        Box::new(move |at, bytes: &[u8]| {
            let start = usize::try_from(at).unwrap() * 512;
            image.borrow_mut()[start..start + bytes.len()].copy_from_slice(bytes);
            works.get()
        })
    }

    /// A FAT32 volume in memory, the least FAT32 takes, holding `a.txt`
    /// of ten bytes and an empty directory `Dir`.
    fn image() -> Image {
        let total = 66_700;
        let image = Rc::new(RefCell::new(vec![0u8; total * 512]));
        let works = Rc::new(Cell::new(true));
        fat::format::format(total as u64, 0, 9, b"SYSTEM     ", writes(&image, &works)).unwrap();
        let mut w = Writer::new(Fat::open(reads(&image)).unwrap(), writes(&image, &works));
        let root = w.fat.volume.root;
        w.create(root, "Dir", true).unwrap();
        let (mut entry, place) = w.create(root, "a.txt", false).unwrap();
        w.write_at(&mut entry, 0, b"0123456789").unwrap();
        w.save(place, &entry).unwrap();
        image
    }

    fn volume(image: &Image, read_only: bool) -> Volume<Reads, Writes> {
        let works = Rc::new(Cell::new(true));
        Volume::new(
            Fat::open(reads(image)).unwrap(),
            writes(image, &works),
            read_only,
        )
    }

    fn system() -> Token {
        Token::system()
    }

    /// The whole of the file `opened`.
    fn whole(v: &mut Volume<Reads, Writes>, opened: &Opened) -> Vec<u8> {
        let mut bytes = vec![0u8; opened.entry.size as usize];
        let mut done = 0;
        while done < bytes.len() {
            done += v.read(opened, done as u64, &mut bytes[done..]).unwrap();
        }
        bytes
    }

    #[test]
    fn the_tree_opens_as_a_directory_read_only_does() {
        let image = image();
        let mut v = volume(&image, true);
        let me = security::Sid::new(5, &[21, 1, 2, 3, 1001]).unwrap();
        let groups = [
            security::Group::of(security::known::EVERYONE),
            security::Group::of(security::known::USERS),
        ];
        let user = Token::new(me, &groups, Privileges::NONE, Integrity::Medium).unwrap();
        for token in [system(), user] {
            for wanted in [0, READ, WRITE, READ | WRITE, file::RUN, file::APPEND] {
                for how in [
                    0,
                    OPEN_EXISTING,
                    OPEN_ALWAYS,
                    CREATE_NEW,
                    CREATE_ALWAYS,
                    TRUNCATE_EXISTING,
                    99,
                ] {
                    assert_eq!(
                        open_tree(&token, wanted, how),
                        v.open(&["Dir"], &token, wanted, how).map(|_| ()),
                        "{wanted:#x} {how}"
                    );
                }
            }
        }
        assert_eq!(open_tree(&system(), READ, OPEN_EXISTING), Ok(()));
        assert_eq!(
            open_tree(&system(), WRITE, OPEN_EXISTING),
            Err(Error::Refused(Refusal::ReadOnly))
        );
    }

    #[test]
    fn open_does_what_it_is_told_by_whether_the_file_is_there() {
        let image = image();
        let mut v = volume(&image, false);
        let a = v.open(&["a.txt"], &system(), READ, 0).unwrap();
        assert_eq!(
            (a.entry.size, a.writes, a.place.is_some()),
            (10, false, true)
        );
        assert_eq!(
            v.open(&["A.TXT"], &system(), READ, OPEN_EXISTING),
            Ok(a),
            "in any case"
        );
        assert_eq!(v.open(&["b.txt"], &system(), READ, 0), Err(Error::NotFound));
        assert_eq!(
            v.open(&["b.txt"], &system(), READ, TRUNCATE_EXISTING),
            Err(Error::NotFound)
        );
        let b = v
            .open(&["b.txt"], &system(), READ | WRITE, CREATE_NEW)
            .unwrap();
        assert_eq!((b.entry.size, b.writes), (0, true));
        assert_eq!(
            v.open(&["b.txt"], &system(), READ, CREATE_NEW),
            Err(Error::Exists)
        );
        assert_eq!(
            v.open(&["b.txt"], &system(), READ, OPEN_ALWAYS)
                .map(|o| o.place),
            Ok(b.place)
        );
        let c = v
            .open(&["Dir", "c.txt"], &system(), READ, OPEN_ALWAYS)
            .unwrap();
        assert_eq!(c.entry.size, 0, "made where it was not");
        assert_eq!(
            v.open(&["Dir", "c.txt"], &system(), READ, 0)
                .map(|o| o.place),
            Ok(c.place)
        );
        assert_eq!(
            v.open(&["a.txt"], &system(), READ, 6),
            Err(Error::BadDisposition)
        );
        assert_eq!(
            v.open(&["z.txt"], &system(), READ, 6),
            Err(Error::BadDisposition)
        );
        assert_eq!(
            v.open(&["No", "x.txt"], &system(), READ, CREATE_NEW),
            Err(Error::NotFound)
        );
        assert_eq!(
            v.open(&["a.txt", "x.txt"], &system(), READ, CREATE_NEW),
            Err(Error::NotFound),
            "a file holds nothing"
        );
        assert_eq!(
            v.open(&["a.txt", "x.txt"], &system(), READ, 0),
            Err(Error::NotFound)
        );
        // A file that holds what a directory entry for X.TXT would, and an
        // empty one, whose first cluster 0 stands for the root in a
        // directory's "..": neither is a directory to look in.
        let mut entry = [0u8; 32];
        entry[..11].copy_from_slice(b"X       TXT");
        entry[11] = 0x20;
        let trick = v
            .open(&["trick"], &system(), READ | WRITE, CREATE_NEW)
            .unwrap();
        v.write(&trick, 0, &entry).unwrap();
        assert_eq!(
            v.open(&["trick", "x.txt"], &system(), READ, 0),
            Err(Error::NotFound)
        );
        v.open(&["empty"], &system(), READ, CREATE_NEW).unwrap();
        assert_eq!(
            v.open(&["empty", "a.txt"], &system(), READ, 0),
            Err(Error::NotFound)
        );
        let root = v.open(&[], &system(), READ, 0).unwrap();
        assert_eq!((root.entry.directory, root.place), (true, None));
    }

    #[test]
    fn emptying_a_file_gives_its_clusters_back_and_stays() {
        let image = image();
        let mut v = volume(&image, false);
        let a = v.open(&["a.txt"], &system(), READ | WRITE, 0).unwrap();
        let first = a.entry.first;
        assert_ne!(first, 0);
        let emptied = v.open(&["a.txt"], &system(), READ, CREATE_ALWAYS).unwrap();
        assert_eq!((emptied.entry.size, emptied.entry.first), (0, 0));
        assert!(!emptied.writes, "emptying asks for no handle that writes");
        let mut again = volume(&image, false);
        let read = again.open(&["a.txt"], &system(), READ, 0).unwrap();
        assert_eq!(
            (read.entry.size, read.entry.first),
            (0, 0),
            "as the disk has it"
        );
        let (sector, offset) = again.fs.fat.volume.entry_of(first);
        let at = usize::try_from(sector).unwrap() * 512 + offset;
        assert_eq!(
            &image.borrow()[at..at + 4],
            [0; 4],
            "its cluster is free in the FAT"
        );
        let b = again
            .open(&["b.txt"], &system(), READ | WRITE, CREATE_NEW)
            .unwrap();
        again.write(&b, 0, b"x").unwrap();
        let t = again
            .open(&["b.txt"], &system(), READ, TRUNCATE_EXISTING)
            .unwrap();
        assert_eq!(t.entry.size, 0);
        let empty = again
            .open(&["b.txt"], &system(), READ, TRUNCATE_EXISTING)
            .unwrap();
        assert_eq!(empty.entry, t.entry, "an empty file empties as it is");
        assert_eq!(
            again.open(&["Dir"], &system(), READ, CREATE_ALWAYS),
            Err(Error::NotAFile)
        );
        assert_eq!(
            again.open(&[], &system(), READ, TRUNCATE_EXISTING),
            Err(Error::NotAFile)
        );
    }

    #[test]
    fn writes_land_where_asked_and_read_back() {
        let image = image();
        let mut v = volume(&image, false);
        let file = v
            .open(&["Dir", "big.bin"], &system(), READ | WRITE, CREATE_NEW)
            .unwrap();
        let bytes: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
        let file = Opened {
            entry: v.write(&file, 0, &bytes).unwrap(),
            ..file
        };
        assert_eq!(file.entry.size, 5000);
        assert_eq!(whole(&mut v, &file), bytes);
        let file = Opened {
            entry: v.write(&file, 7000, b"end").unwrap(),
            ..file
        };
        assert_eq!(file.entry.size, 7003);
        let back = whole(&mut v, &file);
        assert!(
            back[5000..7000].iter().all(|&b| b == 0),
            "zeros past the old end"
        );
        assert_eq!(&back[7000..], b"end");
        let file = Opened {
            entry: v.write(&file, 2, b"ab").unwrap(),
            ..file
        };
        assert_eq!(file.entry.size, 7003, "a write inside grows nothing");
        assert_eq!(&whole(&mut v, &file)[..5], &[0, 1, b'a', b'b', 4]);
        let mut fresh = volume(&image, true);
        let read = fresh.open(&["Dir", "big.bin"], &system(), READ, 0).unwrap();
        assert_eq!(
            whole(&mut fresh, &read),
            whole(&mut v, &file),
            "on the disk"
        );
    }

    #[test]
    fn only_a_file_opened_to_write_takes_writes() {
        let image = image();
        let mut v = volume(&image, false);
        let reader = v.open(&["a.txt"], &system(), READ, 0).unwrap();
        assert_eq!(
            v.write(&reader, 0, b"x"),
            Err(Error::Refused(Refusal::Denied(file::WRITE)))
        );
        let directory = v.open(&["Dir"], &system(), READ | WRITE, 0).unwrap();
        assert_eq!(v.write(&directory, 0, b"x"), Err(Error::NotAFile));
        assert_eq!(v.read(&directory, 0, &mut [0; 4]), Err(Error::NotAFile));
        let root = v.open(&[], &system(), READ | WRITE, 0).unwrap();
        assert_eq!(v.write(&root, 0, b"x"), Err(Error::NotAFile));
        let mut bytes = [0u8; 10];
        assert_eq!(v.read(&reader, 0, &mut bytes), Ok(10));
        assert_eq!(&bytes, b"0123456789", "untouched");
    }

    #[test]
    fn a_volume_mounted_read_only_takes_no_writes() {
        let image = image();
        let before = image.borrow().clone();
        let mut v = volume(&image, true);
        let read_only = Err(Error::Refused(Refusal::ReadOnly));
        let opened = |r: Result<Opened, Error>| r.map(|_| ());
        assert_eq!(
            opened(v.open(&["new.txt"], &system(), READ, CREATE_NEW)),
            read_only
        );
        assert_eq!(
            opened(v.open(&["a.txt"], &system(), READ, CREATE_ALWAYS)),
            read_only
        );
        assert_eq!(
            opened(v.open(&["a.txt"], &system(), READ | WRITE, 0)),
            read_only
        );
        assert_eq!(v.make_directory(&["New"], &system()), read_only);
        assert_eq!(v.remove(&["a.txt"], &system(), |_| false), read_only);
        assert_eq!(
            v.rename(&["a.txt"], &["b.txt"], &system(), |_| false),
            read_only
        );
        assert!(v.open(&["a.txt"], &system(), READ, 0).is_ok());
        assert!(*image.borrow() == before, "not a byte changed");
    }

    #[test]
    fn a_token_below_the_medium_level_writes_nothing() {
        let image = image();
        let mut v = volume(&image, false);
        let low = system().restricted(Restriction {
            integrity: Integrity::Low,
            keep: Privileges::NONE,
        });
        let denied = |r: Result<(), Error>| matches!(r, Err(Error::Refused(Refusal::Denied(_))));
        assert!(denied(
            v.open(&["new.txt"], &low, READ, CREATE_NEW).map(|_| ())
        ));
        assert!(denied(
            v.open(&["a.txt"], &low, READ, TRUNCATE_EXISTING)
                .map(|_| ())
        ));
        assert!(denied(v.make_directory(&["New"], &low)));
        assert!(denied(v.remove(&["a.txt"], &low, |_| false)));
        assert!(denied(v.rename(&["a.txt"], &["b.txt"], &low, |_| false)));
        assert!(v.open(&["a.txt"], &low, READ, 0).is_ok(), "it still reads");
        assert_eq!(
            v.open(&["new.txt"], &system(), READ, 0),
            Err(Error::NotFound)
        );
    }

    #[test]
    fn directories_are_made_listed_and_removed_once_empty() {
        let image = image();
        let mut v = volume(&image, false);
        assert_eq!(v.make_directory(&["Dir", "Inner"], &system()), Ok(()));
        assert_eq!(
            v.make_directory(&["Dir", "inner"], &system()),
            Err(Error::Exists)
        );
        assert_eq!(
            v.make_directory(&["No", "Inner"], &system()),
            Err(Error::NotFound)
        );
        assert_eq!(
            v.make_directory(&["a.txt", "Inner"], &system()),
            Err(Error::NotFound)
        );
        assert_eq!(v.make_directory(&[], &system()), Err(Error::BadPath));
        let dir = v.open(&["Dir"], &system(), READ, 0).unwrap();
        let mut room = [0u8; 16];
        assert_eq!(v.list(&dir, 0, &mut room), Ok((5, true)));
        assert_eq!(&room[..5], b"Inner");
        assert_eq!(v.list(&dir, 1, &mut room), Err(Error::NotFound));
        v.open(&["Dir", "b.txt"], &system(), READ, CREATE_NEW)
            .unwrap();
        assert_eq!(v.list(&dir, 1, &mut room), Ok((5, false)));
        assert_eq!(&room[..5], b"b.txt");
        assert_eq!(v.list(&dir, 0, &mut room), Ok((5, true)), "not the last");
        assert_eq!(&room[..5], b"Inner");
        assert_eq!(v.list(&dir, 2, &mut room), Err(Error::NotFound));
        v.remove(&["Dir", "b.txt"], &system(), |_| false).unwrap();
        assert_eq!(v.list(&dir, 0, &mut room[..4]), Err(Error::TooSmall));
        let a = v.open(&["a.txt"], &system(), READ, 0).unwrap();
        assert_eq!(v.list(&a, 0, &mut room), Err(Error::NotADirectory));
        assert_eq!(
            v.remove(&["Dir"], &system(), |_| false),
            Err(Error::NotEmpty)
        );
        assert_eq!(v.remove(&["Dir", "Inner"], &system(), |_| false), Ok(()));
        assert_eq!(
            v.list(&dir, 0, &mut room),
            Err(Error::NotFound),
            "empty now"
        );
        assert_eq!(v.remove(&["Dir"], &system(), |_| false), Ok(()));
        assert_eq!(v.open(&["Dir"], &system(), READ, 0), Err(Error::NotFound));
    }

    #[test]
    fn an_open_file_stays_and_others_go() {
        let image = image();
        let mut v = volume(&image, false);
        let a = v.open(&["a.txt"], &system(), READ, 0).unwrap();
        let place = a.place.unwrap();
        assert_eq!(
            v.remove(&["a.txt"], &system(), |p| p == place),
            Err(Error::InUse)
        );
        assert!(v.open(&["a.txt"], &system(), READ, 0).is_ok());
        assert_eq!(v.remove(&["a.txt"], &system(), |p| p != place), Ok(()));
        assert_eq!(v.open(&["a.txt"], &system(), READ, 0), Err(Error::NotFound));
        assert_eq!(
            v.remove(&["a.txt"], &system(), |_| false),
            Err(Error::NotFound)
        );
        assert_eq!(
            v.remove(&[], &system(), |_| false),
            Err(Error::BadPath),
            "the root stays"
        );
    }

    #[test]
    fn a_file_moves_and_takes_its_bytes_along() {
        let image = image();
        let mut v = volume(&image, false);
        assert_eq!(
            v.rename(&["a.txt"], &["Dir", "moved.txt"], &system(), |_| false),
            Ok(())
        );
        assert_eq!(v.open(&["a.txt"], &system(), READ, 0), Err(Error::NotFound));
        let moved = v.open(&["dir", "MOVED.TXT"], &system(), READ, 0).unwrap();
        assert_eq!(whole(&mut v, &moved), b"0123456789");
        assert_eq!(
            v.rename(&["Dir"], &["Folder"], &system(), |_| false),
            Ok(()),
            "a directory too"
        );
        let again = v
            .open(&["Folder", "moved.txt"], &system(), READ, 0)
            .unwrap();
        assert_eq!(whole(&mut v, &again), b"0123456789");
        assert_eq!(
            v.rename(&["Folder"], &["Folder", "Inner"], &system(), |_| false),
            Err(Error::Inside)
        );
    }

    #[test]
    fn a_rename_refuses_what_it_cannot_do() {
        let image = image();
        let mut v = volume(&image, false);
        v.make_directory(&["Other"], &system()).unwrap();
        let none = |_| false;
        assert_eq!(
            v.rename(&["a.txt"], &["Other"], &system(), none),
            Err(Error::Exists)
        );
        assert_eq!(
            v.rename(&["nope"], &["b"], &system(), none),
            Err(Error::NotFound)
        );
        assert_eq!(
            v.rename(&["a.txt"], &["No", "b"], &system(), none),
            Err(Error::NotFound),
            "into a directory that is not there"
        );
        assert_eq!(
            v.rename(&["a.txt"], &["a.txt", "b"], &system(), none),
            Err(Error::NotFound),
            "nor into a file"
        );
        assert_eq!(v.rename(&[], &["b"], &system(), none), Err(Error::BadPath));
        assert_eq!(
            v.rename(&["a.txt"], &[], &system(), none),
            Err(Error::BadPath),
            "the root is no name"
        );
        let a = v.open(&["a.txt"], &system(), READ, 0).unwrap();
        let place = a.place.unwrap();
        assert_eq!(
            v.rename(&["a.txt"], &["b.txt"], &system(), |p| p == place),
            Err(Error::InUse)
        );
        assert_eq!(
            v.rename(&["a.txt"], &["b.txt"], &system(), |p| p != place),
            Ok(())
        );
    }

    #[test]
    fn a_disk_that_fails_says_so() {
        let image = image();
        let works = Rc::new(Cell::new(false));
        let mut v = Volume::new(
            Fat::open(reads(&image)).unwrap(),
            writes(&image, &works),
            false,
        );
        assert_eq!(
            v.open(&["new.txt"], &system(), READ, CREATE_NEW),
            Err(Error::Unwritable)
        );
        assert_eq!(
            v.make_directory(&["New"], &system()),
            Err(Error::Unwritable)
        );
        let a = v.open(&["a.txt"], &system(), READ | WRITE, 0).unwrap();
        assert_eq!(v.write(&a, 0, b"x"), Err(Error::Unwritable));
        let broken = Rc::new(RefCell::new(image.borrow().clone()));
        let mut unreadable = volume(&broken, false);
        let root = unreadable
            .fs
            .fat
            .volume
            .first_sector(unreadable.fs.fat.volume.root)
            .unwrap();
        let at = usize::try_from(root).unwrap() * 512;
        broken.borrow_mut().truncate(at);
        assert_eq!(
            unreadable
                .open(&["a.txt"], &system(), READ, CREATE_ALWAYS)
                .map(|_| ()),
            Err(Error::Read),
            "a directory that cannot be read is not one without the file"
        );
        assert_eq!(
            unreadable.open(&["a.txt"], &system(), READ, 0),
            Err(Error::Read),
            "nor is the file missing"
        );
    }

    #[test]
    fn each_trouble_of_fat32_has_its_error() {
        let pairs = [
            (fat::Error::NotFat, Error::Read),
            (fat::Error::NotFat32, Error::Read),
            (fat::Error::Read, Error::Read),
            (fat::Error::BadChain, Error::Read),
            (fat::Error::NotFound, Error::NotFound),
            (fat::Error::NotADirectory, Error::NotFound),
            (fat::Error::Write, Error::Unwritable),
            (fat::Error::Full, Error::DiskFull),
            (fat::Error::TooLarge, Error::DiskFull),
            (fat::Error::Exists, Error::Exists),
            (fat::Error::NotEmpty, Error::NotEmpty),
            (fat::Error::BadName, Error::BadPath),
            (fat::Error::Inside, Error::Inside),
        ];
        for (from, to) in pairs {
            assert_eq!(Error::from(from), to, "{from:?}");
        }
    }
}
