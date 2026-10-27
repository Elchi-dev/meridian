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

/// An open file: what it is, and how many handles stand for it.
#[derive(Clone, Copy)]
struct Open {
    of: Of,
    holders: u32,
}

/// What is open: a file or directory on the volume numbered `volume`, or a
/// directory of the tree itself, above the volumes, at the path of the
/// first `len` bytes of `path`.
#[derive(Clone, Copy)]
enum Of {
    Volume {
        volume: usize,
        opened: Opened,
    },
    Tree {
        path: [u8; vfs::MOUNT_BYTES],
        len: usize,
    },
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
    let of = match tree(path) {
        Some(tree) => {
            kernel::volume::open_tree(token, wanted, how)?;
            tree
        }
        None => on_path(path, |v, names, volume| {
            v.open(names, token, wanted, how)
                .map(|opened| Of::Volume { volume, opened })
        })?,
    };
    let mut open = OPEN.lock();
    let slot = open.iter().position(Option::is_none).ok_or(Error::NoRoom)?;
    open[slot] = Some(Open { of, holders: 1 });
    Ok(slot)
}

/// The directory of the tree itself at `path`, when it is one: the root
/// or a directory above the places volumes hang, which no volume holds.
fn tree(path: &str) -> Option<Of> {
    let path = Path::parse(path)?;
    MOUNTS.lock().above(&path)?;
    let mut text = [0u8; vfs::MOUNT_BYTES];
    let mut len = 0;
    // Names the tree takes as the same may differ in length, so one may
    // not fit where the place of a volume did.
    for name in path.names() {
        let room = text.get_mut(len..len + 1 + name.len())?;
        room[0] = b'/';
        room[1..].copy_from_slice(name.as_bytes());
        len += room.len();
    }
    // The root alone is the one with no names.
    if len == 0 {
        text[0] = b'/';
        len = 1;
    }
    Some(Of::Tree { path: text, len })
}

/// Reads the open file in `slot` from its byte `offset` on into `into`,
/// and returns how many bytes it read.
///
/// # Errors
///
/// When no file is open there, it is a directory, or its volume cannot be
/// read.
pub fn read(slot: usize, offset: u64, into: &mut [u8]) -> Result<usize, Error> {
    let (volume, opened) = on_volume(slot)?;
    let mut mounted = MOUNTED.lock();
    let v = mounted[volume].as_mut().ok_or(Error::NotFound)?;
    v.read(&opened, offset, into)
}

/// The volume the file open in `slot` is on, and what was opened there.
///
/// # Errors
///
/// When nothing is open there, or a directory of the tree itself is,
/// which has no bytes.
fn on_volume(slot: usize) -> Result<(usize, Opened), Error> {
    match open_in(slot)?.of {
        Of::Volume { volume, opened } => Ok((volume, opened)),
        Of::Tree { .. } => Err(Error::NotAFile),
    }
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
    let (volume, written) = on_volume(slot)?;
    let v = mounted[volume].as_mut().ok_or(Error::NotFound)?;
    let entry = v.write(&written, offset, from)?;
    for open in OPEN.lock().iter_mut().flatten() {
        if let Of::Volume {
            volume: on,
            ref mut opened,
        } = open.of
            && on == volume
            && opened.place == written.place
        {
            opened.entry = entry;
        }
    }
    Ok(from.len())
}

/// Where each file open on the volume numbered `volume` lies on it.
fn open_on(volume: usize) -> Vec<fat::Place> {
    OPEN.lock()
        .iter()
        .flatten()
        .filter_map(|o| match o.of {
            Of::Volume { volume: on, opened } if on == volume => opened.place,
            _ => None,
        })
        .collect()
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
        let open = open_on(volume);
        v.remove(names, token, |place| open.contains(&place))
    })
}

/// Moves the file or directory at `from` to `to`, as `token` may, unless
/// it is open.
///
/// # Errors
///
/// When either path is none, the two are on different volumes, or the
/// volume says no.
pub fn rename(from: &str, to: &str, token: &Token) -> Result<(), Error> {
    // Both are paths before either is looked for.
    if Path::parse(from).is_none() {
        return Err(Error::BadPath);
    }
    let to = Path::parse(to).ok_or(Error::BadPath)?;
    let (target, to_names) = MOUNTS.lock().find(&to).ok_or(Error::NotFound)?;
    on_path(from, |v, names, volume| {
        if volume != target {
            return Err(Error::NotSameVolume);
        }
        let open = open_on(volume);
        v.rename(names, to_names, token, |place| open.contains(&place))
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
    let (volume, opened) = match open_in(slot)?.of {
        Of::Volume { volume, opened } => (volume, opened),
        Of::Tree { path, len } => return listed(&path[..len], index, into),
    };
    let mut mounted = MOUNTED.lock();
    let v = mounted[volume].as_mut().ok_or(Error::NotFound)?;
    v.list(&opened, index, into)
}

/// Writes name `index` of the directory of the tree itself at `path` into
/// `into`, as [`list`] does: the place of a volume or a directory above
/// some, either way a directory.
fn listed(path: &[u8], index: u64, into: &mut [u8]) -> Result<(usize, bool), Error> {
    let path = core::str::from_utf8(path).ok().and_then(Path::parse);
    let mounts = MOUNTS.lock();
    let names = path
        .and_then(|path| mounts.above(&path))
        .ok_or(Error::NotFound)?;
    let name = usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i))
        .ok_or(Error::NotFound)?;
    let room = into.get_mut(..name.len()).ok_or(Error::TooSmall)?;
    room.copy_from_slice(name.as_bytes());
    Ok((name.len(), true))
}

/// How big the open file in `slot` is, and whether it is a directory.
pub fn info(slot: usize) -> Option<(u64, bool)> {
    match open_in(slot).ok()?.of {
        Of::Volume { opened, .. } => Some((u64::from(opened.entry.size), opened.entry.directory)),
        Of::Tree { .. } => Some((0, true)),
    }
}

/// One more handle stands for the open file in `slot`. False when none
/// is open there.
pub fn hold(slot: usize) -> bool {
    let mut open = OPEN.lock();
    let Some(file) = open.get_mut(slot).and_then(Option::as_mut) else {
        return false;
    };
    file.holders = file.holders.saturating_add(1);
    true
}

/// Takes away a handle that stood for the open file in `slot`, and with
/// the last closes it.
pub fn close(slot: usize) {
    let mut open = OPEN.lock();
    let Some(entry) = open.get_mut(slot) else {
        return;
    };
    if let Some(file) = entry.as_mut() {
        file.holders = file.holders.saturating_sub(1);
        if file.holders == 0 {
            *entry = None;
        }
    }
}
