//! Security, the parts that need no processor, in the shape [MS-DTYP]
//! gives them (ADR 0014): security identifiers, the tokens processes
//! carry, and the access check that weighs a token against what an object
//! says about who may do what with it.
//!
//! [MS-DTYP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dtyp/

#![cfg_attr(not(test), no_std)]

use core::fmt;

/// A security identifier: an authority and up to 15 numbers below it,
/// such as `S-1-5-18` for SYSTEM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sid {
    authority: u64,
    numbers: [u32; Sid::MAX_NUMBERS],
    count: usize,
}

impl Sid {
    /// The most numbers a SID may have.
    pub const MAX_NUMBERS: usize = 15;
    /// The largest authority, which takes 6 bytes.
    pub const MAX_AUTHORITY: u64 = (1 << 48) - 1;

    /// The SID of `authority` and `numbers`, or none for an authority past
    /// 6 bytes or more than 15 numbers.
    #[must_use]
    pub const fn new(authority: u64, numbers: &[u32]) -> Option<Sid> {
        if authority > Sid::MAX_AUTHORITY || numbers.len() > Sid::MAX_NUMBERS {
            return None;
        }
        Some(Sid::known(authority, numbers))
    }

    /// As [`Sid::new`], for SIDs written into the code, which are all
    /// within its limits.
    const fn known(authority: u64, numbers: &[u32]) -> Sid {
        let mut all = [0; Sid::MAX_NUMBERS];
        let mut i = 0;
        while i < numbers.len() {
            all[i] = numbers[i];
            i += 1;
        }
        Sid {
            authority,
            numbers: all,
            count: numbers.len(),
        }
    }

    #[must_use]
    pub fn authority(&self) -> u64 {
        self.authority
    }

    #[must_use]
    pub fn numbers(&self) -> &[u32] {
        &self.numbers[..self.count]
    }

    /// How many bytes its binary form takes.
    #[must_use]
    pub fn size(&self) -> usize {
        8 + 4 * self.count
    }

    /// Writes the binary form to the start of `out`: revision 1, how many
    /// numbers follow, the authority in 6 bytes with the most significant
    /// first, then each number in 4 with the least significant first.
    /// Returns how many bytes that took, or none when `out` is too short.
    pub fn write(&self, out: &mut [u8]) -> Option<usize> {
        let size = self.size();
        let out = out.get_mut(..size)?;
        out[0] = 1;
        out[1] = u8::try_from(self.count).ok()?;
        out[2..8].copy_from_slice(&self.authority.to_be_bytes()[2..]);
        for (room, number) in out[8..]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.numbers())
        {
            *room = number.to_le_bytes();
        }
        Some(size)
    }

    /// The SID whose binary form starts `bytes`, and how many bytes it
    /// took; none when they hold no SID.
    #[must_use]
    pub fn read(bytes: &[u8]) -> Option<(Sid, usize)> {
        let (&revision, rest) = bytes.split_first()?;
        let count = usize::from(*rest.first()?);
        if revision != 1 || count > Sid::MAX_NUMBERS {
            return None;
        }
        let size = 8 + 4 * count;
        let bytes = bytes.get(..size)?;
        let mut authority = [0u8; 8];
        authority[2..].copy_from_slice(&bytes[2..8]);
        let mut numbers = [0; Sid::MAX_NUMBERS];
        for (number, word) in numbers.iter_mut().zip(bytes[8..].as_chunks::<4>().0) {
            *number = u32::from_le_bytes(*word);
        }
        let sid = Sid {
            authority: u64::from_be_bytes(authority),
            numbers,
            count,
        };
        Some((sid, size))
    }

    /// The SID a text such as `S-1-5-32-544` names: the authority in
    /// decimal, or after `0x` in 12 hexadecimal digits when it does not fit
    /// 32 bits, then each number in decimal.
    #[must_use]
    pub fn parse(text: &str) -> Option<Sid> {
        let mut parts = text.strip_prefix("S-1-")?.split('-');
        let authority = parts.next()?;
        let authority = match authority.strip_prefix("0x") {
            Some(hex) if hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
                u64::from_str_radix(hex, 16).ok()?
            }
            Some(_) => return None,
            None => u64::from(decimal(authority)?),
        };
        let mut numbers = [0; Sid::MAX_NUMBERS];
        let mut count = 0;
        for part in parts {
            *numbers.get_mut(count)? = decimal(part)?;
            count += 1;
        }
        Sid::new(authority, &numbers[..count])
    }
}

/// The number `text` writes in decimal digits and nothing else.
fn decimal(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for Sid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.authority >> 32 == 0 {
            write!(f, "S-1-{}", self.authority)?;
        } else {
            write!(f, "S-1-0x{:012X}", self.authority)?;
        }
        for number in self.numbers() {
            write!(f, "-{number}")?;
        }
        Ok(())
    }
}

/// SIDs every Windows machine knows by the same numbers.
pub mod known {
    use super::Sid;

    pub const EVERYONE: Sid = Sid::known(1, &[0]);
    pub const AUTHENTICATED_USERS: Sid = Sid::known(5, &[11]);
    pub const SYSTEM: Sid = Sid::known(5, &[18]);
    pub const ADMINISTRATORS: Sid = Sid::known(5, &[32, 544]);
    pub const USERS: Sid = Sid::known(5, &[32, 545]);
}

/// How far a process is trusted, lowest first. A process may not write to
/// what is labelled higher than itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Integrity {
    Untrusted,
    Low,
    Medium,
    High,
    System,
}

impl Integrity {
    const ALL: [Integrity; 5] = [
        Integrity::Untrusted,
        Integrity::Low,
        Integrity::Medium,
        Integrity::High,
        Integrity::System,
    ];

    /// The SID that labels it: `S-1-16-` and the number Windows gives it,
    /// from 0 for untrusted up in steps of 0x1000.
    #[must_use]
    pub fn sid(self) -> Sid {
        Sid::known(16, &[u32::from(self.index()) * 0x1000])
    }

    /// The level a label's SID names. Windows has a few levels between
    /// these, which count as the one below them.
    #[must_use]
    pub fn of(sid: &Sid) -> Option<Integrity> {
        match (sid.authority, sid.numbers()) {
            (16, &[number]) => Integrity::from_index(u64::from(number / 0x1000).min(4)),
            _ => None,
        }
    }

    fn index(self) -> u8 {
        match self {
            Integrity::Untrusted => 0,
            Integrity::Low => 1,
            Integrity::Medium => 2,
            Integrity::High => 3,
            Integrity::System => 4,
        }
    }

    fn from_index(index: u64) -> Option<Integrity> {
        Integrity::ALL.get(usize::try_from(index).ok()?).copied()
    }
}

/// What a token lets a process do beyond what objects grant it, one bit
/// each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Privileges(pub u32);

impl Privileges {
    pub const NONE: Privileges = Privileges(0);
    /// Reading and writing the memory of other users' programs.
    pub const DEBUG: Privileges = Privileges(1);
    /// Shutting the machine down or starting it again.
    pub const SHUTDOWN: Privileges = Privileges(1 << 1);
    /// Setting the clock.
    pub const SYSTEM_TIME: Privileges = Privileges(1 << 2);
    /// Loading drivers.
    pub const LOAD_DRIVER: Privileges = Privileges(1 << 3);
    /// Making oneself the owner of any object.
    pub const TAKE_OWNERSHIP: Privileges = Privileges(1 << 4);
    /// Reading any file, to back it up.
    pub const BACKUP: Privileges = Privileges(1 << 5);
    /// Writing any file, to restore it.
    pub const RESTORE: Privileges = Privileges(1 << 6);
    /// Starting programs as another user.
    pub const ASSIGN_TOKEN: Privileges = Privileges(1 << 7);
    pub const ALL: Privileges = Privileges(0xff);

    /// Each privilege with the name Windows gives it.
    const NAMES: [(Privileges, &'static str); 8] = [
        (Privileges::DEBUG, "SeDebugPrivilege"),
        (Privileges::SHUTDOWN, "SeShutdownPrivilege"),
        (Privileges::SYSTEM_TIME, "SeSystemtimePrivilege"),
        (Privileges::LOAD_DRIVER, "SeLoadDriverPrivilege"),
        (Privileges::TAKE_OWNERSHIP, "SeTakeOwnershipPrivilege"),
        (Privileges::BACKUP, "SeBackupPrivilege"),
        (Privileges::RESTORE, "SeRestorePrivilege"),
        (Privileges::ASSIGN_TOKEN, "SeAssignPrimaryTokenPrivilege"),
    ];

    /// Whether every privilege of `other` is among these.
    #[must_use]
    pub fn contains(self, other: Privileges) -> bool {
        self.0 & other.0 == other.0
    }

    /// The privileges both have.
    #[must_use]
    pub fn both(self, other: Privileges) -> Privileges {
        Privileges(self.0 & other.0)
    }

    /// The name Windows gives the one privilege this is, such as
    /// `SeDebugPrivilege`; none for no privilege or several.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        let (_, name) = Privileges::NAMES.iter().find(|(p, _)| *p == self)?;
        Some(name)
    }

    /// The privilege Windows names `name`.
    #[must_use]
    pub fn named(name: &str) -> Option<Privileges> {
        let (p, _) = Privileges::NAMES.iter().find(|(_, n)| *n == name)?;
        Some(*p)
    }
}

/// A group a token's user is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub sid: Sid,
    /// The group only counts where an object denies it something, as the
    /// administrators' group does while their rights are off.
    pub deny_only: bool,
}

impl Group {
    /// A group that counts both ways.
    #[must_use]
    pub const fn of(sid: Sid) -> Group {
        Group {
            sid,
            deny_only: false,
        }
    }
}

/// What a process carries: whose it is, the groups that count for it,
/// what it may do beyond what objects grant, and how far it is trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub user: Sid,
    groups: [Group; Token::MAX_GROUPS],
    count: usize,
    pub privileges: Privileges,
    pub integrity: Integrity,
}

impl Token {
    /// The most groups a token holds.
    pub const MAX_GROUPS: usize = 16;

    /// The token of `user` in `groups`, or none for more than
    /// [`Token::MAX_GROUPS`] of them.
    #[must_use]
    pub fn new(
        user: Sid,
        groups: &[Group],
        privileges: Privileges,
        integrity: Integrity,
    ) -> Option<Token> {
        let mut all = [Group::of(known::EVERYONE); Token::MAX_GROUPS];
        all.get_mut(..groups.len())?.copy_from_slice(groups);
        Some(Token {
            user,
            groups: all,
            count: groups.len(),
            privileges,
            integrity,
        })
    }

    /// The token of the programs the kernel starts itself: SYSTEM, among
    /// the administrators, everyone and the users who logged in, with every
    /// privilege and the system integrity level, as Windows' services have.
    #[must_use]
    pub fn system() -> Token {
        let groups = [
            Group::of(known::ADMINISTRATORS),
            Group::of(known::EVERYONE),
            Group::of(known::AUTHENTICATED_USERS),
        ];
        let mut token = Token {
            user: known::SYSTEM,
            groups: [Group::of(known::EVERYONE); Token::MAX_GROUPS],
            count: groups.len(),
            privileges: Privileges::ALL,
            integrity: Integrity::System,
        };
        token.groups[..groups.len()].copy_from_slice(&groups);
        token
    }

    #[must_use]
    pub fn groups(&self) -> &[Group] {
        &self.groups[..self.count]
    }

    /// Whether `sid` is the user's or that of one of their groups that
    /// counts for them, as `CheckTokenMembership` asks: groups that only
    /// count where something is denied are left out.
    #[must_use]
    pub fn is_member(&self, sid: &Sid) -> bool {
        self.user == *sid || self.groups().iter().any(|g| !g.deny_only && g.sid == *sid)
    }

    /// Whether an entry that denies `sid` something applies to this token:
    /// the user's and every group's, deny-only ones too.
    fn denied_as(&self, sid: &Sid) -> bool {
        self.user == *sid || self.groups().iter().any(|g| g.sid == *sid)
    }

    /// A copy that may do at most what `restriction` leaves: the lower of
    /// the two integrity levels and the privileges both have, so a
    /// restriction never adds to a token.
    #[must_use]
    pub fn restricted(&self, restriction: Restriction) -> Token {
        Token {
            privileges: self.privileges.both(restriction.keep),
            integrity: self.integrity.min(restriction.integrity),
            ..*self
        }
    }

    /// How many bytes [`Token::write`] takes for it.
    #[must_use]
    pub fn size(&self) -> usize {
        let groups: usize = self.groups().iter().map(|g| 1 + g.sid.size()).sum();
        8 + self.user.size() + groups
    }

    /// Writes the token to the start of `out`, as TOKEN hands it to a
    /// program: version 1, the integrity level from 0 for untrusted, how
    /// many groups, a byte of 0, the privileges in 4 bytes with the least
    /// significant first, then the user's SID, and for each group a byte
    /// that is 1 for deny-only and its SID. Returns how many bytes that
    /// took, or none when `out` is too short.
    pub fn write(&self, out: &mut [u8]) -> Option<usize> {
        let size = self.size();
        let out = out.get_mut(..size)?;
        out[..4].copy_from_slice(&[1, self.integrity.index(), u8::try_from(self.count).ok()?, 0]);
        out[4..8].copy_from_slice(&self.privileges.0.to_le_bytes());
        let mut at = 8 + self.user.write(&mut out[8..])?;
        for group in self.groups() {
            out[at] = u8::from(group.deny_only);
            at += 1 + group.sid.write(&mut out[at + 1..])?;
        }
        Some(at)
    }

    /// The token `bytes` hold, all of them, as [`Token::write`] wrote it;
    /// none for anything else.
    #[must_use]
    pub fn read(bytes: &[u8]) -> Option<Token> {
        let head = bytes.get(..8)?;
        if head[0] != 1 || head[3] != 0 {
            return None;
        }
        let integrity = Integrity::from_index(u64::from(head[1]))?;
        let count = usize::from(head[2]);
        let privileges = Privileges(u32::from_le_bytes([head[4], head[5], head[6], head[7]]));
        let (user, mut at) = Sid::read(&bytes[8..])?;
        at += 8;
        let mut groups = [Group::of(known::EVERYONE); Token::MAX_GROUPS];
        for group in groups.get_mut(..count)? {
            let deny_only = match bytes.get(at)? {
                0 => false,
                1 => true,
                _ => return None,
            };
            let (sid, size) = Sid::read(&bytes[at + 1..])?;
            *group = Group { sid, deny_only };
            at += 1 + size;
        }
        if at != bytes.len() {
            return None;
        }
        Some(Token {
            user,
            groups,
            count,
            privileges,
            integrity,
        })
    }
}

/// What SPAWN leaves of the token it hands a program: at most this
/// integrity level, and of the privileges only these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restriction {
    pub integrity: Integrity,
    pub keep: Privileges,
}

impl Restriction {
    /// The restriction in one register: bit 63 set, the integrity level
    /// from 0 for untrusted in bits 32 to 39, and the privileges kept in
    /// the low 32 bits. A word of 0 stands for no restriction at all.
    #[must_use]
    pub fn word(self) -> u64 {
        1 << 63 | u64::from(self.integrity.index()) << 32 | u64::from(self.keep.0)
    }

    /// The restriction `word` holds, or none for a word that holds none,
    /// 0 among them.
    #[must_use]
    pub fn from_word(word: u64) -> Option<Restriction> {
        if word >> 40 != 1 << 23 {
            return None;
        }
        Some(Restriction {
            integrity: Integrity::from_index(word >> 32 & 0xff)?,
            keep: Privileges(u32::try_from(word & 0xffff_ffff).ok()?),
        })
    }
}

/// The rights every kind of object has; bits 0 to 15 are each kind's own.
pub mod rights {
    pub const DELETE: u32 = 1 << 16;
    pub const READ_CONTROL: u32 = 1 << 17;
    pub const WRITE_DAC: u32 = 1 << 18;
    pub const WRITE_OWNER: u32 = 1 << 19;
    pub const SYNCHRONIZE: u32 = 1 << 20;
}

/// An entry of an object's list of who may do what: the rights it allows
/// or denies a user or a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Allow(Sid, u32),
    Deny(Sid, u32),
}

/// What an object says about who may do what with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor<'a> {
    pub owner: Sid,
    /// Who may do what, in the order the entries count; none for an object
    /// anyone may do anything with, while an empty list allows nothing.
    pub list: Option<&'a [Entry]>,
    /// How far it is trusted: a process below it may not write to it.
    pub label: Integrity,
}

/// Whether `token` may do all of `wanted` with the object `descriptor`
/// describes, where `writes` are the rights of its kind that change it;
/// when not, the rights it may not have.
///
/// A process below the object's integrity level gets none of the rights
/// that write. Then its owner may always read and change who may do what,
/// and the privilege to take ownership gives the right to. The list's
/// entries count in their order: one that allows grants what it allows,
/// and one that denies a right not granted yet refuses the whole request.
/// What no entry granted is refused.
///
/// # Errors
///
/// The rights of `wanted` the token may not have.
pub fn access(
    token: &Token,
    descriptor: &Descriptor<'_>,
    wanted: u32,
    writes: u32,
) -> Result<(), u32> {
    if token.integrity < descriptor.label && wanted & writes != 0 {
        return Err(wanted & writes);
    }
    let mut missing = wanted;
    if token.is_member(&descriptor.owner) {
        missing &= !(rights::READ_CONTROL | rights::WRITE_DAC);
    }
    if token.privileges.contains(Privileges::TAKE_OWNERSHIP) {
        missing &= !rights::WRITE_OWNER;
    }
    let Some(list) = descriptor.list else {
        return Ok(());
    };
    for entry in list {
        if missing == 0 {
            break;
        }
        match *entry {
            Entry::Deny(sid, denied) if denied & missing != 0 && token.denied_as(&sid) => {
                return Err(denied & missing);
            }
            Entry::Allow(sid, allowed) if token.is_member(&sid) => missing &= !allowed,
            _ => {}
        }
    }
    if missing == 0 { Ok(()) } else { Err(missing) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(text: &str) -> Sid {
        Sid::parse(text).unwrap()
    }

    fn bytes(sid: &Sid) -> Vec<u8> {
        let mut out = vec![0u8; 80];
        let size = sid.write(&mut out).unwrap();
        out.truncate(size);
        out
    }

    #[test]
    fn a_sid_is_written_as_windows_writes_it() {
        assert_eq!(bytes(&known::SYSTEM), [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0]);
        assert_eq!(
            bytes(&known::ADMINISTRATORS),
            [1, 2, 0, 0, 0, 0, 0, 5, 0x20, 0, 0, 0, 0x20, 2, 0, 0]
        );
        let big = Sid::new(0x0102_0304_0506, &[0xdead_beef]).unwrap();
        assert_eq!(
            bytes(&big),
            [1, 1, 1, 2, 3, 4, 5, 6, 0xef, 0xbe, 0xad, 0xde]
        );
        assert_eq!(known::SYSTEM.size(), 12);
        let mut short = [0u8; 11];
        assert_eq!(known::SYSTEM.write(&mut short), None);
        let mut exact = [0u8; 12];
        assert_eq!(known::SYSTEM.write(&mut exact), Some(12));
    }

    #[test]
    fn a_sid_is_read_back_from_its_bytes() {
        let domain = sid("S-1-5-21-1004336348-1177238915-682003330-512");
        let mut written = bytes(&domain);
        assert_eq!(Sid::read(&written), Some((domain, 28)));
        written.extend_from_slice(&[9, 9]);
        assert_eq!(
            Sid::read(&written),
            Some((domain, 28)),
            "what follows is left alone"
        );
        assert_eq!(Sid::read(&written[..27]), None, "one byte short");
        let mut revision = bytes(&domain);
        revision[0] = 2;
        assert_eq!(Sid::read(&revision), None);
        let mut too_many = vec![1, 16, 0, 0, 0, 0, 0, 5];
        too_many.extend_from_slice(&[0; 64]);
        assert_eq!(Sid::read(&too_many), None, "16 numbers");
        let mut fifteen = vec![1, 15, 0, 0, 0, 0, 0, 5];
        fifteen.extend_from_slice(&[7; 60]);
        assert_eq!(
            Sid::read(&fifteen).map(|(s, n)| (s.numbers().len(), n)),
            Some((15, 68))
        );
        assert_eq!(Sid::read(&[]), None);
        assert_eq!(Sid::read(&[1]), None);
        let (empty, size) = Sid::read(&[1, 0, 0, 0, 0, 0, 0, 3]).unwrap();
        assert_eq!((empty.authority(), empty.numbers(), size), (3, &[][..], 8));
    }

    #[test]
    fn a_sid_is_read_from_its_text_and_written_back() {
        for text in [
            "S-1-5-18",
            "S-1-1-0",
            "S-1-5-32-544",
            "S-1-16-8192",
            "S-1-5-21-1004336348-1177238915-682003330-512",
            "S-1-5",
            "S-1-4294967295-4294967295",
            "S-1-0x000100000000-7",
            "S-1-0xFFFFFFFFFFFF",
        ] {
            assert_eq!(sid(text).to_string(), text);
        }
        assert_eq!(sid("S-1-5-32-544"), known::ADMINISTRATORS);
        assert_eq!(
            sid("S-1-0x000000000005-18"),
            known::SYSTEM,
            "a small authority in hex"
        );
        assert_eq!(sid("S-1-0x000100000000").authority(), 1 << 32);
        assert_eq!(
            sid("S-1-5-1-2-3-4-5-6-7-8-9-10-11-12-13-14-15")
                .numbers()
                .len(),
            15
        );
    }

    #[test]
    fn only_a_sid_reads_as_one() {
        for text in [
            "",
            "S-1",
            "S-1-",
            "S-2-5-18",
            "s-1-5-18",
            "S-1-5-",
            "S-1--18",
            "S-1-5-+18",
            "S-1-5-18x",
            "S-1-5-4294967296",
            "S-1-4294967296",
            "S-1-0x1-5",
            "S-1-0x0001000000000-5",
            "S-1-0x00010000000G",
            "S-1-5-1-2-3-4-5-6-7-8-9-10-11-12-13-14-15-16",
        ] {
            assert_eq!(Sid::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_sid_has_room_for_what_the_specification_allows() {
        assert!(Sid::new(Sid::MAX_AUTHORITY, &[1; 15]).is_some());
        assert_eq!(Sid::new(Sid::MAX_AUTHORITY + 1, &[1]), None);
        assert_eq!(Sid::new(5, &[1; 16]), None);
        assert_eq!(Sid::MAX_AUTHORITY, 0xffff_ffff_ffff);
        assert_eq!(Sid::new(5, &[18]), Some(known::SYSTEM));
    }

    #[test]
    fn the_well_known_sids_are_windows_own() {
        let names = [
            (known::EVERYONE, "S-1-1-0"),
            (known::AUTHENTICATED_USERS, "S-1-5-11"),
            (known::SYSTEM, "S-1-5-18"),
            (known::ADMINISTRATORS, "S-1-5-32-544"),
            (known::USERS, "S-1-5-32-545"),
        ];
        for (sid, text) in names {
            assert_eq!(sid.to_string(), text);
        }
    }

    #[test]
    fn integrity_levels_have_windows_labels() {
        let labels: Vec<String> = Integrity::ALL.iter().map(|l| l.sid().to_string()).collect();
        assert_eq!(
            labels,
            [
                "S-1-16-0",
                "S-1-16-4096",
                "S-1-16-8192",
                "S-1-16-12288",
                "S-1-16-16384"
            ]
        );
        for level in Integrity::ALL {
            assert_eq!(Integrity::of(&level.sid()), Some(level));
        }
        assert_eq!(
            Integrity::of(&sid("S-1-16-8448")),
            Some(Integrity::Medium),
            "medium plus"
        );
        assert_eq!(
            Integrity::of(&sid("S-1-16-20480")),
            Some(Integrity::System),
            "protected"
        );
        assert_eq!(
            Integrity::of(&sid("S-1-16-4095")),
            Some(Integrity::Untrusted)
        );
        assert_eq!(Integrity::of(&sid("S-1-16")), None);
        assert_eq!(Integrity::of(&sid("S-1-16-1-2")), None);
        assert_eq!(Integrity::of(&sid("S-1-5-8192")), None);
        assert!(Integrity::Low < Integrity::Medium && Integrity::High < Integrity::System);
    }

    #[test]
    fn privileges_have_windows_names() {
        assert_eq!(Privileges::DEBUG.name(), Some("SeDebugPrivilege"));
        assert_eq!(
            Privileges::ASSIGN_TOKEN.name(),
            Some("SeAssignPrimaryTokenPrivilege")
        );
        let mut seen = 0;
        for (p, name) in Privileges::NAMES {
            assert_eq!(p.0.count_ones(), 1, "{name}");
            assert_eq!(Privileges::named(name), Some(p));
            assert_eq!(p.name(), Some(name));
            seen |= p.0;
        }
        assert_eq!(seen, Privileges::ALL.0, "every privilege has a name");
        assert_eq!(Privileges::NONE.name(), None);
        assert_eq!(Privileges::ALL.name(), None);
        assert_eq!(Privileges::named("SeDebug"), None);
        assert_eq!(Privileges::named("sedebugprivilege"), None);
    }

    #[test]
    fn privileges_combine_bit_by_bit() {
        let some = Privileges(Privileges::DEBUG.0 | Privileges::SHUTDOWN.0);
        assert!(some.contains(Privileges::DEBUG));
        assert!(some.contains(some));
        assert!(some.contains(Privileges::NONE));
        assert!(!some.contains(Privileges::BACKUP));
        assert!(!some.contains(Privileges::ALL));
        assert_eq!(
            some.both(Privileges(Privileges::SHUTDOWN.0 | Privileges::BACKUP.0)),
            Privileges::SHUTDOWN
        );
    }

    /// The token of a user who is an administrator with their rights off,
    /// as Meridian's one user will run.
    fn user() -> Token {
        let groups = [
            Group::of(known::EVERYONE),
            Group::of(known::USERS),
            Group {
                sid: known::ADMINISTRATORS,
                deny_only: true,
            },
        ];
        let me = sid("S-1-5-21-1-2-3-1000");
        Token::new(me, &groups, Privileges::SHUTDOWN, Integrity::Medium).unwrap()
    }

    #[test]
    fn the_kernels_own_programs_run_as_system() {
        let t = Token::system();
        assert_eq!(t.user, known::SYSTEM);
        assert_eq!(t.privileges, Privileges::ALL);
        assert_eq!(t.integrity, Integrity::System);
        let groups: Vec<Sid> = t.groups().iter().map(|g| g.sid).collect();
        assert_eq!(
            groups,
            [
                known::ADMINISTRATORS,
                known::EVERYONE,
                known::AUTHENTICATED_USERS
            ]
        );
        assert!(t.groups().iter().all(|g| !g.deny_only));
        assert!(t.is_member(&known::SYSTEM) && t.is_member(&known::ADMINISTRATORS));
        assert!(!t.is_member(&known::USERS));
    }

    #[test]
    fn a_deny_only_group_does_not_make_a_member() {
        let t = user();
        assert!(t.is_member(&sid("S-1-5-21-1-2-3-1000")), "the user");
        assert!(t.is_member(&known::USERS));
        assert!(
            !t.is_member(&known::ADMINISTRATORS),
            "only counts against them"
        );
        assert!(t.denied_as(&known::ADMINISTRATORS));
        assert!(t.denied_as(&known::USERS) && t.denied_as(&t.user));
        assert!(!t.denied_as(&known::SYSTEM));
    }

    #[test]
    fn a_token_holds_sixteen_groups() {
        let groups = [Group::of(known::USERS); 16];
        let full = Token::new(known::SYSTEM, &groups, Privileges::NONE, Integrity::Low).unwrap();
        assert_eq!(full.groups().len(), 16);
        let more = [Group::of(known::USERS); 17];
        assert_eq!(
            Token::new(known::SYSTEM, &more, Privileges::NONE, Integrity::Low),
            None
        );
        let none = Token::new(known::SYSTEM, &[], Privileges::NONE, Integrity::Low).unwrap();
        assert!(none.groups().is_empty() && !none.is_member(&known::EVERYONE));
    }

    #[test]
    fn a_restriction_never_adds_to_a_token() {
        let t = user();
        let lower = t.restricted(Restriction {
            integrity: Integrity::Low,
            keep: Privileges::ALL,
        });
        assert_eq!(
            (lower.integrity, lower.privileges),
            (Integrity::Low, Privileges::SHUTDOWN)
        );
        assert_eq!(
            (lower.user, lower.groups()),
            (t.user, t.groups()),
            "the same user and groups"
        );
        let higher = t.restricted(Restriction {
            integrity: Integrity::System,
            keep: Privileges(Privileges::DEBUG.0 | Privileges::SHUTDOWN.0),
        });
        assert_eq!(
            (higher.integrity, higher.privileges),
            (Integrity::Medium, Privileges::SHUTDOWN)
        );
        let nothing = Token::system().restricted(Restriction {
            integrity: Integrity::Untrusted,
            keep: Privileges::NONE,
        });
        assert_eq!(
            (nothing.integrity, nothing.privileges),
            (Integrity::Untrusted, Privileges::NONE)
        );
    }

    #[test]
    fn a_restriction_fits_one_register() {
        let r = Restriction {
            integrity: Integrity::Low,
            keep: Privileges::SHUTDOWN,
        };
        assert_eq!(r.word(), 0x8000_0001_0000_0002);
        assert_eq!(Restriction::from_word(r.word()), Some(r));
        for level in Integrity::ALL {
            let r = Restriction {
                integrity: level,
                keep: Privileges(u32::MAX),
            };
            assert_eq!(Restriction::from_word(r.word()), Some(r));
        }
        assert_eq!(Restriction::from_word(0), None, "no restriction");
        assert_eq!(
            Restriction::from_word(0x0000_0001_0000_0002),
            None,
            "without bit 63"
        );
        assert_eq!(
            Restriction::from_word(0x8000_0005_0000_0000),
            None,
            "no such level"
        );
        assert_eq!(
            Restriction::from_word(0x8000_0100_0000_0000),
            None,
            "bit 40"
        );
        assert_eq!(
            Restriction::from_word(0xc000_0000_0000_0000),
            None,
            "bit 62"
        );
        assert_eq!(
            Restriction::from_word(0x8000_00ff_0000_0000),
            None,
            "level 255"
        );
    }

    fn written(t: &Token) -> Vec<u8> {
        let mut out = vec![0u8; t.size()];
        assert_eq!(t.write(&mut out), Some(out.len()));
        out
    }

    #[test]
    fn a_token_is_written_for_a_program_to_read() {
        let system = written(&Token::system());
        let head = [1, 4, 3, 0, 0xff, 0, 0, 0];
        assert_eq!(system[..8], head);
        assert_eq!(
            system[8..20],
            [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0],
            "SYSTEM"
        );
        assert_eq!(
            system[20..37],
            [0, 1, 2, 0, 0, 0, 0, 0, 5, 0x20, 0, 0, 0, 0x20, 2, 0, 0],
            "the administrators"
        );
        assert_eq!(system.len(), 8 + 12 + 17 + 13 + 13);
        let u = written(&user());
        assert_eq!(u[..8], [1, 2, 3, 0, 2, 0, 0, 0]);
        assert_eq!(u[u.len() - 17], 1, "the administrators only count against");
        let mut short = vec![0u8; system.len() - 1];
        assert_eq!(Token::system().write(&mut short), None);
    }

    #[test]
    fn a_token_is_read_back_as_it_was() {
        for t in [
            Token::system(),
            user(),
            user().restricted(Restriction {
                integrity: Integrity::Untrusted,
                keep: Privileges::NONE,
            }),
        ] {
            assert_eq!(Token::read(&written(&t)), Some(t));
        }
        let none =
            Token::new(known::SYSTEM, &[], Privileges(0x8000_0001), Integrity::High).unwrap();
        assert_eq!(Token::read(&written(&none)), Some(none));
    }

    #[test]
    fn only_a_whole_token_reads_as_one() {
        let good = written(&user());
        assert_eq!(Token::read(&good[..good.len() - 1]), None, "a byte short");
        let mut longer = good.clone();
        longer.push(0);
        assert_eq!(Token::read(&longer), None, "a byte more");
        let broken = |at: usize, value: u8| {
            let mut b = good.clone();
            b[at] = value;
            Token::read(&b)
        };
        assert_eq!(broken(0, 2), None, "version");
        assert_eq!(broken(1, 5), None, "integrity");
        assert_eq!(broken(2, 17), None, "groups");
        assert_eq!(broken(2, 2), None, "fewer groups than follow");
        assert_eq!(broken(3, 1), None, "the byte of 0");
        assert_eq!(broken(8 + 28, 2), None, "a group's deny-only byte");
        assert_eq!(broken(9, 9), None, "the user's SID longer than there is");
        assert_eq!(Token::read(&good[..7]), None);
        assert_eq!(Token::read(&[]), None);
    }

    const READ: u32 = 1;
    const WRITE: u32 = 1 << 1;
    const RUN: u32 = 1 << 2;
    /// The rights of a made-up kind of object that change it.
    const WRITES: u32 = WRITE | rights::DELETE | rights::WRITE_DAC | rights::WRITE_OWNER;

    fn object(list: Option<&[Entry]>) -> Descriptor<'_> {
        Descriptor {
            owner: known::SYSTEM,
            list,
            label: Integrity::Medium,
        }
    }

    #[test]
    fn an_object_without_a_list_lets_anyone_do_anything() {
        assert_eq!(
            access(
                &user(),
                &object(None),
                READ | WRITE | rights::DELETE,
                WRITES
            ),
            Ok(())
        );
        assert_eq!(
            access(&user(), &object(None), 0, WRITES),
            Ok(()),
            "nothing asked"
        );
    }

    #[test]
    fn an_empty_list_allows_nothing_but_the_owners_rights() {
        let empty: &[Entry] = &[];
        assert_eq!(
            access(&user(), &object(Some(empty)), READ, WRITES),
            Err(READ)
        );
        assert_eq!(access(&user(), &object(Some(empty)), 0, WRITES), Ok(()));
        let owners = rights::READ_CONTROL | rights::WRITE_DAC;
        assert_eq!(
            access(&Token::system(), &object(Some(empty)), owners, WRITES),
            Ok(())
        );
        assert_eq!(
            access(&user(), &object(Some(empty)), owners, WRITES),
            Err(owners)
        );
        assert_eq!(
            access(
                &Token::system(),
                &object(Some(empty)),
                owners | READ,
                WRITES
            ),
            Err(READ),
            "the owner gets those two only"
        );
    }

    #[test]
    fn entries_allow_what_they_name_for_whom_they_name() {
        let list = [
            Entry::Allow(known::USERS, READ | RUN),
            Entry::Allow(known::SYSTEM, WRITE),
        ];
        let o = object(Some(&list));
        assert_eq!(access(&user(), &o, READ, WRITES), Ok(()));
        assert_eq!(access(&user(), &o, READ | RUN, WRITES), Ok(()));
        assert_eq!(
            access(&user(), &o, READ | WRITE, WRITES),
            Err(WRITE),
            "only SYSTEM writes"
        );
        assert_eq!(access(&Token::system(), &o, WRITE, WRITES), Ok(()));
        assert_eq!(
            access(&Token::system(), &o, READ, WRITES),
            Err(READ),
            "SYSTEM is no user here"
        );
        let split = [
            Entry::Allow(known::USERS, READ),
            Entry::Allow(known::EVERYONE, RUN),
        ];
        assert_eq!(
            access(&user(), &object(Some(&split)), READ | RUN, WRITES),
            Ok(()),
            "from two entries"
        );
    }

    #[test]
    fn an_entry_that_denies_counts_where_it_stands() {
        let denied_first = [
            Entry::Deny(known::EVERYONE, WRITE),
            Entry::Allow(known::USERS, READ | WRITE),
        ];
        let o = object(Some(&denied_first));
        assert_eq!(access(&user(), &o, READ | WRITE, WRITES), Err(WRITE));
        assert_eq!(
            access(&user(), &o, READ, WRITES),
            Ok(()),
            "what it does not deny"
        );
        let allowed_first = [
            Entry::Allow(known::USERS, READ | WRITE),
            Entry::Deny(known::EVERYONE, WRITE),
        ];
        assert_eq!(
            access(&user(), &object(Some(&allowed_first)), READ | WRITE, WRITES),
            Ok(()),
            "granted before the denial came"
        );
        let between = [
            Entry::Allow(known::USERS, READ),
            Entry::Deny(known::EVERYONE, READ | WRITE),
            Entry::Allow(known::USERS, WRITE),
        ];
        assert_eq!(
            access(&user(), &object(Some(&between)), READ | WRITE, WRITES),
            Err(WRITE)
        );
        let others = [
            Entry::Deny(known::SYSTEM, READ),
            Entry::Allow(known::USERS, READ),
        ];
        assert_eq!(
            access(&user(), &object(Some(&others)), READ, WRITES),
            Ok(()),
            "a denial for someone else"
        );
    }

    #[test]
    fn a_deny_only_group_is_denied_but_never_allowed() {
        let admins_may = [Entry::Allow(known::ADMINISTRATORS, WRITE)];
        assert_eq!(
            access(&user(), &object(Some(&admins_may)), WRITE, WRITES),
            Err(WRITE)
        );
        assert_eq!(
            access(&Token::system(), &object(Some(&admins_may)), WRITE, WRITES),
            Ok(())
        );
        let admins_may_not = [
            Entry::Deny(known::ADMINISTRATORS, READ),
            Entry::Allow(known::USERS, READ),
        ];
        assert_eq!(
            access(&user(), &object(Some(&admins_may_not)), READ, WRITES),
            Err(READ)
        );
    }

    #[test]
    fn nothing_writes_up() {
        let list = [Entry::Allow(known::EVERYONE, READ | WRITE)];
        let high = Descriptor {
            label: Integrity::High,
            ..object(Some(&list))
        };
        assert_eq!(
            access(&user(), &high, READ, WRITES),
            Ok(()),
            "reading up is allowed"
        );
        assert_eq!(access(&user(), &high, READ | WRITE, WRITES), Err(WRITE));
        assert_eq!(
            access(&user(), &high, rights::DELETE, WRITES),
            Err(rights::DELETE)
        );
        let level = Descriptor {
            label: Integrity::Medium,
            ..high
        };
        assert_eq!(
            access(&user(), &level, WRITE, WRITES),
            Ok(()),
            "at the same level"
        );
        let low = user().restricted(Restriction {
            integrity: Integrity::Low,
            keep: Privileges::ALL,
        });
        assert_eq!(access(&low, &level, WRITE, WRITES), Err(WRITE));
        assert_eq!(
            access(&low, &object(None), WRITE | READ, WRITES),
            Err(WRITE),
            "even with no list"
        );
    }

    #[test]
    fn taking_ownership_is_a_privilege() {
        let empty: &[Entry] = &[];
        let o = Descriptor {
            owner: known::USERS,
            ..object(Some(empty))
        };
        assert_eq!(
            access(&Token::system(), &o, rights::WRITE_OWNER, WRITES),
            Ok(())
        );
        assert_eq!(
            access(&user(), &o, rights::WRITE_OWNER, WRITES),
            Err(rights::WRITE_OWNER)
        );
        let without = Token::system().restricted(Restriction {
            integrity: Integrity::System,
            keep: Privileges::NONE,
        });
        assert_eq!(
            access(&without, &o, rights::WRITE_OWNER, WRITES),
            Err(rights::WRITE_OWNER)
        );
        assert_eq!(
            access(
                &user(),
                &o,
                rights::READ_CONTROL | rights::WRITE_DAC,
                WRITES
            ),
            Ok(()),
            "the users' group owns it"
        );
    }

    #[test]
    fn the_standard_rights_are_windows_own() {
        assert_eq!(
            [
                rights::DELETE,
                rights::READ_CONTROL,
                rights::WRITE_DAC,
                rights::WRITE_OWNER,
                rights::SYNCHRONIZE
            ],
            [
                0x0001_0000,
                0x0002_0000,
                0x0004_0000,
                0x0008_0000,
                0x0010_0000
            ]
        );
    }
}
