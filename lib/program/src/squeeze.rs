//! What compress, uncompress and zcat share, on lib/lzw: their options,
//! the names of files with `.Z` and without, and each file compressed or
//! uncompressed a piece at a time into a file made anew or onto the
//! standard output. A file goes only once what it became is whole, and
//! compress keeps one it cannot make smaller.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::file::{CREATE_ALWAYS, READ, WRITE};
use lzw::{Decoder, Encoder, Fault};

use crate::text::{self, Pieces, Source};
use crate::{Error, Handle};

/// What the three are asked.
#[derive(Debug, PartialEq, Eq)]
pub struct Asked<'a> {
    /// `-c`: onto the standard output, every file kept.
    pub to_output: bool,
    /// `-f`: over a file there, and a file no smaller too.
    pub force: bool,
    /// `-v`: say how each file went.
    pub verbose: bool,
    /// `-b`: the most bits a code has.
    pub bits: u8,
    pub files: &'a [String],
}

/// What the words of `tool` ask, which takes the option letters
/// `letters`, `b` among them taking a number of bits.
///
/// # Errors
///
/// For a letter it does not take, or bits not from 9 to 16.
pub fn asked<'a>(tool: &str, arguments: &'a [String], letters: &str) -> Result<Asked<'a>, String> {
    let mut asked = Asked {
        to_output: false,
        force: false,
        verbose: false,
        bits: lzw::MOST_BITS,
        files: arguments,
    };
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        words = rest;
        for (at, letter) in word.char_indices().skip(1) {
            if !letters.contains(letter) {
                return Err(format!("{tool}: -{letter} is no option\n"));
            }
            match letter {
                'c' => asked.to_output = true,
                'f' => asked.force = true,
                'v' => asked.verbose = true,
                _ => {
                    // -b takes the rest of the word, or the next.
                    let attached = &word[at + 1..];
                    let value = if attached.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("{tool}: -b wants a number of bits\n"))?;
                        words = rest;
                        value.as_str()
                    } else {
                        attached
                    };
                    asked.bits = value
                        .parse()
                        .ok()
                        .filter(|bits| (lzw::LEAST_BITS..=lzw::MOST_BITS).contains(bits))
                        .ok_or_else(|| format!("{tool}: -b {value}: bits go from 9 to 16\n"))?;
                    break;
                }
            }
        }
    }
    asked.files = words;
    Ok(asked)
}

/// The files and streams the three work on.
pub trait Files {
    /// Whether something is at `path`.
    fn there(&mut self, path: &str) -> bool;

    /// Opens the file at `path` to read, or with none the standard
    /// input.
    ///
    /// # Errors
    ///
    /// When it cannot be opened.
    fn open(&mut self, path: Option<&str>) -> Result<(), Error>;

    /// The next piece of what was opened; none at its end.
    ///
    /// # Errors
    ///
    /// When it cannot be read.
    fn read(&mut self) -> Result<Vec<u8>, Error>;

    /// Closes what was opened.
    fn close(&mut self);

    /// Makes the file at `path` anew for what is put next, or with none
    /// takes the standard output.
    ///
    /// # Errors
    ///
    /// When the file cannot be made.
    fn create(&mut self, path: Option<&str>) -> Result<(), Error>;

    /// Writes `bytes` where [`Files::create`] said.
    ///
    /// # Errors
    ///
    /// When they cannot be written.
    fn put(&mut self, bytes: &[u8]) -> Result<(), Error>;

    /// Ends what was made, and gives how many bytes went to it.
    ///
    /// # Errors
    ///
    /// When it cannot be closed.
    fn done(&mut self) -> Result<u64, Error>;

    /// Removes the file at `path`.
    ///
    /// # Errors
    ///
    /// When it cannot be removed.
    fn remove(&mut self, path: &str) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// How a file went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Went {
    Done,
    /// compress left it as it was, as it would grow.
    Kept,
    Failed,
}

/// What the three end with: 1 when a file failed, else 2 when compress
/// kept one, else 0.
pub fn status(went: impl IntoIterator<Item = Went>) -> u64 {
    went.into_iter()
        .fold(0, |status, went| match (status, went) {
            (1, _) | (_, Went::Failed) => 1,
            (_, Went::Kept) => 2,
            (status, Went::Done) => status,
        })
}

/// How `run` went with each of `files`, `-` and no file at all standing
/// for the standard input.
pub fn each(files: &[String], mut run: impl FnMut(Option<&str>) -> Went) -> Vec<Went> {
    if files.is_empty() {
        return alloc::vec![run(None)];
    }
    files
        .iter()
        .map(|file| run(Some(file.as_str()).filter(|&f| f != "-")))
        .collect()
}

/// How much smaller `written` is than `read`, in percent to two places.
fn smaller(read: u64, written: u64) -> String {
    let hundredths = read.saturating_sub(written) * 10_000 / read.max(1);
    format!("{}.{:02}%", hundredths / 100, hundredths % 100)
}

/// `path` without its `.Z`, if it has one: in capitals or not, as
/// Meridian's names are the same either way.
fn without_z(path: &str) -> Option<&str> {
    let cut = path.len().checked_sub(2)?;
    let (stem, end) = (path.get(..cut)?, path.get(cut..)?);
    end.eq_ignore_ascii_case(".Z").then_some(stem)
}

/// What a fault in a compressed file is said as.
fn says(fault: Fault) -> String {
    match fault {
        Fault::NotCompressed => String::from("not what compress writes"),
        Fault::TooManyBits(bits) => format!("codes of {bits} bits, past the 16 there may be"),
        Fault::Corrupt => String::from("damaged: a code names no string"),
    }
}

/// Compressing or uncompressing.
enum Coder {
    Squeeze(Encoder),
    Spread(Decoder),
}

impl Coder {
    fn feed(&mut self, piece: &[u8]) -> Result<Vec<u8>, Fault> {
        match self {
            Coder::Squeeze(encoder) => Ok(encoder.feed(piece)),
            Coder::Spread(decoder) => decoder.feed(piece),
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>, Fault> {
        match self {
            Coder::Squeeze(encoder) => Ok(encoder.finish()),
            // A text without even its header is none.
            Coder::Spread(decoder) if !decoder.whole() => Err(Fault::NotCompressed),
            Coder::Spread(_) => Ok(Vec::new()),
        }
    }
}

/// Codes what `from` holds into what `to` makes, and gives how many
/// bytes were read and written; or says what went wrong, and takes away
/// a file made by halves.
fn through(
    files: &mut impl Files,
    tool: &str,
    from: Option<&str>,
    to: Option<&str>,
    coder: &mut Coder,
) -> Option<(u64, u64)> {
    let (from_name, to_name) = (from.unwrap_or("-"), to.unwrap_or("-"));
    if let Err(e) = files.open(from) {
        files.say(&text::trouble(tool, from_name, e));
        return None;
    }
    if let Err(e) = files.create(to) {
        files.close();
        files.say(&text::trouble(tool, to_name, e));
        return None;
    }
    let mut read = 0;
    let mut wrong = None;
    loop {
        let piece = match files.read() {
            Ok(piece) if piece.is_empty() => break,
            Ok(piece) => piece,
            Err(e) => {
                wrong = Some(text::trouble(tool, from_name, e));
                break;
            }
        };
        read += piece.len() as u64;
        let put = coder
            .feed(&piece)
            .map_err(|fault| format!("{tool}: {from_name}: {}\n", says(fault)))
            .and_then(|out| files.put(&out).map_err(|e| text::trouble(tool, to_name, e)));
        if let Err(why) = put {
            wrong = Some(why);
            break;
        }
    }
    files.close();
    if wrong.is_none() {
        let last = coder
            .finish()
            .map_err(|fault| format!("{tool}: {from_name}: {}\n", says(fault)))
            .and_then(|out| files.put(&out).map_err(|e| text::trouble(tool, to_name, e)));
        wrong = last.err();
    }
    let written = files.done();
    if let (None, Err(e)) = (&wrong, &written) {
        wrong = Some(text::trouble(tool, to_name, *e));
    }
    if let Some(why) = wrong {
        files.say(&why);
        if let Some(to) = to {
            let _ = files.remove(to);
        }
        return None;
    }
    Some((read, written.unwrap_or(0)))
}

/// Compresses the file at `path`, into the same name with `.Z` or onto
/// the standard output, or with none the standard input.
pub fn compress(files: &mut impl Files, path: Option<&str>, asked: &Asked) -> Went {
    let mut coder = Coder::Squeeze(Encoder::new(asked.bits));
    let Some(path) = path else {
        return match through(files, "compress", None, None, &mut coder) {
            Some(_) => Went::Done,
            None => Went::Failed,
        };
    };
    if without_z(path).is_some() {
        files.say(&format!("compress: {path}: has .Z already\n"));
        return Went::Failed;
    }
    let to = format!("{path}.Z");
    if asked.to_output {
        return match through(files, "compress", Some(path), None, &mut coder) {
            Some(_) => Went::Done,
            None => Went::Failed,
        };
    }
    if !asked.force && files.there(&to) {
        files.say(&format!(
            "compress: {to}: there already, so {path} is kept\n"
        ));
        return Went::Failed;
    }
    let Some((read, written)) = through(files, "compress", Some(path), Some(&to), &mut coder)
    else {
        return Went::Failed;
    };
    if written >= read && !asked.force {
        let _ = files.remove(&to);
        if asked.verbose {
            files.say(&format!("{path}: no smaller compressed, so kept\n"));
        }
        return Went::Kept;
    }
    if let Err(e) = files.remove(path) {
        files.say(&text::trouble("compress", path, e));
        return Went::Failed;
    }
    if asked.verbose {
        files.say(&format!(
            "{path}: {} smaller, now {to}\n",
            smaller(read, written)
        ));
    }
    Went::Done
}

/// Uncompresses the file `path` or `path.Z`, into the name without
/// `.Z` or onto the standard output, or with none the standard input.
pub fn uncompress(files: &mut impl Files, tool: &str, path: Option<&str>, asked: &Asked) -> Went {
    let mut coder = Coder::Spread(Decoder::new());
    let Some(path) = path else {
        return match through(files, tool, None, None, &mut coder) {
            Some(_) => Went::Done,
            None => Went::Failed,
        };
    };
    let (from, to) = match without_z(path) {
        Some(to) => (String::from(path), String::from(to)),
        None => (format!("{path}.Z"), String::from(path)),
    };
    if asked.to_output {
        return match through(files, tool, Some(&from), None, &mut coder) {
            Some(_) => Went::Done,
            None => Went::Failed,
        };
    }
    if !asked.force && files.there(&to) {
        files.say(&format!("{tool}: {to}: there already, so {from} is kept\n"));
        return Went::Failed;
    }
    if through(files, tool, Some(&from), Some(&to), &mut coder).is_none() {
        return Went::Failed;
    }
    if let Err(e) = files.remove(&from) {
        files.say(&text::trouble(tool, &from, e));
        return Went::Failed;
    }
    if asked.verbose {
        files.say(&format!("{from}: now {to}\n"));
    }
    Went::Done
}

/// Meridian's files and the program's streams.
pub struct System {
    input: Option<Source>,
    output: Option<(Handle, u64)>,
    written: u64,
    out: text::Out<text::Write>,
}

impl Default for System {
    fn default() -> Self {
        System {
            input: None,
            output: None,
            written: 0,
            out: text::standard_output(),
        }
    }
}

impl System {
    /// Writes what is held on the standard output; whether every write
    /// went.
    #[must_use]
    pub fn finish(self) -> bool {
        self.out.finish()
    }
}

impl Files for System {
    fn there(&mut self, path: &str) -> bool {
        crate::open(path, READ).is_ok_and(|file| {
            let _ = crate::close(file);
            true
        })
    }

    fn open(&mut self, path: Option<&str>) -> Result<(), Error> {
        self.input = Some(Source::open(path.unwrap_or("-"))?);
        Ok(())
    }

    fn read(&mut self) -> Result<Vec<u8>, Error> {
        let mut piece = Vec::new();
        if let Some(input) = &mut self.input {
            input.more(&mut piece)?;
        }
        Ok(piece)
    }

    fn close(&mut self) {
        if let Some(input) = self.input.take() {
            input.close();
        }
    }

    fn create(&mut self, path: Option<&str>) -> Result<(), Error> {
        self.written = 0;
        self.output = match path {
            Some(path) => Some((crate::open_with(path, WRITE, CREATE_ALWAYS)?, 0)),
            None => None,
        };
        Ok(())
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.written += bytes.len() as u64;
        match &mut self.output {
            Some((file, at)) => {
                crate::write_file(*file, *at, bytes)?;
                *at += bytes.len() as u64;
            }
            None => self.out.put(bytes),
        }
        Ok(())
    }

    fn done(&mut self) -> Result<u64, Error> {
        match self.output.take() {
            Some((file, _)) => crate::close(file)?,
            None => self.out.flush(),
        }
        Ok(self.written)
    }

    fn remove(&mut self, path: &str) -> Result<(), Error> {
        crate::remove(path)
    }

    fn say(&mut self, text: &str) {
        self.out.flush();
        let _ = crate::write_error(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::VecDeque;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake reaches the host's own files.
    fn confined(root: &std::path::Path, path: &str) -> std::path::PathBuf {
        let mut depth = 0usize;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .unwrap_or_else(|| panic!("{path} climbs out of the test's directory"));
                }
                _ => depth += 1,
            }
        }
        root.join(path.trim_start_matches('/'))
    }

    /// Files among the host's temporary ones, gone once dropped, and the
    /// streams: the standard input in pieces of 128 bytes, as a channel
    /// gives them.
    struct Host {
        root: std::path::PathBuf,
        input: VecDeque<Vec<u8>>,
        reading: Option<VecDeque<Vec<u8>>>,
        made: Option<std::path::PathBuf>,
        written: u64,
        output: Vec<u8>,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root = std::env::temp_dir()
                .join(format!("meridian-squeeze-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("d")).unwrap();
            Host {
                root,
                input: VecDeque::new(),
                reading: None,
                made: None,
                written: 0,
                output: Vec::new(),
                said: String::new(),
            }
        }

        fn put_file(&self, path: &str, bytes: &[u8]) {
            std::fs::write(self.root.join(path), bytes).unwrap();
        }

        fn holds(&self, path: &str) -> Option<Vec<u8>> {
            std::fs::read(self.root.join(path)).ok()
        }

        fn give(&mut self, bytes: &[u8]) {
            self.input = bytes.chunks(128).map(<[u8]>::to_vec).collect();
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn error_of(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            std::io::ErrorKind::IsADirectory => Error::NotAFile,
            _ => Error::Unwritable,
        }
    }

    impl Files for Host {
        fn there(&mut self, path: &str) -> bool {
            confined(&self.root, path).exists()
        }

        fn open(&mut self, path: Option<&str>) -> Result<(), Error> {
            let pieces = match path {
                Some(path) => {
                    let bytes =
                        std::fs::read(confined(&self.root, path)).map_err(|e| error_of(&e))?;
                    bytes.chunks(4096).map(<[u8]>::to_vec).collect()
                }
                None => core::mem::take(&mut self.input),
            };
            self.reading = Some(pieces);
            Ok(())
        }

        fn read(&mut self) -> Result<Vec<u8>, Error> {
            Ok(self
                .reading
                .as_mut()
                .and_then(VecDeque::pop_front)
                .unwrap_or_default())
        }

        fn close(&mut self) {
            self.reading = None;
        }

        fn create(&mut self, path: Option<&str>) -> Result<(), Error> {
            self.written = 0;
            self.made = match path {
                Some(path) => {
                    let at = confined(&self.root, path);
                    std::fs::write(&at, b"").map_err(|e| error_of(&e))?;
                    Some(at)
                }
                None => None,
            };
            Ok(())
        }

        fn put(&mut self, bytes: &[u8]) -> Result<(), Error> {
            use std::io::Write as _;
            self.written += bytes.len() as u64;
            let Some(at) = &self.made else {
                self.output.extend_from_slice(bytes);
                return Ok(());
            };
            std::fs::OpenOptions::new()
                .append(true)
                .open(at)
                .and_then(|mut f| f.write_all(bytes))
                .map_err(|e| error_of(&e))
        }

        fn done(&mut self) -> Result<u64, Error> {
            self.made = None;
            Ok(self.written)
        }

        fn remove(&mut self, path: &str) -> Result<(), Error> {
            std::fs::remove_file(confined(&self.root, path)).map_err(|e| error_of(&e))
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// What the host's gzip, which reads compress's files, makes of
    /// `bytes`.
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut child = std::process::Command::new("gzip")
            .arg("-dc")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let input = bytes.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&input).unwrap());
        let done = child.wait_with_output().unwrap();
        writer.join().unwrap();
        done.stdout
    }

    /// A text compress makes much smaller.
    fn repeating() -> Vec<u8> {
        b"the quick brown fox jumps over the lazy dog\n".repeat(200)
    }

    fn plain() -> Asked<'static> {
        Asked {
            to_output: false,
            force: false,
            verbose: false,
            bits: 16,
            files: &[],
        }
    }

    #[test]
    fn options_come_before_the_files() {
        let given = words("-cfv -b 12 a b");
        assert_eq!(
            asked("compress", &given, "cfvb"),
            Ok(Asked {
                to_output: true,
                force: true,
                verbose: true,
                bits: 12,
                files: &given[3..]
            })
        );
        let given = words("-b9 -- -c");
        let taken = asked("compress", &given, "cfvb").unwrap();
        assert_eq!((taken.bits, taken.files), (9, &given[2..]));
        let given = words("a -c");
        assert_eq!(
            asked("uncompress", &given, "cfv").map(|a| a.files),
            Ok(&given[..])
        );
        let given = words("-");
        assert_eq!(asked("zcat", &given, "").map(|a| a.files), Ok(&given[..]));
        assert_eq!(
            asked("uncompress", &words("-b 12"), "cfv"),
            Err("uncompress: -b is no option\n".into())
        );
        assert_eq!(
            asked("zcat", &words("-c"), ""),
            Err("zcat: -c is no option\n".into())
        );
        assert_eq!(
            asked("compress", &words("-b"), "cfvb"),
            Err("compress: -b wants a number of bits\n".into())
        );
        for bits in ["8", "17", "x", ""] {
            assert_eq!(
                asked(
                    "compress",
                    &[String::from("-b"), String::from(bits)],
                    "cfvb"
                ),
                Err(format!("compress: -b {bits}: bits go from 9 to 16\n")),
                "{bits:?}"
            );
        }
    }

    #[test]
    fn z_ends_a_name_in_capitals_or_not() {
        assert_eq!(without_z("f.Z"), Some("f"));
        assert_eq!(without_z("f.z"), Some("f"));
        assert_eq!(without_z(".Z"), Some(""));
        assert_eq!(without_z("fZ"), None);
        assert_eq!(without_z("Z"), None);
        assert_eq!(without_z(""), None);
        assert_eq!(without_z("f.Zz"), None);
        assert_eq!(without_z("é.Z"), Some("é"));
        assert_eq!(without_z("a.é"), None);
        assert_eq!(without_z("a€"), None, "a cut inside a character is no .Z");
    }

    #[test]
    fn no_file_and_a_dash_are_the_standard_input() {
        let mut seen = Vec::new();
        let went = each(&[], |path| {
            seen.push(path.map(String::from));
            Went::Done
        });
        assert_eq!((went, seen), (alloc::vec![Went::Done], alloc::vec![None]));
        let mut seen = Vec::new();
        let given = words("a - b");
        let went = each(&given, |path| {
            seen.push(path.map(String::from));
            if path.is_none() {
                Went::Kept
            } else {
                Went::Done
            }
        });
        assert_eq!(went, [Went::Done, Went::Kept, Went::Done]);
        assert_eq!(
            seen,
            [Some("a".into()), None, Some("b".into())],
            "in their order"
        );
    }

    #[test]
    fn the_status_says_the_worst_that_went() {
        assert_eq!(status([]), 0);
        assert_eq!(status([Went::Done, Went::Done]), 0);
        assert_eq!(status([Went::Done, Went::Kept]), 2);
        assert_eq!(status([Went::Kept, Went::Failed, Went::Kept]), 1);
        assert_eq!(status([Went::Failed, Went::Done]), 1);
        assert_eq!(smaller(1000, 250), "75.00%");
        assert_eq!(smaller(3, 2), "33.33%");
        assert_eq!(smaller(10, 20), "0.00%");
        assert_eq!(smaller(0, 3), "0.00%");
        assert_eq!(says(Fault::NotCompressed), "not what compress writes");
        assert_eq!(
            says(Fault::TooManyBits(20)),
            "codes of 20 bits, past the 16 there may be"
        );
        assert_eq!(says(Fault::Corrupt), "damaged: a code names no string");
    }

    #[test]
    fn the_standard_input_goes_to_the_standard_output() {
        let mut host = Host::new("streams");
        let text = repeating();
        host.give(&text);
        assert_eq!(compress(&mut host, None, &plain()), Went::Done);
        let squeezed = core::mem::take(&mut host.output);
        assert_eq!(gzip(&squeezed), text, "gzip reads it");
        assert!(squeezed.len() < text.len() / 5);
        host.give(&squeezed);
        assert_eq!(
            uncompress(&mut host, "uncompress", None, &plain()),
            Went::Done
        );
        assert_eq!(host.output, text);
        assert_eq!(host.said, "");
    }

    #[test]
    fn a_file_becomes_its_name_with_z_and_back() {
        let mut host = Host::new("files");
        let text = repeating();
        host.put_file("f", &text);
        assert_eq!(compress(&mut host, Some("f"), &plain()), Went::Done);
        assert_eq!(host.holds("f"), None, "the file goes");
        let squeezed = host.holds("f.Z").unwrap();
        assert_eq!(gzip(&squeezed), text);
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("f"), &plain()),
            Went::Done
        );
        assert_eq!(host.holds("f").as_ref(), Some(&text), "f finds f.Z");
        assert_eq!(host.holds("f.Z"), None);
        compress(&mut host, Some("f"), &plain());
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("f.Z"), &plain()),
            Went::Done
        );
        assert_eq!(
            host.holds("f").as_ref(),
            Some(&text),
            "and f.Z names itself"
        );
        assert_eq!(host.said, "");
        assert_eq!(host.output, b"");
    }

    #[test]
    fn c_writes_to_the_standard_output_and_keeps_the_file() {
        let mut host = Host::new("output");
        let text = repeating();
        host.put_file("f", &text);
        let to_output = Asked {
            to_output: true,
            ..plain()
        };
        assert_eq!(compress(&mut host, Some("f"), &to_output), Went::Done);
        assert_eq!(host.holds("f").as_ref(), Some(&text));
        assert_eq!(host.holds("f.Z"), None);
        let squeezed = core::mem::take(&mut host.output);
        host.put_file("g.Z", &squeezed);
        assert_eq!(
            uncompress(&mut host, "zcat", Some("g"), &to_output),
            Went::Done
        );
        assert_eq!(host.output, text);
        assert_eq!(host.holds("g.Z").as_ref(), Some(&squeezed), "kept");
        assert_eq!(host.holds("g"), None, "and nothing made");
    }

    #[test]
    fn a_file_that_would_grow_is_kept_unless_forced() {
        let mut host = Host::new("grow");
        host.put_file("small", b"ab");
        assert_eq!(compress(&mut host, Some("small"), &plain()), Went::Kept);
        assert_eq!(host.holds("small").as_deref(), Some(&b"ab"[..]));
        assert_eq!(host.holds("small.Z"), None, "what it made goes");
        assert_eq!(host.said, "", "said only when asked");
        let verbose = Asked {
            verbose: true,
            ..plain()
        };
        assert_eq!(compress(&mut host, Some("small"), &verbose), Went::Kept);
        assert_eq!(host.said, "small: no smaller compressed, so kept\n");
        let forced = Asked {
            force: true,
            ..plain()
        };
        assert_eq!(compress(&mut host, Some("small"), &forced), Went::Done);
        assert_eq!(host.holds("small"), None);
        assert_eq!(gzip(&host.holds("small.Z").unwrap()), b"ab");
    }

    #[test]
    fn verbose_says_how_much_smaller() {
        let mut host = Host::new("verbose");
        host.put_file("f", &repeating());
        let verbose = Asked {
            verbose: true,
            ..plain()
        };
        compress(&mut host, Some("f"), &verbose);
        let squeezed = host.holds("f.Z").unwrap().len() as u64;
        assert_eq!(
            host.said,
            format!(
                "f: {} smaller, now f.Z\n",
                smaller(repeating().len() as u64, squeezed)
            )
        );
        host.said.clear();
        uncompress(&mut host, "uncompress", Some("f.Z"), &verbose);
        assert_eq!(host.said, "f.Z: now f\n");
    }

    #[test]
    fn a_file_there_is_kept_unless_forced() {
        let mut host = Host::new("there");
        host.put_file("f", &repeating());
        host.put_file("f.Z", b"old");
        assert_eq!(compress(&mut host, Some("f"), &plain()), Went::Failed);
        assert_eq!(host.said, "compress: f.Z: there already, so f is kept\n");
        assert_eq!(host.holds("f.Z").as_deref(), Some(&b"old"[..]));
        let forced = Asked {
            force: true,
            ..plain()
        };
        assert_eq!(compress(&mut host, Some("f"), &forced), Went::Done);
        host.put_file("f", b"new");
        host.said.clear();
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("f"), &plain()),
            Went::Failed
        );
        assert_eq!(host.said, "uncompress: f: there already, so f.Z is kept\n");
        assert_eq!(host.holds("f").as_deref(), Some(&b"new"[..]));
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("f"), &forced),
            Went::Done
        );
        assert_eq!(host.holds("f"), Some(repeating()));
    }

    #[test]
    fn what_cannot_be_read_is_said_and_nothing_is_left_by_halves() {
        let mut host = Host::new("trouble");
        assert_eq!(compress(&mut host, Some("nope"), &plain()), Went::Failed);
        assert_eq!(host.said, "compress: nope: no such file\n");
        assert_eq!(host.holds("nope.Z"), None);
        host.said.clear();
        assert_eq!(compress(&mut host, Some("f.Z"), &plain()), Went::Failed);
        assert_eq!(host.said, "compress: f.Z: has .Z already\n");
        host.said.clear();
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("nope"), &plain()),
            Went::Failed
        );
        assert_eq!(host.said, "uncompress: nope.Z: no such file\n");
        host.said.clear();
        host.put_file("plain.Z", b"just text");
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("plain"), &plain()),
            Went::Failed
        );
        assert_eq!(host.said, "uncompress: plain.Z: not what compress writes\n");
        assert_eq!(host.holds("plain"), None, "what was begun goes");
        assert!(host.holds("plain.Z").is_some(), "and the file stays");
        host.said.clear();
        host.put_file("empty.Z", b"");
        assert_eq!(
            uncompress(&mut host, "zcat", Some("empty"), &plain()),
            Went::Failed
        );
        assert_eq!(host.said, "zcat: empty.Z: not what compress writes\n");
        host.said.clear();
        let mut cut = Encoder::new(16).feed(&repeating());
        cut.extend([0xff, 0xff, 0xff]);
        host.put_file("bad.Z", &cut);
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("bad.Z"), &plain()),
            Went::Failed
        );
        assert_eq!(
            host.said,
            "uncompress: bad.Z: damaged: a code names no string\n"
        );
        assert_eq!(host.holds("bad"), None);
        host.said.clear();
        assert_eq!(compress(&mut host, Some("d"), &plain()), Went::Failed);
        assert_eq!(host.said, "compress: d: a directory, not a file\n");
        assert_eq!(host.holds("d.Z"), None);
    }

    #[test]
    fn every_size_of_file_comes_back_through_both() {
        let mut host = Host::new("sizes");
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for len in [0usize, 1, 127, 128, 129, 4095, 4096, 4097, 70_000] {
            let text: Vec<u8> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    b"abcd"[usize::try_from(seed % 4).unwrap()]
                })
                .collect();
            host.give(&text);
            compress(&mut host, None, &plain());
            let squeezed = core::mem::take(&mut host.output);
            host.give(&squeezed);
            assert_eq!(
                uncompress(&mut host, "zcat", None, &plain()),
                Went::Done,
                "{len}"
            );
            assert_eq!(core::mem::take(&mut host.output), text, "{len}");
            assert_eq!(gzip(&squeezed), text, "{len}");
        }
        assert_eq!(host.said, "");
        assert_eq!(host.output, b"");
    }

    #[test]
    fn files_go_one_by_one_and_the_status_is_the_worst() {
        let mut host = Host::new("several");
        host.put_file("big", &repeating());
        host.put_file("small", b"ab");
        let went: Vec<Went> = ["big", "small"]
            .iter()
            .map(|f| compress(&mut host, Some(f), &plain()))
            .collect();
        assert_eq!(went, [Went::Done, Went::Kept]);
        assert_eq!(status(went), 2, "one kept, none failed");
        let went: Vec<Went> = ["small", "nope", "big.Z"]
            .iter()
            .map(|f| compress(&mut host, Some(f), &plain()))
            .collect();
        assert_eq!(went, [Went::Kept, Went::Failed, Went::Failed]);
        assert_eq!(status(went), 1);
        assert_eq!(
            host.said,
            "compress: nope: no such file\ncompress: big.Z: has .Z already\n"
        );
        assert_eq!(host.holds("big"), None, "the first went as it should");
        assert_eq!(host.holds("small").as_deref(), Some(&b"ab"[..]));
    }

    #[test]
    fn b_sets_the_most_bits_in_the_header() {
        let mut host = Host::new("bits");
        let text = repeating();
        host.put_file("f", &text);
        let nine = Asked { bits: 9, ..plain() };
        assert_eq!(compress(&mut host, Some("f"), &nine), Went::Done);
        let squeezed = host.holds("f.Z").unwrap();
        assert_eq!(squeezed[..3], [0x1f, 0x9d, 0x89]);
        assert_eq!(gzip(&squeezed), text);
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("f.Z"), &plain()),
            Went::Done
        );
        assert_eq!(host.holds("f"), Some(text));
    }

    #[test]
    fn what_stands_where_the_file_goes_is_said() {
        let mut host = Host::new("in-the-way");
        host.put_file("x", &repeating());
        std::fs::create_dir(host.root.join("x.Z")).unwrap();
        let forced = Asked {
            force: true,
            ..plain()
        };
        assert_eq!(compress(&mut host, Some("x"), &forced), Went::Failed);
        assert_eq!(host.said, "compress: x.Z: a directory, not a file\n");
        assert_eq!(host.holds("x"), Some(repeating()), "the file stays");
        assert!(host.root.join("x.Z").is_dir(), "and so does what was there");
    }

    #[test]
    fn zcat_and_c_write_file_after_file() {
        let mut host = Host::new("after");
        host.put_file(
            "a",
            b"first line, long enough to get smaller, smaller, smaller\n",
        );
        host.put_file(
            "b",
            b"second line, long enough to get smaller, smaller, smaller\n",
        );
        let to_output = Asked {
            to_output: true,
            ..plain()
        };
        compress(&mut host, Some("a"), &plain());
        compress(&mut host, Some("b"), &plain());
        for (tool, files) in [("zcat", ["a", "b"]), ("zcat", ["a.Z", "b.Z"])] {
            for file in files {
                assert_eq!(
                    uncompress(&mut host, tool, Some(file), &to_output),
                    Went::Done
                );
            }
            assert_eq!(
                core::mem::take(&mut host.output),
                b"first line, long enough to get smaller, smaller, smaller\n\
                  second line, long enough to get smaller, smaller, smaller\n"
            );
        }
        // compress -c of two files writes two texts, each with its header.
        host.put_file("c", b"x");
        host.put_file("e", b"y");
        compress(&mut host, Some("c"), &to_output);
        compress(&mut host, Some("e"), &to_output);
        assert_eq!(
            host.output,
            [0x1f, 0x9d, 0x90, b'x', 0x00, 0x1f, 0x9d, 0x90, b'y', 0x00]
        );
        assert_eq!(host.holds("c").as_deref(), Some(&b"x"[..]));
    }

    #[test]
    fn an_empty_file_is_kept_and_a_small_z_names_the_same() {
        let mut host = Host::new("edges");
        host.put_file("empty", b"");
        assert_eq!(compress(&mut host, Some("empty"), &plain()), Went::Kept);
        assert_eq!(host.holds("empty").as_deref(), Some(&b""[..]));
        assert_eq!(host.holds("empty.Z"), None);
        let forced = Asked {
            force: true,
            ..plain()
        };
        assert_eq!(compress(&mut host, Some("empty"), &forced), Went::Done);
        assert_eq!(
            host.holds("empty.Z").as_deref(),
            Some(&[0x1f, 0x9d, 0x90][..])
        );
        std::fs::rename(host.root.join("empty.Z"), host.root.join("empty.z")).unwrap();
        assert_eq!(
            uncompress(&mut host, "uncompress", Some("empty.z"), &plain()),
            Went::Done
        );
        assert_eq!(
            host.holds("empty").as_deref(),
            Some(&b""[..]),
            "an empty text back"
        );
        assert_eq!(host.holds("empty.z"), None);
        assert_eq!(compress(&mut host, Some("empty.z"), &plain()), Went::Failed);
        assert_eq!(host.said, "compress: empty.z: has .Z already\n");
    }
}
