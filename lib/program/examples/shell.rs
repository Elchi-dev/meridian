//! The shell: it reads keys, shows what they type on the console, and runs
//! the line typed once Enter goes down. It knows help, echo, ls, cat,
//! clear, which cls is too, services, which asks init over the line init
//! started it with, and exit, reads paths through the tree of files, and
//! starts any other word as a program, waiting until it ended.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use program::{Ended, Error, Handle, KeyEvent, abi::file, layout};
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

/// The line being typed, and which shift keys are held.
#[derive(Default)]
struct Editor {
    line: String,
    left_shift: bool,
    right_shift: bool,
}

impl Editor {
    fn feed(&mut self, key: KeyEvent) -> Edit {
        match key.usage {
            0xe1 => self.left_shift = key.pressed,
            0xe5 => self.right_shift = key.pressed,
            _ if key.pressed => {}
            _ => return Edit::Nothing,
        }
        let shift = self.left_shift || self.right_shift;
        match layout::us(key.usage, shift).filter(|_| key.pressed) {
            Some('\n') => Edit::Run(core::mem::take(&mut self.line)),
            Some('\u{8}') => match self.line.pop() {
                Some(_) => Edit::Erase,
                None => Edit::Nothing,
            },
            Some(c) => {
                self.line.push(c);
                Edit::Type(c)
            }
            None => Edit::Nothing,
        }
    }
}

/// What a command asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command<'a> {
    Help,
    Echo(&'a str),
    /// `echo` into a file: its text, the file's path, and whether it goes
    /// after what the file holds rather than in its place.
    EchoInto(&'a str, &'a str, bool),
    List(&'a str),
    Show(&'a str),
    MakeDirectory(&'a str),
    Remove(&'a str),
    Clear,
    Services,
    Exit,
    Empty,
    /// A program, by its name in [`PROGRAMS`] or by its path.
    Program(&'a str),
    Missing(&'a str),
}

/// What `help` says.
const HELP: &str = "help, echo TEXT [> PATH], ls [PATH], cat PATH, mkdir or md PATH, \
                    rm or del PATH, clear or cls, services, exit, or a program\n";

/// Where `ls` looks without a path.
const HOME: &str = "/System/Boot";

/// Where a program named without a path is.
const PROGRAMS: &str = "/System/Boot/meridian/programs";

fn parse(line: &str) -> Command<'_> {
    let line = line.trim();
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match word {
        "" => Command::Empty,
        "help" => Command::Help,
        "echo" => echo(rest),
        "ls" if rest.is_empty() => Command::List(HOME),
        "ls" => Command::List(rest),
        "cat" | "mkdir" | "md" | "rm" | "del" if rest.is_empty() => Command::Missing(word),
        "cat" => Command::Show(rest),
        "mkdir" | "md" => Command::MakeDirectory(rest),
        "rm" | "del" => Command::Remove(rest),
        "clear" | "cls" => Command::Clear,
        "services" => Command::Services,
        "exit" => Command::Exit,
        other => Command::Program(other),
    }
}

/// `echo` with `rest` after it: the text alone, or with `> PATH` the text
/// into the file at PATH, made new or emptied, and with `>> PATH` after
/// what the file holds, as in sh.
fn echo(rest: &str) -> Command<'_> {
    let Some((text, into)) = rest.split_once('>') else {
        return Command::Echo(rest);
    };
    let (path, append) = match into.strip_prefix('>') {
        Some(path) => (path.trim(), true),
        None => (into.trim(), false),
    };
    if path.is_empty() {
        return Command::Missing("echo >");
    }
    Command::EchoInto(text.trim(), path, append)
}

/// The file the program `word` names: the path itself, or for a name the
/// program of that name in [`PROGRAMS`].
fn program_path(word: &str) -> String {
    if word.starts_with('/') {
        word.into()
    } else {
        format!("{PROGRAMS}/{word}.mexec")
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

/// The line to init the shell started with, 0 when init did not start it.
static LINE: AtomicU64 = AtomicU64::new(0);

#[cfg_attr(test, allow(dead_code))]
fn shell(line: Handle) -> u64 {
    LINE.store(line.0, SeqCst);
    let _ = program::write("Meridian shell. Type help to see what it knows.\n> ");
    let mut editor = Editor::default();
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
                if !run(&line) {
                    return 0;
                }
                program::write("> ")
            }
            Edit::Nothing => Ok(()),
        };
    }
}

/// Runs `line`, and says whether the shell goes on.
#[cfg_attr(test, allow(dead_code))]
fn run(line: &str) -> bool {
    let _ = match parse(line) {
        Command::Help => program::write(HELP),
        Command::Echo(text) => program::write(&format!("{text}\n")),
        Command::EchoInto(text, path, append) => {
            echo_into(text, path, append).or_else(|e| program::write(&trouble(path, e)))
        }
        Command::List(path) => list(path),
        Command::Show(path) => show(path),
        Command::MakeDirectory(path) => {
            program::make_directory(path).or_else(|e| program::write(&trouble(path, e)))
        }
        Command::Remove(path) => {
            program::remove(path).or_else(|e| program::write(&trouble(path, e)))
        }
        Command::Clear => program::write("\u{c}"),
        Command::Services => services(),
        Command::Exit => return false,
        Command::Empty => Ok(()),
        Command::Program(word) => start(word),
        Command::Missing(word) => program::write(&format!("{word} needs a path\n")),
    };
    true
}

/// Asks init about each of its services in turn and shows what it says.
#[cfg_attr(test, allow(dead_code))]
fn services() -> Result<(), Error> {
    let line = Handle(LINE.load(SeqCst));
    if line.0 == 0 {
        return program::write("services: init did not start this shell\n");
    }
    program::write(&listing((0..=u8::MAX).map(|place| ask(line, place))))
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

/// Starts the program `word` names and waits until it ended.
#[cfg_attr(test, allow(dead_code))]
fn start(word: &str) -> Result<(), Error> {
    let child = match program::spawn(&program_path(word), None) {
        Ok(child) => child,
        Err(Error::NotFound) if !word.starts_with('/') => {
            return program::write(&format!("{word}: no such command\n"));
        }
        Err(e) => return program::write(&trouble(word, e)),
    };
    let ended = program::wait(child);
    let _ = program::close(child);
    match ended {
        Ok(ended) => ending(word, ended).map_or(Ok(()), |line| program::write(&line)),
        Err(e) => program::write(&trouble(word, e)),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn list(path: &str) -> Result<(), Error> {
    let directory = match program::open(path, file::READ) {
        Ok(d) => d,
        Err(e) => return program::write(&trouble(path, e)),
    };
    let mut room = [0u8; 1024];
    for index in 0.. {
        match program::list(directory, index, &mut room) {
            Ok((len, is_directory)) => {
                let name = String::from_utf8_lossy(&room[..len]);
                let slash = if is_directory { "/" } else { "" };
                program::write(&format!("{name}{slash}\n"))?;
            }
            Err(Error::NotFound) => break,
            Err(e) => {
                program::write(&trouble(path, e))?;
                break;
            }
        }
    }
    program::close(directory)
}

/// Writes `text` and a line's end into the file at `path`, after what it
/// holds when `append`, and in its place otherwise.
#[cfg_attr(test, allow(dead_code))]
fn echo_into(text: &str, path: &str, append: bool) -> Result<(), Error> {
    let how = if append {
        file::OPEN_ALWAYS
    } else {
        file::CREATE_ALWAYS
    };
    let handle = program::open_with(path, file::WRITE, how)?;
    let end = if append {
        program::info(handle)?.size
    } else {
        0
    };
    let written = program::write_file(handle, end, format!("{text}\n").as_bytes());
    program::close(handle)?;
    written
}

#[cfg_attr(test, allow(dead_code))]
fn show(path: &str) -> Result<(), Error> {
    let file = match program::open(path, file::READ) {
        Ok(f) => f,
        Err(e) => return program::write(&trouble(path, e)),
    };
    let mut room = [0u8; 4096];
    let mut offset = 0;
    loop {
        match program::read(file, offset, &mut room) {
            Ok(0) => break,
            Ok(len) => {
                program::write(&String::from_utf8_lossy(&room[..len]))?;
                offset += len as u64;
            }
            Err(e) => {
                program::write(&trouble(path, e))?;
                break;
            }
        }
    }
    program::close(file)
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
    }

    #[test]
    fn a_line_is_read_as_its_command() {
        assert_eq!(parse("help"), Command::Help);
        assert_eq!(parse("  echo  hello there  "), Command::Echo("hello there"));
        assert_eq!(parse("echo"), Command::Echo(""));
        assert_eq!(
            parse("echo hello there > /System/a.txt"),
            Command::EchoInto("hello there", "/System/a.txt", false)
        );
        assert_eq!(
            parse("echo more >>/System/a.txt "),
            Command::EchoInto("more", "/System/a.txt", true)
        );
        assert_eq!(
            parse("echo >/System/empty"),
            Command::EchoInto("", "/System/empty", false)
        );
        assert_eq!(
            parse("echo a > b > c"),
            Command::EchoInto("a", "b > c", false),
            "only the first > leads into a file"
        );
        assert_eq!(parse("echo hi >"), Command::Missing("echo >"));
        assert_eq!(parse("echo hi >>  "), Command::Missing("echo >"));
        assert_eq!(parse("ls"), Command::List("/System/Boot"));
        assert_eq!(
            parse("ls /System/Boot/meridian"),
            Command::List("/System/Boot/meridian")
        );
        assert_eq!(
            parse("cat /System/Boot/meridian/boot.meri"),
            Command::Show("/System/Boot/meridian/boot.meri")
        );
        assert_eq!(parse("cat"), Command::Missing("cat"));
        assert_eq!(
            parse("mkdir /System/New"),
            Command::MakeDirectory("/System/New")
        );
        assert_eq!(
            parse("md  /System/A b "),
            Command::MakeDirectory("/System/A b")
        );
        assert_eq!(parse("rm /System/x.txt"), Command::Remove("/System/x.txt"));
        assert_eq!(parse("del /System/x.txt"), Command::Remove("/System/x.txt"));
        for word in ["mkdir", "md", "rm", "del"] {
            assert_eq!(parse(word), Command::Missing(word));
        }
        assert_eq!(
            (parse("clear"), parse("exit"), parse("   ")),
            (Command::Clear, Command::Exit, Command::Empty)
        );
        assert_eq!(parse(" cls "), Command::Clear, "as on Windows");
        assert_eq!(parse("services"), Command::Services);
        assert_eq!(
            parse("services now"),
            Command::Services,
            "it takes no words"
        );
        assert_eq!(parse("dir C:"), Command::Program("dir"));
        assert_eq!(
            parse("rm -rf /"),
            Command::Remove("-rf /"),
            "no flags: a path that is none, which the kernel refuses"
        );
        assert_eq!(
            parse("Help"),
            Command::Program("Help"),
            "commands are lower case"
        );
        assert_eq!(
            parse(" /System/Boot/meridian/programs/hello.mexec "),
            Command::Program("/System/Boot/meridian/programs/hello.mexec")
        );
    }

    #[test]
    fn a_program_is_found_by_its_name_or_its_path() {
        assert_eq!(
            program_path("hello"),
            "/System/Boot/meridian/programs/hello.mexec"
        );
        assert_eq!(program_path("/a/b.mexec"), "/a/b.mexec");
        assert_eq!(
            program_path("a/b"),
            "/System/Boot/meridian/programs/a/b.mexec"
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
            "help", "echo", "ls", "cat", "mkdir", "md", "rm", "del", "clear", "cls", "services",
            "exit",
        ];
        for word in known {
            let named = HELP
                .split(|c: char| !c.is_ascii_alphabetic())
                .any(|w| w == word);
            assert!(named, "help leaves out {word}");
            assert!(
                !matches!(parse(word), Command::Program(_)),
                "{word} is no command"
            );
        }
        let words = HELP
            .split(|c: char| !c.is_ascii_alphabetic())
            .filter(|w| w.chars().all(|c| c.is_ascii_lowercase()) && !w.is_empty());
        for word in words {
            assert!(
                known.contains(&word) || ["or", "a", "program"].contains(&word),
                "help names {word}, which the shell does not know"
            );
        }
        assert_eq!(parse("nope"), Command::Program("nope"));
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
