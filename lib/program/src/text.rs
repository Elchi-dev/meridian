//! Text that comes in pieces, from a file or a stream, taken a line at a
//! time, and the pieces a command-line tool reads: of each file it is
//! given, or of its standard input without one.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{Error, abi};

/// What the tool `tool` says when the file at `path` could not be read.
#[must_use]
pub fn trouble(tool: &str, path: &str, e: Error) -> String {
    let why = match e {
        Error::NotFound => "no such file",
        Error::NotAFile => "a directory, not a file",
        Error::BadArgument => "not a path",
        Error::Denied => "not allowed",
        Error::TooLong => "too long a path",
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
}
