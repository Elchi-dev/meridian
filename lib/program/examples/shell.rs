//! The shell: it reads keys, shows what they type on the console, and runs
//! the line typed once Enter goes down. It knows help, echo, ls, cat,
//! clear and exit, and reads paths through the tree of files.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::{Error, Handle, KeyEvent, abi::file, layout};

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
    List(&'a str),
    Show(&'a str),
    Clear,
    Exit,
    Empty,
    Unknown(&'a str),
    Missing(&'a str),
}

/// Where `ls` looks without a path.
const HOME: &str = "/System/Boot";

fn parse(line: &str) -> Command<'_> {
    let line = line.trim();
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();
    match word {
        "" => Command::Empty,
        "help" => Command::Help,
        "echo" => Command::Echo(rest),
        "ls" if rest.is_empty() => Command::List(HOME),
        "ls" => Command::List(rest),
        "cat" if rest.is_empty() => Command::Missing("cat"),
        "cat" => Command::Show(rest),
        "clear" => Command::Clear,
        "exit" => Command::Exit,
        other => Command::Unknown(other),
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
        _ => "could not be read",
    };
    format!("{path}: {why}\n")
}

#[cfg_attr(test, allow(dead_code))]
fn shell(_: Handle) -> u64 {
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
        Command::Help => program::write("help, echo TEXT, ls [PATH], cat PATH, clear, exit\n"),
        Command::Echo(text) => program::write(&format!("{text}\n")),
        Command::List(path) => list(path),
        Command::Show(path) => show(path),
        Command::Clear => program::write("\u{c}"),
        Command::Exit => return false,
        Command::Empty => Ok(()),
        Command::Unknown(word) => program::write(&format!("{word}: no such command\n")),
        Command::Missing(word) => program::write(&format!("{word} needs a path\n")),
    };
    true
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
            (parse("clear"), parse("exit"), parse("   ")),
            (Command::Clear, Command::Exit, Command::Empty)
        );
        assert_eq!(parse("rm -rf /"), Command::Unknown("rm"));
        assert_eq!(
            parse("Help"),
            Command::Unknown("Help"),
            "commands are lower case"
        );
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
        assert_eq!(trouble("/a", Error::Unreadable), "/a: could not be read\n");
    }
}
