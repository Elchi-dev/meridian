//! The shell: it reads keys, shows what they type on the console, and runs
//! what is typed in Meridian's shell language (decision record 0016) once
//! Enter goes down, taking more lines while a block is open. Its own
//! commands are help, echo, ls, cat, mkdir, rm, clear and services, which
//! asks init over the line init started it with, each under its Windows
//! name too; any other word starts a program, which the shell waits for.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use ::shell::Code;
use ::shell::expand::{self, Piece};
use ::shell::parse::{self, Redirect, Statement};
use ::shell::run::{Flow, Machine, Ready, Session};
use program::abi::{MESSAGE_BYTES, file};
use program::layout::{Held, Typed};
use program::{Ended, Error, Handle, KeyEvent, start::Sink};
use services::{About, MESSAGE};

program::main!(shell);

/// What a key does to the line being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Edit {
    /// The character goes on the line, and on the console.
    Type(char),
    /// The last character goes.
    Erase,
    /// The line is done.
    Run(String),
    /// Nothing changes.
    Nothing,
}

/// The line being typed, and which shift and control keys are held.
#[derive(Default)]
struct Editor {
    line: String,
    held: Held,
}

impl Editor {
    fn feed(&mut self, key: KeyEvent) -> Edit {
        match self.held.feed(key) {
            Typed::Char('\n') => Edit::Run(core::mem::take(&mut self.line)),
            Typed::Char('\u{8}') => match self.line.pop() {
                Some(_) => Edit::Erase,
                None => Edit::Nothing,
            },
            Typed::Char(c) => {
                self.line.push(c);
                Edit::Type(c)
            }
            Typed::End | Typed::Nothing => Edit::Nothing,
        }
    }
}

/// What the words of a command ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command<'a> {
    Help,
    Echo(&'a [String]),
    List(&'a [String]),
    Show(&'a [String]),
    MakeDirectory(&'a [String]),
    Remove(&'a [String]),
    Clear,
    Services,
    Empty,
    /// `true` or `false` in a pipeline, which end so.
    Status(u8),
    /// A program, by its name in [`PROGRAMS`] or by its path, and the
    /// words it is handed.
    Program(&'a str, &'a [String]),
    /// A command that takes paths and was given none.
    Missing(&'a str),
}

/// What `help` says: the commands on its first line.
const HELP: &str = "help, echo, ls or dir, cat or type, mkdir or md, rm or del, clear or cls, \
                    services, exit, or a program\n\
                    let, if, for, while and fn as blocks in braces; > PATH or >> PATH after a \
                    command sends what it writes into a file\n";

/// Where `ls` looks without a path.
const HOME: &str = "/System/Boot";

/// Where a program named without a path is.
const PROGRAMS: &str = "/System/Boot/meridian/programs";

fn command(words: &[String]) -> Command<'_> {
    let Some((word, rest)) = words.split_first() else {
        return Command::Empty;
    };
    match word.as_str() {
        "help" => Command::Help,
        "echo" => Command::Echo(rest),
        "ls" | "dir" => Command::List(rest),
        "mkdir" | "md" | "rm" | "del" if rest.is_empty() => Command::Missing(word),
        "cat" | "type" => Command::Show(rest),
        "mkdir" | "md" => Command::MakeDirectory(rest),
        "rm" | "del" => Command::Remove(rest),
        "clear" | "cls" => Command::Clear,
        "services" => Command::Services,
        "true" => Command::Status(0),
        "false" => Command::Status(1),
        _ => Command::Program(word, rest),
    }
}

/// What SPAWN is asked for to start the program `word` names: the path
/// itself, or for a name the program of that name in [`PROGRAMS`], and
/// where `tree` has no such file, as on a CD, the program of that name the
/// loader brought.
fn program_name(word: &str, tree: &mut impl Tree) -> String {
    if word.starts_with('/') {
        return word.into();
    }
    let path = format!("{PROGRAMS}/{word}.mexec");
    if tree.has(&path) {
        path
    } else {
        format!("{word}.mexec")
    }
}

/// What the shell says once the program `word` ended so: nothing when it
/// exited with 0.
fn ending(word: &str, ended: Ended) -> Option<String> {
    match ended {
        Ended::Exited(0) => None,
        Ended::Exited(code) => Some(format!("{word} ended with {code}\n")),
        Ended::Faulted(vector) => Some(format!("{word} stopped by exception {vector}\n")),
    }
}

/// What the shell says when a call about `path` failed so.
fn trouble(path: &str, e: Error) -> String {
    let why = match e {
        Error::NotFound => "no such file",
        Error::NotADirectory => "not a directory",
        Error::NotAFile => "a directory, not a file",
        Error::Denied => "not allowed",
        Error::BadArgument => "not a path from the root",
        Error::TooLong => "too long a path",
        Error::NotAProgram => "not a program",
        Error::ReadOnly => "on a volume that is only read",
        Error::Exists => "there already",
        Error::NotEmpty => "a directory that holds something",
        Error::InUse => "open, so it stays",
        Error::DiskFull => "no room left on its disk",
        Error::Unwritable => "could not be written",
        _ => "could not be read",
    };
    format!("{path}: {why}\n")
}

/// What the shell says about a service init told it of.
fn shown(about: &About<'_>) -> String {
    let times = match about.starts {
        1 => "once".into(),
        n => format!("{n} times"),
    };
    format!("{}: {}, started {times}\n", about.name, about.state.word())
}

/// What `$?` holds once a program ended so: its exit code, 255 for one
/// past that, and 128 and the vector when an exception stopped it.
fn status(ended: Ended) -> u8 {
    match ended {
        Ended::Exited(code) => u8::try_from(code).unwrap_or(u8::MAX),
        Ended::Faulted(vector) => u8::try_from(vector).map_or(u8::MAX, |v| v.saturating_add(128)),
    }
}

/// What the lines typed so far come to.
#[derive(Debug, PartialEq, Eq)]
enum Lines {
    /// A whole script, to run.
    Script(Vec<Statement>),
    /// A block still open, which the next line goes on with.
    Open,
    /// Something that does not read.
    Wrong(::shell::Error),
}

fn lines(typed: &str) -> Lines {
    match parse::script(typed) {
        Ok(script) => Lines::Script(script),
        Err(e) if e.code == Code::UnclosedBlock => Lines::Open,
        Err(e) => Lines::Wrong(e),
    }
}

/// How a redirection opens its file: `>` makes it new or empties it, and
/// `>>` keeps what it holds. `<` opens none, since nothing reads a
/// standard input yet.
fn opening(how: Redirect) -> Option<u64> {
    match how {
        Redirect::Out => Some(file::CREATE_ALWAYS),
        Redirect::Append => Some(file::OPEN_ALWAYS),
        Redirect::In => None,
    }
}

/// What the shell asks of a tree of files to match patterns in it.
trait Tree {
    /// The names in the directory at `path`, none when it is no directory.
    fn names(&mut self, path: &str) -> Vec<String>;

    /// Whether something is at `path`.
    fn has(&mut self, path: &str) -> bool;
}

/// The paths `pattern` matches in `tree`, from `directory` when it does
/// not start at the root. Each part between slashes with `*` or `?` is
/// matched against the names in the directory it is in, in the order of
/// those names in capitals; a part without either stays as the pattern
/// wrote it, and is not listed for, since a directory such as the root
/// holds places that are no names in it. A path that ends in such a part
/// counts once something is there.
fn matching(directory: &str, pattern: &[Piece], tree: &mut impl Tree) -> Vec<String> {
    let absolute = pattern.first() == Some(&Piece::Char('/'));
    let base = if absolute { "/" } else { directory };
    // Each path found so far: as the word shows it, and from the root.
    let mut found = Vec::from([(String::new(), String::from(base))]);
    let mut listed_last = true;
    for (pieces, text) in expand::segments(pattern) {
        if pieces.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        for (shown, real) in &found {
            let names = if let Some(text) = &text {
                Vec::from([text.clone()])
            } else {
                let mut names = tree.names(real);
                names.retain(|name| expand::matches(&pieces, name));
                names.sort_by_cached_key(|name| name.to_uppercase());
                names
            };
            for name in names {
                let shown = if shown.is_empty() && !absolute {
                    name.clone()
                } else {
                    format!("{shown}/{name}")
                };
                let slash = if real.ends_with('/') { "" } else { "/" };
                next.push((shown, format!("{real}{slash}{name}")));
            }
        }
        listed_last = text.is_none();
        found = next;
    }
    found
        .into_iter()
        .filter(|(_, real)| listed_last || tree.has(real))
        .map(|(shown, _)| shown)
        .collect()
}

/// The line to init the shell started with, 0 when init did not start it.
static LINE: AtomicU64 = AtomicU64::new(0);

/// What the shell was started to do, by the words it was handed.
#[derive(Debug, PartialEq, Eq)]
enum Asked<'a> {
    /// Run the lines typed at the console.
    Typed,
    /// Run the script after `-c`.
    Text(&'a str),
    /// Run the script in the file at this path.
    File(&'a str),
    /// Something else, which it does not know.
    Unknown,
}

fn asked(arguments: &[String]) -> Asked<'_> {
    match arguments {
        [] => Asked::Typed,
        [flag, text] if flag == "-c" => Asked::Text(text),
        [path] if path != "-c" => Asked::File(path),
        _ => Asked::Unknown,
    }
}

#[cfg_attr(test, allow(dead_code))]
fn shell(_: Handle) -> u64 {
    let line = program::start::given().line;
    LINE.store(line.map_or(0, |h| h.0), SeqCst);
    let status = match asked(program::arguments()) {
        Asked::Typed => typed(),
        Asked::Text(text) => script(text),
        Asked::File(path) => match text_of(path) {
            Ok(text) => script(&text),
            Err(e) => {
                let _ = program::write(&trouble(path, e));
                2
            }
        },
        Asked::Unknown => {
            let _ =
                program::write("shell: start it with no words, -c SCRIPT, or a script's path\n");
            2
        }
    };
    u64::from(status)
}

/// Runs `text` as a script, saying what does not read or run, and answers
/// the status it ended with: 2 when something did not.
#[cfg_attr(test, allow(dead_code))]
fn script(text: &str) -> u8 {
    let mut session = Session::new();
    let ran = parse::script(text).and_then(|script| session.run(&script, &mut System));
    match ran {
        Ok(_) => session.status(),
        Err(e) => {
            let _ = program::write(&format!("{e}\n"));
            2
        }
    }
}

/// What the file at `path` holds, read as text.
#[cfg_attr(test, allow(dead_code))]
fn text_of(path: &str) -> Result<String, Error> {
    let file = program::open(path, file::READ)?;
    let mut bytes = Vec::new();
    let mut room = [0u8; 4096];
    let read = loop {
        match program::read(file, bytes.len() as u64, &mut room) {
            Ok(0) => break Ok(()),
            Ok(len) => bytes.extend_from_slice(&room[..len]),
            Err(e) => break Err(e),
        }
    };
    let _ = program::close(file);
    read.map(|()| String::from_utf8_lossy(&bytes).into_owned())
}

/// Runs the lines typed at the console until `exit`, and answers the
/// status it gave.
#[cfg_attr(test, allow(dead_code))]
fn typed() -> u8 {
    let _ = program::write("Meridian shell. Type help to see what it knows.\n> ");
    let mut editor = Editor::default();
    let mut session = Session::new();
    let mut typed = String::new();
    loop {
        let Some(key) = program::key() else {
            program::sleep(1);
            continue;
        };
        let mut room = [0u8; 4];
        let _ = match editor.feed(key) {
            Edit::Type(c) => program::write(c.encode_utf8(&mut room)),
            Edit::Erase => program::write("\u{8}"),
            Edit::Run(line) => {
                let _ = program::write("\n");
                typed.push_str(&line);
                typed.push('\n');
                let prompt = match lines(&typed) {
                    Lines::Open => "... ",
                    Lines::Script(script) => {
                        typed.clear();
                        match session.run(&script, &mut System) {
                            Ok(Flow::Exit) => return session.status(),
                            Ok(_) => {}
                            Err(e) => {
                                let _ = program::write(&format!("{e}\n"));
                            }
                        }
                        "> "
                    }
                    Lines::Wrong(e) => {
                        typed.clear();
                        let _ = program::write(&format!("{e}\n"));
                        "> "
                    }
                };
                program::write(prompt)
            }
            Edit::Nothing => Ok(()),
        };
    }
}

/// What the shell runs commands on: the calls of the runtime.
struct System;

impl Machine for System {
    fn run(&mut self, pipeline: &[Ready]) -> u8 {
        let Some(stages) = plumbed(pipeline) else {
            return 1;
        };
        let alone = stages.len() == 1;
        let going: Vec<Going> = stages
            .into_iter()
            .map(|(words, input, out)| begin(words, input, out, alone))
            .collect();
        // Each one has to be waited for; the last one's status is the
        // pipeline's.
        going.into_iter().fold(0, |_, command| command.finish())
    }

    fn matching(&mut self, directory: &str, pattern: &[Piece]) -> Vec<String> {
        matching(directory, pattern, self)
    }
}

impl Tree for System {
    fn names(&mut self, path: &str) -> Vec<String> {
        let Ok(directory) = program::open(path, file::READ) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        let mut room = [0u8; 1024];
        for index in 0.. {
            match program::list(directory, index, &mut room) {
                Ok((len, _)) => names.push(String::from_utf8_lossy(&room[..len]).into_owned()),
                Err(_) => break,
            }
        }
        let _ = program::close(directory);
        names
    }

    fn has(&mut self, path: &str) -> bool {
        program::open(path, file::READ).is_ok_and(|handle| {
            let _ = program::close(handle);
            true
        })
    }
}

/// Where a command reads: the shell's own input, a file from its byte
/// `at` on, or the pipe from the command before it.
enum In {
    Mine,
    File { handle: Handle, at: u64 },
    Pipe(Handle),
}

impl In {
    /// Takes the next piece of what is read into `room`, and answers its
    /// length: 0 at the end.
    #[cfg_attr(test, allow(dead_code))]
    fn take(&mut self, room: &mut [u8; MESSAGE_BYTES]) -> Result<usize, Error> {
        match self {
            In::Mine => program::input(room),
            In::File { handle, at } => {
                let len = program::read(*handle, *at, room)?;
                *at += len as u64;
                Ok(len)
            }
            In::Pipe(end) => program::stream(*end, room),
        }
    }

    /// The channel end a program reading here is handed, which goes with
    /// it: the pipe, one a thread of the shell fills from the file, or a
    /// duplicate of the shell's own input; none for the keyboard.
    #[cfg_attr(test, allow(dead_code))]
    fn handed(self) -> Option<Handle> {
        match self {
            In::Pipe(end) => Some(end),
            In::Mine => program::duplicate(program::start::given().input?).ok(),
            In::File { .. } => {
                let (into, from) = program::channel().ok()?;
                let mut file = self;
                let fed = program::thread(move || {
                    let mut pipe = Out::Pipe(into);
                    let copied = copy(&mut file, &mut pipe);
                    file.close();
                    pipe.close();
                    u64::from(!copied)
                });
                if fed.is_err() {
                    let _ = (program::close(into), program::close(from));
                    return None;
                }
                Some(from)
            }
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    fn close(self) {
        if let In::File { handle, .. } | In::Pipe(handle) = self {
            let _ = program::close(handle);
        }
    }
}

/// Where a command's text goes: the shell's own output, a file from its
/// byte `at` on, or the pipe to the command after it.
enum Out {
    Mine,
    File { handle: Handle, at: u64 },
    Pipe(Handle),
}

impl Out {
    #[cfg_attr(test, allow(dead_code))]
    fn say(&mut self, text: &str) -> Result<(), Error> {
        self.put(text.as_bytes(), &mut Vec::new())
    }

    /// Writes `bytes` as they are into a file or a pipe. The shell's own
    /// output takes text, so there a character cut short waits in `held`
    /// for its rest, and what can never be text shows as such.
    #[cfg_attr(test, allow(dead_code))]
    fn put(&mut self, bytes: &[u8], held: &mut Vec<u8>) -> Result<(), Error> {
        match self {
            Out::Mine => {
                held.extend_from_slice(bytes);
                let whole = whole(held);
                let text = String::from_utf8_lossy(&held[..whole]).into_owned();
                held.drain(..whole);
                program::write(&text)
            }
            Out::File { handle, at } => {
                program::write_file(*handle, *at, bytes)?;
                *at += bytes.len() as u64;
                Ok(())
            }
            Out::Pipe(end) => program::start::pieces(bytes)
                .try_for_each(|piece| program::send_waiting(*end, piece)),
        }
    }

    /// Where a program writing here writes: the file or the pipe, which go
    /// with it, or the shell's own output.
    #[cfg_attr(test, allow(dead_code))]
    fn handed(self) -> Option<Sink> {
        match self {
            Out::File { handle, .. } => Some(Sink::File(handle)),
            Out::Pipe(end) => Some(Sink::Channel(end)),
            Out::Mine => inherited(),
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    fn close(self) {
        if let Out::File { handle, .. } | Out::Pipe(handle) = self {
            let _ = program::close(handle);
        }
    }
}

/// Copies what `input` holds to `out` up to its end, byte for byte but on
/// the shell's own output, and says whether all of it went.
#[cfg_attr(test, allow(dead_code))]
fn copy(input: &mut In, out: &mut Out) -> bool {
    let mut room = [0u8; MESSAGE_BYTES];
    let mut held = Vec::new();
    loop {
        match input.take(&mut room) {
            Ok(0) => return held.is_empty() || out.say(&String::from_utf8_lossy(&held)).is_ok(),
            Ok(len) => {
                if out.put(&room[..len], &mut held).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

/// How much of `bytes` can be shown now: all of it, but for a character
/// at its end that the next piece may finish.
fn whole(bytes: &[u8]) -> usize {
    match core::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        _ => bytes.len(),
    }
}

/// Each command of `pipeline` with where it reads and where it writes:
/// the files its redirections lead to, else the pipes between it and the
/// commands beside it, else the shell's own. None once something did not
/// open, after saying so and closing what had.
#[cfg_attr(test, allow(dead_code))]
fn plumbed(pipeline: &[Ready]) -> Option<Vec<(&[String], In, Out)>> {
    let mut stages: Vec<(&[String], In, Out)> = Vec::new();
    let mut next = In::Mine;
    let mut failed = false;
    for (i, ready) in pipeline.iter().enumerate() {
        let Some((from, into)) = redirected(&ready.redirections) else {
            failed = true;
            break;
        };
        let input = match from {
            Some(file) => {
                core::mem::replace(&mut next, In::Mine).close();
                file
            }
            None => core::mem::replace(&mut next, In::Mine),
        };
        let out = if i + 1 == pipeline.len() {
            into.unwrap_or(Out::Mine)
        } else {
            let Ok((write, read)) = program::channel() else {
                let _ = program::write("|: no channel is left for the pipe\n");
                input.close();
                if let Some(file) = into {
                    file.close();
                }
                failed = true;
                break;
            };
            next = In::Pipe(read);
            match into {
                Some(file) => {
                    let _ = program::close(write);
                    file
                }
                None => Out::Pipe(write),
            }
        };
        stages.push((&ready.words, input, out));
    }
    if failed {
        next.close();
        for (_, input, out) in stages {
            input.close();
            out.close();
        }
        return None;
    }
    Some(stages)
}

/// Opens the files `redirections` lead to, in their order as sh does, and
/// answers the last one to read and the last one to write, at its end.
/// Says what went wrong and answers none when one does not open.
#[cfg_attr(test, allow(dead_code))]
fn redirected(redirections: &[(Redirect, String)]) -> Option<(Option<In>, Option<Out>)> {
    let (mut from, mut into) = (None::<In>, None::<Out>);
    for (how, path) in redirections {
        let opened = match opening(*how) {
            Some(how) => program::open_with(path, file::WRITE, how),
            None => program::open(path, file::READ),
        };
        let handle = match opened {
            Ok(handle) => handle,
            Err(e) => {
                let _ = program::write(&trouble(path, e));
                if let Some(file) = from {
                    file.close();
                }
                if let Some(file) = into {
                    file.close();
                }
                return None;
            }
        };
        if *how == Redirect::In {
            if let Some(file) = from.replace(In::File { handle, at: 0 }) {
                file.close();
            }
        } else {
            let at = program::info(handle).map_or(0, |i| i.size);
            if let Some(file) = into.replace(Out::File { handle, at }) {
                file.close();
            }
        }
    }
    Some((from, into))
}

/// A command of a pipeline on its way: done already, a program, or one of
/// the shell's own in a thread, which sends its status over a channel.
enum Going {
    Done(u8),
    Program(String, Handle),
    Thread(Handle),
}

impl Going {
    /// Waits until the command ended, and answers its status.
    #[cfg_attr(test, allow(dead_code))]
    fn finish(self) -> u8 {
        match self {
            Going::Done(status) => status,
            Going::Program(word, child) => {
                let ended = program::wait(child);
                let _ = program::close(child);
                match ended {
                    Ok(ended) => {
                        if let Some(line) = ending(&word, ended) {
                            let _ = program::write(&line);
                        }
                        status(ended)
                    }
                    Err(e) => {
                        let _ = program::write(&trouble(&word, e));
                        1
                    }
                }
            }
            Going::Thread(end) => {
                let mut room = [0u8; 1];
                let status = loop {
                    match program::receive(end, &mut room) {
                        Ok((1, _)) => break room[0],
                        Err(Error::Empty) => program::sleep(1),
                        _ => break 1,
                    }
                };
                let _ = program::close(end);
                status
            }
        }
    }
}

/// Starts the command `words` with what it reads and where it writes:
/// a program, or one of the shell's own, at once when it is `alone` and in
/// a thread of its own in a pipeline.
#[cfg_attr(test, allow(dead_code))]
fn begin(words: &[String], input: In, out: Out, alone: bool) -> Going {
    if let Command::Program(word, arguments) = command(words) {
        return start(word, arguments, input, out);
    }
    if alone {
        let (mut input, mut out) = (input, out);
        let status = run(command(words), &mut input, &mut out);
        input.close();
        out.close();
        return Going::Done(status);
    }
    let Ok((mine, theirs)) = program::channel() else {
        let _ = program::write("|: no channel is left for the pipe\n");
        input.close();
        out.close();
        return Going::Done(1);
    };
    let words = words.to_vec();
    let started = program::thread(move || {
        let (mut input, mut out) = (input, out);
        let status = run(command(&words), &mut input, &mut out);
        input.close();
        out.close();
        let _ = program::send(theirs, &[status], None);
        let _ = program::close(theirs);
        0
    });
    if started.is_err() {
        let _ = program::write("|: no thread is left for the pipe\n");
        let _ = program::close(mine);
        return Going::Done(1);
    }
    Going::Thread(mine)
}

/// Runs `command`, reading `input` and its text going to `out`, and
/// answers its status: a program's, or for the shell's own 0, and 1 when
/// something went wrong.
#[cfg_attr(test, allow(dead_code))]
fn run(command: Command<'_>, input: &mut In, out: &mut Out) -> u8 {
    let done = match command {
        Command::Program(word, arguments) => {
            let input = core::mem::replace(input, In::Mine);
            let out = core::mem::replace(out, Out::Mine);
            return start(word, arguments, input, out).finish();
        }
        Command::Status(status) => return status,
        Command::Help => out.say(HELP).is_ok(),
        Command::Echo(words) => out.say(&format!("{}\n", words.join(" "))).is_ok(),
        Command::List([]) => list(HOME, out),
        Command::List(paths) => every(paths, |path| list(path, out)),
        Command::Show([]) => copy(input, out),
        Command::Show(paths) => every(paths, |path| show(path, out)),
        Command::MakeDirectory(paths) => every(paths, |path| done(path, program::make_directory)),
        Command::Remove(paths) => every(paths, |path| done(path, program::remove)),
        Command::Clear => out.say("\u{c}").is_ok(),
        Command::Services => services(out),
        Command::Empty => true,
        Command::Missing(word) => {
            let _ = program::write(&format!("{word} needs a path\n"));
            false
        }
    };
    u8::from(!done)
}

/// Does `work` with each of `paths`, the ones after one that went wrong
/// too, and says whether all went well.
fn every(paths: &[String], mut work: impl FnMut(&str) -> bool) -> bool {
    paths.iter().filter(|path| !work(path)).count() == 0
}

/// Does `call` with `path`, says what went wrong when it did, and says
/// whether it went well.
#[cfg_attr(test, allow(dead_code))]
fn done(path: &str, call: fn(&str) -> Result<(), Error>) -> bool {
    call(path)
        .inspect_err(|&e| {
            let _ = program::write(&trouble(path, e));
        })
        .is_ok()
}

/// Asks init about each of its services in turn and shows what it says.
#[cfg_attr(test, allow(dead_code))]
fn services(out: &mut Out) -> bool {
    let line = Handle(LINE.load(SeqCst));
    if line.0 == 0 {
        let _ = program::write("services: init did not start this shell\n");
        return false;
    }
    out.say(&listing((0..=u8::MAX).map(|place| ask(line, place))))
        .is_ok()
}

/// What the shell shows for init's answers about the services, place
/// after place: each service until init says none is left, and what went
/// wrong when it gives no answer or something else. Asks no more once it
/// stopped.
fn listing(answers: impl IntoIterator<Item = Option<Vec<u8>>>) -> String {
    let mut text = String::new();
    for answer in answers {
        let Some(answer) = answer else {
            text.push_str("services: init does not answer\n");
            break;
        };
        match services::read(&answer) {
            Ok(Some(about)) => text.push_str(&shown(&about)),
            Ok(None) => break,
            Err(_) => {
                text.push_str("services: init answered something else\n");
                break;
            }
        }
    }
    text
}

/// Asks init over `line` about the service at `place`, and waits up to
/// a second for the answer.
#[cfg_attr(test, allow(dead_code))]
fn ask(line: Handle, place: u8) -> Option<Vec<u8>> {
    program::send(line, &services::ask(place), None).ok()?;
    let mut room = [0u8; MESSAGE];
    for _ in 0..100 {
        match program::receive(line, &mut room) {
            Ok((len, _)) => return Some(room[..len].to_vec()),
            Err(Error::Empty) => program::sleep(1),
            Err(_) => return None,
        }
    }
    None
}

/// Starts the program `word` names, handing it `arguments`, what it
/// reads and where it writes, which go with it.
#[cfg_attr(test, allow(dead_code))]
fn start(word: &str, arguments: &[String], input: In, out: Out) -> Going {
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let given = program::start::Given {
        input: input.handed(),
        output: out.handed(),
        ..program::start::Given::default()
    };
    match program::spawn_handing(&program_name(word, &mut System), &arguments, given) {
        Ok((child, start)) => {
            let _ = program::close(start);
            Going::Program(word.into(), child)
        }
        Err(Error::NotFound | Error::NoSuchProgram) if !word.starts_with('/') => {
            let _ = program::write(&format!("{word}: no such command\n"));
            Going::Done(127)
        }
        Err(e) => {
            let _ = program::write(&trouble(word, e));
            Going::Done(126)
        }
    }
}

/// A second handle to the shell's own standard output, for a program to
/// write there too (decision record 0019); none while the shell writes to
/// the console.
#[cfg_attr(test, allow(dead_code))]
fn inherited() -> Option<Sink> {
    Some(match program::start::given().output? {
        Sink::Channel(end) => Sink::Channel(program::duplicate(end).ok()?),
        Sink::File(file) => Sink::File(program::duplicate(file).ok()?),
    })
}

/// Shows the names in the directory at `path`, or the path of a file, and
/// says whether that went well.
#[cfg_attr(test, allow(dead_code))]
fn list(path: &str, out: &mut Out) -> bool {
    let directory = match program::open(path, file::READ) {
        Ok(d) => d,
        Err(e) => {
            let _ = program::write(&trouble(path, e));
            return false;
        }
    };
    let mut room = [0u8; 1024];
    let mut done = true;
    for index in 0.. {
        let said = match program::list(directory, index, &mut room) {
            Ok((len, is_directory)) => {
                let name = String::from_utf8_lossy(&room[..len]);
                let slash = if is_directory { "/" } else { "" };
                out.say(&format!("{name}{slash}\n"))
            }
            Err(Error::NotFound) => break,
            Err(Error::NotADirectory) => {
                done = out.say(&format!("{path}\n")).is_ok();
                break;
            }
            Err(e) => {
                let _ = program::write(&trouble(path, e));
                done = false;
                break;
            }
        };
        if said.is_err() {
            done = false;
            break;
        }
    }
    let _ = program::close(directory);
    done
}

/// Shows what the file at `path` holds, byte for byte but on the shell's
/// own output, and says whether that went well.
#[cfg_attr(test, allow(dead_code))]
fn show(path: &str, out: &mut Out) -> bool {
    let file = match program::open(path, file::READ) {
        Ok(f) => f,
        Err(e) => {
            let _ = program::write(&trouble(path, e));
            return false;
        }
    };
    let mut room = [0u8; 4096];
    let mut offset = 0;
    let mut held = Vec::new();
    let done = loop {
        match program::read(file, offset, &mut room) {
            Ok(0) => break held.is_empty() || out.say(&String::from_utf8_lossy(&held)).is_ok(),
            Ok(len) => {
                if out.put(&room[..len], &mut held).is_err() {
                    break false;
                }
                offset += len as u64;
            }
            Err(e) => {
                let _ = program::write(&trouble(path, e));
                break false;
            }
        }
    };
    let _ = program::close(file);
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(usage: u16) -> KeyEvent {
        KeyEvent {
            usage,
            pressed: true,
        }
    }

    fn up(usage: u16) -> KeyEvent {
        KeyEvent {
            usage,
            pressed: false,
        }
    }

    /// What typing `keys` one after another does.
    fn edits(keys: &[KeyEvent]) -> Vec<Edit> {
        let mut editor = Editor::default();
        keys.iter().map(|&k| editor.feed(k)).collect()
    }

    #[test]
    fn keys_type_a_line_that_enter_runs() {
        let typed = edits(&[down(0x0b), up(0x0b), down(0x0c), up(0x0c), down(0x28)]);
        assert_eq!(
            typed,
            [
                Edit::Type('h'),
                Edit::Nothing,
                Edit::Type('i'),
                Edit::Nothing,
                Edit::Run("hi".into())
            ]
        );
        let mut editor = Editor::default();
        editor.feed(down(0x04));
        editor.feed(down(0x28));
        assert_eq!(
            editor.feed(down(0x28)),
            Edit::Run(String::new()),
            "the line starts empty again"
        );
    }

    #[test]
    fn either_shift_makes_capitals_while_it_is_held() {
        let typed = edits(&[
            down(0xe1),
            down(0x16),
            up(0xe1),
            down(0x16),
            down(0xe5),
            down(0x38),
            up(0xe5),
        ]);
        assert_eq!(
            typed,
            [
                Edit::Nothing,
                Edit::Type('S'),
                Edit::Nothing,
                Edit::Type('s'),
                Edit::Nothing,
                Edit::Type('?'),
                Edit::Nothing
            ]
        );
        let both = edits(&[down(0xe1), down(0xe5), up(0xe1), down(0x04)]);
        assert_eq!(both[3], Edit::Type('A'), "the right one still held");
    }

    #[test]
    fn backspace_takes_back_what_was_typed() {
        let typed = edits(&[
            down(0x04),
            down(0x05),
            down(0x2a),
            down(0x2a),
            down(0x2a),
            down(0x06),
            down(0x28),
        ]);
        assert_eq!(
            typed,
            [
                Edit::Type('a'),
                Edit::Type('b'),
                Edit::Erase,
                Edit::Erase,
                Edit::Nothing,
                Edit::Type('c'),
                Edit::Run("c".into())
            ]
        );
    }

    #[test]
    fn keys_that_type_nothing_change_nothing() {
        assert_eq!(
            edits(&[down(0x52), down(0x29), down(0x3a), up(0x04)]),
            vec![Edit::Nothing; 4]
        );
        assert_eq!(
            edits(&[down(0xe0), down(0x04), down(0x07), up(0xe0), down(0x04)]),
            [
                Edit::Nothing,
                Edit::Nothing,
                Edit::Nothing,
                Edit::Nothing,
                Edit::Type('a')
            ],
            "nor do keys with control held"
        );
    }

    /// The words of `text`, as a command has them.
    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_first_word_says_which_command_runs() {
        assert_eq!(command(&words("help me")), Command::Help);
        let echo = words("echo hello there");
        assert_eq!(command(&echo), Command::Echo(&echo[1..]));
        assert_eq!(command(&words("echo")), Command::Echo(&[]));
        for name in ["ls", "dir"] {
            let w = words(&format!("{name} /a /b"));
            assert_eq!(command(&w), Command::List(&w[1..]));
            assert_eq!(command(&words(name)), Command::List(&[]), "or none");
        }
        for name in ["cat", "type"] {
            let w = words(&format!("{name} /a /b"));
            assert_eq!(command(&w), Command::Show(&w[1..]));
        }
        for name in ["mkdir", "md"] {
            let w = words(&format!("{name} /a"));
            assert_eq!(command(&w), Command::MakeDirectory(&w[1..]));
        }
        for name in ["rm", "del"] {
            let w = words(&format!("{name} /a /b"));
            assert_eq!(command(&w), Command::Remove(&w[1..]));
        }
        for name in ["mkdir", "md", "rm", "del"] {
            assert_eq!(command(&words(name)), Command::Missing(name));
        }
        for name in ["cat", "type"] {
            assert_eq!(
                command(&words(name)),
                Command::Show(&[]),
                "it shows what it reads"
            );
        }
        assert_eq!(
            (command(&words("true")), command(&words("false x"))),
            (Command::Status(0), Command::Status(1)),
            "for a pipeline, where the language does not see them"
        );
        for name in ["clear", "cls", "cls now"] {
            assert_eq!(command(&words(name)), Command::Clear);
        }
        assert_eq!(command(&words("services now")), Command::Services);
        assert_eq!(command(&[]), Command::Empty);
        let program = words("cp a b");
        assert_eq!(command(&program), Command::Program("cp", &program[1..]));
        assert_eq!(
            command(&words("Help")),
            Command::Program("Help", &[]),
            "commands are lower case"
        );
        let path = words("/System/Boot/meridian/programs/hello.mexec x");
        assert_eq!(
            command(&path),
            Command::Program("/System/Boot/meridian/programs/hello.mexec", &path[1..])
        );
        assert_eq!(
            command(&words("exit")),
            Command::Program("exit", &[]),
            "exit is the language's, and only a quoted one comes here"
        );
    }

    #[test]
    fn its_words_say_what_the_shell_runs() {
        assert_eq!(asked(&[]), Asked::Typed);
        let text = words("-c echo");
        assert_eq!(asked(&text[..1]), Asked::Unknown, "-c needs its script");
        assert_eq!(asked(&text), Asked::Text("echo"));
        let script = [String::from("-c"), String::from("echo a; echo b")];
        assert_eq!(asked(&script), Asked::Text("echo a; echo b"));
        assert_eq!(asked(&words("/System/a.sh")), Asked::File("/System/a.sh"));
        assert_eq!(asked(&words("/System/a.sh x")), Asked::Unknown);
        assert_eq!(asked(&words("-c a b")), Asked::Unknown);
        assert_eq!(asked(&words("-x a")), Asked::Unknown);
    }

    #[test]
    fn a_program_leaves_its_exit_code_or_its_exception_as_the_status() {
        let cases = [
            (Ended::Exited(0), 0),
            (Ended::Exited(7), 7),
            (Ended::Exited(255), 255),
            (Ended::Exited(256), 255),
            (Ended::Exited(2989), 255),
            (Ended::Faulted(0), 128),
            (Ended::Faulted(14), 142),
            (Ended::Faulted(127), 255),
            (Ended::Faulted(200), 255),
            (Ended::Faulted(256), 255),
        ];
        for (ended, want) in cases {
            assert_eq!(status(ended), want, "{ended:?}");
        }
    }

    #[test]
    fn lines_run_once_every_block_they_open_is_closed() {
        let statements = |typed: &str| match lines(typed) {
            Lines::Script(script) => script.len(),
            other => panic!("{typed:?} is {other:?}"),
        };
        assert_eq!(statements("echo hi\n"), 1);
        assert_eq!(statements("\n"), 0);
        assert_eq!(lines("if true {\n"), Lines::Open);
        assert_eq!(lines("if true {\necho a\n"), Lines::Open);
        assert_eq!(statements("if true {\necho a\n}\n"), 1);
        assert_eq!(
            lines("fn f {\nfor x in a {\n}\n"),
            Lines::Open,
            "one of two still open"
        );
        assert_eq!(statements("fn f {\nfor x in a {\n}\n}; f\n"), 2);
        assert_eq!(
            lines("}\n"),
            Lines::Wrong(::shell::Error {
                at: ::shell::Pos { line: 1, column: 1 },
                code: Code::UnexpectedClose
            })
        );
        assert!(
            matches!(lines("echo 'a\n"), Lines::Wrong(e) if e.code == Code::UnterminatedQuote),
            "a quote goes on no other line"
        );
        assert!(
            matches!(lines("if a {\n}}\n"), Lines::Wrong(e) if e.at.line == 2),
            "an error counts lines from the first"
        );
    }

    #[test]
    fn every_path_is_worked_on_after_one_that_went_wrong() {
        let paths = words("a b c d");
        let mut seen = Vec::new();
        let all = every(&paths, |path| {
            seen.push(String::from(path));
            path != "b"
        });
        assert!(!all);
        assert_eq!(seen, paths);
        assert!(every(&paths, |_| true));
        assert!(every(&[], |_| false), "none went wrong");
        assert!(!every(&paths, |path| path != "d"), "nor the last");
    }

    #[test]
    fn a_character_cut_short_waits_for_the_next_piece() {
        let e = "\u{e9}".as_bytes();
        assert_eq!(whole(b"abc"), 3);
        assert_eq!(whole(b""), 0);
        assert_eq!(whole(&[b'a', e[0]]), 1, "the first half of the \u{e9}");
        assert_eq!(whole(&[b'a', e[0], e[1]]), 3);
        let euro = "\u{20ac}".as_bytes();
        assert_eq!(whole(&euro[..2]), 0, "two bytes of three");
        assert_eq!(whole(&[0xff, b'a']), 2, "what can never be text goes on");
        assert_eq!(whole(&[b'a', 0xff, e[0]]), 3, "so does what comes after");
    }

    #[test]
    fn a_redirection_makes_its_file_new_or_adds_to_it() {
        assert_eq!(opening(Redirect::Out), Some(file::CREATE_ALWAYS));
        assert_eq!(opening(Redirect::Append), Some(file::OPEN_ALWAYS));
        assert_eq!(opening(Redirect::In), None);
    }

    /// Directories and files on the host, in a directory of their own
    /// among its temporary files, for patterns to match against; gone once
    /// dropped. Paths go from the tree's root and are found in capitals,
    /// as Meridian's tree of files finds them, and it keeps every path it
    /// was asked to list or to find.
    struct Host {
        root: std::path::PathBuf,
        listed: Vec<String>,
        asked: Vec<String>,
    }

    impl Host {
        /// The tree that holds `paths`, those that end in `/` directories.
        fn new(name: &str, paths: &[&str]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-shell-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for path in paths {
                let at = root.join(path.trim_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, b"").unwrap();
                }
            }
            Host {
                root,
                listed: Vec::new(),
                asked: Vec::new(),
            }
        }

        /// Where `path` is on the host, each name found in capitals; none
        /// when nothing is there or the path does not start at the root.
        fn find(&self, path: &str) -> Option<std::path::PathBuf> {
            let mut at = self.root.clone();
            for name in path.strip_prefix('/')?.split('/').filter(|n| !n.is_empty()) {
                let entry = std::fs::read_dir(&at).ok()?.find(|e| {
                    e.as_ref().is_ok_and(|e| {
                        e.file_name().to_string_lossy().to_uppercase() == name.to_uppercase()
                    })
                })?;
                at = entry.ok()?.path();
            }
            Some(at)
        }

        fn matching(&mut self, directory: &str, pattern: &str) -> Vec<String> {
            let pieces: Vec<Piece> = pattern
                .chars()
                .map(|c| match c {
                    '*' => Piece::Any,
                    '?' => Piece::One,
                    c => Piece::Char(c),
                })
                .collect();
            matching(directory, &pieces, self)
        }
    }

    impl Tree for Host {
        fn names(&mut self, path: &str) -> Vec<String> {
            self.listed.push(path.into());
            self.find(path)
                .and_then(|at| std::fs::read_dir(at).ok())
                .map(|dir| {
                    dir.map(|e| e.unwrap().file_name().into_string().unwrap())
                        .collect()
                })
                .unwrap_or_default()
        }

        fn has(&mut self, path: &str) -> bool {
            self.asked.push(path.into());
            self.find(path).is_some()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_pattern_finds_the_paths_it_matches_part_by_part() {
        let mut tree = Host::new(
            "find",
            &[
                "System/a.meri",
                "System/B.MERI",
                "System/c.txt",
                "System/d.meri/x",
                "Other/x",
                "Other/y/",
                ".hidden",
            ],
        );
        assert_eq!(
            tree.matching("/", "/System/*.meri"),
            ["/System/a.meri", "/System/B.MERI", "/System/d.meri"],
            "in the order of their names in capitals"
        );
        assert_eq!(tree.matching("/", "/System/?.txt"), ["/System/c.txt"]);
        assert_eq!(
            tree.matching("/", "/system/C.*"),
            ["/system/c.txt"],
            "a part without a wildcard as written"
        );
        assert_eq!(tree.matching("/", "/*/x"), ["/Other/x"]);
        assert_eq!(tree.matching("/", "/*/*/x"), ["/System/d.meri/x"]);
        assert_eq!(tree.matching("/", "/Other/*"), ["/Other/x", "/Other/y"]);
        assert_eq!(
            tree.matching("/System", "*.txt"),
            ["c.txt"],
            "from the directory, as written"
        );
        assert_eq!(tree.matching("/System/", "d*/x"), ["d.meri/x"]);
        assert_eq!(
            tree.matching("/System", "/Other/*"),
            ["/Other/x", "/Other/y"],
            "from the root wherever the directory is"
        );
        assert_eq!(tree.matching("/", "*"), [".hidden", "Other", "System"]);
        assert_eq!(tree.matching("/", "/System//c.*"), ["/System/c.txt"]);
        let none: [&str; 0] = [];
        assert_eq!(tree.matching("/", "/nope/*"), none);
        assert_eq!(
            tree.matching("/", "/System/c.txt/*"),
            none,
            "a file holds none"
        );
        assert_eq!(tree.matching("/", "/System/e*"), none);
        assert_eq!(tree.matching("/", "/Other/x/z?"), none);
        assert_eq!(
            tree.matching("/", "/*/X"),
            ["/Other/X"],
            "a last part as written, once something is there"
        );
        assert_eq!(tree.matching("/", "/*/nope"), none);
    }

    #[test]
    fn only_the_directories_a_wildcard_is_in_are_listed() {
        let mut tree = Host::new("listed", &["System/Boot/a.meri", "System/Boot/b.txt"]);
        assert_eq!(
            tree.matching("/", "/System/Boot/*.meri"),
            ["/System/Boot/a.meri"]
        );
        assert_eq!(
            tree.listed,
            ["/System/Boot"],
            "not the root, which holds mounts, nor /System"
        );
        tree.listed.clear();
        assert_eq!(tree.matching("/", "/S*/Boot/a*"), ["/System/Boot/a.meri"]);
        assert_eq!(tree.listed, ["/", "/System/Boot"]);
        tree.listed.clear();
        assert_eq!(tree.matching("/System", "Boot/?.txt"), ["Boot/b.txt"]);
        assert_eq!(tree.listed, ["/System/Boot"]);
        assert_eq!(
            tree.asked,
            Vec::<String>::new(),
            "what a listing found is there"
        );
        assert_eq!(tree.matching("/", "/S*/boot"), ["/System/boot"]);
        assert_eq!(tree.asked, ["/System/boot"], "and the rest is looked for");
    }

    #[test]
    fn a_program_is_found_by_its_name_or_its_path() {
        let mut tree = Host::new(
            "programs",
            &[
                "System/Boot/meridian/programs/hello.mexec",
                "System/Boot/meridian/programs/a/b.mexec",
            ],
        );
        assert_eq!(
            program_name("hello", &mut tree),
            "/System/Boot/meridian/programs/hello.mexec"
        );
        assert_eq!(
            program_name("a/b", &mut tree),
            "/System/Boot/meridian/programs/a/b.mexec"
        );
        assert_eq!(
            program_name("wc", &mut tree),
            "wc.mexec",
            "the loader's, without the file"
        );
        assert_eq!(program_name("/a/b.mexec", &mut tree), "/a/b.mexec");
        assert_eq!(
            tree.asked,
            [
                "/System/Boot/meridian/programs/hello.mexec",
                "/System/Boot/meridian/programs/a/b.mexec",
                "/System/Boot/meridian/programs/wc.mexec"
            ],
            "a path is taken as it is"
        );
        assert_eq!(PROGRAMS, "/System/Boot/meridian/programs");
    }

    #[test]
    fn a_program_that_ended_well_says_nothing() {
        assert_eq!(ending("hello", Ended::Exited(0)), None);
        assert_eq!(
            ending("exitwith", Ended::Exited(2989)).as_deref(),
            Some("exitwith ended with 2989\n")
        );
        assert_eq!(
            ending("deep", Ended::Faulted(14)).as_deref(),
            Some("deep stopped by exception 14\n")
        );
        assert_eq!(
            ending("one", Ended::Exited(1)).as_deref(),
            Some("one ended with 1\n")
        );
    }

    #[test]
    fn a_service_is_shown_with_what_it_does_and_how_often_it_started() {
        use services::State;
        let about = |name, state, starts| About {
            name,
            state,
            starts,
        };
        assert_eq!(
            shown(&about("exitwith", State::Ended, 1)),
            "exitwith: ended, started once\n"
        );
        assert_eq!(
            shown(&about("whoami", State::Resting, 6)),
            "whoami: resting, started 6 times\n"
        );
        assert_eq!(
            shown(&about("disk", State::Waiting, 0)),
            "disk: waiting, started 0 times\n"
        );
        assert_eq!(
            shown(&about("shell", State::Running, 2)),
            "shell: running, started 2 times\n"
        );
    }

    /// init's answer about `about`, as it sends it.
    fn told(about: Option<About<'_>>) -> Option<Vec<u8>> {
        let mut room = [0u8; MESSAGE];
        let len = services::answer(about, &mut room);
        room.get(..len).map(<[u8]>::to_vec)
    }

    fn running(name: &str, starts: u32) -> About<'_> {
        About {
            name,
            state: services::State::Running,
            starts,
        }
    }

    #[test]
    fn the_services_are_listed_until_init_says_none_is_left() {
        let ended = About {
            name: "exitwith",
            state: services::State::Ended,
            starts: 1,
        };
        let answers = [
            told(Some(ended)),
            told(Some(running("shell", 2))),
            told(None),
            told(Some(running("never shown", 1))),
        ];
        assert_eq!(
            listing(answers),
            "exitwith: ended, started once\nshell: running, started 2 times\n"
        );
        assert_eq!(listing([told(None)]), "", "no services");
        assert_eq!(listing([]), "");
    }

    #[test]
    fn the_listing_says_when_init_stops_answering_or_answers_something_else() {
        let shell = told(Some(running("shell", 1)));
        assert_eq!(
            listing([shell.clone(), None, told(None)]),
            "shell: running, started once\nservices: init does not answer\n"
        );
        assert_eq!(
            listing([Some(b"?\x01".to_vec()), shell.clone()]),
            "services: init answered something else\n"
        );
        assert_eq!(
            listing([shell, Some(Vec::new())]),
            "shell: running, started once\nservices: init answered something else\n"
        );
    }

    #[test]
    fn the_listing_asks_no_more_once_init_said_none_is_left() {
        let mut asked = 0;
        let text = listing((0..=u8::MAX).map(|place| {
            asked += 1;
            told((place < 2).then(|| running("s", u32::from(place))))
        }));
        assert_eq!(asked, 3);
        assert_eq!(
            text,
            "s: running, started 0 times\ns: running, started once\n"
        );
    }

    #[test]
    fn help_names_every_command_the_shell_knows() {
        let known = [
            "help", "echo", "ls", "dir", "cat", "type", "mkdir", "md", "rm", "del", "clear", "cls",
            "services", "exit",
        ];
        let (first, second) = HELP.split_once('\n').unwrap();
        let named: Vec<&str> = first
            .split(|c: char| !c.is_ascii_alphabetic())
            .filter(|w| !w.is_empty())
            .collect();
        for word in known {
            assert!(named.contains(&word), "help leaves out {word}");
            assert!(
                word == "exit" || !matches!(command(&words(word)), Command::Program(..)),
                "{word} is no command"
            );
        }
        for word in named {
            assert!(
                known.contains(&word) || ["or", "a", "program"].contains(&word),
                "help names {word}, which the shell does not know"
            );
        }
        assert_eq!(
            parse::script("exit"),
            Ok(Vec::from([Statement::Exit(None)]))
        );
        for keyword in ["let", "if", "for", "while", "fn", "> PATH", ">> PATH"] {
            assert!(second.contains(keyword), "help leaves out {keyword}");
        }
        assert!(second.ends_with('\n') && !second.trim_end().contains('\n'));
        assert_eq!(command(&words("nope")), Command::Program("nope", &[]));
    }

    #[test]
    fn trouble_says_what_went_wrong_with_which_path() {
        assert_eq!(trouble("/x", Error::NotFound), "/x: no such file\n");
        assert_eq!(
            trouble("/System/Boot/meridian/boot.meri", Error::NotADirectory),
            "/System/Boot/meridian/boot.meri: not a directory\n"
        );
        assert_eq!(
            trouble("/a", Error::NotAFile),
            "/a: a directory, not a file\n"
        );
        assert_eq!(trouble("/a", Error::Denied), "/a: not allowed\n");
        assert_eq!(
            trouble("a", Error::BadArgument),
            "a: not a path from the root\n"
        );
        assert_eq!(trouble("/a", Error::TooLong), "/a: too long a path\n");
        assert_eq!(trouble("/a", Error::NotAProgram), "/a: not a program\n");
        assert_eq!(trouble("/a", Error::Unreadable), "/a: could not be read\n");
        let writes = [
            (Error::ReadOnly, "/a: on a volume that is only read\n"),
            (Error::Exists, "/a: there already\n"),
            (Error::NotEmpty, "/a: a directory that holds something\n"),
            (Error::InUse, "/a: open, so it stays\n"),
            (Error::DiskFull, "/a: no room left on its disk\n"),
            (Error::Unwritable, "/a: could not be written\n"),
        ];
        for (e, said) in writes {
            assert_eq!(trouble("/a", e), said);
        }
    }
}
