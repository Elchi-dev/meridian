//! The tree every file is in, the parts that need no processor: the paths
//! programs give, where each volume hangs in the tree, names compared as
//! Windows compares them, and who may open what on volumes that keep no
//! security of their own (QUESTIONS.md 9.1, 9.4 and 9.6).

use abi::file;
use security::{Descriptor, Entry, Integrity, Token, access, known, rights as standard};

/// The most names a path may have.
pub const DEPTH: usize = 32;

/// The longest path a mount may have, in bytes.
pub const MOUNT_BYTES: usize = 128;

/// A path from the root of the tree, as its names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Path<'a> {
    names: [&'a str; DEPTH],
    len: usize,
}

impl<'a> Path<'a> {
    /// The path `text` names. It starts at the root with `/`, and its
    /// names follow, each after a `/`: an empty name or `.` stays where it
    /// is, and `..` goes up one. None for a path that does not start at
    /// the root, goes above it, is longer than [`abi::PATH_BYTES`], has
    /// more than [`DEPTH`] names, or a name with a NUL in it.
    #[must_use]
    pub fn parse(text: &'a str) -> Option<Path<'a>> {
        if text.len() > abi::PATH_BYTES || text.contains('\0') {
            return None;
        }
        let mut path = Path {
            names: [""; DEPTH],
            len: 0,
        };
        for name in text.strip_prefix('/')?.split('/') {
            match name {
                "" | "." => {}
                ".." => path.len = path.len.checked_sub(1)?,
                _ => {
                    *path.names.get_mut(path.len)? = name;
                    path.len += 1;
                }
            }
        }
        Some(path)
    }

    #[must_use]
    pub fn names(&self) -> &[&'a str] {
        &self.names[..self.len]
    }
}

/// Whether `a` and `b` name one file: compared character by character in
/// capitals, as Windows compares names, where a character whose capital is
/// more than one stays as it is.
#[must_use]
pub fn same_name(a: &str, b: &str) -> bool {
    a.chars().map(capital).eq(b.chars().map(capital))
}

fn capital(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

/// Writes the name `units`, in UTF-16 as FAT32 keeps names, into `into` in
/// UTF-8, a unit that is no character as U+FFFD, and returns how many bytes
/// it took; none when they do not fit.
#[must_use]
pub fn name_into(units: &[u16], into: &mut [u8]) -> Option<usize> {
    let mut len = 0;
    for c in char::decode_utf16(units.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        let end = len + c.len_utf8();
        c.encode_utf8(into.get_mut(len..end)?);
        len = end;
    }
    Some(len)
}

/// Why a volume could not be mounted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountError {
    /// The place is no path, or longer than [`MOUNT_BYTES`].
    NotAPath,
    /// A volume hangs there already.
    Taken,
    /// Every mount there can be is taken.
    Full,
}

#[derive(Clone, Copy)]
struct Mount {
    path: [u8; MOUNT_BYTES],
    len: usize,
    volume: usize,
}

impl Mount {
    fn path(&self) -> Path<'_> {
        let text = core::str::from_utf8(&self.path[..self.len]).unwrap_or("");
        Path::parse(text).unwrap_or(Path {
            names: [""; DEPTH],
            len: 0,
        })
    }
}

/// Where up to `N` volumes hang in the tree.
pub struct Mounts<const N: usize> {
    mounts: [Option<Mount>; N],
}

impl<const N: usize> Default for Mounts<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Mounts<N> {
    #[must_use]
    pub const fn new() -> Self {
        Mounts { mounts: [None; N] }
    }

    /// Hangs the volume numbered `volume` at the path `at`.
    ///
    /// # Errors
    ///
    /// When `at` is no path or too long, a volume hangs there already, or
    /// every mount is taken.
    pub fn mount(&mut self, at: &str, volume: usize) -> Result<(), MountError> {
        let path = Path::parse(at)
            .filter(|_| at.len() <= MOUNT_BYTES)
            .ok_or(MountError::NotAPath)?;
        let taken = self.mounts.iter().flatten().any(|m| {
            let other = m.path();
            other.len == path.len && starts(&path, &other)
        });
        if taken {
            return Err(MountError::Taken);
        }
        let room = self
            .mounts
            .iter_mut()
            .find(|m| m.is_none())
            .ok_or(MountError::Full)?;
        let mut mount = Mount {
            path: [0; MOUNT_BYTES],
            len: at.len(),
            volume,
        };
        mount.path[..at.len()].copy_from_slice(at.as_bytes());
        *room = Some(mount);
        Ok(())
    }

    /// The volume the file at `path` is on, and the names it has there: of
    /// the volumes whose place starts the path, the one deepest in the
    /// tree.
    #[must_use]
    pub fn find<'s, 'p>(&self, path: &'s Path<'p>) -> Option<(usize, &'s [&'p str])> {
        let (depth, volume) = self
            .mounts
            .iter()
            .flatten()
            .filter_map(|m| {
                let at = m.path();
                starts(path, &at).then_some((at.len, m.volume))
            })
            .max_by_key(|&(depth, _)| depth)?;
        Some((volume, &path.names()[depth..]))
    }
}

/// Whether the names of `prefix` start those of `path`.
fn starts(path: &Path<'_>, prefix: &Path<'_>) -> bool {
    prefix.len <= path.len
        && path
            .names()
            .iter()
            .zip(prefix.names())
            .all(|(a, b)| same_name(a, b))
}

/// The rights that change a file.
pub const WRITES: u32 =
    file::WRITE | file::APPEND | standard::DELETE | standard::WRITE_DAC | standard::WRITE_OWNER;

/// Who may do what with each file of a volume that keeps no security of
/// its own, as FAT32 keeps none: everyone may read and run it, and the
/// administrators may do anything.
const PLAIN: [Entry; 2] = [
    Entry::Allow(
        known::EVERYONE,
        file::READ | file::RUN | standard::READ_CONTROL | standard::SYNCHRONIZE,
    ),
    Entry::Allow(known::ADMINISTRATORS, u32::MAX),
];

/// What a volume that keeps no security of its own says about each of its
/// files: SYSTEM owns it, [`PLAIN`] says who may do what, and it is
/// labelled at the medium level, so a process below that cannot write to
/// it.
#[must_use]
pub const fn plain() -> Descriptor<'static> {
    Descriptor {
        owner: known::SYSTEM,
        list: Some(&PLAIN),
        label: Integrity::Medium,
    }
}

/// Why a file could not be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Its volume is mounted read only, and `wanted` writes.
    ReadOnly,
    /// The token may not have these rights.
    Denied(u32),
}

/// Whether `token` may open a file that `descriptor` describes, on a
/// volume mounted `read_only` or not, for the rights `wanted`.
///
/// # Errors
///
/// When the volume is read only and a right of `wanted` writes, or the
/// token may not have them.
pub fn may_open(
    token: &Token,
    descriptor: &Descriptor<'_>,
    wanted: u32,
    read_only: bool,
) -> Result<(), Refusal> {
    if read_only && wanted & WRITES != 0 {
        return Err(Refusal::ReadOnly);
    }
    access(token, descriptor, wanted, WRITES).map_err(Refusal::Denied)
}

#[cfg(test)]
mod tests {
    use security::{Group, Privileges, Restriction};

    use super::*;

    fn names(text: &str) -> Option<Vec<&str>> {
        Path::parse(text).map(|p| p.names().to_vec())
    }

    #[test]
    fn a_path_is_its_names_from_the_root() {
        assert_eq!(
            names("/System/Boot/meridian/boot.meri").unwrap(),
            ["System", "Boot", "meridian", "boot.meri"]
        );
        assert_eq!(names("/").unwrap(), Vec::<&str>::new());
        assert_eq!(
            names("//Users///me/").unwrap(),
            ["Users", "me"],
            "empty names"
        );
        assert_eq!(names("/Users/./me/.").unwrap(), ["Users", "me"]);
        assert_eq!(names("/Users/me/../you").unwrap(), ["Users", "you"]);
        assert_eq!(names("/Users/..").unwrap(), Vec::<&str>::new());
        assert_eq!(
            names("/a b/c.d.e/...").unwrap(),
            ["a b", "c.d.e", "..."],
            "three dots is a name"
        );
    }

    #[test]
    fn only_a_path_from_the_root_within_bounds_parses() {
        assert_eq!(names(""), None);
        assert_eq!(names("Users/me"), None, "not from the root");
        assert_eq!(names("/.."), None, "above the root");
        assert_eq!(names("/Users/../.."), None);
        assert_eq!(names("/Users/me\0"), None, "a NUL");
        let deep = "/a".repeat(DEPTH);
        assert_eq!(names(&deep).map(|n| n.len()), Some(DEPTH));
        assert_eq!(names(&format!("{deep}/a")), None, "one name too many");
        assert_eq!(
            names(&format!("{deep}/..")).map(|n| n.len()),
            Some(DEPTH - 1)
        );
        let long = format!("/{}", "x".repeat(abi::PATH_BYTES - 1));
        assert!(names(&long).is_some());
        assert_eq!(names(&format!("{long}x")), None, "past PATH_BYTES");
    }

    #[test]
    fn names_are_one_whatever_their_case() {
        assert!(same_name("boot.meri", "BOOT.MERI"));
        assert!(same_name("Meridian", "mERIDIAN"));
        assert!(same_name("\u{e4}rger", "\u{c4}RGER"), "beyond ASCII");
        assert!(!same_name("boot.meri", "boot.mer"));
        assert!(!same_name("boot.meri", "boot.meri "));
        assert!(
            same_name("stra\u{df}e", "STRA\u{df}E"),
            "a letter whose capital is two stays"
        );
        assert!(!same_name("stra\u{df}e", "STRASSE"));
        assert!(same_name("", ""));
    }

    #[test]
    fn a_name_is_handed_out_in_utf8() {
        let mut room = [0u8; 16];
        let units: Vec<u16> = "boot.meri".encode_utf16().collect();
        assert_eq!(name_into(&units, &mut room), Some(9));
        assert_eq!(&room[..9], b"boot.meri");
        let wide: Vec<u16> = "\u{e4}\u{20ac}\u{1f600}".encode_utf16().collect();
        assert_eq!(name_into(&wide, &mut room), Some(2 + 3 + 4));
        assert_eq!(&room[..9], "\u{e4}\u{20ac}\u{1f600}".as_bytes());
        assert_eq!(
            name_into(&[0xd800, 0x41], &mut room),
            Some(4),
            "a lone half of a pair"
        );
        assert_eq!(&room[..4], "\u{fffd}A".as_bytes());
        assert_eq!(name_into(&units, &mut room[..8]), None, "a byte short");
        assert_eq!(name_into(&units, &mut room[..9]), Some(9), "just enough");
        assert_eq!(
            name_into(&wide, &mut room[..8]),
            None,
            "no character cut in two"
        );
        assert_eq!(name_into(&[], &mut []), Some(0));
    }

    #[test]
    fn a_file_is_on_the_volume_deepest_in_the_tree() {
        let mut m = Mounts::<4>::new();
        assert_eq!(m.mount("/", 0), Ok(()));
        assert_eq!(m.mount("/System/Boot", 1), Ok(()));
        assert_eq!(m.mount("/Volumes/USB", 2), Ok(()));
        let path = Path::parse("/system/boot/meridian/boot.meri").unwrap();
        assert_eq!(m.find(&path), Some((1, &["meridian", "boot.meri"][..])));
        let other = Path::parse("/System/Bootstrap/x").unwrap();
        assert_eq!(
            m.find(&other),
            Some((0, &["System", "Bootstrap", "x"][..])),
            "a name is matched whole"
        );
        let usb = Path::parse("/Volumes/USB").unwrap();
        assert_eq!(m.find(&usb), Some((2, &[][..])), "the volume's own root");
        let above = Path::parse("/Volumes").unwrap();
        assert_eq!(m.find(&above), Some((0, &["Volumes"][..])));
        let none = Mounts::<1>::new();
        assert_eq!(none.find(&path), None);
    }

    #[test]
    fn a_place_takes_one_volume() {
        let mut m = Mounts::<2>::new();
        assert_eq!(m.mount("/System/Boot", 1), Ok(()));
        assert_eq!(
            m.mount("/system/BOOT/", 2),
            Err(MountError::Taken),
            "the same place in other words"
        );
        assert_eq!(m.mount("/System", 3), Ok(()), "above it is another place");
        assert_eq!(m.mount("/Volumes/X", 4), Err(MountError::Full));
        assert_eq!(m.mount("Volumes/X", 4), Err(MountError::NotAPath));
        let long = format!("/{}", "x".repeat(MOUNT_BYTES));
        assert_eq!(
            Mounts::<1>::new().mount(&long, 0),
            Err(MountError::NotAPath)
        );
        let longest = format!("/{}", "x".repeat(MOUNT_BYTES - 1));
        let mut room = Mounts::<1>::new();
        assert_eq!(room.mount(&longest, 7), Ok(()));
        let inside = format!("{longest}/file");
        assert_eq!(
            room.find(&Path::parse(&inside).unwrap()),
            Some((7, &["file"][..]))
        );
    }

    fn user(integrity: Integrity) -> Token {
        let groups = [Group::of(known::EVERYONE), Group::of(known::USERS)];
        let me = security::Sid::parse("S-1-5-21-1-2-3-1000").unwrap();
        Token::new(me, &groups, Privileges::NONE, integrity).unwrap()
    }

    #[test]
    fn everyone_reads_a_plain_volume_and_administrators_change_it() {
        let file = plain();
        let medium = user(Integrity::Medium);
        assert_eq!(
            may_open(&medium, &file, file::READ | file::RUN, true),
            Ok(())
        );
        assert_eq!(
            may_open(&user(Integrity::Untrusted), &file, file::READ, false),
            Ok(()),
            "reading up"
        );
        assert_eq!(
            may_open(
                &medium,
                &file,
                standard::READ_CONTROL | standard::SYNCHRONIZE,
                true
            ),
            Ok(()),
            "everyone may also read who may do what, and wait for it"
        );
        assert_eq!(
            may_open(&medium, &file, file::WRITE, false),
            Err(Refusal::Denied(file::WRITE))
        );
        assert_eq!(
            may_open(
                &Token::system(),
                &file,
                file::WRITE | standard::DELETE,
                false
            ),
            Ok(())
        );
        let low_admin = Token::system().restricted(Restriction {
            integrity: Integrity::Low,
            keep: Privileges::ALL,
        });
        assert_eq!(
            may_open(&low_admin, &file, file::WRITE, false),
            Err(Refusal::Denied(file::WRITE)),
            "no writing up, even for an administrator"
        );
    }

    #[test]
    fn nothing_writes_to_a_volume_mounted_read_only() {
        let file = plain();
        for wanted in [
            file::WRITE,
            file::APPEND,
            standard::DELETE,
            standard::WRITE_DAC,
            standard::WRITE_OWNER,
        ] {
            assert_eq!(
                may_open(&Token::system(), &file, wanted | file::READ, true),
                Err(Refusal::ReadOnly)
            );
        }
        assert_eq!(
            may_open(
                &Token::system(),
                &file,
                file::READ | standard::READ_CONTROL,
                true
            ),
            Ok(())
        );
    }
}
