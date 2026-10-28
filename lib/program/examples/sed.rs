//! sed: runs a script on the lines of each file it is given in turn, or
//! of its standard input without one, as POSIX's sed does, with lib/sed.
//! `-n` writes only what the script says to, `-E` reads extended
//! expressions, `-e` gives a part of the script and `-f` a file that
//! holds one, as often as wanted; without either the first word is the
//! script. It ends with 1 for words or a script it cannot take, 2 when a
//! file could not be read, and 4 when one `w` writes could not be made.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::file::{CREATE_ALWAYS, READ, WRITE};
use program::{Handle, text};
use regex::Syntax;
use sed::run::{Sed, World};
use sed::script;

// lib/regex keeps its deepest search within half of this.
program::main!(sed, stack = 256);

/// A part of the script: as it is, or the file that holds it.
#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Text(&'a str),
    File(&'a str),
}

/// What sed was asked for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    parts: Vec<Part<'a>>,
    files: &'a [String],
    quiet: bool,
    syntax: Syntax,
}

fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut parts = Vec::new();
    let (mut quiet, mut syntax) = (false, Syntax::Basic);
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        words = rest;
        for (at, letter) in letters.char_indices() {
            match letter {
                'n' => quiet = true,
                'E' => syntax = Syntax::Extended,
                'e' | 'f' => {
                    // The rest of the word is what the option takes, or
                    // else the next word.
                    let after = &letters[at + 1..];
                    let value = if after.is_empty() {
                        let wanted = if letter == 'e' { "a script" } else { "a file" };
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("sed: -{letter} wants {wanted}\n"))?;
                        words = rest;
                        value.as_str()
                    } else {
                        after
                    };
                    parts.push(if letter == 'e' {
                        Part::Text(value)
                    } else {
                        Part::File(value)
                    });
                    break;
                }
                _ => return Err(format!("sed: -{letter} is no option\n")),
            }
        }
    }
    if parts.is_empty() {
        let (script, rest) = words
            .split_first()
            .ok_or_else(|| String::from("sed: needs a script\n"))?;
        parts.push(Part::Text(script));
        words = rest;
    }
    Ok(Asked {
        parts,
        files: words,
        quiet,
        syntax,
    })
}

/// The line and the column, both counted from 1, of the character `at` of
/// `text`.
fn place(text: &str, at: usize) -> (usize, usize) {
    let before: Vec<char> = text.chars().take(at).collect();
    let line = before.iter().filter(|&&c| c == '\n').count() + 1;
    let column = before.iter().rev().take_while(|&&c| c != '\n').count() + 1;
    (line, column)
}

/// What sed ends with when a file it reads could not be read, or one it
/// writes could not be made or written.
fn ending(unread: bool, unwritten: bool) -> u64 {
    match (unread, unwritten) {
        (_, true) => 4,
        (true, false) => 2,
        (false, false) => 0,
    }
}

/// All of the file at `path`.
#[cfg_attr(test, allow(dead_code))]
fn whole(path: &str) -> Result<Vec<u8>, program::Error> {
    let mut bytes = Vec::new();
    text::pieces(Some(path), |piece| {
        bytes.extend_from_slice(piece);
        true
    })?;
    Ok(bytes)
}

/// The file at `path`, all of it, as the script it holds.
#[cfg_attr(test, allow(dead_code))]
fn script_in(path: &str) -> Result<String, String> {
    let bytes = whole(path).map_err(|e| text::trouble("sed", path, e))?;
    String::from_utf8(bytes).map_err(|_| format!("sed: {path}: a script that is no UTF-8\n"))
}

/// Bytes read from the sources one after another, taken a line at a time.
#[derive(Default)]
struct Held {
    bytes: Vec<u8>,
}

impl Held {
    /// The first whole line held, without its line feed.
    fn line(&mut self) -> Option<Vec<u8>> {
        let feed = self.bytes.iter().position(|&b| b == b'\n')?;
        let mut line: Vec<u8> = self.bytes.drain(..=feed).collect();
        line.pop();
        Some(line)
    }

    /// What is held at the end of a source: its last line, which no line
    /// feed ended, when anything is.
    fn rest(&mut self) -> Option<Vec<u8>> {
        (!self.bytes.is_empty()).then(|| core::mem::take(&mut self.bytes))
    }
}

/// What a source of lines is being read through.
enum Reading {
    Input,
    File(Handle, u64),
}

/// The files and streams sed works with.
struct System<'a> {
    /// The files to read in turn, none for the standard input.
    sources: Vec<Option<&'a str>>,
    next: usize,
    reading: Option<Reading>,
    /// Bytes read that make no whole line yet.
    held: Held,
    /// The line read ahead to know whether the one before was the last.
    ahead: Option<Vec<u8>>,
    out: text::Out<text::Write>,
    /// Each file `w` writes, and where its next line goes.
    written: Vec<(&'a str, Handle, u64)>,
    unread: bool,
    unwritten: bool,
}

#[cfg_attr(test, allow(dead_code))]
impl System<'_> {
    /// The next line of the sources, none once all are read. A source's
    /// last line ends with it, whether a line feed ends it or not.
    fn next_line(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(line) = self.held.line() {
                return Some(line);
            }
            if self.more() {
                continue;
            }
            if let Some(last) = self.held.rest() {
                return Some(last);
            }
            if !self.open_next() {
                return None;
            }
        }
    }

    /// Reads more of the source being read; false at its end.
    fn more(&mut self) -> bool {
        let mut room = [0u8; abi::MESSAGE_BYTES];
        let read = match &mut self.reading {
            None => return false,
            Some(Reading::Input) => program::input(&mut room),
            Some(Reading::File(file, at)) => program::read(*file, *at, &mut room).inspect(|&len| {
                *at += len as u64;
            }),
        };
        match read {
            Ok(len) if len > 0 => {
                self.held.bytes.extend_from_slice(&room[..len]);
                true
            }
            ended => {
                if let Some(Reading::File(file, _)) = self.reading.take() {
                    let _ = program::close(file);
                }
                if let Err(e) = ended {
                    self.unread = true;
                    let path = self.sources[self.next - 1].unwrap_or("-");
                    let _ = program::write_error(&text::trouble("sed", path, e));
                }
                false
            }
        }
    }

    /// Opens the next source that opens; false when none is left.
    fn open_next(&mut self) -> bool {
        while let Some(&source) = self.sources.get(self.next) {
            self.next += 1;
            let Some(path) = source else {
                self.reading = Some(Reading::Input);
                return true;
            };
            match program::open(path, READ) {
                Ok(file) => {
                    self.reading = Some(Reading::File(file, 0));
                    return true;
                }
                Err(e) => {
                    self.unread = true;
                    let _ = program::write_error(&text::trouble("sed", path, e));
                }
            }
        }
        false
    }
}

impl World for System<'_> {
    fn line(&mut self) -> Option<Vec<u8>> {
        self.ahead.take().or_else(|| self.next_line())
    }

    fn last(&mut self) -> bool {
        if self.ahead.is_none() {
            self.ahead = self.next_line();
        }
        self.ahead.is_none()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.out.put(bytes);
    }

    fn append(&mut self, path: &str, bytes: &[u8]) {
        if let Some((_, file, at)) = self.written.iter_mut().find(|(name, ..)| *name == path) {
            self.unwritten |= program::write_file(*file, *at, bytes).is_err();
            *at += bytes.len() as u64;
        }
    }

    fn read(&mut self, path: &str) -> Vec<u8> {
        whole(path).unwrap_or_default()
    }
}

#[cfg_attr(test, allow(dead_code))]
fn sed(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut texts = Vec::new();
    for part in &asked.parts {
        match part {
            Part::Text(text) => texts.push(String::from(*text)),
            Part::File(path) => match script_in(path) {
                Ok(text) => texts.push(text),
                Err(why) => {
                    let _ = program::write_error(&why);
                    return 1;
                }
            },
        }
    }
    let text = texts.join("\n");
    let script = match script::parse(&text, asked.syntax) {
        Ok(script) => script,
        Err(e) => {
            let (line, column) = place(&text, e.at);
            let _ = program::write_error(&format!("sed: {line}:{column}: {}\n", e.why.says()));
            return 1;
        }
    };
    let mut system = System {
        sources: text::sources(asked.files),
        next: 0,
        reading: None,
        held: Held::default(),
        ahead: None,
        out: text::standard_output(),
        written: Vec::new(),
        unread: false,
        unwritten: false,
    };
    for path in script.written() {
        match program::open_with(path, WRITE, CREATE_ALWAYS) {
            Ok(file) => system.written.push((path, file, 0)),
            Err(e) => {
                let _ = program::write_error(&text::trouble("sed", path, e));
                system.unwritten = true;
            }
        }
    }
    let ran = if system.unwritten {
        Ok(())
    } else {
        Sed::new(&script, asked.quiet).run(&mut system)
    };
    let System {
        out,
        written,
        reading,
        unread,
        unwritten,
        ..
    } = system;
    let printed = out.finish();
    for (_, file, _) in written {
        let _ = program::close(file);
    }
    if let Some(Reading::File(file, _)) = reading {
        let _ = program::close(file);
    }
    if let Err(trouble) = ran {
        let _ = program::write_error(&format!("sed: {}\n", trouble.says()));
        return 1;
    }
    ending(unread, unwritten || !printed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    #[test]
    fn the_first_word_is_the_script_without_e_or_f() {
        let w = words("s/a/b/ /x /y");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.parts, a.files, a.quiet, a.syntax),
            (
                Vec::from([Part::Text("s/a/b/")]),
                &w[1..],
                false,
                Syntax::Basic
            )
        );
        let w = words("-n p");
        assert!(asked(&w).unwrap().quiet);
        let w = words("-E p");
        assert_eq!(asked(&w).unwrap().syntax, Syntax::Extended);
        let w = words("-nE p /f");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.quiet, a.syntax, a.files),
            (true, Syntax::Extended, &w[2..])
        );
        let w = words("-- -n");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.parts, a.quiet),
            (Vec::from([Part::Text("-n")]), false),
            "after -- the script"
        );
        let w = words("p -n");
        assert_eq!(asked(&w).unwrap().files, ["-n"], "options come first");
    }

    #[test]
    fn e_and_f_give_parts_of_the_script_in_their_order() {
        let w = words("-e 1d -f /s -e p /x");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.parts, a.files),
            (
                Vec::from([Part::Text("1d"), Part::File("/s"), Part::Text("p")]),
                &w[6..]
            )
        );
        let w = words("-ep -f/s");
        assert_eq!(
            asked(&w).unwrap().parts,
            [Part::Text("p"), Part::File("/s")],
            "the rest of the word"
        );
        let w = words("-ne -n");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.parts, a.quiet),
            (Vec::from([Part::Text("-n")]), true),
            "the next word, dash or not"
        );
        let w = words("-e p /x");
        assert_eq!(
            asked(&w).unwrap().files,
            ["/x"],
            "no word is taken for a script then"
        );
    }

    #[test]
    fn words_sed_cannot_take_are_said() {
        assert_eq!(asked(&words("-e")), Err("sed: -e wants a script\n".into()));
        assert_eq!(asked(&words("-n -f")), Err("sed: -f wants a file\n".into()));
        assert_eq!(asked(&words("-x p")), Err("sed: -x is no option\n".into()));
        assert_eq!(asked(&words("-nx p")), Err("sed: -x is no option\n".into()));
        assert_eq!(asked(&words("-n")), Err("sed: needs a script\n".into()));
        assert_eq!(asked(&[]), Err("sed: needs a script\n".into()));
    }

    #[test]
    fn a_place_in_the_script_is_its_line_and_column() {
        assert_eq!(place("p", 0), (1, 1));
        assert_eq!(place("p;k", 2), (1, 3));
        assert_eq!(place("p\nk", 2), (2, 1));
        assert_eq!(place("p\n\n  k", 5), (3, 3));
        assert_eq!(place("ä\nöü", 4), (2, 3), "counted in characters");
        assert_eq!(place("ab", 2), (1, 3), "just past the end");
        assert_eq!(
            place("p\r\nk", 3),
            (2, 1),
            "a carriage return is the line's before it, as a script from Windows has it"
        );
        assert_eq!(place("p\r\nk", 1), (1, 2));
        assert_eq!(place("\n\n\n", 3), (4, 1));
    }

    #[test]
    fn it_ends_with_what_went_wrong_worst() {
        assert_eq!(ending(false, false), 0);
        assert_eq!(ending(true, false), 2);
        assert_eq!(ending(false, true), 4);
        assert_eq!(ending(true, true), 4);
    }

    /// The lines `held` gives for `pieces` of one source, each piece
    /// taken in only once the lines before it are taken out.
    fn lines(pieces: &[&[u8]]) -> Vec<String> {
        let mut held = Held::default();
        let mut lines = Vec::new();
        for piece in pieces {
            held.bytes.extend_from_slice(piece);
            while let Some(line) = held.line() {
                lines.push(String::from_utf8(line).unwrap());
            }
        }
        if let Some(last) = held.rest() {
            lines.push(String::from_utf8(last).unwrap());
        }
        lines
    }

    #[test]
    fn lines_come_whole_wherever_the_pieces_are_cut() {
        assert_eq!(lines(&[b"a\nb\n"]), ["a", "b"]);
        assert_eq!(lines(&[b"he", b"llo\nwo", b"rld\n"]), ["hello", "world"]);
        assert_eq!(lines(&[b"\n\n"]), ["", ""], "empty lines count");
        assert_eq!(
            lines(&[b"x\r\n"]),
            ["x\r"],
            "a carriage return is the line's"
        );
        assert_eq!(lines(&[]), Vec::<String>::new());
        assert_eq!(lines(&[b"", b""]), Vec::<String>::new());
        assert_eq!(
            lines(&["ü\n".as_bytes()[..1].as_ref(), &"ü\n".as_bytes()[1..]]),
            ["ü"]
        );
    }

    #[test]
    fn a_last_line_without_its_line_feed_ends_with_its_source() {
        assert_eq!(lines(&[b"a\nb"]), ["a", "b"]);
        assert_eq!(lines(&[b"only"]), ["only"]);
        let mut held = Held::default();
        assert_eq!(held.rest(), None, "nothing held, no line");
        held.bytes.extend_from_slice(b"part");
        assert_eq!(held.line(), None, "no line feed yet");
        assert_eq!(held.rest(), Some(b"part".to_vec()));
        assert_eq!(held.rest(), None, "given once");
        held.bytes.extend_from_slice(b"one\ntwo\nthr");
        assert_eq!(held.line(), Some(b"one".to_vec()));
        assert_eq!(held.line(), Some(b"two".to_vec()));
        assert_eq!(held.line(), None);
        assert_eq!(held.bytes, b"thr");
    }

    #[test]
    fn options_gather_in_any_order_before_the_files() {
        let w = words("-n -e p -E -e p /x");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.parts, a.quiet, a.syntax, a.files),
            (
                Vec::from([Part::Text("p"), Part::Text("p")]),
                true,
                Syntax::Extended,
                &w[6..]
            )
        );
        let w = words("-f /a -n");
        let a = asked(&w).unwrap();
        assert_eq!((a.parts, a.quiet), (Vec::from([Part::File("/a")]), true));
        let w = words("-nf /a");
        assert_eq!(asked(&w).unwrap().parts, [Part::File("/a")]);
        let w = words("-- -e p");
        assert_eq!(
            asked(&w).unwrap().parts,
            [Part::Text("-e")],
            "after -- even -e is the script"
        );
    }

    #[test]
    fn lines_of_bytes_come_whole_a_byte_at_a_time() {
        let text = b"one\n\xff\xfe\n\ntwo";
        let bytes: Vec<&[u8]> = text.chunks(1).collect();
        let mut held = Held::default();
        let mut lines = Vec::new();
        for piece in bytes {
            held.bytes.extend_from_slice(piece);
            while let Some(line) = held.line() {
                lines.push(line);
            }
        }
        lines.extend(held.rest());
        assert_eq!(
            lines,
            [
                b"one".to_vec(),
                Vec::from([0xff, 0xfe]),
                Vec::new(),
                b"two".to_vec()
            ]
        );
    }
}
