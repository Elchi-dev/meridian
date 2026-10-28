//! The archives POSIX's pax writes and reads: ustar, blocks of 512
//! bytes, each member a header block and its data filled to a whole
//! block, two empty blocks at the end; and cpio's portable kind, a header
//! of octal text, the name and the data. Meridian's files have no modes,
//! owners or times, so members are written with 644 for files, 755 for
//! directories and zeros for the rest. Reading takes POSIX's extended
//! headers and GNU's long names for names past ustar's room.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// The size of a ustar block.
pub const BLOCK: usize = 512;

/// What a member is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    /// A link, device or anything else, which Meridian's files cannot
    /// be.
    Other(u8),
}

/// A member's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    /// Its mode as the archive has it, which Meridian does not keep.
    pub mode: u32,
}

/// The kinds of archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Ustar,
    Cpio,
}

/// Why an archive cannot be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// A header that is none of its kind's, or whose sum is wrong.
    Header,
    /// A name too long for ustar, which keeps 255 bytes at most.
    NameTooLong(String),
    /// A number in a header that is no number.
    Number,
    /// The archive ends inside a member.
    Cut,
}

/// The digits of `n` in octal, `width` of them with zeros before, then
/// the byte after.
fn octal(n: u64, width: usize) -> Vec<u8> {
    let digits = format!("{n:o}");
    let mut out = vec![b'0'; width.saturating_sub(digits.len())];
    out.extend_from_slice(digits.as_bytes());
    out
}

/// The number written in octal in `field`, blanks and zeros around it.
fn number(field: &[u8]) -> Result<u64, Fault> {
    let text = field
        .iter()
        .copied()
        .skip_while(|&b| b == b' ')
        .take_while(|&b| b != 0 && b != b' ')
        .collect::<Vec<u8>>();
    if text.is_empty() {
        return Ok(0);
    }
    text.iter().try_fold(0u64, |n, &b| match b {
        b'0'..=b'7' => n
            .checked_mul(8)
            .map(|n| n + u64::from(b - b'0'))
            .ok_or(Fault::Number),
        _ => Err(Fault::Number),
    })
}

/// The text of a field, up to its first zero.
fn field_text(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// The ustar header block of `member`.
///
/// # Errors
///
/// For a name ustar has no room for.
pub fn ustar_header(member: &Member) -> Result<[u8; BLOCK], Fault> {
    let mut h = [0u8; BLOCK];
    // A directory's name ends with a slash, as tar writes it.
    let slashed;
    let name = if member.kind == Kind::Directory && !member.name.ends_with('/') {
        slashed = format!("{}/", member.name);
        slashed.as_bytes()
    } else {
        member.name.as_bytes()
    };
    // A name past 100 bytes goes in two, at a slash: the first part,
    // up to 155 bytes, in the prefix.
    let (prefix, rest) = if name.len() <= 100 {
        (&name[..0], name)
    } else {
        let cut = name
            .iter()
            .enumerate()
            .rev()
            .find(|&(at, &b)| b == b'/' && at <= 155 && name.len() - at - 1 <= 100 && at > 0)
            .map(|(at, _)| at)
            .ok_or_else(|| Fault::NameTooLong(member.name.clone()))?;
        (&name[..cut], &name[cut + 1..])
    };
    h[..rest.len()].copy_from_slice(rest);
    let put = |h: &mut [u8; BLOCK], at: usize, digits: &[u8]| {
        h[at..at + digits.len()].copy_from_slice(digits);
    };
    put(&mut h, 100, &octal(u64::from(member.mode & 0o7777), 7));
    put(&mut h, 108, &octal(0, 7));
    put(&mut h, 116, &octal(0, 7));
    put(&mut h, 124, &octal(member.size, 11));
    put(&mut h, 136, &octal(0, 11));
    h[156] = match member.kind {
        Kind::File => b'0',
        Kind::Directory => b'5',
        Kind::Other(kind) => kind,
    };
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    put(&mut h, 329, &octal(0, 7));
    put(&mut h, 337, &octal(0, 7));
    h[345..345 + prefix.len()].copy_from_slice(prefix);
    // The sum is of every byte, the sum's own field taken as blanks.
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    let mut field = octal(u64::from(sum), 6);
    field.extend_from_slice(b"\0 ");
    h[148..156].copy_from_slice(&field);
    Ok(h)
}

/// The cpio header of `member`, its name with it.
#[must_use]
pub fn cpio_header(member: &Member, inode: u64) -> Vec<u8> {
    let kind = match member.kind {
        Kind::Directory => 0o040_000,
        _ => 0o100_000,
    };
    let mut h = Vec::with_capacity(76 + member.name.len() + 1);
    h.extend_from_slice(b"070707");
    h.extend(octal(0, 6));
    h.extend(octal(inode % 0o1_000_000, 6));
    h.extend(octal(u64::from(kind | (member.mode & 0o7777)), 6));
    h.extend(octal(0, 6));
    h.extend(octal(0, 6));
    h.extend(octal(1, 6));
    h.extend(octal(0, 6));
    h.extend(octal(0, 11));
    h.extend(octal(member.name.len() as u64 + 1, 6));
    h.extend(octal(member.size, 11));
    h.extend_from_slice(member.name.as_bytes());
    h.push(0);
    h
}

/// What ends a cpio archive.
#[must_use]
pub fn cpio_trailer() -> Vec<u8> {
    cpio_header(
        &Member {
            name: String::from("TRAILER!!!"),
            kind: Kind::File,
            size: 0,
            mode: 0,
        },
        0,
    )
}

/// The bytes a record of ustar holds: archives are filled to whole ones,
/// as tar fills them.
pub const RECORD: u64 = 10_240;

/// Writes an archive a member at a time, counting what it wrote so that
/// it ends on a whole record.
pub struct Writer {
    format: Format,
    written: u64,
    members: u64,
}

impl Writer {
    #[must_use]
    pub fn new(format: Format) -> Writer {
        Writer {
            format,
            written: 0,
            members: 0,
        }
    }

    /// The header of `member`, its data to come after it.
    ///
    /// # Errors
    ///
    /// For a name ustar has no room for.
    pub fn begin(&mut self, member: &Member) -> Result<Vec<u8>, Fault> {
        self.members += 1;
        let header = match self.format {
            Format::Ustar => ustar_header(member)?.to_vec(),
            Format::Cpio => cpio_header(member, self.members),
        };
        self.written += header.len() as u64;
        Ok(header)
    }

    /// Counts `len` bytes of a member's data written.
    pub fn wrote(&mut self, len: usize) {
        self.written += len as u64;
    }

    /// What fills a member of `size` bytes to a whole block.
    pub fn end(&mut self, size: u64) -> Vec<u8> {
        let fill = match self.format {
            Format::Ustar => (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64,
            Format::Cpio => 0,
        };
        self.written += fill;
        vec![0; usize::try_from(fill).unwrap_or(0)]
    }

    /// What ends the archive: ustar's two empty blocks, or cpio's
    /// trailer, filled to a whole record, or for cpio a block.
    pub fn finish(&mut self) -> Vec<u8> {
        let (mut out, unit) = match self.format {
            Format::Ustar => (vec![0u8; 2 * BLOCK], RECORD),
            Format::Cpio => (cpio_trailer(), BLOCK as u64),
        };
        self.written += out.len() as u64;
        let fill = (unit - self.written % unit) % unit;
        out.resize(out.len() + usize::try_from(fill).unwrap_or(0), 0);
        self.written += fill;
        out
    }
}

/// What a reader holds of an archive while it is read.
#[derive(Debug, PartialEq, Eq)]
pub enum Item {
    /// A member's header; its data comes next, `size` bytes.
    Member(Member),
    /// The next of the member's data.
    Data(Vec<u8>),
    /// The end of the archive.
    End,
}

/// What reading a header came to.
enum Headed {
    /// Not all of it has come.
    More,
    /// A header that names the next member only: read on.
    Again,
    Item(Item),
}

/// Where a reader is.
#[derive(Debug)]
enum At {
    /// Before a header.
    Header,
    /// Inside a member's data: how much is left of it, and of the fill
    /// after it.
    Data(u64, u64),
    Ended,
}

/// Reads an archive of either kind a piece at a time, knowing it by its
/// first bytes.
pub struct Reader {
    held: Vec<u8>,
    at: At,
    format: Option<Format>,
    /// A name an extended header or a GNU long name gave the next member.
    long_name: Option<String>,
    long_size: Option<u64>,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    #[must_use]
    pub fn new() -> Reader {
        Reader {
            held: Vec::new(),
            at: At::Header,
            format: None,
            long_name: None,
            long_size: None,
        }
    }

    /// The kind of archive, once its first bytes came.
    #[must_use]
    pub fn format(&self) -> Option<Format> {
        self.format
    }

    /// Takes the next bytes of the archive.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.held.extend_from_slice(bytes);
    }

    /// The next item read from what was fed; none until more comes.
    ///
    /// # Errors
    ///
    /// For a header that is none, or when `ended` and the archive ends
    /// inside a member.
    pub fn next(&mut self, ended: bool) -> Result<Option<Item>, Fault> {
        loop {
            match self.at {
                At::Ended => return Ok(Some(Item::End)),
                At::Data(0, fill) => {
                    let skip = usize::try_from(fill).unwrap_or(usize::MAX);
                    if self.held.len() < skip {
                        return if ended { Err(Fault::Cut) } else { Ok(None) };
                    }
                    self.held.drain(..skip);
                    self.at = At::Header;
                }
                At::Data(left, fill) => {
                    if self.held.is_empty() {
                        return if ended { Err(Fault::Cut) } else { Ok(None) };
                    }
                    let take = self
                        .held
                        .len()
                        .min(usize::try_from(left).unwrap_or(usize::MAX));
                    let data: Vec<u8> = self.held.drain(..take).collect();
                    self.at = At::Data(left - take as u64, fill);
                    return Ok(Some(Item::Data(data)));
                }
                At::Header => {
                    if self.format.is_none() {
                        if self.held.len() < 6 {
                            return self.short(ended);
                        }
                        self.format = Some(if self.held.starts_with(b"070707") {
                            Format::Cpio
                        } else {
                            Format::Ustar
                        });
                    }
                    let read = match self.format {
                        Some(Format::Cpio) => self.cpio()?,
                        _ => self.ustar()?,
                    };
                    match read {
                        Headed::Item(item) => return Ok(Some(item)),
                        Headed::Again => {}
                        Headed::More => return self.short(ended),
                    }
                }
            }
        }
    }

    fn short(&self, ended: bool) -> Result<Option<Item>, Fault> {
        if !ended {
            return Ok(None);
        }
        // An archive with nothing at all is empty, not cut.
        if self.held.is_empty() && self.format.is_none() {
            return Ok(Some(Item::End));
        }
        Err(Fault::Cut)
    }

    /// The next ustar header: none when its block has not all come, an
    /// item, or nothing to give for a header that only names the next.
    fn ustar(&mut self) -> Result<Headed, Fault> {
        if self.held.len() < BLOCK {
            return Ok(Headed::More);
        }
        let h: Vec<u8> = self.held.drain(..BLOCK).collect();
        if h.iter().all(|&b| b == 0) {
            self.at = At::Ended;
            return Ok(Headed::Item(Item::End));
        }
        let mut blanked = h.clone();
        blanked[148..156].copy_from_slice(b"        ");
        let sum: u64 = blanked.iter().map(|&b| u64::from(b)).sum();
        if number(&h[148..156])? != sum {
            return Err(Fault::Header);
        }
        let size = self
            .long_size
            .take()
            .map_or_else(|| number(&h[124..136]), Ok)?;
        let fill = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;
        let kind = h[156];
        if matches!(kind, b'x' | b'L' | b'g' | b'K') {
            // The next member's name, or for g every one's and for K a
            // link's, which are passed over: its data is needed whole.
            let total = usize::try_from(size + fill).map_err(|_| Fault::Header)?;
            if self.held.len() < total {
                self.held.splice(..0, h);
                return Ok(Headed::More);
            }
            let data: Vec<u8> = self.held.drain(..total).collect();
            let data = &data[..usize::try_from(size).unwrap_or(0)];
            match kind {
                b'L' => self.long_name = Some(field_text(data)),
                b'x' => self.extended(data),
                _ => {}
            }
            return Ok(Headed::Again);
        }
        let name = self.long_name.take().unwrap_or_else(|| {
            let (rest, prefix) = (field_text(&h[..100]), field_text(&h[345..500]));
            let ustar = &h[257..262] == b"ustar";
            if ustar && !prefix.is_empty() {
                format!("{prefix}/{rest}")
            } else {
                rest
            }
        });
        // A directory's name comes without the slash tar ends it with.
        let name = String::from(name.trim_end_matches('/'));
        let kind = match kind {
            b'0' | 0 | b'7' => Kind::File,
            b'5' => Kind::Directory,
            other => Kind::Other(other),
        };
        let mode = u32::try_from(number(&h[100..108])?).map_err(|_| Fault::Number)?;
        // Links, devices and directories have no data, whatever their
        // size says; other kinds' data is passed over as data.
        let size = if matches!(kind, Kind::Directory | Kind::Other(b'1'..=b'6')) {
            0
        } else {
            size
        };
        let fill = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;
        self.at = At::Data(size, fill);
        Ok(Headed::Item(Item::Member(Member {
            name,
            kind,
            size,
            mode,
        })))
    }

    /// Takes the path and size of a POSIX extended header's records,
    /// each `length key=value\n`.
    fn extended(&mut self, mut data: &[u8]) {
        while let Some(space) = data.iter().position(|&b| b == b' ') {
            let Some(length) = core::str::from_utf8(&data[..space])
                .ok()
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|&n| n > space && n <= data.len())
            else {
                return;
            };
            let record = &data[space + 1..length];
            let record = record.strip_suffix(b"\n").unwrap_or(record);
            if let Some(path) = record.strip_prefix(b"path=") {
                self.long_name = Some(String::from_utf8_lossy(path).into_owned());
            } else if let Some(size) = record.strip_prefix(b"size=") {
                self.long_size = core::str::from_utf8(size).ok().and_then(|n| n.parse().ok());
            }
            data = &data[length..];
        }
    }

    /// The next cpio header, its name with it.
    fn cpio(&mut self) -> Result<Headed, Fault> {
        if self.held.len() < 76 {
            return Ok(Headed::More);
        }
        if !self.held.starts_with(b"070707") {
            return Err(Fault::Header);
        }
        let field = |at: usize, len: usize| number(&self.held[at..at + len]);
        let mode = field(18, 6)?;
        let name_size = usize::try_from(field(59, 6)?).map_err(|_| Fault::Header)?;
        let size = field(65, 11)?;
        if self.held.len() < 76 + name_size {
            return Ok(Headed::More);
        }
        let name = field_text(&self.held[76..76 + name_size]);
        self.held.drain(..76 + name_size);
        if name == "TRAILER!!!" {
            self.at = At::Ended;
            return Ok(Headed::Item(Item::End));
        }
        let kind = match mode & 0o170_000 {
            0o100_000 | 0 => Kind::File,
            0o040_000 => Kind::Directory,
            other => Kind::Other(u8::try_from(other >> 12).unwrap_or(0)),
        };
        self.at = At::Data(size, 0);
        Ok(Headed::Item(Item::Member(Member {
            name,
            kind,
            size,
            mode: u32::try_from(mode & 0o7777).unwrap_or(0),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the host's `program` writes given `input` on its standard
    /// input, in `dir`.
    fn run(dir: &std::path::Path, program: &str, words: &[&str], input: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut child = std::process::Command::new(program)
            .args(words)
            .current_dir(dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let input = input.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let done = child.wait_with_output().unwrap();
        writer.join().unwrap();
        assert!(
            done.status.success(),
            "{program} {words:?}: {}",
            String::from_utf8_lossy(&done.stderr)
        );
        done.stdout
    }

    /// A directory of the host's own for a test, gone once dropped.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = std::env::temp_dir()
                .join(format!("meridian-archive-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Scratch(root)
        }

        fn put(&self, path: &str, bytes: &[u8]) {
            let at = self.0.join(path);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, bytes).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The members of `archive` and their data, read in pieces of
    /// `piece`.
    fn read_all(archive: &[u8], piece: usize) -> Result<Vec<(Member, Vec<u8>)>, Fault> {
        let mut reader = Reader::new();
        let mut members: Vec<(Member, Vec<u8>)> = Vec::new();
        let mut pieces = archive.chunks(piece.max(1));
        loop {
            match reader.next(false)? {
                Some(Item::Member(member)) => members.push((member, Vec::new())),
                Some(Item::Data(data)) => members.last_mut().unwrap().1.extend(data),
                Some(Item::End) => return Ok(members),
                None => match pieces.next() {
                    Some(piece) => reader.feed(piece),
                    None => {
                        return match reader.next(true)? {
                            Some(Item::End) | None => Ok(members),
                            Some(item) => panic!("{item:?} after the end"),
                        };
                    }
                },
            }
        }
    }

    /// An archive of `format` holding `members`, each with its data.
    fn written(format: Format, members: &[(Member, &[u8])]) -> Vec<u8> {
        let mut writer = Writer::new(format);
        let mut out = Vec::new();
        for (member, data) in members {
            out.extend(writer.begin(member).unwrap());
            out.extend_from_slice(data);
            writer.wrote(data.len());
            out.extend(writer.end(member.size));
        }
        out.extend(writer.finish());
        out
    }

    fn file(name: &str, size: u64) -> Member {
        Member {
            name: name.into(),
            kind: Kind::File,
            size,
            mode: 0o644,
        }
    }

    fn directory(name: &str) -> Member {
        Member {
            name: name.into(),
            kind: Kind::Directory,
            size: 0,
            mode: 0o755,
        }
    }

    /// A name of `parts` directories of `width` letters and a file.
    fn deep(parts: usize, width: usize) -> String {
        let mut name = String::new();
        for part in 0..parts {
            name.push_str(&"d".repeat(width - 1));
            name.push(char::from(b'a' + u8::try_from(part % 26).unwrap()));
            name.push('/');
        }
        name.push_str("file");
        name
    }

    #[test]
    fn numbers_go_in_octal_and_come_back_whatever_surrounds_them() {
        assert_eq!(octal(0o644, 7), b"0000644");
        assert_eq!(octal(5, 11), b"00000000005");
        assert_eq!(
            octal(0o12_345_670, 3),
            b"12345670",
            "no fewer digits than it has"
        );
        assert_eq!(number(b"0000644\0"), Ok(0o644));
        assert_eq!(number(b"   644 \0"), Ok(0o644), "blanks before and after");
        assert_eq!(number(b"\0\0\0"), Ok(0), "nothing is 0");
        assert_eq!(number(b"0000948\0"), Err(Fault::Number));
        assert_eq!(
            number(b"77777777777777777777777"),
            Err(Fault::Number),
            "past a u64"
        );
        assert_eq!(field_text(b"name\0rest"), "name");
        assert_eq!(field_text(b"full"), "full");
    }

    #[test]
    fn a_ustar_header_has_its_fields_where_posix_puts_them() {
        let h = ustar_header(&file("a.txt", 5)).unwrap();
        assert_eq!(&h[..6], b"a.txt\0");
        assert_eq!(&h[100..108], b"0000644\0");
        assert_eq!(&h[124..136], b"00000000005\0");
        assert_eq!(h[156], b'0');
        assert_eq!(&h[257..265], b"ustar\x0000");
        let mut blanked = h;
        blanked[148..156].copy_from_slice(b"        ");
        let sum: u64 = blanked.iter().map(|&b| u64::from(b)).sum();
        assert_eq!(number(&h[148..156]), Ok(sum), "its sum");
        assert_eq!(&h[154..156], b"\0 ");
        assert_eq!(ustar_header(&directory("d")).unwrap()[156], b'5');
        assert_eq!(
            &ustar_header(&directory("d")).unwrap()[100..108],
            b"0000755\0"
        );
    }

    #[test]
    fn long_names_part_at_a_slash_or_do_not_fit() {
        let hundred = "n".repeat(100);
        let h = ustar_header(&file(&hundred, 0)).unwrap();
        assert_eq!(&h[..100], hundred.as_bytes(), "100 fill the name");
        assert_eq!(h[345], 0, "with no prefix");
        let name = deep(3, 50);
        let h = ustar_header(&file(&name, 0)).unwrap();
        assert_eq!(field_text(&h[345..500]), name[..name.rfind('/').unwrap()]);
        assert_eq!(field_text(&h[..100]), "file");
        let longest = format!("{}/{}", "p".repeat(155), "n".repeat(100));
        assert!(ustar_header(&file(&longest, 0)).is_ok(), "155 and 100");
        for too_long in [
            "n".repeat(101),
            format!("{}/{}", "p".repeat(156), "n".repeat(10)),
            format!("{}/{}", "p".repeat(10), "n".repeat(101)),
            format!("/{}", "n".repeat(101)),
        ] {
            assert_eq!(
                ustar_header(&file(&too_long, 0)),
                Err(Fault::NameTooLong(too_long.clone())),
                "{}",
                too_long.len()
            );
        }
    }

    #[test]
    fn tar_and_cpio_read_what_it_writes() {
        let scratch = Scratch::new("written");
        let long = deep(3, 50);
        let members: [(Member, &[u8]); 5] = [
            (directory("d"), b""),
            (file("d/f", 6), b"hello\n"),
            (file("empty", 0), b""),
            (file("big", 2000), &[b'z'; 2000]),
            (file(&long, 3), b"abc"),
        ];
        let tar = written(Format::Ustar, &members);
        assert_eq!(tar.len() % 10_240, 0, "whole records");
        let listed = run(&scratch.0, "tar", &["-tf", "-"], &tar);
        assert_eq!(
            String::from_utf8_lossy(&listed),
            format!("d/\nd/f\nempty\nbig\n{long}\n")
        );
        assert_eq!(
            run(&scratch.0, "bsdtar", &["-xOf", "-", "d/f"], &tar),
            b"hello\n"
        );
        assert_eq!(
            run(&scratch.0, "bsdtar", &["-xOf", "-", "big"], &tar),
            [b'z'; 2000]
        );
        assert_eq!(run(&scratch.0, "tar", &["-xOf", "-", &long], &tar), b"abc");
        let cpio = written(Format::Cpio, &members);
        assert_eq!(cpio.len() % BLOCK, 0, "whole blocks");
        let listed = run(&scratch.0, "cpio", &["-it", "--quiet"], &cpio);
        assert_eq!(
            String::from_utf8_lossy(&listed),
            format!("d\nd/f\nempty\nbig\n{long}\n")
        );
        assert_eq!(
            run(&scratch.0, "bsdtar", &["-xOf", "-", "d/f"], &cpio),
            b"hello\n"
        );
        assert_eq!(
            run(&scratch.0, "bsdtar", &["-xOf", "-", &long], &cpio),
            b"abc"
        );
    }

    #[test]
    fn it_reads_what_bsdtar_gnu_tar_and_cpio_write() {
        let scratch = Scratch::new("read");
        let long = deep(5, 60);
        scratch.put("a/one", b"one\n");
        scratch.put("a/two", &[7u8; 1500]);
        scratch.put(&long, b"far down\n");
        let names = ["a", &long];
        for (program, words) in [
            ("bsdtar", &["-cf", "-", "--format", "ustar"][..]),
            ("bsdtar", &["-cf", "-", "--format", "pax"]),
            ("bsdtar", &["-cf", "-", "--format", "odc"]),
            ("tar", &["-cf", "-", "--format", "gnu"]),
            ("tar", &["-cf", "-", "--format", "posix"]),
        ] {
            if program == "bsdtar" && words.contains(&"ustar") {
                // ustar has no room for the long name.
                continue;
            }
            let mut all = words.to_vec();
            all.extend(names);
            let archive = run(&scratch.0, program, &all, b"");
            for piece in [1, 511, 512, 4096, archive.len()] {
                let mut read = read_all(&archive, piece).unwrap();
                read.sort_by(|a, b| a.0.name.cmp(&b.0.name));
                let seen: Vec<(String, Kind, Vec<u8>)> = read
                    .into_iter()
                    .map(|(m, data)| (m.name.trim_end_matches('/').into(), m.kind, data))
                    .collect();
                assert_eq!(
                    seen,
                    [
                        ("a".into(), Kind::Directory, Vec::new()),
                        ("a/one".into(), Kind::File, b"one\n".to_vec()),
                        ("a/two".into(), Kind::File, alloc::vec![7u8; 1500]),
                        (long.clone(), Kind::File, b"far down\n".to_vec()),
                    ],
                    "{program} {words:?} in pieces of {piece}"
                );
            }
        }
        let cpio = run(
            &scratch.0,
            "cpio",
            &["-o", "-H", "odc", "--quiet"],
            b"a/one\na/two\n",
        );
        let read = read_all(&cpio, 100).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].1, b"one\n");
        assert_eq!(read[1].0.size, 1500);
    }

    #[test]
    fn an_archive_wrong_or_cut_short_is_said() {
        let tar = written(Format::Ustar, &[(file("f", 3), b"abc")]);
        let mut bad = tar.clone();
        bad[0] = b'g';
        assert_eq!(read_all(&bad, 512), Err(Fault::Header), "the sum is off");
        assert_eq!(
            read_all(&tar[..600], 600),
            Err(Fault::Cut),
            "inside the data"
        );
        assert_eq!(
            read_all(&tar[..512 + 3], 512),
            Err(Fault::Cut),
            "before its fill"
        );
        assert_eq!(
            read_all(&tar[..100], 100),
            Err(Fault::Cut),
            "inside a header"
        );
        assert_eq!(
            read_all(&[], 1),
            Ok(Vec::new()),
            "nothing is no archive and no fault"
        );
        let cpio = written(Format::Cpio, &[(file("f", 3), b"abc")]);
        let mut wrong = cpio.clone();
        wrong[76 + 2 + 3] = b'X';
        assert_eq!(
            read_all(&wrong, 1000),
            Err(Fault::Header),
            "the trailer's magic"
        );
        assert_eq!(read_all(&cpio[..80], 80), Err(Fault::Cut));
        let mut number = tar.clone();
        number[124] = b'9';
        assert!(read_all(&number, 512).is_err(), "a size that is no octal");
    }

    #[test]
    fn writers_count_what_they_wrote_to_fill_the_last_record() {
        let mut writer = Writer::new(Format::Ustar);
        assert_eq!(writer.begin(&file("f", 700)).unwrap().len(), BLOCK);
        writer.wrote(700);
        assert_eq!(writer.end(700).len(), 324, "to 1024");
        assert_eq!(writer.end(1024).len(), 0, "a whole block needs none");
        let end = writer.finish();
        assert_eq!(1536 + end.len(), 10_240, "one record in all");
        assert!(end.iter().all(|&b| b == 0));
        let mut writer = Writer::new(Format::Cpio);
        let header = writer.begin(&file("name", 3)).unwrap();
        assert_eq!(header.len(), 76 + 5);
        assert_eq!(&header[..6], b"070707");
        assert_eq!(&header[18..24], b"100644", "a file and its mode");
        writer.wrote(3);
        assert_eq!(writer.end(3), Vec::<u8>::new(), "cpio has no fill");
        let end = writer.finish();
        assert_eq!((81 + 3 + end.len()) % BLOCK, 0);
        assert!(end.starts_with(&cpio_trailer()));
        assert_eq!(&cpio_header(&directory("d"), 1)[18..24], b"040755");
    }

    #[test]
    fn kinds_other_than_files_and_directories_are_named_and_passed_over() {
        let mut link = ustar_header(&file("l", 0)).unwrap();
        link[156] = b'2';
        link[157..158].copy_from_slice(b"f");
        link[148..156].copy_from_slice(b"        ");
        let sum: u64 = link.iter().map(|&b| u64::from(b)).sum();
        let mut field = octal(sum, 6);
        field.extend_from_slice(b"\0 ");
        link[148..156].copy_from_slice(&field);
        let mut archive = link.to_vec();
        archive.extend(written(Format::Ustar, &[(file("f", 2), b"ok")]));
        let read = read_all(&archive, 512).unwrap();
        assert_eq!(read[0].0.kind, Kind::Other(b'2'));
        assert_eq!(
            read[1],
            (file("f", 2), b"ok".to_vec()),
            "and what comes after it is whole"
        );
    }

    /// A ustar header of `kind` for `name`, its size `size`, its sum
    /// made right.
    fn raw_header(name: &str, kind: u8, size: u64) -> Vec<u8> {
        let mut h = ustar_header(&file(name, size)).unwrap();
        h[156] = kind;
        h[148..156].copy_from_slice(b"        ");
        let sum: u64 = h.iter().map(|&b| u64::from(b)).sum();
        let mut field = octal(sum, 6);
        field.extend_from_slice(b"\0 ");
        h[148..156].copy_from_slice(&field);
        h.to_vec()
    }

    /// `data` filled to a whole block.
    fn filled(data: &[u8]) -> Vec<u8> {
        let mut out = data.to_vec();
        out.resize(data.len().div_ceil(BLOCK) * BLOCK, 0);
        out
    }

    /// A POSIX extended record: its length counts itself.
    fn record(key: &str, value: &str) -> String {
        let body = format!(" {key}={value}\n");
        let mut length = body.len() + 1;
        while format!("{length}").len() + body.len() != length {
            length += 1;
        }
        format!("{length}{body}")
    }

    #[test]
    fn extended_headers_give_the_next_member_its_path_and_size() {
        let records =
            record("path", "a much longer name") + &record("size", "4") + &record("mtime", "5");
        let mut archive = raw_header("PaxHeader", b'x', records.len() as u64);
        archive.extend(filled(records.as_bytes()));
        archive.extend(written(Format::Ustar, &[(file("short", 9), b"data")]));
        let read = read_all(&archive, 7).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].0.name, "a much longer name");
        assert_eq!(read[0].0.size, 4, "the size the record gives");
        assert_eq!(read[0].1, b"data");
        assert_eq!(record("path", "x"), "9 path=x\n");
        assert_eq!(
            record("path", &"y".repeat(95)),
            format!("105 path={}\n", "y".repeat(95))
        );
    }

    #[test]
    fn records_that_do_not_read_end_the_reading_of_them() {
        let mut reader = Reader::new();
        reader.extended(b"14 path=first\nxx path=second\n");
        assert_eq!(reader.long_name.as_deref(), Some("first"));
        let mut reader = Reader::new();
        reader.extended(b"999 path=long\n");
        assert_eq!(reader.long_name, None, "a length past the records");
        reader.extended(b"3 path=x\n");
        assert_eq!(reader.long_name, None, "a length within its own number");
        reader.extended(b"no space");
        assert_eq!(reader.long_name, None);
        reader.extended(b"9 size=x\n");
        assert_eq!(reader.long_size, None, "a size that is no number");
        reader.extended(b"13 path=one\r\n");
        assert_eq!(
            reader.long_name.as_deref(),
            Some("one\r"),
            "only the newline goes"
        );
    }

    #[test]
    fn global_headers_and_long_link_names_are_passed_over() {
        let global = record("comment", "for every member");
        let mut archive = raw_header("global", b'g', global.len() as u64);
        archive.extend(filled(global.as_bytes()));
        let link = "a target past a hundred bytes ".repeat(5);
        archive.extend(raw_header("././@LongLink", b'K', link.len() as u64 + 1));
        archive.extend(filled(format!("{link}\0").as_bytes()));
        archive.extend(raw_header("l", b'1', 0));
        let gnu_name = "g".repeat(150);
        archive.extend(raw_header("././@LongLink", b'L', gnu_name.len() as u64 + 1));
        archive.extend(filled(format!("{gnu_name}\0").as_bytes()));
        archive.extend(written(Format::Ustar, &[(file("short", 2), b"hi")]));
        for piece in [1, 100, 512, archive.len()] {
            let read = read_all(&archive, piece).unwrap();
            let seen: Vec<(&str, Kind)> = read
                .iter()
                .map(|(m, _)| (m.name.as_str(), m.kind))
                .collect();
            assert_eq!(
                seen,
                [("l", Kind::Other(b'1')), (gnu_name.as_str(), Kind::File)],
                "{piece}"
            );
            assert_eq!(read[1].1, b"hi");
        }
    }

    #[test]
    fn cpio_kinds_come_from_their_mode() {
        let mut archive = cpio_header(&directory("d"), 1);
        let mut link = cpio_header(&file("l", 6), 2);
        link[18..24].copy_from_slice(b"120777");
        archive.extend(link);
        archive.extend_from_slice(b"target");
        archive.extend(cpio_header(&file("f", 2), 3));
        archive.extend_from_slice(b"ok");
        archive.extend(cpio_trailer());
        let read = read_all(&archive, 5).unwrap();
        let seen: Vec<(&str, Kind, u32)> = read
            .iter()
            .map(|(m, _)| (m.name.as_str(), m.kind, m.mode))
            .collect();
        assert_eq!(
            seen,
            [
                ("d", Kind::Directory, 0o755),
                ("l", Kind::Other(0o12), 0o777),
                ("f", Kind::File, 0o644)
            ]
        );
        assert_eq!(read[1].1, b"target", "a link's target passed as its data");
        assert_eq!(read[2].1, b"ok");
        let mut reader = Reader::new();
        assert_eq!(reader.format(), None);
        reader.feed(&archive[..6]);
        assert_eq!(reader.next(false), Ok(None));
        assert_eq!(reader.format(), Some(Format::Cpio));
        let mut reader = Reader::default();
        reader.feed(&written(Format::Ustar, &[])[..10]);
        assert_eq!(reader.next(false), Ok(None));
        assert_eq!(reader.format(), Some(Format::Ustar));
    }

    #[test]
    fn nothing_after_the_end_is_read() {
        let mut archive = written(Format::Ustar, &[(file("f", 1), b"x")]);
        archive.extend(raw_header("after", b'0', 0));
        let mut reader = Reader::new();
        reader.feed(&archive);
        assert!(matches!(reader.next(false), Ok(Some(Item::Member(_)))));
        assert_eq!(reader.next(false), Ok(Some(Item::Data(b"x".to_vec()))));
        assert_eq!(reader.next(false), Ok(Some(Item::End)));
        assert_eq!(reader.next(true), Ok(Some(Item::End)), "and it stays ended");
    }

    #[test]
    fn names_with_blanks_and_letters_of_any_script_go_through_tar() {
        let scratch = Scratch::new("names");
        let names = ["with blank", "gr\u{fc}\u{df}e", "\u{65e5}\u{672c}/\u{8a9e}"];
        let members: Vec<(Member, &[u8])> = names.iter().map(|n| (file(n, 1), &b"x"[..])).collect();
        let tar = written(Format::Ustar, &members);
        let listed = run(&scratch.0, "tar", &["-tf", "-"], &tar);
        assert_eq!(
            String::from_utf8(listed).unwrap(),
            format!("{}\n", names.join("\n"))
        );
        let read = read_all(&tar, 512).unwrap();
        let back: Vec<&str> = read.iter().map(|(m, _)| m.name.as_str()).collect();
        assert_eq!(back, names);
    }

    #[test]
    fn random_members_come_back_through_both_kinds_in_random_pieces() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..30 {
            let count = usize::try_from(next() % 6).unwrap();
            let mut members = Vec::new();
            let mut datas = Vec::new();
            for at in 0..count {
                let size = usize::try_from(next() % 1300).unwrap();
                let name = if next() % 4 == 0 {
                    format!(
                        "{}/{at}",
                        "p".repeat(usize::try_from(next() % 140).unwrap() + 1)
                    )
                } else {
                    format!("m{round}-{at}")
                };
                let data: Vec<u8> = (0..size).map(|i| (i % 251).to_le_bytes()[0]).collect();
                let kind = if next() % 5 == 0 {
                    Kind::Directory
                } else {
                    Kind::File
                };
                let size = if kind == Kind::File { size } else { 0 };
                members.push(Member {
                    name,
                    kind,
                    size: size as u64,
                    mode: if kind == Kind::File { 0o644 } else { 0o755 },
                });
                datas.push(data[..size].to_vec());
            }
            let given: Vec<(Member, &[u8])> = members
                .iter()
                .cloned()
                .zip(datas.iter().map(Vec::as_slice))
                .collect();
            for format in [Format::Ustar, Format::Cpio] {
                let archive = written(format, &given);
                let piece = usize::try_from(next() % 700).unwrap() + 1;
                let read = read_all(&archive, piece).unwrap();
                let back: Vec<(Member, Vec<u8>)> =
                    given.iter().map(|(m, d)| (m.clone(), d.to_vec())).collect();
                assert_eq!(read, back, "round {round}, {format:?}, pieces of {piece}");
            }
        }
    }

    /// `header` with its sum made right after it was changed.
    fn summed(mut header: Vec<u8>) -> Vec<u8> {
        header[148..156].copy_from_slice(b"        ");
        let sum: u64 = header.iter().map(|&b| u64::from(b)).sum();
        let mut field = octal(sum, 6);
        field.extend_from_slice(b"\0 ");
        header[148..156].copy_from_slice(&field);
        header
    }

    #[test]
    fn old_tar_headers_have_no_prefix_and_old_kinds_are_files() {
        // Before ustar there was no magic, and what is now the prefix was
        // not one.
        let mut old = ustar_header(&file("plain", 2)).unwrap().to_vec();
        old[257..265].fill(0);
        old[345..350].copy_from_slice(b"junk!");
        let mut archive = summed(old);
        archive.extend(filled(b"ab"));
        for kind in [0, b'7'] {
            let mut header = ustar_header(&file(&format!("kind{kind}"), 1))
                .unwrap()
                .to_vec();
            header[156] = kind;
            archive.extend(summed(header));
            archive.extend(filled(b"k"));
        }
        // A directory whose size is not zero has no data all the same.
        let mut dir = ustar_header(&directory("d")).unwrap().to_vec();
        dir[124..136].copy_from_slice(b"00000002000\0");
        archive.extend(summed(dir));
        archive.extend(written(Format::Ustar, &[(file("after", 1), b"z")]));
        let read = read_all(&archive, 512).unwrap();
        let seen: Vec<(&str, Kind, &[u8])> = read
            .iter()
            .map(|(m, d)| (m.name.as_str(), m.kind, d.as_slice()))
            .collect();
        assert_eq!(
            seen,
            [
                ("plain", Kind::File, &b"ab"[..]),
                ("kind0", Kind::File, b"k"),
                ("kind55", Kind::File, b"k"),
                ("d", Kind::Directory, b""),
                ("after", Kind::File, b"z"),
            ]
        );
    }

    #[test]
    fn writing_a_name_with_no_room_says_so() {
        let mut writer = Writer::new(Format::Ustar);
        let long = "x".repeat(300);
        assert_eq!(
            writer.begin(&file(&long, 0)),
            Err(Fault::NameTooLong(long.clone()))
        );
        let mut writer = Writer::new(Format::Cpio);
        let header = writer.begin(&file(&long, 0)).unwrap();
        assert_eq!(header.len(), 76 + 301, "cpio has room for any name");
        assert_eq!(
            &header[59..65],
            b"000455",
            "its length and the zero after it"
        );
    }

    #[test]
    fn sizes_go_up_to_eleven_octal_digits() {
        assert_eq!(number(b"77777777777\0"), Ok(8_589_934_591));
        let mut writer = Writer::new(Format::Ustar);
        let header = writer.begin(&file("huge", 8_589_934_591)).unwrap();
        assert_eq!(&header[124..136], b"77777777777\0");
        let mut reader = Reader::new();
        reader.feed(&header);
        match reader.next(false) {
            Ok(Some(Item::Member(member))) => assert_eq!(member.size, 8_589_934_591),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_archive_cut_in_its_first_bytes_is_cut() {
        let mut reader = Reader::new();
        reader.feed(b"0707");
        assert_eq!(reader.next(false), Ok(None), "more may come");
        assert_eq!(reader.next(true), Err(Fault::Cut));
        let mut reader = Reader::new();
        reader.feed(b"07070");
        assert_eq!(reader.format(), None);
        assert_eq!(reader.next(true), Err(Fault::Cut));
    }
}
