//! Text that comes in pieces, from a file or a stream, taken a line at a
//! time, and the pieces a command-line tool reads: of each file it is
//! given, or of its standard input without one.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{Error, Handle, abi};

/// What the tool `tool` says when the file at `path` could not be read,
/// or written by one that writes files.
#[must_use]
pub fn trouble(tool: &str, path: &str, e: Error) -> String {
    let why = match e {
        Error::NotFound => "no such file",
        Error::NotAFile => "a directory, not a file",
        Error::BadArgument => "not a path",
        Error::Denied => "not allowed",
        Error::TooLong => "too long a path",
        Error::ReadOnly => "on a volume that is only read",
        Error::DiskFull => "no room left on its disk",
        Error::Unwritable => "could not be written",
        _ => "could not be read",
    };
    format!("{tool}: {path}: {why}\n")
}

/// Bytes that come in pieces, cut into lines. A line ends at a line feed,
/// which it does not keep; the last may end without one.
#[derive(Default)]
pub struct Lines {
    held: Vec<u8>,
}

impl Lines {
    /// Takes in `bytes`, and hands each line they finish to `each`, until
    /// that answers false. Says whether it went on to the end.
    pub fn feed(&mut self, bytes: &[u8], mut each: impl FnMut(&[u8]) -> bool) -> bool {
        let mut rest = bytes;
        while let Some(at) = rest.iter().position(|&b| b == b'\n') {
            let whole = if self.held.is_empty() {
                each(&rest[..at])
            } else {
                self.held.extend_from_slice(&rest[..at]);
                let line = core::mem::take(&mut self.held);
                each(&line)
            };
            rest = &rest[at + 1..];
            if !whole {
                return false;
            }
        }
        self.held.extend_from_slice(rest);
        true
    }

    /// Hands the last line to `each` when one is left without its line
    /// feed.
    pub fn finish(self, each: impl FnOnce(&[u8])) {
        if !self.held.is_empty() {
            each(&self.held);
        }
    }
}

/// What a tool makes of one text it reads in pieces.
pub trait Reader {
    /// Takes in `piece`, and says whether more are wanted.
    fn feed(&mut self, piece: &[u8]) -> bool;

    /// What the tool writes for the text.
    fn finish(self) -> Vec<u8>;
}

/// Reads each of `files`, or the standard input without one, with a
/// reader from `fresh`, and writes what each made, after the line [`named`]
/// writes for it when there are more than one. Says, as `tool`, what went
/// wrong with any, and answers whether all went well.
pub fn each_file<R: Reader>(tool: &str, files: &[String], mut fresh: impl FnMut() -> R) -> bool {
    let sources = sources(files);
    let several = sources.len() > 1;
    let mut failed = false;
    for (i, path) in sources.into_iter().enumerate() {
        let mut reader = fresh();
        if let Err(e) = pieces(path, |piece| reader.feed(piece)) {
            failed = true;
            let _ = crate::write_error(&trouble(tool, path.unwrap_or("-"), e));
            continue;
        }
        let mut said = if several { named(i, path) } else { Vec::new() };
        said.extend(reader.finish());
        failed |= crate::write_bytes(&said).is_err();
    }
    !failed
}

/// The line that names the file at `path`, the `i`th of several, with an
/// empty line before it after the first, as POSIX's head and tail write
/// it; `-` for the standard input.
#[must_use]
pub fn named(i: usize, path: Option<&str>) -> Vec<u8> {
    let gap = if i > 0 { "\n" } else { "" };
    format!("{gap}==> {} <==\n", path.unwrap_or("-")).into_bytes()
}

/// What a tool reads: each of `files`, or its standard input, as none,
/// when it was given no file.
#[must_use]
pub fn sources(files: &[String]) -> Vec<Option<&str>> {
    if files.is_empty() {
        Vec::from([None])
    } else {
        files.iter().map(|f| Some(f.as_str())).collect()
    }
}

/// Hands each piece of the file at `path`, or of the standard input
/// without one, to `each`, until that answers false or it ended.
///
/// # Errors
///
/// When the file does not open or cannot be read, or the input cannot.
pub fn pieces(path: Option<&str>, mut each: impl FnMut(&[u8]) -> bool) -> Result<(), Error> {
    let mut room = [0u8; abi::MESSAGE_BYTES];
    let Some(path) = path else {
        loop {
            match crate::input(&mut room)? {
                0 => return Ok(()),
                len => {
                    if !each(&room[..len]) {
                        return Ok(());
                    }
                }
            }
        }
    };
    let file = crate::open(path, abi::file::READ)?;
    let mut at = 0;
    let read = loop {
        match crate::read(file, at, &mut room) {
            Ok(0) => break Ok(()),
            Ok(len) => {
                at += len as u64;
                if !each(&room[..len]) {
                    break Ok(());
                }
            }
            Err(e) => break Err(e),
        }
    };
    let _ = crate::close(file);
    read
}

/// What gives a text a piece at a time, as it is asked for.
pub trait Pieces {
    /// Reads the next piece onto the end of `held`, and says whether
    /// there was one.
    ///
    /// # Errors
    ///
    /// When the text cannot be read.
    fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error>;
}

/// A file, or the standard input, read a piece at a time.
pub enum Source {
    Input,
    File(Handle, u64),
}

impl Source {
    /// The file at `path`, or the standard input for `-`.
    ///
    /// # Errors
    ///
    /// When the file does not open.
    pub fn open(path: &str) -> Result<Source, Error> {
        if path == "-" {
            return Ok(Source::Input);
        }
        Ok(Source::File(crate::open(path, abi::file::READ)?, 0))
    }

    /// Closes the file, when it is one.
    pub fn close(self) {
        if let Source::File(file, _) = self {
            let _ = crate::close(file);
        }
    }
}

impl Pieces for Source {
    fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
        let mut room = [0u8; abi::MESSAGE_BYTES];
        let len = match self {
            Source::Input => crate::input(&mut room)?,
            Source::File(file, at) => {
                let len = crate::read(*file, *at, &mut room)?;
                *at += len as u64;
                len
            }
        };
        held.extend_from_slice(&room[..len]);
        Ok(len > 0)
    }
}

/// The lines of a text that comes in pieces, taken one at a time as they
/// are asked for, so that no more of it is held than a line.
pub struct LineReader<P> {
    pieces: P,
    held: Vec<u8>,
    open: bool,
}

impl<P: Pieces> LineReader<P> {
    pub fn new(pieces: P) -> LineReader<P> {
        LineReader {
            pieces,
            held: Vec::new(),
            open: true,
        }
    }

    /// The next line, without its line feed; none at the end, where a
    /// last line without one still comes.
    ///
    /// # Errors
    ///
    /// When the text cannot be read.
    pub fn line(&mut self) -> Result<Option<Vec<u8>>, Error> {
        loop {
            if let Some(feed) = self.held.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = self.held.drain(..=feed).collect();
                line.pop();
                return Ok(Some(line));
            }
            if !self.open {
                return Ok((!self.held.is_empty()).then(|| core::mem::take(&mut self.held)));
            }
            self.open = self.pieces.more(&mut self.held)?;
        }
    }

    /// What the lines came from.
    pub fn into_pieces(self) -> P {
        self.pieces
    }
}

/// Output held until a message's worth has come and then written at once,
/// so a long text goes out in as few messages as it can, remembering
/// whether a write did not go through.
pub struct Out<W: FnMut(&[u8]) -> Result<(), Error>> {
    held: Vec<u8>,
    write: W,
    failed: bool,
}

impl<W: FnMut(&[u8]) -> Result<(), Error>> Out<W> {
    /// Output that `write` writes.
    pub fn new(write: W) -> Self {
        Out {
            held: Vec::new(),
            write,
            failed: false,
        }
    }

    /// Takes `bytes`, and writes all that is held once it is a message's
    /// worth or more.
    pub fn put(&mut self, bytes: &[u8]) {
        self.held.extend_from_slice(bytes);
        if self.held.len() >= abi::MESSAGE_BYTES {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.held.is_empty() {
            self.failed |= (self.write)(&self.held).is_err();
            self.held.clear();
        }
    }

    /// Writes what is still held, and says whether every write went
    /// through.
    #[must_use]
    pub fn finish(mut self) -> bool {
        self.flush();
        !self.failed
    }
}

/// What writes the program's standard output.
pub type Write = fn(&[u8]) -> Result<(), Error>;

/// Output onto the program's standard output.
#[must_use]
pub fn standard_output() -> Out<Write> {
    Out::new(crate::write_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines `pieces` make, fed one after another, the last as
    /// [`Lines::finish`] hands it.
    fn cut(pieces: &[&str]) -> Vec<String> {
        let mut lines = Lines::default();
        let mut out = Vec::new();
        for piece in pieces {
            assert!(lines.feed(piece.as_bytes(), |l| {
                out.push(String::from_utf8(l.to_vec()).unwrap());
                true
            }));
        }
        lines.finish(|l| out.push(String::from_utf8(l.to_vec()).unwrap()));
        out
    }

    #[test]
    fn lines_end_at_each_line_feed_wherever_the_pieces_are_cut() {
        assert_eq!(cut(&["a\nb\n"]), ["a", "b"]);
        assert_eq!(cut(&["a\nb"]), ["a", "b"], "the last without its end");
        assert_eq!(cut(&["he", "llo\nwor", "ld\n"]), ["hello", "world"]);
        assert_eq!(cut(&["\n\n"]), ["", ""], "empty lines count");
        assert_eq!(cut(&["a", "", "\n"]), ["a"]);
        assert_eq!(cut(&[]), Vec::<String>::new());
        assert_eq!(cut(&["", ""]), Vec::<String>::new());
        assert_eq!(cut(&["x\r\n"]), ["x\r"], "a carriage return is the line's");
    }

    #[test]
    fn trouble_says_which_tool_could_not_read_which_file_and_why() {
        assert_eq!(
            trouble("wc", "/x", Error::NotFound),
            "wc: /x: no such file\n"
        );
        assert_eq!(
            trouble("head", "/System", Error::NotAFile),
            "head: /System: a directory, not a file\n"
        );
        assert_eq!(
            trouble("grep", "a\0b", Error::BadArgument),
            "grep: a\0b: not a path\n"
        );
        assert_eq!(
            trouble("sort", "/x", Error::Denied),
            "sort: /x: not allowed\n"
        );
        assert_eq!(
            trouble("wc", "/x", Error::TooLong),
            "wc: /x: too long a path\n"
        );
        assert_eq!(
            trouble("wc", "/x", Error::Unreadable),
            "wc: /x: could not be read\n"
        );
        assert_eq!(
            trouble("tee", "/System/x", Error::ReadOnly),
            "tee: /System/x: on a volume that is only read\n"
        );
        assert_eq!(
            trouble("tee", "/x", Error::DiskFull),
            "tee: /x: no room left on its disk\n"
        );
        assert_eq!(
            trouble("tee", "/x", Error::Unwritable),
            "tee: /x: could not be written\n"
        );
    }

    #[test]
    fn each_of_several_files_comes_after_a_line_that_names_it() {
        assert_eq!(named(0, Some("/a")), b"==> /a <==\n");
        assert_eq!(named(1, Some("/b")), b"\n==> /b <==\n");
        assert_eq!(named(2, None), b"\n==> - <==\n", "the input as -");
    }

    #[test]
    fn a_tool_reads_its_files_or_else_its_input() {
        assert_eq!(sources(&[]), [None]);
        let files = [String::from("/a"), String::from("b")];
        assert_eq!(sources(&files), [Some("/a"), Some("b")]);
    }

    #[test]
    fn a_line_answered_false_stops_the_rest() {
        let mut lines = Lines::default();
        let mut seen = Vec::new();
        let went_on = lines.feed(b"one\ntwo\nthree\n", |l| {
            seen.push(l.to_vec());
            seen.len() < 2
        });
        assert!(!went_on);
        assert_eq!(seen, [b"one".to_vec(), b"two".to_vec()]);
        let mut held = Lines::default();
        assert!(held.feed(b"part", |_| false), "nothing ended to stop at");
        let mut last = Vec::new();
        held.finish(|l| last.extend_from_slice(l));
        assert_eq!(last, b"part");
    }

    /// A text in memory, given in the pieces it was cut into.
    struct Cut(alloc::collections::VecDeque<Vec<u8>>);

    impl Pieces for Cut {
        fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
            let Some(piece) = self.0.pop_front() else {
                return Ok(false);
            };
            held.extend_from_slice(&piece);
            Ok(true)
        }
    }

    /// The lines a reader takes from `text` cut into pieces of `size`.
    fn read(text: &[u8], size: usize) -> Vec<Vec<u8>> {
        let mut reader = LineReader::new(Cut(text.chunks(size).map(<[u8]>::to_vec).collect()));
        let mut lines = Vec::new();
        while let Some(line) = reader.line().unwrap() {
            lines.push(line);
        }
        assert_eq!(reader.line(), Ok(None), "and none after the end");
        lines
    }

    #[test]
    fn a_reader_takes_lines_whole_wherever_the_pieces_are_cut() {
        for size in [1, 2, 3, 128] {
            assert_eq!(
                read(b"one\ntwo\n\nfour", size),
                [
                    b"one".to_vec(),
                    b"two".to_vec(),
                    Vec::new(),
                    b"four".to_vec()
                ],
                "pieces of {size}"
            );
        }
        assert_eq!(read(b"", 4), Vec::<Vec<u8>>::new());
        assert_eq!(read(b"\n", 4), [Vec::<u8>::new()]);
        assert_eq!(
            read(b"last\n", 2),
            [b"last".to_vec()],
            "no empty line after the last feed"
        );
        assert_eq!(
            read(&[0xff, b'\n', 0xfe], 1),
            [Vec::from([0xff]), Vec::from([0xfe])]
        );
    }

    #[test]
    fn a_reader_holds_no_more_than_the_line_it_is_on() {
        let mut reader =
            LineReader::new(Cut(Vec::from([b"a\nb".to_vec(), b"\nc\n".to_vec()]).into()));
        assert_eq!(reader.line(), Ok(Some(b"a".to_vec())));
        assert_eq!(reader.held, b"b", "the rest of its piece waits");
        assert_eq!(reader.line(), Ok(Some(b"b".to_vec())));
        assert_eq!(reader.pieces.0.len(), 0, "the second piece was read for it");
        assert_eq!(reader.line(), Ok(Some(b"c".to_vec())));
        assert_eq!(reader.line(), Ok(None));
        assert_eq!(reader.into_pieces().0.len(), 0);
    }

    /// The writes an [`Out`] made, and what it writes them with: the
    /// writes whose numbers, from 0, are in `failing` go wrong.
    struct Writes {
        made: core::cell::RefCell<Vec<Vec<u8>>>,
        failing: Vec<usize>,
    }

    impl Writes {
        fn new(failing: &[usize]) -> Writes {
            Writes {
                made: core::cell::RefCell::new(Vec::new()),
                failing: failing.to_vec(),
            }
        }

        fn out(&self) -> Out<impl FnMut(&[u8]) -> Result<(), Error> + '_> {
            Out::new(|bytes: &[u8]| {
                let mut made = self.made.borrow_mut();
                made.push(bytes.to_vec());
                if self.failing.contains(&(made.len() - 1)) {
                    Err(Error::Closed)
                } else {
                    Ok(())
                }
            })
        }

        fn lengths(&self) -> Vec<usize> {
            self.made.borrow().iter().map(Vec::len).collect()
        }
    }

    #[test]
    fn output_is_written_once_a_message_s_worth_is_held() {
        let writes = Writes::new(&[]);
        let mut out = writes.out();
        out.put(&[b'a'; abi::MESSAGE_BYTES - 1]);
        assert_eq!(writes.lengths(), [0usize; 0], "one byte short");
        out.put(b"b");
        assert_eq!(writes.lengths(), [abi::MESSAGE_BYTES], "exactly a message");
        out.put(b"cd");
        assert_eq!(writes.lengths(), [abi::MESSAGE_BYTES]);
        assert!(out.finish());
        assert_eq!(writes.lengths(), [abi::MESSAGE_BYTES, 2]);
        assert_eq!(writes.made.borrow()[1], b"cd");
    }

    #[test]
    fn a_long_piece_goes_out_in_one_write() {
        let writes = Writes::new(&[]);
        let mut out = writes.out();
        out.put(b"x");
        out.put(&[b'y'; 300]);
        assert_eq!(writes.lengths(), [301]);
        assert!(out.finish());
        assert_eq!(
            writes.lengths(),
            [301],
            "nothing held, nothing more written"
        );
    }

    #[test]
    fn output_with_nothing_held_writes_nothing() {
        let writes = Writes::new(&[]);
        assert!(writes.out().finish());
        assert_eq!(writes.lengths(), [0usize; 0]);
        let mut out = writes.out();
        out.put(b"");
        assert!(out.finish());
        assert_eq!(writes.lengths(), [0usize; 0]);
    }

    #[test]
    fn a_write_that_went_wrong_is_remembered() {
        let writes = Writes::new(&[0]);
        let mut out = writes.out();
        out.put(&[b'a'; abi::MESSAGE_BYTES]);
        out.put(b"more");
        assert!(!out.finish(), "the first of two writes went wrong");
        assert_eq!(writes.lengths(), [abi::MESSAGE_BYTES, 4]);
        let writes = Writes::new(&[1]);
        let mut out = writes.out();
        out.put(&[b'a'; abi::MESSAGE_BYTES]);
        out.put(b"last");
        assert!(!out.finish(), "the last went wrong");
    }

    #[test]
    fn output_comes_out_as_it_was_put_in_any_pieces() {
        let mut seed = 0x0bad_5eed_1234_5678_u64;
        for _ in 0..200 {
            let writes = Writes::new(&[]);
            let mut out = writes.out();
            let mut put = Vec::new();
            for _ in 0..seed % 20 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let len = usize::try_from(seed % 200).unwrap();
                let piece: Vec<u8> = (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect();
                out.put(&piece);
                put.extend(piece);
            }
            assert!(out.finish());
            let made = writes.made.borrow();
            assert_eq!(made.concat(), put);
            let (last, all_but) = made
                .split_last()
                .map_or((0, &[][..]), |(l, a)| (l.len(), a));
            assert!(all_but.iter().all(|w| w.len() >= abi::MESSAGE_BYTES));
            assert!(last > 0 || made.is_empty());
        }
    }
}
