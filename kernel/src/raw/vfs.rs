//! The tree of files: the volumes hanging in it, read and written through
//! their disks, and the files programs opened. Every volume is FAT32. One
//! is written only when it was mounted with a way to write it, which the
//! kernel gives only where QUESTIONS.md 9.6 and decision record 0015 let
//! it write.

use alloc::boxed::Box;
use alloc::vec::Vec;

use kernel::vfs::{self, Mounts, Path};
pub use kernel::volume::Error;
use kernel::volume::{Opened, Volume};
use security::Token;

use super::memory::Spin;

/// What reads a volume's sectors, from its first on, into the room it is
/// given, saying whether it could.
pub type Reader = Box<dyn FnMut(u64, &mut [u8]) -> bool + Send>;

/// What writes a volume's sectors, from the one it is given on, saying
/// whether it could.
pub type Writes = Box<dyn FnMut(u64, &[u8]) -> bool + Send>;

/// The most volumes that hang in the tree, and files open at once.
const VOLUMES: usize = 5;
const FILES: usize = 64;

static MOUNTED: Spin<[Option<Volume<Reader, Writes>>; VOLUMES]> =
    Spin::new([const { None }; VOLUMES]);
static MOUNTS: Spin<Mounts<VOLUMES>> = Spin::new(Mounts::new());

/// An open file: the volume it is on, and what was opened there.
#[derive(Clone, Copy)]
struct Open {
    volume: usize,
    opened: Opened,
}

static OPEN: Spin<[Option<Open>; FILES]> = Spin::new([None; FILES]);

/// Why a volume could not be mounted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountError {
    /// It holds no FAT32 volume, or could not be read.
    NotFat,
    /// Every volume there can be hangs somewhere already.
    Full,
    /// The place is none, or taken.
    Place(vfs::MountError),
}

/// Hangs the FAT32 volume `read` reads at the path `at`, written through
/// `write`, or read only without it.
///
/// # Errors
///
/// When it holds no FAT32 volume, every volume there can be hangs in the
/// tree already, or the place is none or taken.
pub fn mount(at: &str, read: Reader, write: Option<Writes>) -> Result<(), MountError> {
    let fat = fat::Fat::open(read).map_err(|_| MountError::NotFat)?;
    let read_only = write.is_none();
    let write = write.unwrap_or_else(|| Box::new(|_, _: &[u8]| false));
    let mut mounted = MOUNTED.lock();
    let free = mounted
        .iter()
        .position(Option::is_none)
        .ok_or(MountError::Full)?;
    MOUNTS.lock().mount(at, free).map_err(MountError::Place)?;
    mounted[free] = Some(Volume::new(fat, write, read_only));
    Ok(())
}

/// Does `f` with the volume the file at `path` is on, the names it has
/// there, and that volume's number.
fn on_path<T>(
    path: &str,
    f: impl FnOnce(&mut Volume<Reader, Writes>, &[&str], usize) -> Result<T, Error>,
) -> Result<T, Error> {
    let path = Path::parse(path).ok_or(Error::BadPath)?;
    let (volume, names) = MOUNTS.lock().find(&path).ok_or(Error::NotFound)?;
    let mut mounted = MOUNTED.lock();
    let v = mounted[volume].as_mut().ok_or(Error::NotFound)?;
    f(v, names, volume)
}

/// What is open in `slot`.
fn open_in(slot: usize) -> Result<Open, Error> {
    OPEN.lock()
        .get(slot)
        .copied()
        .flatten()
        .ok_or(Error::NotFound)
}

/// Opens the file at `path` for the rights `wanted`, as `token` may, doing
/// what `how` says by whether it is there, as OPEN takes it, and returns
/// its slot.
///
/// # Errors
///
/// When the path is none, the volume says no, or every slot is taken.
pub fn open(path: &str, token: &Token, wanted: u32, how: u64) -> Result<usize, Error> {
    let (volume, opened) = on_path(path, |v, names, volume| {
        v.open(names, token, wanted, how).map(|o| (volume, o))
    })?;
    let mut open = OPEN.lock();
    let slot = open.iter().position(Option::is_none).ok_or(Error::NoRoom)?;
    open[slot] = Some(Open { volume, opened });
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
    let file = open_in(slot)?;
    let mut mounted = MOUNTED.lock();
    let v = mounted[file.volume].as_mut().ok_or(Error::NotFound)?;
    v.read(&file.opened, offset, into)
}

/// Writes `from` into the open file in `slot` from its byte `offset` on,
/// which may lie past its end, and returns how many bytes it wrote: all
/// of them. Every slot open on the file sees its new size.
///
/// # Errors
///
/// When no file is open there, or the volume says no.
pub fn write(slot: usize, offset: u64, from: &[u8]) -> Result<usize, Error> {
    let mut mounted = MOUNTED.lock();
    let file = open_in(slot)?;
    let v = mounted[file.volume].as_mut().ok_or(Error::NotFound)?;
    let entry = v.write(&file.opened, offset, from)?;
    for open in OPEN.lock().iter_mut().flatten() {
        if open.volume == file.volume && open.opened.place == file.opened.place {
            open.opened.entry = entry;
        }
    }
    Ok(from.len())
}

/// Removes the file or the empty directory at `path`, as `token` may,
/// unless it is open.
///
/// # Errors
///
/// When the path is none, or the volume says no.
pub fn remove(path: &str, token: &Token) -> Result<(), Error> {
    on_path(path, |v, names, volume| {
        // Nothing opens on the volume while it is held, so what is open now
        // stays all that is.
        let open: Vec<_> = OPEN
            .lock()
            .iter()
            .flatten()
            .filter(|o| o.volume == volume)
            .filter_map(|o| o.opened.place)
            .collect();
        v.remove(names, token, |place| open.contains(&place))
    })
}

/// Makes a directory at `path`, as `token` may.
///
/// # Errors
///
/// When the path is none, or the volume says no.
pub fn make_directory(path: &str, token: &Token) -> Result<(), Error> {
    on_path(path, |v, names, _| v.make_directory(names, token))
}

/// Writes the name of entry `index` of the open directory in `slot`,
/// counted from 0, into `into` in UTF-8, and returns its length and
/// whether that entry is a directory itself.
///
/// # Errors
///
/// When nothing is open there, or the volume says no.
pub fn list(slot: usize, index: u64, into: &mut [u8]) -> Result<(usize, bool), Error> {
    let file = open_in(slot)?;
    let mut mounted = MOUNTED.lock();
    let v = mounted[file.volume].as_mut().ok_or(Error::NotFound)?;
    v.list(&file.opened, index, into)
}

/// How big the open file in `slot` is, and whether it is a directory.
pub fn info(slot: usize) -> Option<(u64, bool)> {
    let entry = open_in(slot).ok()?.opened.entry;
    Some((u64::from(entry.size), entry.directory))
}

/// Closes the open file in `slot`.
pub fn close(slot: usize) {
    if let Some(file) = OPEN.lock().get_mut(slot) {
        *file = None;
    }
}
