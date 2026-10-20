//! The tree of files: the volumes hanging in it, read through their disks,
//! and the files programs opened. Every volume is FAT32 and mounted read
//! only for now, as nothing may write to a partition Meridian did not
//! create (QUESTIONS.md 9.6).

use alloc::boxed::Box;

use kernel::vfs::{self, Mounts, Path, Refusal};
use security::Token;

use super::memory::Spin;

/// What reads a volume's sectors, from its first on, into the room it is
/// given, saying whether it could.
pub type Reader = Box<dyn FnMut(u64, &mut [u8]) -> bool + Send>;

/// The most volumes that hang in the tree, and files open at once.
const VOLUMES: usize = 4;
const FILES: usize = 64;

static MOUNTED: Spin<[Option<fat::Fat<Reader>>; VOLUMES]> = Spin::new([const { None }; VOLUMES]);
static MOUNTS: Spin<Mounts<VOLUMES>> = Spin::new(Mounts::new());

/// An open file: the volume it is on and what its directory entry says.
#[derive(Clone, Copy)]
struct Open {
    volume: usize,
    entry: fat::Entry,
}

static OPEN: Spin<[Option<Open>; FILES]> = Spin::new([None; FILES]);

/// Why a file could not be opened or read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The path is none.
    BadPath,
    /// No file has the path.
    NotFound,
    /// It is a directory, which has no bytes to read.
    NotAFile,
    /// The token may not open it so.
    Refused(Refusal),
    /// Every file that can be open is.
    NoRoom,
    /// Its volume could not be read.
    Read,
}

/// Hangs the FAT32 volume `read` reads at the path `at`, read only. Says
/// whether it does.
pub fn mount(at: &str, read: Reader) -> bool {
    let Ok(volume) = fat::Fat::open(read) else {
        return false;
    };
    let mut mounted = MOUNTED.lock();
    let Some(free) = mounted.iter().position(Option::is_none) else {
        return false;
    };
    if MOUNTS.lock().mount(at, free).is_err() {
        return false;
    }
    mounted[free] = Some(volume);
    true
}

/// Opens the file at `path` for the rights `wanted`, as `token` may, and
/// returns its slot.
///
/// # Errors
///
/// When the path is none, no file has it, the token may not open it so,
/// every slot is taken, or its volume cannot be read.
pub fn open(path: &str, token: &Token, wanted: u32) -> Result<usize, Error> {
    let path = Path::parse(path).ok_or(Error::BadPath)?;
    let (volume, names) = MOUNTS.lock().find(&path).ok_or(Error::NotFound)?;
    vfs::may_open(token, &vfs::plain(), wanted, true).map_err(Error::Refused)?;
    let entry = {
        let mut mounted = MOUNTED.lock();
        let fat = mounted[volume].as_mut().ok_or(Error::NotFound)?;
        let mut entry = fat::Entry {
            first: fat.volume.root,
            size: 0,
            directory: true,
        };
        for name in names {
            if !entry.directory {
                return Err(Error::NotFound);
            }
            entry = fat.lookup(entry.first, name).map_err(|e| match e {
                fat::Error::NotFound => Error::NotFound,
                _ => Error::Read,
            })?;
        }
        entry
    };
    let mut open = OPEN.lock();
    let slot = open.iter().position(Option::is_none).ok_or(Error::NoRoom)?;
    open[slot] = Some(Open { volume, entry });
    Ok(slot)
}

/// Reads the open file in `slot` from its byte `offset` on into `into`,
/// and returns how many bytes it read.
///
/// # Errors
///
/// When no file is open there, it is a directory, or its volume cannot be
/// read.
pub fn read(slot: usize, offset: u64, into: &mut [u8]) -> Result<usize, Error> {
    let file = OPEN
        .lock()
        .get(slot)
        .copied()
        .flatten()
        .ok_or(Error::NotFound)?;
    if file.entry.directory {
        return Err(Error::NotAFile);
    }
    let mut mounted = MOUNTED.lock();
    let fat = mounted[file.volume].as_mut().ok_or(Error::NotFound)?;
    fat.read_at(&file.entry, offset, into)
        .map_err(|_| Error::Read)
}

/// How big the open file in `slot` is, and whether it is a directory.
pub fn info(slot: usize) -> Option<(u64, bool)> {
    let file = OPEN.lock().get(slot).copied().flatten()?;
    Some((u64::from(file.entry.size), file.entry.directory))
}

/// Closes the open file in `slot`.
pub fn close(slot: usize) {
    if let Some(file) = OPEN.lock().get_mut(slot) {
        *file = None;
    }
}
