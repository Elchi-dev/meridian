//! pax: reads and writes archives, as POSIX's pax does, in ustar and in
//! cpio's portable kind, with lib/archive. With neither `-r` nor `-w` it
//! lists the members of the archive on its standard input, or in the file
//! `-f` names; `-r` reads them out into files, `-w` writes the files its
//! operands name, or those its standard input names a line each, into an
//! archive on its standard output or in `-f`'s file, and `-r -w` copies
//! them into the directory its last operand names. A directory takes all
//! it holds along, unless `-d` says it goes alone. Patterns, written as
//! the shell's but with `*` taking a `/` too, pick the members listed or
//! read out, and what a directory they pick holds; `-c` picks the others,
//! and `-n` only the first each pattern finds. `-v` lists as `ls -l` does,
//! and else names each file on the standard error as it goes; `-k` keeps
//! a file that is there already, and `-x` says which kind is written,
//! `ustar` or `cpio`. Meridian's files keep no modes, owners or times, so
//! they go into an archive as 644 for files, 755 for directories and
//! zeros, and `-p` and `-t` change nothing; with no links `-H`, `-L` and
//! `-l` change nothing either, and `-b`'s blocks are those the archive's
//! kind writes. A member whose name climbs out with `..` is passed over.
//! It ends with 0 when everything went, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use archive::{Fault, Format, Item, Kind, Member, Owner, Reader, Writer};
use program::abi::file::{CREATE_ALWAYS, READ, WRITE};
use program::date::{self, Moment};
use program::text::{self, LineReader, Pieces, Source};
use program::{Error, Handle};
use shell::sh::pattern::Pattern;

program::main!(pax);

/// What pax does, as `-r` and `-w` say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    List,
    Read,
    Write,
    Copy,
}

/// Which members and files are taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Picking {
    /// `-d`: a directory goes alone.
    itself: bool,
    /// `-c`: what no pattern picks.
    others: bool,
    /// `-n`: the first member each pattern picks.
    first: bool,
}

/// What pax's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    mode: Mode,
    /// The archive's file, else the standard input or output.
    archive: Option<&'w str>,
    verbose: bool,
    /// `-k`: a file that is there already stays.
    keep: bool,
    format: Format,
    picking: Picking,
    /// The patterns, or the files and for copying the directory last.
    operands: &'w [String],
}

/// What pax's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let (mut read, mut write) = (false, false);
    let mut asked = Asked {
        mode: Mode::List,
        archive: None,
        verbose: false,
        keep: false,
        format: Format::Ustar,
        picking: Picking::default(),
        operands: &[],
    };
    let mut at = 0;
    while let Some(word) = arguments.get(at) {
        if word == "--" {
            at += 1;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        at += 1;
        for (i, letter) in letters.char_indices() {
            match letter {
                'r' => read = true,
                'w' => write = true,
                'v' => asked.verbose = true,
                'd' => asked.picking.itself = true,
                'c' => asked.picking.others = true,
                'n' => asked.picking.first = true,
                'k' => asked.keep = true,
                'H' | 'L' | 'l' | 't' => {}
                'f' | 'x' | 'b' | 'p' => {
                    // The value goes on in the word, or is the next one.
                    let glued = &letters[i + 1..];
                    let value = if glued.is_empty() {
                        let next = arguments
                            .get(at)
                            .ok_or_else(|| format!("pax: -{letter} wants a value after it\n"))?;
                        at += 1;
                        next.as_str()
                    } else {
                        glued
                    };
                    took(&mut asked, letter, value)?;
                    break;
                }
                _ => return Err(format!("pax: -{letter} is no option\n")),
            }
        }
    }
    asked.operands = &arguments[at..];
    asked.mode = match (read, write) {
        (false, false) => Mode::List,
        (true, false) => Mode::Read,
        (false, true) => Mode::Write,
        (true, true) => Mode::Copy,
    };
    if asked.mode == Mode::Copy {
        if asked.operands.is_empty() {
            return Err(String::from(
                "pax: -r -w copies files into a directory, which comes last\n",
            ));
        }
        if asked.archive.is_some() {
            return Err(String::from("pax: -r -w copies with no archive for -f\n"));
        }
    }
    Ok(asked)
}

/// Takes the value of the option `letter`.
fn took<'w>(asked: &mut Asked<'w>, letter: char, value: &'w str) -> Result<(), String> {
    match letter {
        'f' => asked.archive = Some(value),
        'x' => {
            asked.format = match value {
                "ustar" => Format::Ustar,
                "cpio" => Format::Cpio,
                _ => return Err(format!("pax: -x {value}: writes ustar and cpio\n")),
            };
        }
        'b' => {
            let size = value.strip_suffix(['b', 'k']).unwrap_or(value);
            if size.is_empty()
                || !size.bytes().all(|b| b.is_ascii_digit())
                || size.bytes().all(|b| b == b'0')
            {
                return Err(format!(
                    "pax: -b {value}: a size of blocks is a number above 0\n"
                ));
            }
        }
        _ => {
            if !value.chars().all(|c| "aemop".contains(c)) {
                return Err(format!("pax: -p {value}: keeps a, e, m, o or p\n"));
            }
        }
    }
    Ok(())
}

/// What a file is, as pax sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    Directory,
    File(u64),
}

/// What pax does with files and its streams.
trait Files {
    /// What the file at `path` is.
    fn look(&mut self, path: &str) -> Result<Look, Error>;

    /// The names in the directory at `path`.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// Up to `room`'s worth of the file at `path` from the byte `at`, and
    /// how much that was; 0 at its end.
    fn read(&mut self, path: &str, at: u64, room: &mut [u8]) -> Result<usize, Error>;

    /// Makes the file at `path` new or empty, for `write` to fill.
    fn create(&mut self, path: &str) -> Result<(), Error>;

    /// Writes `bytes` after what the file `create` made holds.
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error>;

    /// Closes the file `create` made.
    fn done(&mut self) -> Result<(), Error>;

    /// Makes a directory at `path`.
    fn make_directory(&mut self, path: &str) -> Result<(), Error>;

    /// `path` from the root, as the tree of files compares paths.
    fn compared(&mut self, path: &str) -> String;

    /// The next piece of the archive; none at its end.
    fn archive(&mut self) -> Result<Vec<u8>, Error>;

    /// Writes `bytes` of the archive.
    fn put_archive(&mut self, bytes: &[u8]) -> Result<(), Error>;

    /// The next line of the standard input, without its line feed.
    fn line(&mut self) -> Result<Option<String>, Error>;

    /// Writes `text` on the standard output.
    fn print(&mut self, text: &str);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// `name` in the directory at `directory`.
fn joined(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// Whether `path` climbs out of where it starts with a `..`.
fn climbs(path: &str) -> bool {
    path.split('/').any(|part| part == "..")
}

/// The directories `path` lies in, each with those above it: for `a/b/c`
/// `a` and then `a/b`.
fn above(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .map(move |(at, _)| &path[..at])
        .filter(|lead| !lead.is_empty())
}

/// Makes the directories `path` lies in that are not there yet; one that
/// cannot be made shows when the file in it is made.
fn make_above(files: &mut impl Files, path: &str) {
    for lead in above(path) {
        let _ = files.make_directory(lead);
    }
}

/// The files `path` names: it first and then, for a directory unless
/// `itself`, all it holds in the order of their names; each with what it
/// is, or what went wrong when it was looked at or listed.
fn walked(files: &mut impl Files, path: &str, itself: bool) -> Vec<(String, Result<Look, Error>)> {
    let mut found = Vec::new();
    walk(files, path, itself, &mut found);
    found
}

fn walk(
    files: &mut impl Files,
    path: &str,
    itself: bool,
    found: &mut Vec<(String, Result<Look, Error>)>,
) {
    let look = files.look(path);
    found.push((String::from(path), look));
    if look != Ok(Look::Directory) || itself {
        return;
    }
    match files.names(path) {
        Ok(mut names) => {
            names.sort_unstable();
            for name in names {
                walk(files, &joined(path, &name), false, found);
            }
        }
        Err(e) => found.push((String::from(path), Err(e))),
    }
}

/// Which members the patterns pick, in list and read mode.
struct Picker {
    patterns: Vec<(Pattern, String)>,
    /// For each pattern, the member it picked first: what `-n` keeps to,
    /// with all it holds.
    found: Vec<Option<String>>,
    picking: Picking,
}

impl Picker {
    fn new(asked: &Asked) -> Picker {
        let patterns: Vec<(Pattern, String)> = asked
            .operands
            .iter()
            .map(|written| (pattern(written), written.clone()))
            .collect();
        Picker {
            found: alloc::vec![None; patterns.len()],
            patterns,
            picking: asked.picking,
        }
    }

    /// Whether the member `name` is picked.
    fn picks(&mut self, name: &str) -> bool {
        if self.patterns.is_empty() {
            return true;
        }
        let under = |top: &str| {
            name.strip_prefix(top)
                .is_some_and(|rest| rest.starts_with('/'))
        };
        let Picking {
            itself,
            others,
            first,
        } = self.picking;
        let mut picked = false;
        for ((pattern, _), found) in self.patterns.iter().zip(&mut self.found) {
            let hit = if let (Some(top), true) = (found.as_deref(), first) {
                // Past its first, -n keeps a pattern to what that holds.
                !itself && under(top)
            } else {
                // The member itself, or a directory it lies in.
                let at = if pattern.matches(name) {
                    Some(name)
                } else if itself {
                    None
                } else {
                    above(name).find(|lead| pattern.matches(lead))
                };
                if let Some(at) = at {
                    found.get_or_insert_with(|| String::from(at));
                }
                at.is_some()
            };
            picked |= hit;
        }
        picked != others
    }

    /// The patterns that picked nothing.
    fn unfound(&self) -> impl Iterator<Item = &str> {
        self.patterns
            .iter()
            .zip(&self.found)
            .filter(|(_, found)| found.is_none())
            .map(|((_, written), _)| written.as_str())
    }
}

/// A pattern as pax's words write one, a backslash making the character
/// after it stand for itself.
fn pattern(written: &str) -> Pattern {
    let mut chars = Vec::new();
    let mut letters = written.chars();
    while let Some(c) = letters.next() {
        if c == '\\'
            && let Some(next) = letters.next()
        {
            chars.push((next, true));
        } else {
            chars.push((c, false));
        }
    }
    Pattern::new(&chars)
}

/// A member's kind and mode as `ls -l` writes them.
fn mode_text(member: &Member) -> String {
    // ustar names the kinds by a letter, cpio by the bits of its mode.
    let kind = match member.kind {
        Kind::File | Kind::Other(b'1') => '-',
        Kind::Directory => 'd',
        Kind::Other(b'2' | 0o12) => 'l',
        Kind::Other(b'3' | 0o02) => 'c',
        Kind::Other(b'4' | 0o06) => 'b',
        Kind::Other(b'6' | 0o01) => 'p',
        Kind::Other(0o14) => 's',
        Kind::Other(_) => '?',
    };
    let mode = member.mode;
    let mut text = String::from(kind);
    for (shift, special, set) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let bit = |n: u32| (mode >> (shift + n)) & 1 == 1;
        text.push(if bit(2) { 'r' } else { '-' });
        text.push(if bit(1) { 'w' } else { '-' });
        text.push(match (mode & special != 0, bit(0)) {
            (true, true) => set,
            (true, false) => set.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    text
}

/// The line `-v` lists `member` with, as `ls -l` writes one: the
/// owners by name where the archive has them, and the date in UTC, as
/// Meridian has no time of day to tell a recent one by.
fn listed(member: &Member) -> String {
    let owner = &member.owner;
    let by = |name: &str, number: u64| {
        if name.is_empty() {
            format!("{number}")
        } else {
            String::from(name)
        }
    };
    let when = date::formatted("%b %e  %Y", &Moment::at(member.time));
    format!(
        "{} 1 {} {} {} {when} {}\n",
        mode_text(member),
        by(&owner.user_name, owner.user),
        by(&owner.group_name, owner.group),
        member.size,
        member.name
    )
}

/// What went wrong with the archive `name`, as pax says it.
fn faulted(name: &str, fault: &Fault) -> String {
    match fault {
        Fault::Header => format!("pax: {name}: holds a header that is none\n"),
        Fault::Number => format!("pax: {name}: a header holds a number that is none\n"),
        Fault::Cut => format!("pax: {name}: ends inside a member\n"),
        Fault::NameTooLong(member) => format!("pax: {member}: too long a name for ustar\n"),
    }
}

/// The next item of the archive, reading on as it wants; none at its end.
fn next_item(
    files: &mut impl Files,
    reader: &mut Reader,
    ended: &mut bool,
) -> Result<Option<Item>, String> {
    loop {
        match reader.next(*ended) {
            Ok(Some(Item::End)) => return Ok(None),
            Ok(Some(item)) => return Ok(Some(item)),
            Ok(None) if *ended => return Ok(None),
            Ok(None) => match files.archive() {
                Ok(piece) if piece.is_empty() => *ended = true,
                Ok(piece) => reader.feed(&piece),
                Err(e) => return Err(text::trouble("pax", "the archive", e)),
            },
            Err(fault) => return Err(faulted("the archive", &fault)),
        }
    }
}

/// Where the data of the member at hand goes, in read mode.
enum Into {
    Nowhere,
    File(String),
}

/// Lists the archive's members, or reads them out, as `asked` says;
/// whether everything went.
fn read_archive(files: &mut impl Files, asked: &Asked) -> bool {
    let mut picker = Picker::new(asked);
    let mut reader = Reader::new();
    let mut ended = false;
    let mut went = true;
    let mut into = Into::Nowhere;
    loop {
        let item = match next_item(files, &mut reader, &mut ended) {
            Ok(item) => item,
            Err(why) => {
                files.say(&why);
                went = false;
                None
            }
        };
        match item {
            Some(Item::Data(data)) => {
                if let Into::File(path) = &into
                    && let Err(e) = files.write(&data)
                {
                    files.say(&text::trouble("pax", path, e));
                    went = false;
                    let _ = files.done();
                    into = Into::Nowhere;
                }
            }
            Some(Item::Member(member)) => {
                went &= closed(files, &mut into);
                if picker.picks(&member.name) {
                    went &= begun(files, asked, &member, &mut into);
                }
            }
            Some(Item::End) | None => {
                went &= closed(files, &mut into);
                break;
            }
        }
    }
    // What comes after the end is read too, as tar reads whole records,
    // so that the pax writing into a pipe gets to its end.
    while !ended && files.archive().is_ok_and(|piece| !piece.is_empty()) {}
    if !asked.picking.others {
        let unfound: Vec<String> = picker.unfound().map(String::from).collect();
        for written in &unfound {
            files.say(&format!("pax: {written}: picks no member\n"));
        }
        went &= unfound.is_empty();
    }
    went
}

/// Ends the file the last member went into; whether it closed.
fn closed(files: &mut impl Files, into: &mut Into) -> bool {
    let Into::File(path) = core::mem::replace(into, Into::Nowhere) else {
        return true;
    };
    match files.done() {
        Ok(()) => true,
        Err(e) => {
            files.say(&text::trouble("pax", &path, e));
            false
        }
    }
}

/// Lists the picked `member`, or makes what it is; whether that went.
fn begun(files: &mut impl Files, asked: &Asked, member: &Member, into: &mut Into) -> bool {
    let name = member.name.as_str();
    if asked.mode == Mode::List {
        if asked.verbose {
            files.print(&listed(member));
        } else {
            files.print(&format!("{name}\n"));
        }
        return true;
    }
    if asked.verbose {
        files.say(&format!("{name}\n"));
    }
    if climbs(name) {
        files.say(&format!(
            "pax: {name}: climbs out with .., so it is passed over\n"
        ));
        return false;
    }
    match member.kind {
        Kind::Directory => {
            make_above(files, name);
            match files.make_directory(name) {
                Ok(()) | Err(Error::Exists) => true,
                Err(e) => {
                    files.say(&text::trouble("pax", name, e));
                    false
                }
            }
        }
        Kind::File => {
            if asked.keep && files.look(name).is_ok() {
                return true;
            }
            make_above(files, name);
            match files.create(name) {
                Ok(()) => {
                    *into = Into::File(String::from(name));
                    true
                }
                Err(e) => {
                    files.say(&text::trouble("pax", name, e));
                    false
                }
            }
        }
        Kind::Other(_) => {
            files.say(&format!(
                "pax: {name}: Meridian has no links, devices or pipes, so it is passed over\n"
            ));
            false
        }
    }
}

/// The paths to write or copy: the operands, or else the lines of the
/// standard input.
fn paths(files: &mut impl Files, operands: &[String]) -> Result<Vec<String>, String> {
    if !operands.is_empty() {
        return Ok(operands.to_vec());
    }
    let mut lines = Vec::new();
    loop {
        match files.line() {
            Ok(Some(line)) if line.is_empty() => {}
            Ok(Some(line)) => lines.push(line),
            Ok(None) => return Ok(lines),
            Err(e) => return Err(text::trouble("pax", "the standard input", e)),
        }
    }
}

/// Writes an archive of the files `asked` names; whether everything went.
fn write_archive(files: &mut impl Files, asked: &Asked) -> bool {
    let mut writer = Writer::new(asked.format);
    let mut went = true;
    let paths = match paths(files, asked.operands) {
        Ok(paths) => paths,
        Err(why) => {
            files.say(&why);
            went = false;
            Vec::new()
        }
    };
    for path in &paths {
        for (found, look) in walked(files, path, asked.picking.itself) {
            went &= written(files, asked, &mut writer, &found, look);
        }
    }
    went & put(files, &writer.finish())
}

/// Writes `bytes` of the archive; whether they went.
fn put(files: &mut impl Files, bytes: &[u8]) -> bool {
    match files.put_archive(bytes) {
        Ok(()) => true,
        Err(e) => {
            files.say(&text::trouble("pax", "the archive", e));
            false
        }
    }
}

/// Writes the member of the file `path` into the archive; whether it went.
fn written(
    files: &mut impl Files,
    asked: &Asked,
    writer: &mut Writer,
    path: &str,
    look: Result<Look, Error>,
) -> bool {
    let look = match look {
        Ok(look) => look,
        Err(e) => {
            files.say(&text::trouble("pax", path, e));
            return false;
        }
    };
    let (kind, size, mode) = match look {
        Look::Directory => (Kind::Directory, 0, 0o755),
        Look::File(size) => (Kind::File, size, 0o644),
    };
    // What cannot be read at all gets no member.
    let mut room = alloc::vec![0u8; 4096];
    let mut held = match look {
        Look::File(_) => match files.read(path, 0, &mut room) {
            Ok(len) => len,
            Err(e) => {
                files.say(&text::trouble("pax", path, e));
                return false;
            }
        },
        Look::Directory => 0,
    };
    if asked.verbose {
        files.say(&format!("{path}\n"));
    }
    let member = Member {
        name: String::from(path),
        kind,
        size,
        mode,
        time: 0,
        owner: Owner::default(),
    };
    let header = match writer.begin(&member) {
        Ok(header) => header,
        Err(fault) => {
            files.say(&faulted(path, &fault));
            return false;
        }
    };
    let mut went = put(files, &header);
    let mut at = 0;
    let mut short = None;
    while at < size {
        let left = usize::try_from(size - at).unwrap_or(usize::MAX);
        if held == 0 {
            // The header said the size: zeros make up what did not come,
            // so the archive goes on whole.
            let why = short.unwrap_or_else(|| format!("pax: {path}: ended short of its size\n"));
            files.say(&why);
            room.fill(0);
            let mut zeros = left;
            while zeros > 0 {
                let take = zeros.min(room.len());
                writer.wrote(take);
                put(files, &room[..take]);
                zeros -= take;
            }
            went = false;
            break;
        }
        let take = held.min(left);
        writer.wrote(take);
        went &= put(files, &room[..take]);
        at += take as u64;
        if at < size {
            held = files.read(path, at, &mut room).unwrap_or_else(|e| {
                short = Some(text::trouble("pax", path, e));
                0
            });
        }
    }
    went & put(files, &writer.end(size))
}

/// Copies the files `asked` names into its directory; whether everything
/// went.
fn copy_all(files: &mut impl Files, asked: &Asked) -> bool {
    let Some((target, sources)) = asked.operands.split_last() else {
        return false;
    };
    if files.look(target) != Ok(Look::Directory) {
        files.say(&format!("pax: {target}: is no directory to copy into\n"));
        return false;
    }
    let mut went = true;
    let sources = match paths(files, sources) {
        Ok(paths) => paths,
        Err(why) => {
            files.say(&why);
            went = false;
            Vec::new()
        }
    };
    let into = files.compared(target);
    for source in &sources {
        let from = files.compared(source);
        if !asked.picking.itself && (into == from || into.starts_with(&joined(&from, ""))) {
            files.say(&format!(
                "pax: {source} holds {target}, so it is not copied into it\n"
            ));
            went = false;
            continue;
        }
        for (path, look) in walked(files, source, asked.picking.itself) {
            went &= copied(files, asked, target, &path, look);
        }
    }
    went
}

/// Copies the file `path` into the directory `target`; whether it went.
fn copied(
    files: &mut impl Files,
    asked: &Asked,
    target: &str,
    path: &str,
    look: Result<Look, Error>,
) -> bool {
    let look = match look {
        Ok(look) => look,
        Err(e) => {
            files.say(&text::trouble("pax", path, e));
            return false;
        }
    };
    if asked.verbose {
        files.say(&format!("{path}\n"));
    }
    if climbs(path) {
        files.say(&format!(
            "pax: {path}: climbs out with .., so it is not copied\n"
        ));
        return false;
    }
    let to = joined(target, path.trim_start_matches('/'));
    make_above(files, &to);
    let size = match look {
        Look::Directory => {
            return match files.make_directory(&to) {
                Ok(()) | Err(Error::Exists) => true,
                Err(e) => {
                    files.say(&text::trouble("pax", &to, e));
                    false
                }
            };
        }
        Look::File(size) => size,
    };
    if asked.keep && files.look(&to).is_ok() {
        return true;
    }
    if let Err(e) = files.create(&to) {
        files.say(&text::trouble("pax", &to, e));
        return false;
    }
    let mut room = alloc::vec![0u8; 4096];
    let mut at = 0;
    let mut went = true;
    while at < size {
        match files.read(path, at, &mut room) {
            Ok(0) => break,
            Ok(len) => {
                if let Err(e) = files.write(&room[..len]) {
                    files.say(&text::trouble("pax", &to, e));
                    went = false;
                    break;
                }
                at += len as u64;
            }
            Err(e) => {
                files.say(&text::trouble("pax", path, e));
                went = false;
                break;
            }
        }
    }
    match files.done() {
        Ok(()) => went,
        Err(e) => {
            files.say(&text::trouble("pax", &to, e));
            false
        }
    }
}

/// Does what `asked` says; whether everything went.
fn run(files: &mut impl Files, asked: &Asked) -> bool {
    match asked.mode {
        Mode::List | Mode::Read => read_archive(files, asked),
        Mode::Write => write_archive(files, asked),
        Mode::Copy => copy_all(files, asked),
    }
}

/// Where the archive goes when one is written.
enum Out {
    Standard,
    File(Handle, u64),
}

/// Meridian's files and the program's streams.
struct System {
    archive: Option<Source>,
    out: Out,
    standard: text::Out<text::Write>,
    lines: Option<LineReader<Source>>,
    /// The file `read` read last, kept open for the next piece of it.
    reading: Option<(String, Handle)>,
    writing: Option<(Handle, u64)>,
}

impl System {
    /// The streams `asked` wants, its archive's file opened.
    fn new(asked: &Asked) -> Result<System, String> {
        let name = asked.archive.unwrap_or("-");
        let trouble = |e| text::trouble("pax", name, e);
        let (archive, out) = match asked.mode {
            Mode::List | Mode::Read => (Some(Source::open(name).map_err(trouble)?), Out::Standard),
            Mode::Write if name != "-" => (
                None,
                Out::File(
                    program::open_with(name, WRITE, CREATE_ALWAYS).map_err(trouble)?,
                    0,
                ),
            ),
            Mode::Write | Mode::Copy => (None, Out::Standard),
        };
        Ok(System {
            archive,
            out,
            standard: text::standard_output(),
            lines: None,
            reading: None,
            writing: None,
        })
    }

    /// Closes what is open and writes what is held; whether every write
    /// went.
    fn finish(mut self) -> bool {
        if let Some((_, file)) = self.reading.take() {
            let _ = program::close(file);
        }
        if let Some(archive) = self.archive.take() {
            archive.close();
        }
        let closed = match self.out {
            Out::File(file, _) => program::close(file).is_ok(),
            Out::Standard => true,
        };
        self.standard.finish() && closed
    }
}

impl Files for System {
    fn look(&mut self, path: &str) -> Result<Look, Error> {
        let file = program::open(path, READ)?;
        let info = program::info(file);
        let _ = program::close(file);
        let info = info?;
        Ok(if info.directory {
            Look::Directory
        } else {
            Look::File(info.size)
        })
    }

    fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
        let directory = program::open(path, READ)?;
        let mut names = Vec::new();
        let mut room = [0u8; 1024];
        let mut index = 0;
        let listed = loop {
            match program::list(directory, index, &mut room) {
                Ok((len, _)) => names.push(String::from_utf8_lossy(&room[..len]).into_owned()),
                Err(Error::NotFound) => break Ok(names),
                Err(e) => break Err(e),
            }
            index += 1;
        };
        let _ = program::close(directory);
        listed
    }

    fn read(&mut self, path: &str, at: u64, room: &mut [u8]) -> Result<usize, Error> {
        let file = match &self.reading {
            Some((open, file)) if open == path => *file,
            _ => {
                if let Some((_, file)) = self.reading.take() {
                    let _ = program::close(file);
                }
                let file = program::open(path, READ)?;
                self.reading = Some((String::from(path), file));
                file
            }
        };
        program::read(file, at, room)
    }

    fn create(&mut self, path: &str) -> Result<(), Error> {
        self.writing = Some((program::open_with(path, WRITE, CREATE_ALWAYS)?, 0));
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let Some((file, at)) = &mut self.writing else {
            return Err(Error::BadHandle);
        };
        program::write_file(*file, *at, bytes)?;
        *at += bytes.len() as u64;
        Ok(())
    }

    fn done(&mut self) -> Result<(), Error> {
        match self.writing.take() {
            Some((file, _)) => program::close(file),
            None => Ok(()),
        }
    }

    fn make_directory(&mut self, path: &str) -> Result<(), Error> {
        program::make_directory(path)
    }

    fn compared(&mut self, path: &str) -> String {
        program::full_path(path).to_uppercase()
    }

    fn archive(&mut self) -> Result<Vec<u8>, Error> {
        let mut piece = Vec::new();
        if let Some(archive) = &mut self.archive {
            archive.more(&mut piece)?;
        }
        Ok(piece)
    }

    fn put_archive(&mut self, bytes: &[u8]) -> Result<(), Error> {
        match &mut self.out {
            Out::Standard => self.standard.put(bytes),
            Out::File(file, at) => {
                program::write_file(*file, *at, bytes)?;
                *at += bytes.len() as u64;
            }
        }
        Ok(())
    }

    fn line(&mut self) -> Result<Option<String>, Error> {
        let lines = self
            .lines
            .get_or_insert_with(|| LineReader::new(Source::Input));
        Ok(lines
            .line()?
            .map(|line| String::from_utf8_lossy(&line).into_owned()))
    }

    fn print(&mut self, text: &str) {
        self.standard.put(text.as_bytes());
    }

    fn say(&mut self, text: &str) {
        self.standard.flush();
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn pax(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut system = match System::new(&asked) {
        Ok(system) => system,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let went = run(&mut system, &asked);
    let finished = system.finish();
    u64::from(!(went && finished))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{Read as _, Seek as _, Write as _};
    use std::path::{Path, PathBuf};

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// The parts of `path` that name something, `..` taking back the one
    /// before. One that climbs above where it starts stops the test, so
    /// that no mistake in pax reaches the host's own files.
    fn parts(path: &str) -> Vec<&str> {
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts
                        .pop()
                        .unwrap_or_else(|| panic!("{path} climbs out of the test's directory"));
                }
                _ => parts.push(part),
            }
        }
        parts
    }

    fn confined(root: &Path, path: &str) -> PathBuf {
        parts(path)
            .iter()
            .fold(root.to_path_buf(), |at, part| at.join(part))
    }

    /// What a host test does to a file once pax read from it, as another
    /// program might while pax reads.
    enum Change {
        /// Cuts it to this many bytes.
        Cut(u64),
        /// Takes away the right to open it.
        Shut,
    }

    /// Files among the host's temporary ones, gone once dropped, and the
    /// streams: the archive read in pieces, the one written, the lines of
    /// the standard input, and what pax printed and said.
    struct Host {
        root: PathBuf,
        pieces: VecDeque<Vec<u8>>,
        archive: Vec<u8>,
        lines: VecDeque<String>,
        printed: String,
        said: String,
        writing: Option<std::fs::File>,
        change: Option<(String, Change)>,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-pax-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                pieces: VecDeque::new(),
                archive: Vec::new(),
                lines: VecDeque::new(),
                printed: String::new(),
                said: String::new(),
                writing: None,
                change: None,
            }
        }

        /// A file at `path` holding `bytes`, the directories above it made.
        fn put(&self, path: &str, bytes: &[u8]) {
            let at = confined(&self.root, path);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, bytes).unwrap();
        }

        fn directory(&self, path: &str) {
            std::fs::create_dir_all(confined(&self.root, path)).unwrap();
        }

        fn holds(&self, path: &str) -> Option<Vec<u8>> {
            std::fs::read(confined(&self.root, path)).ok()
        }

        /// The archive `bytes`, to be read in pieces of `piece`.
        fn reading(&mut self, bytes: &[u8], piece: usize) {
            self.pieces = bytes.chunks(piece).map(<[u8]>::to_vec).collect();
        }

        /// Every path under the root, in order, directories with a `/`.
        fn left(&self) -> Vec<String> {
            fn walk(at: &Path, from: &Path, all: &mut Vec<String>) {
                for entry in std::fs::read_dir(at).unwrap() {
                    let path = entry.unwrap().path();
                    let name = path
                        .strip_prefix(from)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    if path.is_dir() {
                        all.push(name + "/");
                        walk(&path, from, all);
                    } else {
                        all.push(name);
                    }
                }
            }
            let mut all = Vec::new();
            walk(&self.root, &self.root, &mut all);
            all.sort();
            all
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            std::io::ErrorKind::AlreadyExists => Error::Exists,
            std::io::ErrorKind::NotADirectory => Error::NotADirectory,
            std::io::ErrorKind::IsADirectory => Error::NotAFile,
            std::io::ErrorKind::PermissionDenied => Error::Denied,
            _ => Error::Unwritable,
        }
    }

    impl Files for Host {
        fn look(&mut self, path: &str) -> Result<Look, Error> {
            let found = std::fs::metadata(confined(&self.root, path)).map_err(|e| error(&e))?;
            Ok(if found.is_dir() {
                Look::Directory
            } else {
                Look::File(found.len())
            })
        }

        fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
            std::fs::read_dir(confined(&self.root, path))
                .map_err(|e| error(&e))?
                .map(|entry| {
                    entry
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .map_err(|e| error(&e))
                })
                .collect()
        }

        fn read(&mut self, path: &str, at: u64, room: &mut [u8]) -> Result<usize, Error> {
            let at_path = confined(&self.root, path);
            let mut file = std::fs::File::open(&at_path).map_err(|e| error(&e))?;
            file.seek(std::io::SeekFrom::Start(at)).unwrap();
            let len = file.read(room).map_err(|e| error(&e))?;
            if self
                .change
                .as_ref()
                .is_some_and(|(changed, _)| changed == path)
            {
                match self.change.take().map(|(_, change)| change) {
                    Some(Change::Cut(size)) => file_cut(&at_path, size),
                    Some(Change::Shut) => shut(&at_path, true),
                    None => {}
                }
            }
            Ok(len)
        }

        fn create(&mut self, path: &str) -> Result<(), Error> {
            self.writing =
                Some(std::fs::File::create(confined(&self.root, path)).map_err(|e| error(&e))?);
            Ok(())
        }

        fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
            self.writing
                .as_mut()
                .ok_or(Error::BadHandle)?
                .write_all(bytes)
                .map_err(|e| error(&e))
        }

        fn done(&mut self) -> Result<(), Error> {
            self.writing.take().ok_or(Error::BadHandle).map(drop)
        }

        fn make_directory(&mut self, path: &str) -> Result<(), Error> {
            std::fs::create_dir(confined(&self.root, path)).map_err(|e| error(&e))
        }

        fn compared(&mut self, path: &str) -> String {
            format!("/{}", parts(path).join("/"))
        }

        fn archive(&mut self) -> Result<Vec<u8>, Error> {
            Ok(self.pieces.pop_front().unwrap_or_default())
        }

        fn put_archive(&mut self, bytes: &[u8]) -> Result<(), Error> {
            self.archive.extend_from_slice(bytes);
            Ok(())
        }

        fn line(&mut self) -> Result<Option<String>, Error> {
            Ok(self.lines.pop_front())
        }

        fn print(&mut self, text: &str) {
            self.printed.push_str(text);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    fn file_cut(path: &Path, size: u64) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(size)
            .unwrap();
    }

    /// Takes the rights to `path` away, or gives them back.
    fn shut(path: &Path, closed: bool) {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = match (closed, path.is_dir()) {
            (true, _) => 0o000,
            (false, true) => 0o755,
            (false, false) => 0o644,
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Runs pax with the words `line` on `host`; whether everything went.
    fn ran(host: &mut Host, line: &str) -> bool {
        let words = words(line);
        let asked = asked(&words).unwrap();
        run(host, &asked)
    }

    /// What the host's `program` writes for `words` and `input`, run in
    /// `dir`, times in UTC.
    fn tool(dir: &Path, program: &str, words: &[&str], input: &[u8]) -> Vec<u8> {
        let mut child = std::process::Command::new(program)
            .args(words)
            .env("TZ", "UTC")
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

    fn text_of(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap()
    }

    fn member(name: &str, kind: Kind, size: u64, mode: u32) -> Member {
        Member {
            name: name.into(),
            kind,
            size,
            mode,
            time: 0,
            owner: Owner::default(),
        }
    }

    /// An archive of `format` holding `members` and their data.
    fn archive_of(format: Format, members: &[(Member, &[u8])]) -> Vec<u8> {
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

    #[test]
    fn its_words_are_options_and_their_values_then_operands() {
        let line = words("-rv -f a.tar -xcpio -b 10k -p e x y");
        let got = asked(&line).unwrap();
        assert_eq!(
            (got.mode, got.archive, got.verbose, got.format),
            (Mode::Read, Some("a.tar"), true, Format::Cpio)
        );
        assert_eq!(got.operands, &line[8..]);
        assert_eq!(asked(&words("")).unwrap().mode, Mode::List);
        assert_eq!(asked(&words("-w")).unwrap().mode, Mode::Write);
        assert_eq!(asked(&words("-w")).unwrap().format, Format::Ustar);
        let line = words("-r -w d");
        let got = asked(&line).unwrap();
        assert_eq!((got.mode, got.operands), (Mode::Copy, &line[2..]));
        assert_eq!(asked(&words("-rw d")).unwrap().mode, Mode::Copy);
        let line = words("-dcnk");
        let got = asked(&line).unwrap();
        assert_eq!(
            (got.picking, got.keep, got.verbose),
            (
                Picking {
                    itself: true,
                    others: true,
                    first: true
                },
                true,
                false
            )
        );
        assert_eq!(asked(&words("-fa.tar")).unwrap().archive, Some("a.tar"));
        let line = words("-vfa");
        let got = asked(&line).unwrap();
        assert_eq!(
            (got.verbose, got.archive),
            (true, Some("a")),
            "f takes the rest"
        );
        let line = words("-- -x");
        assert_eq!(asked(&line).unwrap().operands, &line[1..]);
        let line = words("x -r");
        let got = asked(&line).unwrap();
        assert_eq!((got.mode, got.operands), (Mode::List, &line[..]));
        assert_eq!(
            asked(&words("-H -L -l -t")).unwrap(),
            asked(&words("")).unwrap()
        );
        for fine in ["-b 512b", "-b 0010", "-b 1", "-p aemop", "-x ustar"] {
            assert!(asked(&words(fine)).is_ok(), "{fine}");
        }
    }

    #[test]
    fn words_it_cannot_take_are_said() {
        for (line, why) in [
            ("-q", "pax: -q is no option\n"),
            ("-rq", "pax: -q is no option\n"),
            ("-f", "pax: -f wants a value after it\n"),
            ("-v -x", "pax: -x wants a value after it\n"),
            ("-x tar", "pax: -x tar: writes ustar and cpio\n"),
            ("-b 0", "pax: -b 0: a size of blocks is a number above 0\n"),
            (
                "-b 0b",
                "pax: -b 0b: a size of blocks is a number above 0\n",
            ),
            ("-b k", "pax: -b k: a size of blocks is a number above 0\n"),
            (
                "-b 1x",
                "pax: -b 1x: a size of blocks is a number above 0\n",
            ),
            ("-p q", "pax: -p q: keeps a, e, m, o or p\n"),
            (
                "-rw",
                "pax: -r -w copies files into a directory, which comes last\n",
            ),
            ("-rw -f a d", "pax: -r -w copies with no archive for -f\n"),
        ] {
            assert_eq!(asked(&words(line)), Err(String::from(why)), "{line}");
        }
    }

    #[test]
    fn modes_read_as_ls_writes_them() {
        for (kind, mode, text) in [
            (Kind::File, 0o644, "-rw-r--r--"),
            (Kind::Directory, 0o755, "drwxr-xr-x"),
            (Kind::File, 0o4755, "-rwsr-xr-x"),
            (Kind::File, 0o4644, "-rwSr--r--"),
            (Kind::File, 0o2755, "-rwxr-sr-x"),
            (Kind::File, 0o2745, "-rwxr-Sr-x"),
            (Kind::Directory, 0o1777, "drwxrwxrwt"),
            (Kind::Directory, 0o1776, "drwxrwxrwT"),
            (Kind::File, 0, "----------"),
            (Kind::File, 0o421, "-r---w---x"),
            (Kind::Other(b'1'), 0o644, "-rw-r--r--"),
            (Kind::Other(b'2'), 0o777, "lrwxrwxrwx"),
            (Kind::Other(0o12), 0o777, "lrwxrwxrwx"),
            (Kind::Other(b'3'), 0o600, "crw-------"),
            (Kind::Other(0o02), 0o600, "crw-------"),
            (Kind::Other(b'4'), 0o600, "brw-------"),
            (Kind::Other(0o06), 0o600, "brw-------"),
            (Kind::Other(b'6'), 0o600, "prw-------"),
            (Kind::Other(0o01), 0o600, "prw-------"),
            (Kind::Other(0o14), 0o600, "srw-------"),
            (Kind::Other(b'x'), 0o600, "?rw-------"),
        ] {
            assert_eq!(
                mode_text(&member("m", kind, 0, mode)),
                text,
                "{kind:?} {mode:o}"
            );
        }
    }

    #[test]
    fn a_long_listing_has_ls_s_fields() {
        let mut named = member("a", Kind::File, 6, 0o644);
        named.time = 1_700_000_000;
        named.owner = Owner {
            user: 1000,
            group: 100,
            user_name: "elchi".into(),
            group_name: "users".into(),
        };
        assert_eq!(
            listed(&named),
            "-rw-r--r-- 1 elchi users 6 Nov 14  2023 a\n"
        );
        let mut numbered = member("d/x", Kind::File, 0, 0o600);
        numbered.time = 3 * 86_400 + 86_399;
        numbered.owner.user = 1000;
        numbered.owner.group = 100;
        assert_eq!(
            listed(&numbered),
            "-rw------- 1 1000 100 0 Jan  4  1970 d/x\n"
        );
        let mut december = member("z", Kind::Directory, 0, 0o755);
        december.time = 10_956 * 86_400;
        assert_eq!(listed(&december), "drwxr-xr-x 1 0 0 0 Dec 31  1999 z\n");
        // A link, its owner named and its group not.
        let mut link = member("to", Kind::Other(b'2'), 0, 0o777);
        link.owner.user_name = "root".into();
        link.owner.group = 7;
        assert_eq!(
            listed(&link),
            "lrwxrwxrwx 1 root 7 0 Jan  1  1970 to\n",
            "each by its name, or its number without one"
        );
        // Late on a leap day: the date is the day's, whatever its hour.
        let mut leap_day = member("l", Kind::File, 12, 0o644);
        leap_day.time = 1_709_164_800 + 86_399;
        assert_eq!(listed(&leap_day), "-rw-r--r-- 1 0 0 12 Feb 29  2024 l\n");
    }

    /// The names of `names` that the picking `line` asks for picks, and
    /// the patterns that picked nothing.
    fn picked<'n>(line: &str, names: &[&'n str]) -> (Vec<&'n str>, Vec<String>) {
        let line = words(line);
        let mut picker = Picker::new(&asked(&line).unwrap());
        let chosen = names.iter().copied().filter(|n| picker.picks(n)).collect();
        (chosen, picker.unfound().map(String::from).collect())
    }

    #[test]
    fn patterns_pick_members_and_what_their_directories_hold() {
        let names = ["a", "a/x", "b", "ab", "a/y/z"];
        assert_eq!(picked("", &names).0, names, "no pattern picks every one");
        assert_eq!(picked("a", &names).0, ["a", "a/x", "a/y/z"]);
        assert_eq!(picked("-d a", &names).0, ["a"]);
        assert_eq!(picked("-c a", &names).0, ["b", "ab"]);
        assert_eq!(picked("-c a", &names).1, Vec::<String>::new());
        assert_eq!(
            picked("*.txt", &["x.txt", "d/y.txt", "d/z"]).0,
            ["x.txt", "d/y.txt"],
            "* takes a slash"
        );
        assert_eq!(picked("d", &["d/x", "d/y"]).0, ["d/x", "d/y"]);
        assert_eq!(
            picked("-d d", &["d/x", "d/y"]),
            (Vec::new(), vec!["d".into()])
        );
        assert_eq!(
            picked("-n a*", &["ab", "a/x", "ac", "ab/q"]).0,
            ["ab", "ab/q"],
            "the first and what it holds"
        );
        assert_eq!(picked("-n d", &["d/x", "d/y", "e", "d"]).0, ["d/x", "d/y"]);
        assert_eq!(picked("-n -d a", &["a", "a/x", "a"]).0, ["a"]);
        assert_eq!(
            picked("a zz q*", &["a", "b"]),
            (vec!["a"], vec!["zz".into(), "q*".into()])
        );
        assert_eq!(picked("\\*", &["*", "x"]).0, ["*"]);
        assert!(pattern("a\\").matches("a\\"), "a backslash at the end");
    }

    #[test]
    fn paths_walk_down_in_the_order_of_their_names() {
        let mut host = Host::new("walk");
        host.put("d/b", b"bb");
        host.put("d/a", b"a");
        host.put("d/e/f", b"fff");
        host.put("x", b"");
        let walked_from = |host: &mut Host, path: &str, itself: bool| walked(host, path, itself);
        assert_eq!(
            walked_from(&mut host, "d", false),
            [
                (String::from("d"), Ok(Look::Directory)),
                ("d/a".into(), Ok(Look::File(1))),
                ("d/b".into(), Ok(Look::File(2))),
                ("d/e".into(), Ok(Look::Directory)),
                ("d/e/f".into(), Ok(Look::File(3))),
            ]
        );
        assert_eq!(
            walked_from(&mut host, "d/", true),
            [(String::from("d/"), Ok(Look::Directory))]
        );
        assert_eq!(
            walked_from(&mut host, "d/e/", false)[1],
            (String::from("d/e/f"), Ok(Look::File(3)))
        );
        assert_eq!(
            walked_from(&mut host, "nope", false),
            [(String::from("nope"), Err(Error::NotFound))]
        );
        host.directory("shut");
        let at = confined(&host.root, "shut");
        shut(&at, true);
        let found = walked_from(&mut host, "shut", false);
        shut(&at, false);
        assert_eq!(
            found,
            [
                (String::from("shut"), Ok(Look::Directory)),
                ("shut".into(), Err(Error::Denied)),
            ]
        );
        assert_eq!(above("a/b/c").collect::<Vec<_>>(), ["a", "a/b"]);
        assert_eq!(above("/a/b").collect::<Vec<_>>(), ["/a"]);
        assert_eq!(above("a").count(), 0);
        assert!(climbs("a/../b") && climbs("..") && !climbs("a/..b") && !climbs("a/b.."));
        assert_eq!(joined("a/", "b"), "a/b");
        assert_eq!(joined("a", "b"), "a/b");
    }

    #[test]
    fn write_mode_writes_what_tar_and_cpio_read() {
        let mut host = Host::new("write");
        host.put("d/a", b"one\n");
        let big: Vec<u8> = (0..9000u32).map(|i| (i % 251).to_le_bytes()[0]).collect();
        host.put("d/b/c", &big);
        host.put("top", b"");
        assert!(ran(&mut host, "-w d top"));
        assert_eq!(host.said, "");
        let tar = core::mem::take(&mut host.archive);
        assert_eq!(tar.len() % 10_240, 0, "whole records");
        let listed = text_of(tool(&host.root, "tar", &["-tvf", "-"], &tar));
        let fields: Vec<Vec<&str>> = listed
            .lines()
            .map(|l| l.split_whitespace().collect())
            .collect();
        assert_eq!(
            fields,
            [
                vec!["drwxr-xr-x", "0/0", "0", "1970-01-01", "00:00", "d/"],
                vec!["-rw-r--r--", "0/0", "4", "1970-01-01", "00:00", "d/a"],
                vec!["drwxr-xr-x", "0/0", "0", "1970-01-01", "00:00", "d/b/"],
                vec!["-rw-r--r--", "0/0", "9000", "1970-01-01", "00:00", "d/b/c"],
                vec!["-rw-r--r--", "0/0", "0", "1970-01-01", "00:00", "top"],
            ]
        );
        let out = host.root.join("out");
        std::fs::create_dir(&out).unwrap();
        tool(&out, "tar", &["-xf", "-"], &tar);
        assert_eq!(std::fs::read(out.join("d/a")).unwrap(), b"one\n");
        assert_eq!(std::fs::read(out.join("d/b/c")).unwrap(), big);
        std::fs::remove_dir_all(&out).unwrap();
        assert!(ran(&mut host, "-w -x cpio d/b top"));
        let cpio = core::mem::take(&mut host.archive);
        assert_eq!(cpio.len() % 512, 0);
        assert_eq!(
            text_of(tool(&host.root, "cpio", &["-it"], &cpio)),
            "d/b\nd/b/c\ntop\n"
        );
        let copied = std::env::temp_dir().join(format!("meridian-pax-cpio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&copied);
        std::fs::create_dir(&copied).unwrap();
        tool(&copied, "cpio", &["-id"], &cpio);
        assert_eq!(std::fs::read(copied.join("d/b/c")).unwrap(), big);
        std::fs::remove_dir_all(&copied).unwrap();
    }

    #[test]
    fn write_mode_takes_names_from_its_input_and_says_what_it_cannot() {
        let mut host = Host::new("names");
        host.put("d/a", b"x");
        host.put("top", b"t");
        host.lines = ["top", "", "d/a"].into_iter().map(String::from).collect();
        assert!(ran(&mut host, "-wv"));
        assert_eq!(host.said, "top\nd/a\n", "-v names each as it goes in");
        let tar = core::mem::take(&mut host.archive);
        assert_eq!(
            text_of(tool(&host.root, "tar", &["-tf", "-"], &tar)),
            "top\nd/a\n"
        );
        host.said.clear();
        assert!(ran(&mut host, "-wd d"));
        let tar = core::mem::take(&mut host.archive);
        assert_eq!(
            text_of(tool(&host.root, "tar", &["-tf", "-"], &tar)),
            "d/\n"
        );
        assert!(!ran(&mut host, "-w nope top"));
        assert_eq!(host.said, "pax: nope: no such file\n");
        let tar = core::mem::take(&mut host.archive);
        assert_eq!(
            text_of(tool(&host.root, "tar", &["-tf", "-"], &tar)),
            "top\n"
        );
        // A name of 101 bytes has no slash to cut it at for ustar.
        let long = "n".repeat(101);
        host.put(&long, b"l");
        host.said.clear();
        assert!(!ran(&mut host, &format!("-w {long} top")));
        assert_eq!(
            host.said,
            format!("pax: {long}: too long a name for ustar\n")
        );
        let tar = core::mem::take(&mut host.archive);
        assert_eq!(
            text_of(tool(&host.root, "tar", &["-tf", "-"], &tar)),
            "top\n"
        );
        assert!(ran(&mut host, &format!("-w -x cpio {long}")));
        let cpio = core::mem::take(&mut host.archive);
        assert_eq!(
            text_of(tool(&host.root, "cpio", &["-it"], &cpio)),
            format!("{long}\n")
        );
    }

    #[test]
    fn a_file_that_shrinks_or_shuts_while_read_is_made_up_with_zeros() {
        let mut host = Host::new("short");
        let big: Vec<u8> = (0..5000u32).map(|i| (i % 7 + 1).to_le_bytes()[0]).collect();
        host.put("big", &big);
        host.put("after", b"a");
        host.change = Some(("big".into(), Change::Cut(100)));
        assert!(!ran(&mut host, "-w big after"));
        assert_eq!(host.said, "pax: big: ended short of its size\n");
        let tar = core::mem::take(&mut host.archive);
        let out = host.root.join("out");
        std::fs::create_dir(&out).unwrap();
        tool(&out, "tar", &["-xf", "-"], &tar);
        let mut wanted = big[..4096].to_vec();
        wanted.resize(5000, 0);
        assert_eq!(std::fs::read(out.join("big")).unwrap(), wanted);
        assert_eq!(
            std::fs::read(out.join("after")).unwrap(),
            b"a",
            "and the archive goes on"
        );
        std::fs::remove_dir_all(&out).unwrap();
        host.put("big", &big);
        host.said.clear();
        host.change = Some(("big".into(), Change::Shut));
        assert!(!ran(&mut host, "-w big"));
        shut(&confined(&host.root, "big"), false);
        assert_eq!(host.said, "pax: big: not allowed\n", "said once");
        let read = archive_back(&host.archive);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].1.len(), 5000);
        assert!(read[0].1[4096..].iter().all(|&b| b == 0));
        // What cannot be read at all gets no member.
        host.archive.clear();
        host.said.clear();
        shut(&confined(&host.root, "big"), true);
        assert!(!ran(&mut host, "-w big after"));
        shut(&confined(&host.root, "big"), false);
        assert_eq!(host.said, "pax: big: not allowed\n");
        let names: Vec<String> = archive_back(&host.archive)
            .into_iter()
            .map(|(m, _)| m.name)
            .collect();
        assert_eq!(names, ["after"]);
    }

    /// The members of `archive` and their data, as lib/archive reads them.
    fn archive_back(archive: &[u8]) -> Vec<(Member, Vec<u8>)> {
        let mut reader = Reader::new();
        reader.feed(archive);
        let mut members: Vec<(Member, Vec<u8>)> = Vec::new();
        loop {
            match reader.next(true).unwrap() {
                Some(Item::Member(member)) => members.push((member, Vec::new())),
                Some(Item::Data(data)) => members.last_mut().unwrap().1.extend(data),
                Some(Item::End) | None => return members,
            }
        }
    }

    /// An archive GNU tar makes of `paths` in `dir`, with its owners and
    /// time fixed.
    fn tar_of(dir: &Path, paths: &[&str]) -> Vec<u8> {
        let mut words = vec![
            "--format=ustar",
            "--owner=elchi:1000",
            "--group=users:100",
            "--mtime=@1700000000",
            "--sort=name",
            "-cf",
            "-",
        ];
        words.extend_from_slice(paths);
        tool(dir, "tar", &words, b"")
    }

    #[test]
    fn list_mode_names_the_members_tar_wrote() {
        let mut host = Host::new("list");
        host.put("src/a", b"hello\n");
        host.put("src/d/x", b"xx");
        let tar = tar_of(&host.root.join("src"), &["a", "d"]);
        for piece in [1, 100, tar.len()] {
            host.reading(&tar, piece);
            host.printed.clear();
            assert!(ran(&mut host, ""));
            assert_eq!(host.printed, "a\nd\nd/x\n", "pieces of {piece}");
        }
        host.reading(&tar, 512);
        host.printed.clear();
        assert!(ran(&mut host, "-v"));
        assert_eq!(
            host.printed,
            "-rw-r--r-- 1 elchi users 6 Nov 14  2023 a\n\
             drwxr-xr-x 1 elchi users 0 Nov 14  2023 d\n\
             -rw-r--r-- 1 elchi users 2 Nov 14  2023 d/x\n"
        );
        assert_eq!(host.said, "", "listing says nothing on the error");
        host.reading(&tar, 512);
        host.printed.clear();
        assert!(!ran(&mut host, "d zz"));
        assert_eq!(host.printed, "d\nd/x\n");
        assert_eq!(host.said, "pax: zz: picks no member\n");
        assert_eq!(
            host.left(),
            ["src/", "src/a", "src/d/", "src/d/x"],
            "nothing made"
        );
        // The end comes before the record is whole; the rest is read.
        let ours = archive_of(Format::Ustar, &[(member("f", Kind::File, 1, 0o644), b"f")]);
        host.reading(&ours, 512);
        host.printed.clear();
        host.said.clear();
        assert!(ran(&mut host, ""));
        assert_eq!((host.printed.as_str(), host.said.as_str()), ("f\n", ""));
        assert!(host.pieces.is_empty(), "{} pieces left", host.pieces.len());
    }

    #[test]
    fn an_archive_that_does_not_read_is_said() {
        let mut host = Host::new("broken");
        host.reading(&[b'x'; 512], 512);
        assert!(!ran(&mut host, ""));
        assert_eq!(
            host.said, "pax: the archive: a header holds a number that is none\n",
            "its sum, which is read first"
        );
        let whole = archive_of(
            Format::Ustar,
            &[(member("f", Kind::File, 600, 0o644), &[b'f'; 600])],
        );
        host.said.clear();
        host.reading(&whole[..700], 64);
        assert!(!ran(&mut host, ""));
        assert_eq!(host.printed, "f\n");
        assert_eq!(host.said, "pax: the archive: ends inside a member\n");
        let mut numbered = whole.clone();
        numbered[124] = b'9';
        host.said.clear();
        host.reading(&numbered, 512);
        assert!(!ran(&mut host, ""));
        assert_eq!(
            host.said, "pax: the archive: holds a header that is none\n",
            "its sum no longer fits"
        );
        host.said.clear();
        host.printed.clear();
        host.pieces.clear();
        assert!(ran(&mut host, ""), "no archive at all is an empty one");
        assert_eq!((host.printed.as_str(), host.said.as_str()), ("", ""));
    }

    #[test]
    fn read_mode_makes_the_files_and_directories_members_name() {
        let mut host = Host::new("read");
        let archive = archive_of(
            Format::Ustar,
            &[
                (member("d", Kind::Directory, 0, 0o755), b""),
                (member("d/a", Kind::File, 3, 0o644), b"aaa"),
                (member("e/f/g", Kind::File, 1, 0o644), b"g"),
                (member("d/empty", Kind::File, 0, 0o644), b""),
            ],
        );
        host.reading(&archive, 100);
        assert!(ran(&mut host, "-rv"));
        assert_eq!(host.said, "d\nd/a\ne/f/g\nd/empty\n");
        assert_eq!(host.left(), ["d/", "d/a", "d/empty", "e/", "e/f/", "e/f/g"]);
        assert_eq!(host.holds("d/a").unwrap(), b"aaa");
        assert_eq!(host.holds("d/empty").unwrap(), b"");
        // Again, over what is there, and with -k keeping it.
        host.put("d/a", b"old");
        host.reading(&archive, 100);
        assert!(ran(&mut host, "-r -k"));
        assert_eq!(host.holds("d/a").unwrap(), b"old");
        host.reading(&archive, 100);
        assert!(ran(&mut host, "-r"));
        assert_eq!(host.holds("d/a").unwrap(), b"aaa");
        // Patterns pick what is read out.
        let mut other = Host::new("read-picked");
        other.reading(&archive, 512);
        assert!(ran(&mut other, "-r e"));
        assert_eq!(other.left(), ["e/", "e/f/", "e/f/g"]);
    }

    #[test]
    fn read_mode_reads_what_tar_and_cpio_wrote() {
        let host = Host::new("theirs");
        host.put("src/a", b"hello\n");
        host.put("src/d/x", b"xx");
        let tar = tar_of(&host.root.join("src"), &["a", "d"]);
        let cpio = tool(
            &host.root.join("src"),
            "cpio",
            &["-o", "-H", "odc"],
            b"a\nd\nd/x\n",
        );
        for (name, archive) in [("tar", tar), ("cpio", cpio)] {
            let mut into = Host::new(&format!("theirs-{name}"));
            into.reading(&archive, 333);
            assert!(ran(&mut into, "-r"), "{name}: {}", into.said);
            assert_eq!(into.left(), ["a", "d/", "d/x"], "{name}");
            assert_eq!(into.holds("a").unwrap(), b"hello\n", "{name}");
            assert_eq!(into.holds("d/x").unwrap(), b"xx", "{name}");
        }
    }

    #[test]
    fn read_mode_passes_over_names_that_climb_and_kinds_meridian_has_not() {
        let mut host = Host::new("climb");
        host.directory("in");
        let archive = archive_of(
            Format::Ustar,
            &[
                (member("../x", Kind::File, 1, 0o644), b"x"),
                (member("a/../../y", Kind::Directory, 0, 0o755), b""),
                (member("ok", Kind::File, 2, 0o644), b"ok"),
            ],
        );
        host.reading(&archive, 512);
        assert!(!ran(&mut host, "-r"));
        assert_eq!(
            host.said,
            "pax: ../x: climbs out with .., so it is passed over\n\
             pax: a/../../y: climbs out with .., so it is passed over\n"
        );
        assert_eq!(host.left(), ["in/", "ok"]);
        let linked = Host::new("links");
        linked.put("src/a", b"a");
        std::os::unix::fs::symlink("a", linked.root.join("src/l")).unwrap();
        let tar = tar_of(&linked.root.join("src"), &["l", "a"]);
        let mut into = Host::new("links-into");
        into.reading(&tar, 512);
        assert!(!ran(&mut into, "-r"));
        assert_eq!(
            into.said,
            "pax: l: Meridian has no links, devices or pipes, so it is passed over\n"
        );
        assert_eq!(into.left(), ["a"]);
    }

    #[test]
    fn read_mode_says_what_it_cannot_make_and_goes_on() {
        let mut host = Host::new("unmade");
        host.directory("ro");
        host.put("file", b"f");
        let archive = archive_of(
            Format::Ustar,
            &[
                (member("ro/x", Kind::File, 3, 0o644), b"xxx"),
                (member("ro/d", Kind::Directory, 0, 0o755), b""),
                (member("file/y", Kind::File, 1, 0o644), b"y"),
                (member("ro", Kind::Directory, 0, 0o755), b""),
                (member("after", Kind::File, 1, 0o644), b"z"),
            ],
        );
        let ro = confined(&host.root, "ro");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        host.reading(&archive, 512);
        let went = ran(&mut host, "-r");
        shut(&ro, false);
        assert!(!went);
        assert_eq!(
            host.said,
            "pax: ro/x: not allowed\npax: ro/d: not allowed\npax: file/y: could not be read\n"
        );
        assert_eq!(host.left(), ["after", "file", "ro/"]);
        assert_eq!(host.holds("after").unwrap(), b"z");
    }

    #[test]
    fn copy_mode_copies_files_and_directories_into_one() {
        let mut host = Host::new("copy");
        host.put("src/a", b"a");
        host.put("src/b/c", b"cc");
        host.directory("dst");
        assert!(ran(&mut host, "-rwv src dst"));
        assert_eq!(host.said, "src\nsrc/a\nsrc/b\nsrc/b/c\n");
        assert_eq!(
            host.left(),
            [
                "dst/",
                "dst/src/",
                "dst/src/a",
                "dst/src/b/",
                "dst/src/b/c",
                "src/",
                "src/a",
                "src/b/",
                "src/b/c"
            ]
        );
        assert_eq!(host.holds("dst/src/b/c").unwrap(), b"cc");
        // Again with -k: what is there stays; without, it is written over.
        host.put("dst/src/a", b"kept");
        assert!(ran(&mut host, "-rwk src dst"));
        assert_eq!(host.holds("dst/src/a").unwrap(), b"kept");
        assert!(ran(&mut host, "-rw /src/a dst"));
        assert_eq!(
            host.holds("dst/src/a").unwrap(),
            b"a",
            "a path from the root goes in under it"
        );
        host.lines = [String::from("src/b/c")].into();
        host.directory("lines");
        assert!(ran(&mut host, "-rw lines"));
        assert_eq!(host.holds("lines/src/b/c").unwrap(), b"cc");
    }

    #[test]
    fn copy_mode_refuses_a_directory_into_itself_and_a_target_that_is_none() {
        let mut host = Host::new("into");
        host.put("src/a", b"a");
        host.directory("src/in");
        assert!(!ran(&mut host, "-rw src src/in"));
        assert_eq!(
            host.said,
            "pax: src holds src/in, so it is not copied into it\n"
        );
        assert!(!ran(&mut host, "-rw src/ src"));
        assert_eq!(host.left(), ["src/", "src/a", "src/in/"]);
        assert!(
            ran(&mut host, "-rwd src src/in"),
            "alone it may go into itself"
        );
        assert_eq!(host.left(), ["src/", "src/a", "src/in/", "src/in/src/"]);
        host.said.clear();
        assert!(!ran(&mut host, "-rw src/a nope"));
        assert_eq!(host.said, "pax: nope: is no directory to copy into\n");
        host.said.clear();
        assert!(!ran(&mut host, "-rw src/a src/a"));
        assert_eq!(host.said, "pax: src/a: is no directory to copy into\n");
        host.said.clear();
        host.directory("dst");
        assert!(!ran(&mut host, "-rw src/../src/a nope2 dst"));
        assert_eq!(
            host.said,
            "pax: src/../src/a: climbs out with .., so it is not copied\npax: nope2: no such file\n"
        );
        assert_eq!(
            host.left().iter().filter(|p| p.starts_with("dst")).count(),
            1
        );
    }

    #[test]
    fn copy_mode_says_what_it_cannot_copy_and_goes_on() {
        let mut host = Host::new("uncopied");
        host.put("src/a", b"aaaa");
        host.put("src/b", b"b");
        host.directory("dst/src");
        let ro = confined(&host.root, "dst/src");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let went = ran(&mut host, "-rw src/a dst") | ran(&mut host, "-rw /src/a dst");
        shut(&ro, false);
        assert!(!went);
        assert_eq!(
            host.said, "pax: dst/src/a: not allowed\npax: dst/src/a: not allowed\n",
            "a path from the root goes under the directory with one slash"
        );
        host.said.clear();
        shut(&confined(&host.root, "src/a"), true);
        assert!(!ran(&mut host, "-rw src/a src/b dst"));
        shut(&confined(&host.root, "src/a"), false);
        assert_eq!(host.said, "pax: src/a: not allowed\n");
        assert_eq!(
            host.holds("dst/src/a").unwrap(),
            b"",
            "made, and nothing read into it"
        );
        assert_eq!(host.holds("dst/src/b").unwrap(), b"b");
    }

    /// A tree of random directories and files under `host`'s `top`, and
    /// what each file holds.
    fn random_tree(host: &Host, top: &str, seed: &mut u64) -> Vec<(String, Vec<u8>)> {
        let mut next = || {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            *seed
        };
        let mut files = Vec::new();
        let mut directories = vec![String::from(top)];
        host.directory(top);
        for at in 0..=(next() % 12) {
            let under = directories[usize::try_from(next()).unwrap() % directories.len()].clone();
            let name = format!("{under}/n{at}");
            if next() % 3 == 0 {
                host.directory(&name);
                directories.push(name);
            } else {
                let size = usize::try_from(next() % 3000).unwrap();
                let data: Vec<u8> = (0..size).map(|_| next().to_le_bytes()[0]).collect();
                host.put(&name, &data);
                files.push((name, data));
            }
        }
        files.sort();
        files
    }

    #[test]
    fn random_trees_come_back_through_both_kinds_in_pieces_of_any_size() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for round in 0..12 {
            let mut host = Host::new(&format!("trees-{round}"));
            let files = random_tree(&host, "t", &mut seed);
            let tree = host.left();
            for format in ["ustar", "cpio"] {
                host.archive.clear();
                assert!(
                    ran(&mut host, &format!("-w -x {format} t")),
                    "{}",
                    host.said
                );
                let mut into = Host::new(&format!("trees-{round}-{format}"));
                let piece = usize::try_from(seed % 999).unwrap() + 1;
                into.reading(&host.archive, piece);
                assert!(
                    ran(&mut into, "-r"),
                    "round {round}, {format}: {}",
                    into.said
                );
                assert_eq!(
                    into.left(),
                    tree,
                    "round {round}, {format}, pieces of {piece}"
                );
                for (name, data) in &files {
                    assert_eq!(
                        &into.holds(name).unwrap(),
                        data,
                        "round {round}, {format}: {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn read_and_list_mode_pick_by_pattern_the_others_or_the_first() {
        let archive = archive_of(
            Format::Cpio,
            &[
                (member("a", Kind::File, 1, 0o644), b"1"),
                (member("b/x", Kind::File, 1, 0o644), b"2"),
                (member("a", Kind::File, 1, 0o644), b"3"),
                (member("c", Kind::File, 1, 0o644), b"4"),
            ],
        );
        let mut host = Host::new("picks");
        host.reading(&archive, 64);
        assert!(ran(&mut host, "-c a"));
        assert_eq!(host.printed, "b/x\nc\n");
        host.printed.clear();
        host.reading(&archive, 64);
        assert!(ran(&mut host, "-n a"));
        assert_eq!(host.printed, "a\n", "the first a alone");
        host.reading(&archive, 64);
        assert!(ran(&mut host, "-rn a"));
        assert_eq!(host.holds("a").unwrap(), b"1");
        host.reading(&archive, 64);
        assert!(ran(&mut host, "-r a"));
        assert_eq!(host.holds("a").unwrap(), b"3", "the later one last");
        host.reading(&archive, 64);
        assert!(!ran(&mut host, "-r c q"));
        assert_eq!(host.said, "pax: q: picks no member\n");
        assert_eq!(host.left(), ["a", "c"], "what was picked still goes");
    }

    #[test]
    fn list_mode_names_owners_cpio_keeps_by_number() {
        use std::os::unix::fs::MetadataExt as _;
        let host = Host::new("cpio-list");
        host.put("src/a", b"hello\n");
        let file = confined(&host.root, "src/a");
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(when)
            .unwrap();
        let cpio = tool(&host.root.join("src"), "cpio", &["-o", "-H", "odc"], b"a\n");
        let meta = std::fs::metadata(&file).unwrap();
        let mut list = Host::new("cpio-list-read");
        list.reading(&cpio, 77);
        assert!(ran(&mut list, "-v"));
        assert_eq!(
            list.printed,
            format!(
                "-rw-r--r-- 1 {} {} 6 Nov 14  2023 a\n",
                meta.uid(),
                meta.gid()
            )
        );
        // And what pax writes, bsdtar reads.
        let mut writer = Host::new("bsdtar");
        writer.put("w/x", b"xyz");
        assert!(ran(&mut writer, "-w w"));
        assert_eq!(
            text_of(tool(&writer.root, "bsdtar", &["-tf", "-"], &writer.archive)),
            "w/\nw/x\n"
        );
    }
}
