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
use ::shell::parse::{self, Statement};
use ::shell::run::{Flow, Machine, Ready, Redirection, Session, Target};
use ::shell::sh::{self, run::Stop};
use ::shell::walk::Tree;
use program::abi::{MESSAGE_BYTES, file};
use program::layout::{Held, Typed};
use program::{Ended, Error, Handle, KeyEvent, start::Sink};
use services::{About, MESSAGE};

// The sh dialect runs a script by recursion, a few KiB of stack for each
// command running inside another.
program::main!(shell, stack = 256);

/// What a key does to the line being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Edit {
    /// The character goes on the line, and on the console.
    Type(char),
    /// The last character goes.
    Erase,
    /// The line is done.
    Run(String),
    /// The line shown goes, `erase` characters of it, for `with`, a line
    /// from before or the one that was being typed, or the line completed.
    Recall { erase: usize, with: String },
    /// Tab: the shell completes the line's last word, if it can.
    Complete,
    /// Nothing changes.
    Nothing,
}

/// How many lines run before the editor keeps for the arrow keys.
const HISTORY: usize = 100;

/// The line being typed, which shift and control keys are held, and the
/// lines run before, oldest first, which up and down bring back.
#[derive(Default)]
struct Editor {
    line: String,
    held: Held,
    history: Vec<String>,
    /// Which line of the history is shown, none for the one being typed.
    shown: Option<usize>,
    /// The line that was being typed while one from before is shown.
    kept: String,
}

impl Editor {
    fn feed(&mut self, key: KeyEvent) -> Edit {
        if key.pressed && matches!(key.usage, 0x51 | 0x52) {
            return self.recall(key.usage == 0x52);
        }
        let edit = self.edit(key);
        if !matches!(edit, Edit::Nothing) {
            // A line from before that is changed or run is the one typed.
            self.shown = None;
        }
        if let Edit::Run(line) = &edit {
            self.kept.clear();
            if !line.trim().is_empty() && self.history.last() != Some(line) {
                if self.history.len() == HISTORY {
                    self.history.remove(0);
                }
                self.history.push(line.clone());
            }
        }
        edit
    }

    /// Shows the line before the one shown, when `up`, or the one after,
    /// which past the newest is the line that was being typed.
    fn recall(&mut self, up: bool) -> Edit {
        let newest = self.history.len().checked_sub(1);
        let next = match (self.shown, up) {
            (None, true) => newest,
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) => (i + 1 < self.history.len()).then_some(i + 1),
        };
        if next == self.shown {
            return Edit::Nothing;
        }
        if self.shown.is_none() {
            self.kept = self.line.clone();
        }
        let with = match next {
            Some(i) => self.history[i].clone(),
            None => core::mem::take(&mut self.kept),
        };
        let erase = self.line.chars().count();
        self.line.clone_from(&with);
        self.shown = next;
        Edit::Recall { erase, with }
    }

    /// Puts `line` in place of the one shown, as Tab completed it.
    fn replace(&mut self, line: String) -> Edit {
        let erase = self.line.chars().count();
        self.line.clone_from(&line);
        self.shown = None;
        Edit::Recall { erase, with: line }
    }

    fn edit(&mut self, key: KeyEvent) -> Edit {
        match self.held.feed(key) {
            Typed::Char('\n') => Edit::Run(core::mem::take(&mut self.line)),
            Typed::Char('\t') => Edit::Complete,
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
    /// echo, printf, test and their kin, which lib/shell runs alike for
    /// both dialects (decision record 0021): all the words, the name first.
    Shared(&'a [String]),
    List(&'a [String]),
    Show(&'a [String]),
    MakeDirectory(&'a [String]),
    Remove(&'a [String]),
    RemoveDirectory(&'a [String]),
    /// `cp`: the paths, the last of them where the others go.
    Copy(&'a [String]),
    /// `mv`: the paths, the last of them where the others go.
    Move(&'a [String]),
    Clear,
    Services,
    /// `cd`: where to go, or nothing to show where the shell is.
    ChangeDirectory(&'a [String]),
    WorkingDirectory,
    Empty,
    /// A program, by its name in [`PROGRAMS`] or by its path, and the
    /// words it is handed.
    Program(&'a str, &'a [String]),
    /// A command that takes paths and was given none.
    Missing(&'a str),
}

/// What `help` says: the commands on its first line.
const HELP: &str = "help, echo, printf, test or [, true, false, cd or chdir, pwd, ls or dir, cat or type, cp or copy, \
                    mv or move, mkdir or md, rm or del, rmdir or rd, clear or cls, services, sh, \
                    exit, or a program\n\
                    let, if, for, while and fn as blocks in braces; > PATH or >> PATH after a \
                    command sends what it writes into a file\n";

/// Where a shell that reads what is typed starts, when it is there and
/// the shell was handed no working directory.
const HOME: &str = "/System";

/// Where a program named without a path is.
const PROGRAMS: &str = "/System/Boot/meridian/programs";

fn command(words: &[String]) -> Command<'_> {
    let Some((word, rest)) = words.split_first() else {
        return Command::Empty;
    };
    match word.as_str() {
        name if ::shell::commands::knows(name) => Command::Shared(words),
        "help" => Command::Help,
        "ls" | "dir" => Command::List(rest),
        "mkdir" | "md" | "rm" | "del" | "rmdir" | "rd" if rest.is_empty() => Command::Missing(word),
        "cp" | "copy" | "mv" | "move" if rest.len() < 2 => Command::Missing(word),
        "cat" | "type" => Command::Show(rest),
        "cp" | "copy" => Command::Copy(rest),
        "mv" | "move" => Command::Move(rest),
        "mkdir" | "md" => Command::MakeDirectory(rest),
        "rm" | "del" => Command::Remove(rest),
        "rmdir" | "rd" => Command::RemoveDirectory(rest),
        "clear" | "cls" => Command::Clear,
        "services" => Command::Services,
        "cd" | "chdir" => Command::ChangeDirectory(rest),
        "pwd" => Command::WorkingDirectory,
        _ => Command::Program(word, rest),
    }
}

/// What the shell says when the command `word` was given too few paths.
fn missing(word: &str) -> String {
    match word {
        "cp" | "copy" | "mv" | "move" => format!("{word} needs a path and where it goes\n"),
        _ => format!("{word} needs a path\n"),
    }
}

/// The name `path` ends in, after its last slash.
fn last_name(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    path.rsplit('/').next().unwrap_or(path)
}

/// Where each path but the last of `paths` goes for cp and mv: into the
/// last, by its own name, when that is a directory, or else to the last,
/// which takes one path alone. None for more than one path going to what
/// is no directory.
fn destinations(paths: &[String], into_directory: bool) -> Option<Vec<(&str, String)>> {
    let (target, sources) = paths.split_last()?;
    if into_directory {
        let slash = if target.ends_with('/') { "" } else { "/" };
        return Some(
            sources
                .iter()
                .map(|s| (s.as_str(), format!("{target}{slash}{}", last_name(s))))
                .collect(),
        );
    }
    match sources {
        [one] => Some(Vec::from([(one.as_str(), target.clone())])),
        _ => None,
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
        Error::Inside => "a directory, which cannot go inside itself",
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

/// `line` with its last word completed to the name in `tree` it starts, or
/// as far as the names it starts agree, compared in capitals and written
/// as the first of them is; the word is a path from `directory` when it
/// does not start at the root. None when no name starts with it, or none
/// adds to it.
fn completed(line: &str, directory: &str, tree: &mut impl Tree) -> Option<String> {
    let start = line.rfind(' ').map_or(0, |at| at + 1);
    let word = &line[start..];
    let (folder, begun) = match word.rfind('/') {
        Some(at) => (&word[..=at], &word[at + 1..]),
        None => ("", word),
    };
    let listed = if folder.starts_with('/') {
        String::from(folder)
    } else {
        format!("{}/{folder}", directory.trim_end_matches('/'))
    };
    let begun_upper = begun.to_uppercase();
    let mut names = tree.names(&listed);
    names.retain(|n| n != "." && n != ".." && n.to_uppercase().starts_with(&begun_upper));
    names.sort_by_cached_key(|name| name.to_uppercase());
    let first = names.first()?;
    let agreed = first
        .char_indices()
        .map(|(at, c)| at + c.len_utf8())
        .take_while(|&end| {
            let head = first[..end].to_uppercase();
            names.iter().all(|n| n.to_uppercase().starts_with(&head))
        })
        .last()
        .unwrap_or(0);
    let done = format!("{}{folder}{}", &line[..start], &first[..agreed]);
    (done.chars().count() > line.chars().count()).then_some(done)
}

/// The line to init the shell started with, 0 when init did not start it.
static LINE: AtomicU64 = AtomicU64::new(0);

/// What the shell was started to do, by the words it was handed after a
/// `--sh`, which asks for the sh dialect.
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

/// What the shell was started to do, and whether in the sh dialect.
fn asked(arguments: &[String]) -> (Asked<'_>, bool) {
    match arguments.split_first() {
        Some((flag, rest)) if flag == "--sh" => (what(rest), true),
        _ => (what(arguments), false),
    }
}

fn what(arguments: &[String]) -> Asked<'_> {
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
    let (asked, sh) = asked(program::arguments());
    let status = match asked {
        Asked::Typed if sh => typed(&mut ShTyped(sh::run::Session::new(
            "sh".into(),
            0,
            Vec::new(),
        ))),
        Asked::Typed => typed(&mut OwnTyped(Session::new())),
        Asked::Text(text) => script(text, "shell", sh),
        Asked::File(path) => match text_of(path) {
            Ok(text) => script(&text, path, sh),
            Err(e) => {
                let _ = program::write_error(&trouble(path, e));
                2
            }
        },
        Asked::Unknown => {
            let _ = program::write_error(
                "shell: start it with no words, -c SCRIPT, or a script's path, after --sh for sh\n",
            );
            2
        }
    };
    u64::from(status)
}

/// Whether `text` is a sh script by its first line, `#!/bin/sh` or
/// `#!sh`, blanks after the `#!` or at the end let be, as decision record
/// 0016 has it.
fn is_sh(text: &str) -> bool {
    let first = text.split('\n').next().unwrap_or_default();
    first
        .strip_prefix("#!")
        .is_some_and(|rest| matches!(rest.trim(), "/bin/sh" | "sh"))
}

/// Runs `text` as a script called `zero`, in the sh dialect when `sh` or
/// its first line asks for it, saying what does not read or run, and
/// answers the status it ended with: 2 when something did not.
#[cfg_attr(test, allow(dead_code))]
fn script(text: &str, zero: &str, sh: bool) -> u8 {
    if sh || is_sh(text) {
        return sh_script(text, zero);
    }
    let mut session = Session::new();
    let ran = parse::script(text).and_then(|script| session.run(&script, &mut System::default()));
    match ran {
        Ok(_) => session.status(),
        Err(e) => {
            let _ = program::write_error(&format!("{e}\n"));
            2
        }
    }
}

/// Runs `text` as a sh script called `zero`, as [`script`] does. `$$` is
/// 0: Meridian gives a process no number it can know itself by yet.
#[cfg_attr(test, allow(dead_code))]
fn sh_script(text: &str, zero: &str) -> u8 {
    let items = match sh::parse::script(text) {
        Ok(items) => items,
        Err(e) => {
            let _ = program::write_error(&format!("{e}\n"));
            return 2;
        }
    };
    let mut session = sh::run::Session::new(zero.into(), 0, Vec::new());
    session.set_variable("HOME", HOME.into());
    session.set_variable("PWD", program::directory());
    let mut system = System::default();
    let ran = session.run(&items, &mut system);
    match session.finish(ran, &mut system) {
        Ok(_) => session.status(),
        Err(Stop::Exit(status) | Stop::Said(status)) => status,
        Err(Stop::Error(e)) => {
            let _ = program::write_error(&format!("{e}\n"));
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

/// A dialect lines typed at the console run in.
trait Typing {
    /// What the shell says as it starts, its prompt last.
    fn greeting(&self) -> &'static str;

    /// Runs the lines `typed` so far, once they read, and takes them;
    /// answers the prompt for the next line, or the status the shell ends
    /// with.
    fn lines(&mut self, typed: &mut String) -> Result<&'static str, u8>;
}

/// Meridian's own language, typed.
struct OwnTyped(Session);

impl Typing for OwnTyped {
    fn greeting(&self) -> &'static str {
        "Meridian shell. Type help to see what it knows.\n> "
    }

    #[cfg_attr(test, allow(dead_code))]
    fn lines(&mut self, typed: &mut String) -> Result<&'static str, u8> {
        match lines(typed) {
            Lines::Open => Ok("... "),
            Lines::Script(script) => {
                typed.clear();
                match self.0.run(&script, &mut System::default()) {
                    Ok(Flow::Exit) => return Err(self.0.status()),
                    Ok(_) => {}
                    Err(e) => {
                        let _ = program::write_error(&format!("{e}\n"));
                    }
                }
                Ok("> ")
            }
            Lines::Wrong(e) => {
                typed.clear();
                let _ = program::write_error(&format!("{e}\n"));
                Ok("> ")
            }
        }
    }
}

/// The sh dialect, typed, as `sh` starts it: `$ ` the prompt, and `> `
/// while what was typed wants more lines. What goes wrong ends no more
/// than the lines it is in, but `exit` ends the shell.
struct ShTyped(sh::run::Session);

impl Typing for ShTyped {
    fn greeting(&self) -> &'static str {
        "$ "
    }

    #[cfg_attr(test, allow(dead_code))]
    fn lines(&mut self, typed: &mut String) -> Result<&'static str, u8> {
        if sh::parse::wants_more(typed) {
            return Ok("> ");
        }
        let read = sh::parse::script(typed);
        typed.clear();
        let items = match read {
            Ok(items) => items,
            Err(e) => {
                let _ = program::write_error(&format!("{e}\n"));
                return Ok("$ ");
            }
        };
        let mut system = System::default();
        match self.0.run(&items, &mut system) {
            Err(Stop::Exit(status)) => {
                return Err(match self.0.finish(Err(Stop::Exit(status)), &mut system) {
                    Err(Stop::Exit(status) | Stop::Said(status)) => status,
                    Err(Stop::Error(_)) => 2,
                    Ok(_) => status,
                });
            }
            Err(Stop::Error(e)) => {
                let _ = program::write_error(&format!("{e}\n"));
            }
            Err(Stop::Said(_)) | Ok(_) => {}
        }
        Ok("$ ")
    }
}

/// Runs the lines typed at the console in `dialect` until `exit`, and
/// answers the status it gave.
#[cfg_attr(test, allow(dead_code))]
fn typed(dialect: &mut impl Typing) -> u8 {
    if program::directory() == "/" {
        let _ = program::change_directory(HOME);
    }
    let _ = program::write(dialect.greeting());
    let mut editor = Editor::default();
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
            Edit::Recall { erase, with } => program::write(&("\u{8}".repeat(erase) + &with)),
            Edit::Complete => {
                match completed(&editor.line, &program::directory(), &mut System::default()) {
                    Some(line) => match editor.replace(line) {
                        Edit::Recall { erase, with } => {
                            program::write(&("\u{8}".repeat(erase) + &with))
                        }
                        _ => Ok(()),
                    },
                    None => Ok(()),
                }
            }
            Edit::Run(line) => {
                let _ = program::write("\n");
                typed.push_str(&line);
                typed.push('\n');
                match dialect.lines(&mut typed) {
                    Ok(prompt) => program::write(prompt),
                    Err(status) => return status,
                }
            }
            Edit::Nothing => Ok(()),
        };
    }
}

/// What the shell runs commands on: the calls of the runtime, the streams
/// the redirections of compound commands and functions, the captures of
/// `$(...)` and subshells make, innermost last, and those `exec` made the
/// shell's own.
#[derive(Default)]
struct System {
    levels: Vec<Level>,
    base: Frame,
    /// The commands of each job started, by its number less one, until it
    /// is waited for.
    jobs: Vec<Option<Vec<Going>>>,
    /// What the shell's own input brought past the line `read` took last.
    left: Vec<u8>,
}

/// One of the shell's levels of streams: a capture, none for one no
/// channel or thread was left for, which takes nothing, or the streams
/// redirections made.
enum Level {
    Capture(Option<Capture>),
    Frame(Frame),
}

/// The streams the redirections of a compound command or a function made
/// for all that runs inside it: what stream 0 reads and where 1 and 2 go,
/// none for one they left as it was.
#[derive(Default)]
struct Frame {
    input: Option<In>,
    out: Option<Out>,
    errors: Option<Out>,
    /// What its input brought past the line `read` took last.
    left: Vec<u8>,
    /// Whether it is a subshell's own, where what `exec` changes stays.
    subshell: bool,
}

impl Frame {
    #[cfg_attr(test, allow(dead_code))]
    fn close(self) {
        if let Some(input) = self.input {
            input.close();
        }
        for out in [self.out, self.errors].into_iter().flatten() {
            out.close();
        }
    }
}

/// The streams a pipeline's commands have where their redirections name
/// none: what the first reads, what the last writes, and where each says
/// what went wrong.
struct Defaults {
    input: In,
    out: Out,
    errors: Out,
}

impl System {
    /// The commands of `pipeline` started on their streams, its own ones
    /// in threads of their own when it has more than one or `background`;
    /// the status it ends with, said why, when it cannot start.
    fn going(&mut self, pipeline: &[Ready], background: bool) -> Result<Vec<Going>, u8> {
        let Ok(mut defaults) = self.defaults() else {
            let _ = program::write_error("|: no handle is left for the streams\n");
            return Err(1);
        };
        if background {
            core::mem::replace(&mut defaults.input, In::Closed).close();
        }
        let stages = match plumbed(pipeline, defaults) {
            Ok(stages) => stages,
            Err(said) => {
                let _ = self.err(said.as_bytes());
                return Err(1);
            }
        };
        let alone = stages.len() == 1 && !background;
        Ok(stages
            .into_iter()
            .map(|(words, streams)| begin(words, streams, alone))
            .collect())
    }

    /// Waits for every command `going`, and answers the last one's
    /// status. How they ended is said once all did, after all they wrote.
    fn finished(&mut self, going: Vec<Going>) -> u8 {
        let mut said = String::new();
        let status = going.into_iter().fold(0, |_, command| {
            let (status, line) = command.finish();
            said.extend(line);
            status
        });
        if !said.is_empty() {
            let _ = self.err(said.as_bytes());
        }
        status
    }

    /// A copy of what the innermost level that makes it reads from.
    fn input(&self) -> Result<In, Error> {
        for level in self.levels.iter().rev() {
            if let Level::Frame(Frame {
                input: Some(input), ..
            }) = level
            {
                return input.copy();
            }
        }
        self.base.input.as_ref().map_or(Ok(In::Mine), In::copy)
    }

    /// A copy of where the innermost level that makes it writes.
    fn output(&self) -> Result<Out, Error> {
        for level in self.levels.iter().rev() {
            match level {
                Level::Capture(Some(capture)) => {
                    return program::duplicate(capture.into).map(Out::Pipe);
                }
                Level::Capture(None) => return Ok(Out::Closed),
                Level::Frame(Frame { out: Some(out), .. }) => return out.copy(),
                Level::Frame(_) => {}
            }
        }
        self.base.out.as_ref().map_or(Ok(Out::Mine), Out::copy)
    }

    /// A copy of where the innermost level that makes it says what went
    /// wrong.
    fn errors(&self) -> Result<Out, Error> {
        for level in self.levels.iter().rev() {
            if let Level::Frame(Frame {
                errors: Some(errors),
                ..
            }) = level
            {
                return errors.copy();
            }
        }
        self.base
            .errors
            .as_ref()
            .map_or(Ok(Out::MyErrors), Out::copy)
    }

    /// Where `exec` changes a stream: the innermost frame that sets it, as
    /// `sets` says, or a subshell's own, or else the shell's.
    fn setting(&mut self, sets: impl Fn(&Frame) -> bool) -> &mut Frame {
        let at = self.levels.iter().rposition(
            |level| matches!(level, Level::Frame(frame) if frame.subshell || sets(frame)),
        );
        match at.map(|at| &mut self.levels[at]) {
            Some(Level::Frame(frame)) => frame,
            _ => &mut self.base,
        }
    }

    fn defaults(&self) -> Result<Defaults, Error> {
        let input = self.input()?;
        let out = match self.output() {
            Ok(out) => out,
            Err(e) => {
                input.close();
                return Err(e);
            }
        };
        match self.errors() {
            Ok(errors) => Ok(Defaults { input, out, errors }),
            Err(e) => {
                input.close();
                out.close();
                Err(e)
            }
        }
    }
}

/// A capture: the channel what is taken is written into, and the one the
/// thread that reads it hands it back over once the first has ended.
struct Capture {
    into: Handle,
    back: Handle,
}

/// Starts a capture: a channel the commands write into, read by a thread
/// of its own so that no writer waits for room, which hands all it read
/// back over a second channel once every writer closed its end.
#[cfg_attr(test, allow(dead_code))]
fn capture() -> Option<Capture> {
    let (into, from) = program::channel().ok()?;
    let Ok((back_into, back)) = program::channel() else {
        let _ = (program::close(into), program::close(from));
        return None;
    };
    let read = program::thread(move || {
        let mut text = Vec::new();
        let mut room = [0u8; MESSAGE_BYTES];
        while let Ok(len @ 1..) = program::stream(from, &mut room) {
            text.extend_from_slice(&room[..len]);
        }
        let _ = program::close(from);
        let _ = program::start::pieces(&text)
            .try_for_each(|piece| program::send_waiting(back_into, piece));
        let _ = program::close(back_into);
        0
    });
    if read.is_err() {
        for end in [into, from, back_into, back] {
            let _ = program::close(end);
        }
        return None;
    }
    Some(Capture { into, back })
}

impl Machine for System {
    fn run(&mut self, pipeline: &[Ready]) -> u8 {
        match self.going(pipeline, false) {
            Ok(going) => self.finished(going),
            Err(status) => status,
        }
    }

    /// Starts the pipeline as a job and goes on, its first command's stream
    /// 0 closed unless a redirection gives it one.
    fn start(&mut self, pipeline: &[Ready]) -> u64 {
        let going = self
            .going(pipeline, true)
            .unwrap_or_else(|status| Vec::from([Going::Done(status)]));
        self.jobs.push(Some(going));
        self.jobs.len() as u64
    }

    fn wait(&mut self, job: Option<u64>) -> Option<u8> {
        let Some(job) = job else {
            let jobs: Vec<Vec<Going>> = self.jobs.iter_mut().filter_map(Option::take).collect();
            for going in jobs {
                self.finished(going);
            }
            return Some(0);
        };
        let at = usize::try_from(job).ok()?.checked_sub(1)?;
        let going = self.jobs.get_mut(at)?.take()?;
        Some(self.finished(going))
    }

    // The runtime keeps the working directory, which `cd` changes, so the
    // session's is the root.
    fn out(&mut self, bytes: &[u8]) -> bool {
        self.output().is_ok_and(|mut out| {
            let went = written(&mut out, bytes);
            out.close();
            went
        })
    }

    fn err(&mut self, bytes: &[u8]) -> bool {
        self.errors().is_ok_and(|mut errors| {
            let went = written(&mut errors, bytes);
            errors.close();
            went
        })
    }

    fn begin_capture(&mut self) {
        let capture = capture();
        if capture.is_none() {
            let _ = self.err(b"$(...): no channel or thread is left to take what is written\n");
        }
        self.levels.push(Level::Capture(capture));
    }

    fn end_capture(&mut self) -> Vec<u8> {
        let Some(Level::Capture(Some(capture))) = self.levels.pop() else {
            return Vec::new();
        };
        let _ = program::close(capture.into);
        let mut text = Vec::new();
        let mut room = [0u8; MESSAGE_BYTES];
        while let Ok(len @ 1..) = program::stream(capture.back, &mut room) {
            text.extend_from_slice(&room[..len]);
        }
        let _ = program::close(capture.back);
        text
    }

    /// Makes the redirections a level of its own; a `2>&1` or `1>&2`
    /// before a file of that stream's own takes the stream as it is now.
    fn begin_redirections(&mut self, redirections: &[Redirection]) -> bool {
        let mut files = match redirected(redirections) {
            Ok(files) => files,
            Err(said) => {
                let _ = self.err(said.as_bytes());
                return false;
            }
        };
        let out = match files.into.take() {
            Some(Place::Opened(file)) => Some(file),
            Some(Place::Errors) => self.errors().ok(),
            Some(Place::Output) | None => None,
        };
        let errors = match files.errors.take() {
            Some(Place::Opened(file)) => Some(file),
            Some(Place::Output) => self.output().ok(),
            Some(Place::Errors) | None => None,
        };
        self.levels.push(Level::Frame(Frame {
            input: files.from.take(),
            out,
            errors,
            ..Frame::default()
        }));
        true
    }

    /// Makes the redirections hold from now on, each on the frame
    /// [`System::setting`] finds for its stream, what was there closed.
    fn redirect(&mut self, redirections: &[Redirection]) -> bool {
        let mut files = match redirected(redirections) {
            Ok(files) => files,
            Err(said) => {
                let _ = self.err(said.as_bytes());
                return false;
            }
        };
        let out = match files.into.take() {
            Some(Place::Opened(file)) => Some(file),
            Some(Place::Errors) => self.errors().ok(),
            Some(Place::Output) | None => None,
        };
        let errors = match files.errors.take() {
            Some(Place::Opened(file)) => Some(file),
            Some(Place::Output) => self.output().ok(),
            Some(Place::Errors) | None => None,
        };
        if let Some(input) = files.from.take() {
            let frame = self.setting(|frame| frame.input.is_some());
            frame.left.clear();
            if let Some(was) = frame.input.replace(input) {
                was.close();
            }
        }
        if let Some(out) = out
            && let Some(was) = self.setting(|frame| frame.out.is_some()).out.replace(out)
        {
            was.close();
        }
        if let Some(errors) = errors
            && let Some(was) = self
                .setting(|frame| frame.errors.is_some())
                .errors
                .replace(errors)
        {
            was.close();
        }
        true
    }

    fn begin_subshell(&mut self) {
        self.levels.push(Level::Frame(Frame {
            subshell: true,
            ..Frame::default()
        }));
    }

    fn end_subshell(&mut self) {
        self.end_redirections();
    }

    fn end_redirections(&mut self) {
        if let Some(Level::Frame(frame)) = self.levels.pop() {
            frame.close();
        }
    }

    fn directory(&mut self) -> String {
        program::directory()
    }

    fn set_directory(&mut self, path: &str) -> Result<(), String> {
        program::change_directory(path).map_err(|e| trouble(path, e))
    }

    /// A program is found by its path, or its name among the files of
    /// [`PROGRAMS`]; on a CD, where only the loader brought programs, none
    /// is found.
    fn found(&mut self, name: &str) -> Option<String> {
        let words = [String::from(name)];
        match command(&words) {
            Command::Program(word, _) => {
                let word = if word == "sh" { "shell" } else { word };
                let path = program_name(word, self);
                (path.starts_with('/') && self.has(&path)).then_some(path)
            }
            Command::Empty => None,
            _ => Some(String::from(name)),
        }
    }

    fn text(&mut self, path: &str) -> Result<Vec<u8>, String> {
        text_of(path)
            .map(String::into_bytes)
            .map_err(|e| trouble(path, e))
    }

    fn line(&mut self) -> Option<Vec<u8>> {
        for level in self.levels.iter_mut().rev() {
            if let Level::Frame(Frame {
                input: Some(input),
                left,
                ..
            }) = level
            {
                return input.line(left);
            }
        }
        match &mut self.base.input {
            Some(input) => input.line(&mut self.base.left),
            None => In::Mine.line(&mut self.left),
        }
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
    File {
        handle: Handle,
        at: u64,
    },
    Pipe(Handle),
    /// Nothing, which ends at once.
    Closed,
    /// A text, as a here-document's lines, from its byte `at` on.
    Text {
        text: Vec<u8>,
        at: usize,
    },
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
            In::Closed => Ok(0),
            In::Text { text, at } => {
                let rest = &text[*at..];
                let len = rest.len().min(room.len());
                room[..len].copy_from_slice(&rest[..len]);
                *at += len;
                Ok(len)
            }
        }
    }

    /// The channel end a program reading here is handed, which goes with
    /// it: the pipe, one a thread of the shell fills from the file or the
    /// text, one whose writer is gone for nothing, or a duplicate of the
    /// shell's own input; none for the keyboard.
    #[cfg_attr(test, allow(dead_code))]
    fn handed(self) -> Option<Handle> {
        match self {
            In::Pipe(end) => Some(end),
            In::Mine => program::duplicate(program::start::given().input?).ok(),
            In::Closed => {
                let (into, from) = program::channel().ok()?;
                let _ = program::close(into);
                Some(from)
            }
            In::File { .. } | In::Text { .. } => {
                let (into, from) = program::channel().ok()?;
                let mut file = self;
                // What the thread ends with goes nowhere: the command reading
                // the pipe sees its end either way.
                let fed = program::thread(move || {
                    let mut pipe = Out::Pipe(into);
                    let _ = copy(&mut file, &mut pipe);
                    file.close();
                    pipe.close();
                    0
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

    /// A line read from here, with its end when it has one: from `left`,
    /// what a read before brought past its line, and on. What is left of a
    /// file or a text after the line is read again from there, so what
    /// reads it next starts after the line; none once it ended.
    #[cfg_attr(test, allow(dead_code))]
    fn line(&mut self, left: &mut Vec<u8>) -> Option<Vec<u8>> {
        let mut room = [0u8; MESSAGE_BYTES];
        let end = loop {
            if let Some(end) = left.iter().position(|&b| b == b'\n') {
                break end + 1;
            }
            match self.take(&mut room) {
                Ok(len @ 1..) => left.extend_from_slice(&room[..len]),
                _ if left.is_empty() => return None,
                _ => break left.len(),
            }
        };
        let line = left.drain(..end).collect();
        match self {
            In::File { at, .. } => *at -= left.len() as u64,
            In::Text { at, .. } => *at -= left.len(),
            _ => return Some(line),
        }
        left.clear();
        Some(line)
    }

    /// A second one reading the same, from where this one is.
    #[cfg_attr(test, allow(dead_code))]
    fn copy(&self) -> Result<In, Error> {
        Ok(match self {
            In::Mine => In::Mine,
            In::File { handle, at } => In::File {
                handle: program::duplicate(*handle)?,
                at: *at,
            },
            In::Pipe(end) => In::Pipe(program::duplicate(*end)?),
            In::Closed => In::Closed,
            In::Text { text, at } => In::Text {
                text: text.clone(),
                at: *at,
            },
        })
    }
}

/// Where a command's text goes: the shell's own output or its standard
/// error, a file at its end, or the pipe to the command after it.
enum Out {
    Mine,
    MyErrors,
    File(Handle),
    Pipe(Handle),
    /// Nowhere: each write fails, as one to a pipe nobody reads.
    Closed,
}

impl Out {
    #[cfg_attr(test, allow(dead_code))]
    fn say(&mut self, text: &str) -> Result<(), Error> {
        self.put(text.as_bytes(), &mut Vec::new())
    }

    /// Writes `bytes` as they are into a file or a pipe. The shell's own
    /// output and error take text, so there a character cut short waits in
    /// `held` for its rest, and what can never be text shows as such.
    #[cfg_attr(test, allow(dead_code))]
    fn put(&mut self, bytes: &[u8], held: &mut Vec<u8>) -> Result<(), Error> {
        match self {
            Out::Mine | Out::MyErrors => {
                held.extend_from_slice(bytes);
                let whole = whole(held);
                let text = String::from_utf8_lossy(&held[..whole]).into_owned();
                held.drain(..whole);
                if matches!(self, Out::Mine) {
                    program::write(&text)
                } else {
                    program::write_error(&text)
                }
            }
            // At the end each time, so a file another writer shares, as
            // with 2>&1, loses nothing.
            Out::File(handle) => {
                let end = program::info(*handle)?.size;
                program::write_file(*handle, end, bytes)
            }
            Out::Pipe(end) => program::start::pieces(bytes)
                .try_for_each(|piece| program::send_waiting(*end, piece)),
            Out::Closed => Err(Error::Closed),
        }
    }

    /// Where a program writing here writes: the file or the pipe, which go
    /// with it, or the shell's own output or error.
    #[cfg_attr(test, allow(dead_code))]
    fn handed(self) -> Option<Sink> {
        match self {
            Out::File(handle) => Some(Sink::File(handle)),
            Out::Pipe(end) => Some(Sink::Channel(end)),
            Out::Mine => inherited(program::start::given().output),
            Out::MyErrors => inherited(program::start::given().error),
            Out::Closed => {
                let (write, read) = program::channel().ok()?;
                let _ = program::close(read);
                Some(Sink::Channel(write))
            }
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    fn close(self) {
        if let Out::File(handle) | Out::Pipe(handle) = self {
            let _ = program::close(handle);
        }
    }

    /// A second one to the same place.
    #[cfg_attr(test, allow(dead_code))]
    fn copy(&self) -> Result<Out, Error> {
        Ok(match self {
            Out::Mine => Out::Mine,
            Out::MyErrors => Out::MyErrors,
            Out::File(handle) => Out::File(program::duplicate(*handle)?),
            Out::Pipe(handle) => Out::Pipe(program::duplicate(*handle)?),
            Out::Closed => Out::Closed,
        })
    }
}

/// Copies what `input` holds to `out` up to its end, byte for byte but on
/// the shell's own output.
#[cfg_attr(test, allow(dead_code))]
fn copy(input: &mut In, out: &mut Out) -> Result<(), Error> {
    let mut room = [0u8; MESSAGE_BYTES];
    let mut held = Vec::new();
    loop {
        let len = input.take(&mut room)?;
        if len == 0 {
            return said_last(&held, out);
        }
        out.put(&room[..len], &mut held)?;
    }
}

/// Says what is `held` back at the end of a text, a character the end cut
/// short, which shows as one that is not there.
fn said_last(held: &[u8], out: &mut Out) -> Result<(), Error> {
    if held.is_empty() {
        return Ok(());
    }
    out.say(&String::from_utf8_lossy(held))
}

/// How much of `bytes` can be shown now: all of it, but for a character
/// at its end that the next piece may finish.
fn whole(bytes: &[u8]) -> usize {
    match core::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        _ => bytes.len(),
    }
}

/// Where a command reads, where it writes, and where it says what went
/// wrong.
struct Streams {
    input: In,
    out: Out,
    errors: Out,
}

impl Streams {
    /// The shell's own.
    fn mine() -> Streams {
        Streams {
            input: In::Mine,
            out: Out::Mine,
            errors: Out::MyErrors,
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    fn close(self) {
        self.input.close();
        self.out.close();
        self.errors.close();
    }
}

/// Each command of `pipeline` with its streams: the files its redirections
/// lead to, else the pipes between it and the commands beside it, else
/// `defaults`. What went wrong once something did not open, after closing
/// what had.
#[cfg_attr(test, allow(dead_code))]
fn plumbed(pipeline: &[Ready], defaults: Defaults) -> Result<Vec<(&[String], Streams)>, String> {
    let Defaults {
        input: mut next,
        out: last,
        errors: wrong,
    } = defaults;
    let mut stages: Vec<(&[String], Streams)> = Vec::new();
    let mut last = Some(last);
    for (i, ready) in pipeline.iter().enumerate() {
        let mut files = match redirected(&ready.redirections) {
            Ok(files) => files,
            Err(said) => {
                if let Some(last) = last {
                    last.close();
                }
                wrong.close();
                return abandoned(next, stages, said);
            }
        };
        let input = match files.from.take() {
            Some(file) => {
                core::mem::replace(&mut next, In::Mine).close();
                file
            }
            None => core::mem::replace(&mut next, In::Mine),
        };
        // Where the command writes without a file for it: the pipe to the
        // next one, or for the last the shell's own output or capture.
        let default = if i + 1 == pipeline.len() {
            last.take().unwrap_or(Out::Mine)
        } else if let Ok((write, read)) = program::channel() {
            next = In::Pipe(read);
            Out::Pipe(write)
        } else {
            input.close();
            files.close();
            if let Some(last) = last {
                last.close();
            }
            wrong.close();
            return abandoned(next, stages, "|: no channel is left for the pipe\n".into());
        };
        let errors = match files.errors.take() {
            None | Some(Place::Errors) => wrong.copy().unwrap_or(Out::MyErrors),
            Some(Place::Opened(file)) => file,
            Some(Place::Output) => default.copy().unwrap_or(Out::MyErrors),
        };
        let out = match files.into.take() {
            Some(Place::Opened(file)) => {
                default.close();
                file
            }
            Some(Place::Errors) => {
                default.close();
                wrong.copy().unwrap_or(Out::MyErrors)
            }
            None | Some(Place::Output) => default,
        };
        stages.push((&ready.words, Streams { input, out, errors }));
    }
    wrong.close();
    Ok(stages)
}

/// Closes what a pipeline that cannot start opened: the pipe to the stage
/// that was not reached, and the stages before it, and answers `said`,
/// what went wrong.
#[cfg_attr(test, allow(dead_code))]
fn abandoned(
    next: In,
    stages: Vec<(&[String], Streams)>,
    said: String,
) -> Result<Vec<(&[String], Streams)>, String> {
    next.close();
    for (_, streams) in stages {
        streams.close();
    }
    Err(said)
}

/// Where stream 1 or 2 of a command goes after its redirections: an end
/// a redirection opened or copied, or where the other of the two goes
/// without a file of its own, as `2>&1` before any `>`, or `1>&2` before
/// any `2>`, asks.
enum Place {
    Opened(Out),
    Output,
    Errors,
}

/// What a command's redirections made its streams, the last for each: to
/// read, to write, and for what went wrong.
#[derive(Default)]
struct Files {
    from: Option<In>,
    into: Option<Place>,
    errors: Option<Place>,
}

impl Files {
    #[cfg_attr(test, allow(dead_code))]
    fn close(self) {
        if let Some(file) = self.from {
            file.close();
        }
        for place in [self.into, self.errors].into_iter().flatten() {
            if let Place::Opened(file) = place {
                file.close();
            }
        }
    }

    /// Where stream 1 or 2 goes.
    fn place(&self, stream: u32) -> Option<&Place> {
        if stream == 1 {
            self.into.as_ref()
        } else {
            self.errors.as_ref()
        }
    }

    /// Has stream 1 or 2 go to `place`, closing the end it went to.
    #[cfg_attr(test, allow(dead_code))]
    fn send(&mut self, stream: u32, place: Place) {
        let slot = if stream == 1 {
            &mut self.into
        } else {
            &mut self.errors
        };
        if let Some(Place::Opened(file)) = slot.replace(place) {
            file.close();
        }
    }

    /// Has stream 0 read from `input`, closing what it read from.
    #[cfg_attr(test, allow(dead_code))]
    fn read_from(&mut self, input: In) {
        if let Some(file) = self.from.replace(input) {
            file.close();
        }
    }
}

/// The path, the rights and what OPEN does by whether the file is there,
/// for a redirection to a file.
fn opening(target: &Target) -> Option<(&str, u32, u64)> {
    Some(match target {
        Target::Read(path) => (path, file::READ, file::OPEN_EXISTING),
        Target::Create(path) => (path, file::WRITE, file::CREATE_NEW),
        Target::Write(path) => (path, file::WRITE, file::CREATE_ALWAYS),
        Target::Append(path) => (path, file::WRITE, file::OPEN_ALWAYS),
        Target::ReadWrite(path) => (path, file::READ | file::WRITE, file::OPEN_ALWAYS),
        Target::Copy(_) | Target::Close | Target::Text(_) => return None,
    })
}

/// What the shell says of a redirection Meridian has no way to make.
fn unmade(stream: u32, target: &Target) -> String {
    match target {
        Target::Copy(other) if *other > 2 => {
            format!("{other}: Meridian's programs have the streams 0, 1 and 2\n")
        }
        Target::Copy(other) => format!(
            "{stream}>&{other}: a stream read and one written cannot stand for each other\n"
        ),
        _ => format!("{stream}<<: a here-document is read on stream 0\n"),
    }
}

/// Makes the redirections, opening their files in their order as sh does,
/// and answers what they made each stream; what went wrong when one
/// cannot be made, after closing what had.
#[cfg_attr(test, allow(dead_code))]
fn redirected(redirections: &[Redirection]) -> Result<Files, String> {
    let mut files = Files::default();
    let refused = |files: Files, said: &str| {
        files.close();
        Err(String::from(said))
    };
    for Redirection { stream, target } in redirections {
        let stream = *stream;
        if stream > 2 {
            return refused(
                files,
                &format!("{stream}: Meridian's programs have the streams 0, 1 and 2\n"),
            );
        }
        if let Some((path, rights, how)) = opening(target) {
            let handle = match program::open_with(path, rights, how) {
                Ok(handle) => handle,
                Err(e) => return refused(files, &trouble(path, e)),
            };
            if stream > 0 {
                files.send(stream, Place::Opened(Out::File(handle)));
            } else if program::info(handle).is_ok_and(|info| info.directory) {
                let _ = program::close(handle);
                return refused(files, &trouble(path, Error::NotAFile));
            } else {
                files.read_from(In::File { handle, at: 0 });
            }
            continue;
        }
        match (stream, target) {
            (0, Target::Close) => files.read_from(In::Closed),
            (0, Target::Text(text)) => files.read_from(In::Text {
                text: text.clone(),
                at: 0,
            }),
            (1 | 2, Target::Close) => files.send(stream, Place::Opened(Out::Closed)),
            (_, Target::Copy(other)) if *other == stream => {}
            (1 | 2, Target::Copy(other @ (1 | 2))) => {
                let place = match files.place(*other) {
                    None => {
                        if *other == 1 {
                            Place::Output
                        } else {
                            Place::Errors
                        }
                    }
                    Some(Place::Output) => Place::Output,
                    Some(Place::Errors) => Place::Errors,
                    Some(Place::Opened(file)) => match file.copy() {
                        Ok(copy) => Place::Opened(copy),
                        Err(e) => {
                            return refused(files, &trouble(&format!("{stream}>&{other}"), e));
                        }
                    },
                };
                files.send(stream, place);
            }
            (_, target) => return refused(files, &unmade(stream, target)),
        }
    }
    Ok(files)
}

/// A command of a pipeline on its way: done already, a program, or one of
/// the shell's own in a thread, which sends its status over a channel.
enum Going {
    Done(u8),
    Program(String, Handle),
    Thread(Handle),
}

impl Going {
    /// Waits until the command ended, and answers its status and what the
    /// shell says about how it ended.
    #[cfg_attr(test, allow(dead_code))]
    fn finish(self) -> (u8, Option<String>) {
        match self {
            Going::Done(status) => (status, None),
            Going::Program(word, child) => {
                let ended = program::wait(child);
                let _ = program::close(child);
                match ended {
                    Ok(ended) => (status(ended), ending(&word, ended)),
                    Err(e) => (1, Some(trouble(&word, e))),
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
                (status, None)
            }
        }
    }
}

/// Starts the command `words` with its streams: a program, or one of the
/// shell's own, at once when it is `alone` and in a thread of its own in a
/// pipeline.
#[cfg_attr(test, allow(dead_code))]
fn begin(words: &[String], streams: Streams, alone: bool) -> Going {
    if let Command::Program(word, arguments) = command(words) {
        return start(word, arguments, streams);
    }
    if alone {
        let mut streams = streams;
        let status = run(command(words), &mut streams);
        streams.close();
        return Going::Done(status);
    }
    let Ok((mine, theirs)) = program::channel() else {
        let _ = program::write_error("|: no channel is left for the pipe\n");
        streams.close();
        return Going::Done(1);
    };
    let words = words.to_vec();
    let started = program::thread(move || {
        let mut streams = streams;
        let status = run(command(&words), &mut streams);
        streams.close();
        let _ = program::send(theirs, &[status], None);
        let _ = program::close(theirs);
        0
    });
    if started.is_err() {
        let _ = program::write_error("|: no thread is left for the pipe\n");
        let _ = program::close(mine);
        return Going::Done(1);
    }
    Going::Thread(mine)
}

/// Runs `command` on `streams`, and answers its status: a program's, or
/// for the shell's own 0, and 1 when something went wrong.
#[cfg_attr(test, allow(dead_code))]
fn run(command: Command<'_>, streams: &mut Streams) -> u8 {
    let Streams { input, out, errors } = streams;
    let done = match command {
        Command::Program(word, arguments) => {
            let taken = core::mem::replace(streams, Streams::mine());
            let (status, line) = start(word, arguments, taken).finish();
            if let Some(line) = line {
                let _ = program::write_error(&line);
            }
            return status;
        }
        Command::Shared(words) => {
            let mut reach = Reach { input, out, errors };
            return ::shell::commands::run(words, &mut reach).unwrap_or(127);
        }
        Command::Help => out.say(HELP).is_ok(),
        Command::List([]) => list(&program::directory(), out, errors),
        Command::ChangeDirectory([]) | Command::WorkingDirectory => {
            out.say(&format!("{}\n", program::directory())).is_ok()
        }
        Command::ChangeDirectory([path]) => done(path, program::change_directory, errors),
        Command::ChangeDirectory(_) => {
            let _ = errors.say("cd takes one path\n");
            false
        }
        Command::List(paths) => every(paths, |path| list(path, out, errors)),
        Command::Show([]) => copy(input, out).is_ok(),
        Command::Show(paths) => every(paths, |path| show(path, out, errors)),
        Command::MakeDirectory(paths) => {
            every(paths, |path| done(path, program::make_directory, errors))
        }
        Command::Remove(paths) => every(paths, |path| done(path, program::remove, errors)),
        Command::Clear => out.say("\u{c}").is_ok(),
        Command::Services => services(out, errors),
        Command::Empty => true,
        Command::RemoveDirectory(paths) => {
            every(paths, |path| done(path, remove_directory, errors))
        }
        Command::Copy(paths) => carry(paths, copy_file, errors),
        Command::Move(paths) => carry(paths, move_file, errors),
        Command::Missing(word) => {
            let _ = errors.say(&missing(word));
            false
        }
    };
    u8::from(!done)
}

/// What the commands lib/shell runs reach in the shell: the streams of the
/// command, and the files and streams they ask about.
struct Reach<'a> {
    input: &'a mut In,
    out: &'a mut Out,
    errors: &'a mut Out,
}

/// Writes `bytes` to `out` whole, and says whether all of them went.
fn written(out: &mut Out, bytes: &[u8]) -> bool {
    let mut held = Vec::new();
    out.put(bytes, &mut held)
        .and_then(|()| said_last(&held, out))
        .is_ok()
}

/// Whether the file at `path` opens with the right `wanted`.
fn may(path: &str, wanted: u32) -> bool {
    program::open(path, wanted).is_ok_and(|handle| {
        let _ = program::close(handle);
        true
    })
}

impl ::shell::commands::Streams for Reach<'_> {
    fn out(&mut self, bytes: &[u8]) -> bool {
        written(self.out, bytes)
    }

    fn err(&mut self, bytes: &[u8]) -> bool {
        written(self.errors, bytes)
    }

    /// What is at `path`: a directory may always be gone into, and a file
    /// run when it is a program's image and the right to run it is there.
    fn file(&mut self, path: &str) -> Option<::shell::commands::File> {
        use ::shell::commands::{Allowed, File, Kind};
        let handle = program::open(path, 0).ok()?;
        let info = program::info(handle);
        let _ = program::close(handle);
        let info = info.ok()?;
        Some(File {
            kind: if info.directory {
                Kind::Directory
            } else {
                Kind::File
            },
            size: info.size,
            allowed: Allowed {
                read: may(path, file::READ),
                write: may(path, file::WRITE),
                run: info.directory
                    || path
                        .rsplit_once('.')
                        .is_some_and(|(_, end)| end.eq_ignore_ascii_case("mexec"))
                        && may(path, file::RUN),
            },
        })
    }

    /// A stream is a terminal where the command has the shell's own and
    /// the shell has the console's: the keyboard to read, the screen to
    /// write.
    fn terminal(&mut self, stream: u32) -> bool {
        let given = program::start::given();
        match stream {
            0 => matches!(self.input, In::Mine) && given.input.is_none(),
            1 => matches!(self.out, Out::Mine) && given.output.is_none(),
            2 => matches!(self.errors, Out::MyErrors) && given.error.is_none(),
            _ => false,
        }
    }
}

/// Does `work` with each of `paths`, the ones after one that went wrong
/// too, and says whether all went well.
fn every(paths: &[String], mut work: impl FnMut(&str) -> bool) -> bool {
    paths.iter().filter(|path| !work(path)).count() == 0
}

/// Does `each` with each path but the last of `paths` and where it goes,
/// as [`destinations`] has it, says what went wrong with any, and says
/// whether all went well.
#[cfg_attr(test, allow(dead_code))]
fn carry(
    paths: &[String],
    mut each: impl FnMut(&str, &str) -> Result<(), String>,
    errors: &mut Out,
) -> bool {
    let target = paths.last().map_or("", String::as_str);
    let Some(pairs) = destinations(paths, is_directory(target)) else {
        let _ = errors.say(&format!(
            "{target}: not a directory, so one path alone goes there\n"
        ));
        return false;
    };
    let failed = pairs.iter().filter(|(from, to)| {
        each(from, to)
            .inspect_err(|why| {
                let _ = errors.say(why);
            })
            .is_err()
    });
    failed.count() == 0
}

/// Whether `path` is a directory the shell can open.
#[cfg_attr(test, allow(dead_code))]
fn is_directory(path: &str) -> bool {
    program::open(path, file::READ).is_ok_and(|handle| {
        let directory = program::info(handle).is_ok_and(|i| i.directory);
        let _ = program::close(handle);
        directory
    })
}

/// Whether `from` and `to` lead to one file, as the tree compares names:
/// in capitals.
fn one_file(from: &str, to: &str) -> bool {
    program::full_path(from).to_uppercase() == program::full_path(to).to_uppercase()
}

/// Copies the file at `from` to `to`, made new or emptied, but never onto
/// itself, which emptying would lose, and says what went wrong.
#[cfg_attr(test, allow(dead_code))]
fn copy_file(from: &str, to: &str) -> Result<(), String> {
    if one_file(from, to) {
        return Err(format!("{from}: copied onto itself\n"));
    }
    let source = program::open(from, file::READ).map_err(|e| trouble(from, e))?;
    let copied = copy_from(source, from, to);
    let _ = program::close(source);
    copied
}

/// Copies the open file `source`, which is at `from`, to `to`, and says
/// what went wrong as the shell says it.
#[cfg_attr(test, allow(dead_code))]
fn copy_from(source: Handle, from: &str, to: &str) -> Result<(), String> {
    if program::info(source).is_ok_and(|info| info.directory) {
        return Err(trouble(from, Error::NotAFile));
    }
    let target =
        program::open_with(to, file::WRITE, file::CREATE_ALWAYS).map_err(|e| trouble(to, e))?;
    let mut room = [0u8; 4096];
    let mut at = 0;
    let copied = loop {
        let len = match program::read(source, at, &mut room) {
            Ok(0) => break Ok(()),
            Ok(len) => len,
            Err(e) => break Err(e),
        };
        if let Err(e) = program::write_file(target, at, &room[..len]) {
            break Err(e);
        }
        at += len as u64;
    };
    let _ = program::close(target);
    // Once both are open, what goes wrong is the copy's, so it names both.
    copied.map_err(|e| trouble(&format!("{from} to {to}"), e))
}

/// Moves the file or directory at `from` to `to`, and a file across
/// volumes by copying it and removing it after.
#[cfg_attr(test, allow(dead_code))]
fn move_file(from: &str, to: &str) -> Result<(), String> {
    match program::rename(from, to) {
        Err(Error::NotSameVolume) => {
            copy_file(from, to)?;
            program::remove(from).map_err(|e| trouble(from, e))
        }
        other => other.map_err(|e| trouble(from, e)),
    }
}

/// Removes the directory at `path`, which has to be empty, and no file.
#[cfg_attr(test, allow(dead_code))]
fn remove_directory(path: &str) -> Result<(), Error> {
    let handle = program::open(path, file::READ)?;
    let directory = program::info(handle).map(|i| i.directory);
    let _ = program::close(handle);
    if !directory? {
        return Err(Error::NotADirectory);
    }
    program::remove(path)
}

/// Does `call` with `path`, says what went wrong when it did, and says
/// whether it went well.
#[cfg_attr(test, allow(dead_code))]
fn done(path: &str, call: fn(&str) -> Result<(), Error>, errors: &mut Out) -> bool {
    call(path)
        .inspect_err(|&e| {
            let _ = errors.say(&trouble(path, e));
        })
        .is_ok()
}

/// Asks init about each of its services in turn and shows what it says.
#[cfg_attr(test, allow(dead_code))]
fn services(out: &mut Out, errors: &mut Out) -> bool {
    let line = Handle(LINE.load(SeqCst));
    if line.0 == 0 {
        let _ = errors.say("services: init did not start this shell\n");
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

/// Starts the program `word` names, handing it `arguments` and its
/// streams, which go with it; when it cannot start, says why on its
/// stream 2.
#[cfg_attr(test, allow(dead_code))]
fn start(word: &str, arguments: &[String], streams: Streams) -> Going {
    let mut arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    // sh is the shell itself, in its sh dialect (decision record 0016).
    let program = if word == "sh" {
        arguments.insert(0, "--sh");
        "shell"
    } else {
        word
    };
    let Streams {
        input,
        out,
        mut errors,
    } = streams;
    let given = program::start::Given {
        input: input.handed(),
        output: out.handed(),
        error: errors.copy().ok().and_then(Out::handed),
        ..program::start::Given::default()
    };
    let going = match program::spawn_handing(
        &program_name(program, &mut System::default()),
        &arguments,
        given,
    ) {
        Ok((child, start)) => {
            let _ = program::close(start);
            Going::Program(word.into(), child)
        }
        Err(Error::NotFound | Error::NoSuchProgram) if !word.starts_with('/') => {
            let _ = errors.say(&format!("{word}: no such command\n"));
            Going::Done(127)
        }
        Err(e) => {
            let _ = errors.say(&trouble(word, e));
            Going::Done(126)
        }
    };
    errors.close();
    going
}

/// A second handle to `sink`, the shell's own standard output or error,
/// for a program to write there too (decision record 0019); none while the
/// shell writes it to the console.
#[cfg_attr(test, allow(dead_code))]
fn inherited(sink: Option<Sink>) -> Option<Sink> {
    Some(match sink? {
        Sink::Channel(end) => Sink::Channel(program::duplicate(end).ok()?),
        Sink::File(file) => Sink::File(program::duplicate(file).ok()?),
    })
}

/// Says on `errors` what went wrong with `path` when a call about it
/// failed, and answers whether `done` went well; nothing is said when it
/// was the output that failed, as there is nowhere to say it.
#[cfg_attr(test, allow(dead_code))]
fn reported(done: Result<(), Option<Error>>, path: &str, errors: &mut Out) -> bool {
    if let Err(Some(e)) = done {
        let _ = errors.say(&trouble(path, e));
    }
    done.is_ok()
}

/// Shows the names in the directory at `path`, or the path of a file, and
/// says whether that went well.
#[cfg_attr(test, allow(dead_code))]
fn list(path: &str, out: &mut Out, errors: &mut Out) -> bool {
    let listed = program::open(path, file::READ)
        .map_err(Some)
        .and_then(|directory| {
            let listed = put_names(directory, path, out);
            let _ = program::close(directory);
            listed
        });
    reported(listed, path, errors)
}

/// Puts on `out` each name in the open `directory` at `path`, a
/// directory's with a `/` after it, or `path` when it is a file; the error
/// of a call about it, or none when `out` failed.
#[cfg_attr(test, allow(dead_code))]
fn put_names(directory: Handle, path: &str, out: &mut Out) -> Result<(), Option<Error>> {
    let mut room = [0u8; 1024];
    let mut index = 0;
    loop {
        let line = match program::list(directory, index, &mut room) {
            Ok((len, is_directory)) => {
                let slash = if is_directory { "/" } else { "" };
                format!("{}{slash}\n", String::from_utf8_lossy(&room[..len]))
            }
            Err(Error::NotFound) => return Ok(()),
            Err(Error::NotADirectory) => return out.say(&format!("{path}\n")).map_err(|_| None),
            Err(e) => return Err(Some(e)),
        };
        out.say(&line).map_err(|_| None)?;
        index += 1;
    }
}

/// Shows what the file at `path` holds, byte for byte but on the shell's
/// own output, and says whether that went well.
#[cfg_attr(test, allow(dead_code))]
fn show(path: &str, out: &mut Out, errors: &mut Out) -> bool {
    let shown = program::open(path, file::READ)
        .map_err(Some)
        .and_then(|file| {
            let shown = put_file(file, out);
            let _ = program::close(file);
            shown
        });
    reported(shown, path, errors)
}

/// Puts on `out` what the open `file` holds; the error of a call about it,
/// or none when `out` failed.
#[cfg_attr(test, allow(dead_code))]
fn put_file(file: Handle, out: &mut Out) -> Result<(), Option<Error>> {
    let mut room = [0u8; 4096];
    let mut at = 0;
    let mut held = Vec::new();
    loop {
        let len = program::read(file, at, &mut room).map_err(Some)?;
        if len == 0 {
            return said_last(&held, out).map_err(|_| None);
        }
        out.put(&room[..len], &mut held).map_err(|_| None)?;
        at += len as u64;
    }
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

    const UP: u16 = 0x52;
    const DOWN: u16 = 0x51;

    /// An editor that ran `lines`, typed as keys a letter is.
    fn ran(lines: &[&str]) -> Editor {
        let mut editor = Editor::default();
        for line in lines {
            for c in line.chars() {
                let usage = u16::from(u8::try_from(c).unwrap() - b'a') + 0x04;
                editor.feed(down(usage));
            }
            assert!(matches!(editor.feed(down(0x28)), Edit::Run(_)));
        }
        editor
    }

    fn recalled(erase: usize, with: &str) -> Edit {
        Edit::Recall {
            erase,
            with: with.into(),
        }
    }

    #[test]
    fn up_and_down_bring_back_the_lines_run_before() {
        let mut e = ran(&["ls", "cat", "pwd"]);
        assert_eq!(e.feed(down(UP)), recalled(0, "pwd"));
        assert_eq!(e.feed(down(UP)), recalled(3, "cat"));
        assert_eq!(
            e.feed(up(UP)),
            Edit::Nothing,
            "a key coming up does nothing"
        );
        assert_eq!(e.feed(down(UP)), recalled(3, "ls"));
        assert_eq!(e.feed(down(UP)), Edit::Nothing, "the oldest stays");
        assert_eq!(e.feed(down(DOWN)), recalled(2, "cat"));
        assert_eq!(e.feed(down(DOWN)), recalled(3, "pwd"));
        assert_eq!(
            e.feed(down(DOWN)),
            recalled(3, ""),
            "back to the line being typed"
        );
        assert_eq!(e.feed(down(DOWN)), Edit::Nothing);
        assert_eq!(e.feed(down(0x28)), Edit::Run(String::new()));
        assert_eq!(
            Editor::default().feed(down(UP)),
            Edit::Nothing,
            "no lines yet"
        );
    }

    #[test]
    fn the_line_being_typed_waits_while_older_ones_are_shown() {
        let mut e = ran(&["ls"]);
        e.feed(down(0x06));
        assert_eq!(e.feed(down(UP)), recalled(1, "ls"));
        assert_eq!(e.feed(down(DOWN)), recalled(2, "c"));
        assert_eq!(e.feed(down(0x28)), Edit::Run("c".into()));
    }

    #[test]
    fn a_line_from_before_changed_is_the_one_run() {
        let mut e = ran(&["ls"]);
        e.feed(down(UP));
        assert_eq!(e.feed(down(0x2a)), Edit::Erase);
        assert_eq!(e.feed(down(0x04)), Edit::Type('a'));
        assert_eq!(e.feed(down(0x28)), Edit::Run("la".into()));
        assert_eq!(e.feed(down(UP)), recalled(0, "la"), "the newest now");
        assert_eq!(e.feed(down(UP)), recalled(2, "ls"));
    }

    #[test]
    fn the_history_keeps_no_empty_line_no_line_twice_and_no_more_than_it_holds() {
        let mut e = ran(&["a", "a", "", "b"]);
        assert_eq!(e.history, ["a", "b"]);
        e = ran(&[]);
        for n in 0..HISTORY + 5 {
            e.history.push(format!("{n}"));
            if e.history.len() > HISTORY {
                e.history.remove(0);
            }
        }
        for c in ["x", "y"] {
            e.line = c.into();
            e.feed(down(0x28));
        }
        assert_eq!(e.history.len(), HISTORY);
        assert_eq!(e.history.last().map(String::as_str), Some("y"));
        assert_eq!(e.history[0], "7", "the oldest went first");
    }

    #[test]
    fn tab_asks_for_the_line_to_be_completed_which_then_takes_its_place() {
        let mut e = ran(&[]);
        e.feed(down(0x06));
        assert_eq!(e.feed(down(0x2b)), Edit::Complete, "tab types no tab");
        assert_eq!(e.line, "c");
        assert_eq!(e.replace("cat ".into()), recalled(1, "cat "));
        assert_eq!(e.feed(down(0x28)), Edit::Run("cat ".into()));
    }

    #[test]
    fn a_word_completes_to_the_name_it_starts_or_as_far_as_they_agree() {
        let mut tree = Host::new(
            "complete",
            &[
                "System/Boot/readme.txt",
                "System/Boot/ready.txt",
                "System/made.meri",
                "System/x/",
            ],
        );
        let mut done = |line: &str, directory: &str| completed(line, directory, &mut tree);
        assert_eq!(
            done("cat /System/ma", "/").as_deref(),
            Some("cat /System/made.meri")
        );
        assert_eq!(
            done("cat /system/MA", "/").as_deref(),
            Some("cat /system/made.meri")
        );
        assert_eq!(
            done("cat Boot/rea", "/System").as_deref(),
            Some("cat Boot/read"),
            "as far as they agree"
        );
        assert_eq!(done("cat Boot/read", "/System"), None, "nothing to add");
        assert_eq!(
            done("cat Boot/readm", "/System/").as_deref(),
            Some("cat Boot/readme.txt")
        );
        assert_eq!(done("cat /System/zz", "/"), None);
        assert_eq!(done("ls /Sys", "/").as_deref(), Some("ls /System"));
        let mut cases = Host::new("spelled", &["Rea_b", "rea_a"]);
        assert_eq!(
            completed("re", "/", &mut cases).as_deref(),
            Some("rea_"),
            "spelled as the first of them in capitals"
        );
        assert_eq!(
            done("x", "/System").as_deref(),
            None,
            "the one name it starts, and no more"
        );
        assert_eq!(
            done("echo /System/", "/"),
            None,
            "names that agree on nothing add nothing"
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
    fn the_commands_lib_shell_runs_get_all_their_words() {
        for line in [
            "echo hello there",
            "echo",
            "printf %s x",
            "test -n x",
            "[ x ]",
            "true",
            ":",
            "false x",
        ] {
            let w = words(line);
            assert_eq!(
                command(&w),
                Command::Shared(&w),
                "{line}: lib/shell's, all words"
            );
        }
    }

    #[test]
    fn the_first_word_says_which_command_runs() {
        assert_eq!(command(&words("help me")), Command::Help);
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
        for name in ["clear", "cls", "cls now"] {
            assert_eq!(command(&words(name)), Command::Clear);
        }
        assert_eq!(command(&words("services now")), Command::Services);
        for name in ["cd", "chdir"] {
            let w = words(&format!("{name} /System"));
            assert_eq!(command(&w), Command::ChangeDirectory(&w[1..]));
            assert_eq!(
                command(&words(name)),
                Command::ChangeDirectory(&[]),
                "to show it"
            );
        }
        assert_eq!(command(&words("pwd")), Command::WorkingDirectory);
        assert_eq!(command(&[]), Command::Empty);
        let program = words("edit a b");
        assert_eq!(command(&program), Command::Program("edit", &program[1..]));
        for name in ["cp", "copy"] {
            let w = words(&format!("{name} /a /b /c"));
            assert_eq!(command(&w), Command::Copy(&w[1..]));
        }
        for name in ["mv", "move"] {
            let w = words(&format!("{name} /a /b"));
            assert_eq!(command(&w), Command::Move(&w[1..]));
        }
        for name in ["rmdir", "rd"] {
            let w = words(&format!("{name} /a"));
            assert_eq!(command(&w), Command::RemoveDirectory(&w[1..]));
            assert_eq!(command(&words(name)), Command::Missing(name));
        }
        for name in ["cp", "copy", "mv", "move"] {
            assert_eq!(command(&words(name)), Command::Missing(name));
            assert_eq!(
                command(&words(&format!("{name} /a"))),
                Command::Missing(name),
                "one path is not enough"
            );
        }
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
        assert_eq!(what(&[]), Asked::Typed);
        let text = words("-c echo");
        assert_eq!(what(&text[..1]), Asked::Unknown, "-c needs its script");
        assert_eq!(what(&text), Asked::Text("echo"));
        let script = [String::from("-c"), String::from("echo a; echo b")];
        assert_eq!(what(&script), Asked::Text("echo a; echo b"));
        assert_eq!(what(&words("/System/a.sh")), Asked::File("/System/a.sh"));
        assert_eq!(what(&words("/System/a.sh x")), Asked::Unknown);
        assert_eq!(what(&words("-c a b")), Asked::Unknown);
        assert_eq!(what(&words("-x a")), Asked::Unknown);
    }

    #[test]
    fn a_first_sh_flag_asks_for_the_sh_dialect() {
        assert_eq!(asked(&words("--sh")), (Asked::Typed, true));
        assert_eq!(asked(&words("--sh -c x")), (Asked::Text("x"), true));
        assert_eq!(asked(&words("--sh /a.sh")), (Asked::File("/a.sh"), true));
        assert_eq!(asked(&words("-c x")), (Asked::Text("x"), false));
        assert_eq!(asked(&[]), (Asked::Typed, false));
        assert_eq!(
            asked(&words("--sh --sh")),
            (Asked::File("--sh"), true),
            "only the first"
        );
        assert_eq!(asked(&words("-c --sh")), (Asked::Text("--sh"), false));
        assert_eq!(asked(&words("--sh -c")), (Asked::Unknown, true));
        assert_eq!(asked(&words("--sh /a.sh more")), (Asked::Unknown, true));
        assert_eq!(asked(&words("--sh -c a b")), (Asked::Unknown, true));
        assert_eq!(
            asked(&words("-x --sh")),
            (Asked::Unknown, false),
            "--sh comes first or not at all"
        );
    }

    #[test]
    fn a_first_line_naming_sh_makes_a_sh_script() {
        for first in [
            "#!/bin/sh",
            "#!sh",
            "#! /bin/sh",
            "#!sh  ",
            "#!/bin/sh\r",
            "#!\tsh",
        ] {
            assert!(is_sh(&format!("{first}\necho a")), "{first:?}");
            assert!(is_sh(first), "{first:?} alone");
        }
        for first in [
            "",
            "#!/bin/bash",
            "#!/usr/bin/env sh",
            "#!/bin/sh -e",
            "# !/bin/sh",
            "#/bin/sh",
            "!#/bin/sh",
            " #!/bin/sh",
            "#!/bin/shell",
            "#!ssh",
            "#!",
            "echo a",
        ] {
            assert!(!is_sh(&format!("{first}\necho a")), "{first:?}");
        }
        assert!(!is_sh("echo a\n#!/bin/sh"), "only the first line counts");
    }

    #[test]
    fn a_command_short_of_paths_says_what_it_needs() {
        assert_eq!(missing("rm"), "rm needs a path\n");
        assert_eq!(missing("rd"), "rd needs a path\n");
        for word in ["cp", "copy", "mv", "move"] {
            assert_eq!(
                missing(word),
                format!("{word} needs a path and where it goes\n")
            );
        }
    }

    #[test]
    fn one_file_is_one_whichever_way_its_path_is_written() {
        assert!(one_file("/System/a", "/system/A"), "in capitals");
        assert!(
            one_file("a", "/a"),
            "from the working directory, the root here"
        );
        assert!(one_file("/x/../a", "/a/."));
        assert!(!one_file("/a", "/b"));
        assert!(!one_file("/a", "/a/b"));
    }

    #[test]
    fn a_path_ends_in_its_name() {
        assert_eq!(last_name("/System/a.txt"), "a.txt");
        assert_eq!(last_name("a.txt"), "a.txt");
        assert_eq!(last_name("/System/Dir/"), "Dir");
        assert_eq!(last_name("Dir//"), "Dir");
        assert_eq!(last_name("/"), "");
    }

    #[test]
    fn what_cp_and_mv_carry_goes_into_a_directory_or_takes_the_last_name() {
        let paths = words("/a/x.txt /b/y /c");
        assert_eq!(
            destinations(&paths, true),
            Some(Vec::from([
                ("/a/x.txt", String::from("/c/x.txt")),
                ("/b/y", String::from("/c/y"))
            ]))
        );
        assert_eq!(destinations(&paths, false), None, "two to no directory");
        let one = words("/a/x.txt /c/z.txt");
        assert_eq!(
            destinations(&one, false),
            Some(Vec::from([("/a/x.txt", String::from("/c/z.txt"))]))
        );
        let slash = words("/a/x.txt /c/");
        assert_eq!(
            destinations(&slash, true),
            Some(Vec::from([("/a/x.txt", String::from("/c/x.txt"))])),
            "a slash at its end is not doubled"
        );
        assert_eq!(destinations(&[], true), None);
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
    fn the_shells_own_streams_copy_as_they_are() {
        assert!(matches!(Out::Mine.copy(), Ok(Out::Mine)));
        assert!(matches!(Out::MyErrors.copy(), Ok(Out::MyErrors)));
    }

    #[test]
    fn a_redirection_opens_its_file_as_its_target_asks() {
        let a = || String::from("/a");
        for (target, rights, how) in [
            (Target::Read(a()), file::READ, file::OPEN_EXISTING),
            (Target::Create(a()), file::WRITE, file::CREATE_NEW),
            (Target::Write(a()), file::WRITE, file::CREATE_ALWAYS),
            (Target::Append(a()), file::WRITE, file::OPEN_ALWAYS),
            (
                Target::ReadWrite(a()),
                file::READ | file::WRITE,
                file::OPEN_ALWAYS,
            ),
        ] {
            assert_eq!(opening(&target), Some(("/a", rights, how)), "{target:?}");
        }
        for target in [Target::Copy(1), Target::Close, Target::Text(Vec::new())] {
            assert_eq!(opening(&target), None, "{target:?} opens nothing");
        }
    }

    #[test]
    fn what_meridian_cannot_redirect_is_said_with_its_reason() {
        assert_eq!(
            unmade(2, &Target::Copy(3)),
            "3: Meridian's programs have the streams 0, 1 and 2\n"
        );
        assert_eq!(
            unmade(0, &Target::Copy(1)),
            "0>&1: a stream read and one written cannot stand for each other\n"
        );
        assert_eq!(
            unmade(1, &Target::Text(Vec::new())),
            "1<<: a here-document is read on stream 0\n"
        );
    }

    /// Directories and files on the host, in a directory of their own
    /// among its temporary files, for patterns to match against; gone once
    /// dropped. Paths go from the tree's root and are found in capitals,
    /// as Meridian's tree of files finds them, and it keeps every path it
    /// was asked to list or to find. It lists names the other way round
    /// from the order the shell wants, so nothing leans on how a disk
    /// happens to keep them.
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
    }

    impl Tree for Host {
        fn names(&mut self, path: &str) -> Vec<String> {
            self.listed.push(path.into());
            let mut names: Vec<String> = self
                .find(path)
                .and_then(|at| std::fs::read_dir(at).ok())
                .map(|dir| {
                    dir.map(|e| e.unwrap().file_name().into_string().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            names.sort_by_cached_key(|n| std::cmp::Reverse(n.to_uppercase()));
            names
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
            "help", "echo", "printf", "test", "true", "false", "cd", "chdir", "pwd", "ls", "dir",
            "cat", "type", "cp", "copy", "mv", "move", "mkdir", "md", "rm", "del", "rmdir", "rd",
            "clear", "cls", "services", "sh", "exit",
        ];
        let (first, second) = HELP.split_once('\n').unwrap();
        let named: Vec<&str> = first
            .split(|c: char| !c.is_ascii_alphabetic())
            .filter(|w| !w.is_empty())
            .collect();
        for word in known {
            assert!(named.contains(&word), "help leaves out {word}");
            // exit is the language's, and sh starts the shell again.
            assert!(
                word == "exit"
                    || word == "sh"
                    || !matches!(command(&words(word)), Command::Program(..)),
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
            (
                Error::Inside,
                "/a: a directory, which cannot go inside itself\n",
            ),
            (Error::DiskFull, "/a: no room left on its disk\n"),
            (Error::Unwritable, "/a: could not be written\n"),
        ];
        for (e, said) in writes {
            assert_eq!(trouble("/a", e), said);
        }
    }
}
