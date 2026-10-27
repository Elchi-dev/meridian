//! A machine for the sh dialect's tests to run scripts on: it has no
//! programs, only lib/shell's shared commands and a `cd` and `pwd` of its
//! own, and no files but those of a host tree it may be given for patterns
//! to match, and it keeps what is written, the texts commands were given to
//! read, and the commands it ran.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::commands::{self, File, Streams};
use crate::run::{Machine, Ready, Redirection, Target};
use crate::sh::parse;
use crate::sh::run::{Flow, Session, Stop};
use crate::walk::Tree;
use crate::walk::tests::Host;

/// Where one of the desk's streams goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Out,
    Err,
    /// The capture this many from the outermost.
    Capture(usize),
    /// The pipe to the next command of a pipeline.
    Pipe,
    Closed,
}

#[derive(Debug)]
pub struct Desk {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
    captures: Vec<Vec<u8>>,
    pipe: Vec<u8>,
    /// Where streams 1 and 2 go, a level for each capture and each set of
    /// redirections begun, innermost last.
    levels: Vec<[Place; 2]>,
    /// What stream 0 reads at each level, with how far it was read; none
    /// for a level that left it as it was.
    reading: Vec<Option<(Vec<u8>, usize)>>,
    /// The desk's own input, and how far it was read.
    pub input: Vec<u8>,
    taken: usize,
    /// The texts commands were given to read, in the order given.
    pub fed: Vec<Vec<u8>>,
    /// The words of each command it ran.
    pub ran: Vec<Vec<String>>,
    /// How many commands each pipeline it was asked to run had.
    pub pipelines: Vec<usize>,
    pub directory: String,
    /// The tree patterns match names in, none for one with no files.
    pub files: Option<Host>,
}

impl Default for Desk {
    fn default() -> Desk {
        Desk {
            out: Vec::new(),
            err: Vec::new(),
            captures: Vec::new(),
            pipe: Vec::new(),
            levels: Vec::new(),
            reading: Vec::new(),
            input: Vec::new(),
            taken: 0,
            fed: Vec::new(),
            ran: Vec::new(),
            pipelines: Vec::new(),
            directory: String::from("/"),
            files: None,
        }
    }
}

impl Desk {
    /// A desk whose files are `paths`, those that end in `/` directories,
    /// in a host tree called `name`.
    pub fn with_files(name: &str, paths: &[&str]) -> Desk {
        Desk {
            files: Some(Host::new(name, paths)),
            ..Desk::default()
        }
    }

    /// Where streams 1 and 2 go now.
    fn streams(&self) -> [Place; 2] {
        self.levels
            .last()
            .copied()
            .unwrap_or([Place::Out, Place::Err])
    }

    fn write(&mut self, place: Place, bytes: &[u8]) -> bool {
        let into = match place {
            Place::Out => &mut self.out,
            Place::Err => &mut self.err,
            Place::Capture(i) => &mut self.captures[i],
            Place::Pipe => &mut self.pipe,
            Place::Closed => return false,
        };
        into.extend_from_slice(bytes);
        true
    }

    /// The streams `redirections` make of `streams`, and the text they
    /// give stream 0 to read; none, said, for one the desk cannot make.
    fn made(
        &mut self,
        mut streams: [Place; 2],
        redirections: &[Redirection],
    ) -> Option<([Place; 2], Option<Vec<u8>>)> {
        let mut fed = None;
        for redirection in redirections {
            let stream = redirection.stream;
            let one = stream.wrapping_sub(1) as usize;
            match (&redirection.target, stream) {
                (Target::Text(text), 0) => fed = Some(text.clone()),
                (Target::Close, 0) => fed = Some(Vec::new()),
                (Target::Copy(0), 0) => {}
                (Target::Copy(other @ 1..=2), 1..=2) => streams[one] = streams[*other as usize - 1],
                (Target::Close, 1..=2) => streams[one] = Place::Closed,
                (Target::Read(path), _) => return self.unmade(path, "read"),
                (Target::Create(path), _) => return self.unmade(path, "make"),
                (Target::Write(path), _) => return self.unmade(path, "write"),
                (Target::Append(path), _) => return self.unmade(path, "add to"),
                (Target::ReadWrite(path), _) => return self.unmade(path, "read and write"),
                _ => {
                    let said = format!("{stream}: there is no such stream here\n");
                    self.write(self.streams()[1], said.as_bytes());
                    return None;
                }
            }
        }
        Some((streams, fed))
    }

    /// Says there is no file at `path` to do `what` with: the desk has
    /// none.
    fn unmade<T>(&mut self, path: &str, what: &str) -> Option<T> {
        let said = format!("{path}: no file here to {what}\n");
        self.write(self.streams()[1], said.as_bytes());
        None
    }

    /// Runs the command `words` names, writing where `streams` go.
    fn command(&mut self, words: &[String], streams: [Place; 2]) -> u8 {
        self.ran.push(words.to_vec());
        let mut on = On {
            desk: self,
            streams,
        };
        match words {
            [cd, path] if cd == "cd" => {
                on.desk.directory = if path.starts_with('/') {
                    path.clone()
                } else {
                    format!("{}/{path}", on.desk.directory.trim_end_matches('/'))
                };
                0
            }
            [pwd] if pwd == "pwd" => {
                let said = format!("{}\n", on.desk.directory);
                u8::from(!on.out(said.as_bytes()))
            }
            _ => commands::run(words, &mut on).unwrap_or_else(|| {
                let name = words.first().map_or("", String::as_str);
                on.err(format!("{name}: not found\n").as_bytes());
                127
            }),
        }
    }
}

/// The desk as one command reaches it: where its streams 1 and 2 go.
struct On<'d> {
    desk: &'d mut Desk,
    streams: [Place; 2],
}

impl Streams for On<'_> {
    fn out(&mut self, bytes: &[u8]) -> bool {
        self.desk.write(self.streams[0], bytes)
    }

    fn err(&mut self, bytes: &[u8]) -> bool {
        self.desk.write(self.streams[1], bytes)
    }

    fn file(&mut self, _: &str) -> Option<File> {
        None
    }

    fn terminal(&mut self, _: u32) -> bool {
        false
    }
}

/// Its files are those of its host tree, none without one.
impl Tree for Desk {
    fn names(&mut self, path: &str) -> Vec<String> {
        self.files
            .as_mut()
            .map_or_else(Vec::new, |files| files.names(path))
    }

    fn has(&mut self, path: &str) -> bool {
        self.files.as_mut().is_some_and(|files| files.has(path))
    }
}

impl Machine for Desk {
    /// Runs the commands one after another, what each writes without a
    /// place of its own for it the next one's to read.
    fn run(&mut self, pipeline: &[Ready]) -> u8 {
        self.pipelines.push(pipeline.len());
        let mut status = 0;
        let mut piped = None;
        for (i, ready) in pipeline.iter().enumerate() {
            let mut streams = self.streams();
            if i + 1 < pipeline.len() {
                streams[0] = Place::Pipe;
            }
            let reads = piped.take();
            let Some((streams, fed)) = self.made(streams, &ready.redirections) else {
                status = 1;
                piped = Some(Vec::new());
                continue;
            };
            if let Some(text) = fed.or(reads) {
                self.fed.push(text);
            }
            status = self.command(&ready.words, streams);
            piped = Some(core::mem::take(&mut self.pipe));
        }
        status
    }

    fn out(&mut self, bytes: &[u8]) -> bool {
        self.write(self.streams()[0], bytes)
    }

    fn err(&mut self, bytes: &[u8]) -> bool {
        self.write(self.streams()[1], bytes)
    }

    fn begin_capture(&mut self) {
        let errors = self.streams()[1];
        self.levels
            .push([Place::Capture(self.captures.len()), errors]);
        self.reading.push(None);
        self.captures.push(Vec::new());
    }

    fn end_capture(&mut self) -> Vec<u8> {
        self.levels.pop();
        self.reading.pop();
        self.captures.pop().unwrap_or_default()
    }

    fn begin_redirections(&mut self, redirections: &[Redirection]) -> bool {
        let Some((streams, fed)) = self.made(self.streams(), redirections) else {
            return false;
        };
        if let Some(text) = &fed {
            self.fed.push(text.clone());
        }
        self.levels.push(streams);
        self.reading.push(fed.map(|text| (text, 0)));
        true
    }

    fn end_redirections(&mut self) {
        self.levels.pop();
        self.reading.pop();
    }

    fn directory(&mut self) -> String {
        self.directory.clone()
    }

    fn set_directory(&mut self, path: &str) {
        self.directory = path.into();
    }

    fn line(&mut self) -> Option<Vec<u8>> {
        let (text, at) = match self.reading.iter_mut().rev().flatten().next() {
            Some((text, at)) => (&*text, at),
            None => (&self.input, &mut self.taken),
        };
        let rest = text.get(*at..).filter(|rest| !rest.is_empty())?;
        let len = rest
            .iter()
            .position(|&b| b == b'\n')
            .map_or(rest.len(), |end| end + 1);
        let line = rest[..len].to_vec();
        *at += len;
        Some(line)
    }
}

/// `script` run to its end on a desk by a session called `script`, run by
/// process 42, with `positional` as its parameters: how it ended, the
/// session after, and the desk.
pub fn ran(script: &str, positional: &[&str]) -> (Result<Flow, Stop>, Session, Desk) {
    let mut session = Session::new(
        String::from("script"),
        42,
        positional.iter().map(|p| String::from(*p)).collect(),
    );
    let mut desk = Desk::default();
    let items = parse::script(script).unwrap();
    let ended = session.run(&items, &mut desk);
    (ended, session, desk)
}

/// What `script` writes, and what it says went wrong, with `positional`
/// as its parameters.
pub fn said(script: &str, positional: &[&str]) -> (String, String) {
    let (_, _, desk) = ran(script, positional);
    (
        String::from_utf8(desk.out).unwrap(),
        String::from_utf8(desk.err).unwrap(),
    )
}

/// What `script` writes.
pub fn out(script: &str) -> String {
    said(script, &[]).0
}

/// Where the error that stopped `script` is, and what it is.
pub fn stopped(script: &str) -> (usize, usize, crate::Code) {
    match ran(script, &[]).0 {
        Err(Stop::Error(e)) => (e.at.line, e.at.column, e.code),
        other => panic!("{script} ended {other:?}"),
    }
}

/// What `script` writes, run on a desk whose files are `paths`, those that
/// end in `/` directories, in a host tree called `name`.
pub fn among(name: &str, paths: &[&str], script: &str) -> String {
    let mut session = Session::new(String::from("script"), 42, Vec::new());
    let mut desk = Desk::with_files(name, paths);
    let items = parse::script(script).unwrap();
    let _ = session.run(&items, &mut desk);
    String::from_utf8(desk.out).unwrap()
}

/// What `script` writes, and what it says went wrong, run on a desk whose
/// own input is `input`.
pub fn reading(script: &str, input: &str) -> (String, String) {
    let mut session = Session::new(String::from("script"), 42, Vec::new());
    let mut desk = Desk {
        input: input.as_bytes().to_vec(),
        ..Desk::default()
    };
    let items = parse::script(script).unwrap();
    let _ = session.run(&items, &mut desk);
    (
        String::from_utf8(desk.out).unwrap(),
        String::from_utf8(desk.err).unwrap(),
    )
}

/// How `script` ended once its trap on `EXIT` ran, the session after,
/// and the desk, as [`ran`] answers them.
pub fn finished(script: &str) -> (Result<Flow, Stop>, Session, Desk) {
    let mut session = Session::new(String::from("script"), 42, Vec::new());
    let mut desk = Desk::default();
    let items = parse::script(script).unwrap();
    let ran = session.run(&items, &mut desk);
    let ended = session.finish(ran, &mut desk);
    (ended, session, desk)
}
