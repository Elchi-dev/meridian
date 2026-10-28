//! awk: runs an awk program over the records of the files its operands
//! name, or of its standard input without one, as POSIX's awk does, with
//! lib/awk. The first word is the program, unless `-f` names a file that
//! holds it, as often as wanted; `-F` gives the field separator and `-v`
//! an assignment made before `BEGIN`. Commands, for pipes and `system`,
//! run in the shell's sh dialect. It ends with the status the program
//! gives, or with 2 for words or a program it cannot take and when the
//! program cannot go on.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::file::{CREATE_ALWAYS, OPEN_ALWAYS, READ, WRITE};
use awk::run::{Awk, Fatal, Stream, World};
use program::start::{Given, Sink};
use program::{Ended, Handle, child, text};

// awk runs a program by recursion over its tree (lib/awk's limits).
program::main!(awk, stack = 256);

/// Where the program comes from: the first operand, or files.
#[derive(Debug, PartialEq, Eq)]
enum Source<'a> {
    Text(&'a str),
    Files(Vec<&'a str>),
}

/// What awk was asked for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    source: Source<'a>,
    /// Assignments of `-F` and `-v`, as a name and its value.
    assignments: Vec<(&'a str, &'a str)>,
    operands: &'a [String],
}

/// Whether `name` could name a variable.
fn is_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What awk's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut files = Vec::new();
    let mut assignments = Vec::new();
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        let letter = &word[1..2];
        let attached = &word[2..];
        if !matches!(letter, "f" | "F" | "v") {
            return Err(format!(
                "awk: -{} is no option\n",
                word[1..].chars().next().unwrap_or('-')
            ));
        }
        // The option's value is the rest of its word, or the next word.
        let (value, rest) = if attached.is_empty() {
            match rest.split_first() {
                Some((value, rest)) => (value.as_str(), rest),
                None => return Err(format!("awk: -{letter} wants a value\n")),
            }
        } else {
            (attached, rest)
        };
        match letter {
            "f" => files.push(value),
            "F" => assignments.push(("FS", value)),
            _ => match value.split_once('=') {
                Some((name, value)) if is_name(name) => assignments.push((name, value)),
                _ => return Err(format!("awk: -v {value}: wants a name, =, and its value\n")),
            },
        }
        words = rest;
    }
    let source = if files.is_empty() {
        let Some((text, rest)) = words.split_first() else {
            return Err(String::from(
                "awk: wants a program, or -f and the file that holds it\n",
            ));
        };
        words = rest;
        Source::Text(text)
    } else {
        Source::Files(files)
    };
    Ok(Asked {
        source,
        assignments,
        operands: words,
    })
}

/// What is open by a name.
enum Open {
    Read {
        file: Handle,
        at: u64,
    },
    Write {
        file: Handle,
        at: u64,
    },
    Append(Handle),
    /// A command, and this end of the channel its output comes over or
    /// its input goes over.
    Command {
        process: Handle,
        end: Handle,
    },
}

/// Meridian's files and programs, and awk's own streams.
struct System {
    out: text::Out<text::Write>,
    open: BTreeMap<Vec<u8>, Open>,
}

/// A handle for `sink` of a child's own.
fn handed(sink: Option<Sink>) -> Option<Sink> {
    match sink? {
        Sink::Channel(end) => program::duplicate(end).ok().map(Sink::Channel),
        Sink::File(file) => program::duplicate(file).ok().map(Sink::File),
    }
}

/// The status awk gives for how a program ended.
fn status(ended: Result<Ended, program::Error>) -> i32 {
    match ended {
        Ok(Ended::Exited(code)) => i32::try_from(code).unwrap_or(i32::MAX),
        _ => 256,
    }
}

/// What awk says about `e`.
fn says(e: program::Error) -> String {
    let said = text::trouble("", "", e);
    String::from(said.trim_start_matches(": ").trim_end())
}

/// Closes `open`, and waits for a command: its status.
fn shut(open: &Open) -> i32 {
    match open {
        Open::Read { file, .. } | Open::Write { file, .. } | Open::Append(file) => {
            i32::from(program::close(*file).is_err())
        }
        Open::Command { process, end } => {
            let _ = program::close(*end);
            let ended = program::wait(*process);
            let _ = program::close(*process);
            status(ended)
        }
    }
}

impl System {
    /// Starts `command` in the sh dialect, reading from `input` and
    /// writing to `output`, and its errors to awk's.
    fn start(command: &[u8], input: Handle, output: Option<Sink>) -> Result<Handle, String> {
        let given = Given {
            input: Some(input),
            output,
            error: handed(program::start::given().error),
            line: None,
        };
        let shell = child::path("shell", |path| {
            program::open(path, READ).is_ok_and(|file| {
                let _ = program::close(file);
                true
            })
        });
        let words = ["--sh", "-c", &String::from_utf8_lossy(command)];
        let (process, start) = program::spawn_handing(&shell, &words, given).map_err(says)?;
        let _ = program::close(start);
        Ok(process)
    }

    /// Closes everything still open, as awk ends; whether all output went.
    fn finish(mut self) -> bool {
        self.out.flush();
        let open = core::mem::take(&mut self.open);
        for (_, open) in open {
            shut(&open);
        }
        self.out.finish()
    }
}

impl World for System {
    fn read(&mut self, from: Stream) -> Result<Vec<u8>, String> {
        let mut room = [0u8; program::abi::MESSAGE_BYTES];
        let (name, command) = match from {
            Stream::Standard => {
                let len = program::input(&mut room).map_err(says)?;
                return Ok(room[..len].to_vec());
            }
            Stream::File(name) => (name, false),
            Stream::Command(name) => (name, true),
            Stream::Error | Stream::Append(_) => return Err(String::from("not for reading")),
        };
        if !self.open.contains_key(name) {
            let open = if command {
                self.out.flush();
                let (mine, theirs) = program::channel().map_err(says)?;
                let (input, ended) = program::channel().map_err(says)?;
                let _ = program::close(ended);
                let process = Self::start(name, input, Some(Sink::Channel(theirs)))?;
                Open::Command { process, end: mine }
            } else {
                let path = String::from_utf8_lossy(name);
                let file = program::open(&path, READ).map_err(says)?;
                Open::Read { file, at: 0 }
            };
            self.open.insert(name.to_vec(), open);
        }
        match self.open.get_mut(name) {
            Some(Open::Read { file, at }) => {
                let len = program::read(*file, *at, &mut room).map_err(says)?;
                *at += len as u64;
                Ok(room[..len].to_vec())
            }
            Some(Open::Command { end, .. }) => {
                let len = program::stream(*end, &mut room).map_err(says)?;
                Ok(room[..len].to_vec())
            }
            _ => Err(String::from("open for writing")),
        }
    }

    fn write(&mut self, to: Stream, bytes: &[u8]) -> Result<(), String> {
        let name = match to {
            Stream::Standard => {
                self.out.put(bytes);
                return Ok(());
            }
            Stream::Error => {
                self.out.flush();
                return program::write_error(&String::from_utf8_lossy(bytes)).map_err(says);
            }
            Stream::File(name) | Stream::Append(name) | Stream::Command(name) => name,
        };
        if !self.open.contains_key(name) {
            let path = String::from_utf8_lossy(name);
            let open = match to {
                Stream::File(_) => Open::Write {
                    file: program::open_with(&path, WRITE, CREATE_ALWAYS).map_err(says)?,
                    at: 0,
                },
                Stream::Append(_) => {
                    Open::Append(program::open_with(&path, WRITE, OPEN_ALWAYS).map_err(says)?)
                }
                _ => {
                    // What awk wrote before goes out before what the
                    // command writes.
                    self.out.flush();
                    let (mine, theirs) = program::channel().map_err(says)?;
                    let process =
                        Self::start(name, theirs, handed(program::start::given().output))?;
                    Open::Command { process, end: mine }
                }
            };
            self.open.insert(name.to_vec(), open);
        }
        match self.open.get_mut(name) {
            Some(Open::Write { file, at }) => {
                program::write_file(*file, *at, bytes).map_err(says)?;
                *at += bytes.len() as u64;
            }
            Some(Open::Append(file)) => {
                let end = program::info(*file).map_err(says)?.size;
                program::write_file(*file, end, bytes).map_err(says)?;
            }
            Some(Open::Command { end, .. }) => {
                for piece in program::start::pieces(bytes) {
                    program::send_waiting(*end, piece).map_err(says)?;
                }
            }
            _ => return Err(String::from("open for reading")),
        }
        Ok(())
    }

    fn close(&mut self, name: &[u8]) -> Option<i32> {
        let open = self.open.remove(name)?;
        Some(shut(&open))
    }

    fn flush(&mut self, _: Option<&[u8]>) -> bool {
        self.out.flush();
        true
    }

    fn system(&mut self, command: &[u8]) -> i32 {
        self.out.flush();
        let words = ["--sh", "-c", &String::from_utf8_lossy(command)];
        match child::run("shell", &words) {
            child::Ran::Ended(ended) => status(Ok(ended)),
            _ => 127,
        }
    }

    fn ticks(&mut self) -> u64 {
        program::ticks()
    }
}

/// The text the files `paths` hold, one after the other on lines of
/// their own, or why one could not be read.
fn program_in(paths: &[&str]) -> Result<String, String> {
    let mut texts = Vec::new();
    for path in paths {
        let mut bytes = Vec::new();
        text::pieces(Some(*path).filter(|&p| p != "-"), |piece| {
            bytes.extend_from_slice(piece);
            true
        })
        .map_err(|e| text::trouble("awk", path, e))?;
        texts.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    Ok(texts.join("\n"))
}

#[cfg_attr(test, allow(dead_code))]
fn awk(_: Handle) -> u64 {
    let say = |why: &str| {
        let _ = program::write_error(why);
        2
    };
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => return say(&why),
    };
    let text = match &asked.source {
        Source::Text(text) => String::from(*text),
        Source::Files(paths) => match program_in(paths) {
            Ok(text) => text,
            Err(why) => return say(&why),
        },
    };
    let program = match awk::parse::parse(&text) {
        Ok(program) => program,
        Err(e) => return say(&format!("awk: line {}: {}\n", e.line, e.what)),
    };
    let operands: Vec<Vec<u8>> = asked
        .operands
        .iter()
        .map(|o| o.as_bytes().to_vec())
        .collect();
    let mut run = Awk::new(&program, &operands);
    for (name, value) in &asked.assignments {
        run.assign(name.as_bytes(), value.as_bytes());
    }
    let mut system = System {
        out: text::standard_output(),
        open: BTreeMap::new(),
    };
    let ran = run.run(&mut system);
    let written = system.finish();
    match ran {
        Ok(status) if written => u64::try_from(status).unwrap_or(2),
        Ok(_) => say("awk: the standard output could not be written\n"),
        Err(Fatal(why)) => say(&format!("awk: {why}\n")),
    }
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
    fn the_first_word_is_the_program_and_the_rest_operands() {
        let given = words("{print} a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                source: Source::Text("{print}"),
                assignments: Vec::new(),
                operands: &given[1..],
            })
        );
        let given = words("-- -x a");
        assert_eq!(
            asked(&given).map(|a| a.source),
            Ok(Source::Text("-x")),
            "-- ends the options"
        );
        let given = words("- a");
        assert_eq!(
            asked(&given).map(|a| (a.source, a.operands.len())),
            Ok((Source::Text("-"), 1))
        );
    }

    #[test]
    fn options_take_their_values_attached_or_in_the_next_word() {
        let given = words("-F : -vx=1 -v y=a=b -F\t, -f p1 -fp2 op");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                source: Source::Files(alloc::vec!["p1", "p2"]),
                assignments: alloc::vec![("FS", ":"), ("x", "1"), ("y", "a=b"), ("FS", "\t,")],
                operands: &given[given.len() - 1..],
            })
        );
        let given = words("-f prog");
        assert_eq!(
            asked(&given).map(|a| a.operands.len()),
            Ok(0),
            "with -f every word left is an operand"
        );
    }

    #[test]
    fn words_it_cannot_take_are_refused() {
        for (text, why) in [
            (
                "",
                "awk: wants a program, or -f and the file that holds it\n",
            ),
            (
                "-F :",
                "awk: wants a program, or -f and the file that holds it\n",
            ),
            ("-x prog", "awk: -x is no option\n"),
            ("-F", "awk: -F wants a value\n"),
            ("-f", "awk: -f wants a value\n"),
            ("-v x", "awk: -v x: wants a name, =, and its value\n"),
            (
                "-v 1x=2 p",
                "awk: -v 1x=2: wants a name, =, and its value\n",
            ),
            ("-v =2 p", "awk: -v =2: wants a name, =, and its value\n"),
        ] {
            assert_eq!(asked(&words(text)), Err(String::from(why)), "{text:?}");
        }
    }

    #[test]
    fn names_are_what_a_variable_may_be_called() {
        for name in ["x", "_", "a1", "FS", "_b_2"] {
            assert!(is_name(name), "{name}");
        }
        for name in ["", "1a", "a-b", "a b", "ä"] {
            assert!(!is_name(name), "{name}");
        }
    }

    #[test]
    fn a_status_is_how_the_command_ended() {
        assert_eq!(status(Ok(Ended::Exited(3))), 3);
        assert_eq!(status(Ok(Ended::Exited(u64::MAX))), i32::MAX);
        assert_eq!(status(Ok(Ended::Faulted(14))), 256);
        assert_eq!(status(Err(program::Error::NotFound)), 256);
        assert_eq!(says(program::Error::NotFound), "no such file");
    }
}
