//! ed: edits the file its operand names, or an empty buffer without one,
//! with the commands of its standard input, as POSIX's ed does, with
//! lib/ed. `-s` leaves out the byte counts and the `!` after a command,
//! and `-p` gives a prompt and turns it on. Commands for `!`, and for
//! `e`, `r` and `w` with `!`, run in the shell's sh dialect. Commands
//! read from anything but the console stop at the first mistake. It ends
//! with 0 when no command went wrong, 1 when one did, and 2 for words it
//! cannot take.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::file::{CREATE_ALWAYS, OPEN_ALWAYS, WRITE};
use ed::{Ed, World};
use program::start::{Given, Sink};
use program::text::{self, LineReader, Source};
use program::{Handle, child};

program::main!(ed);

/// What ed's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    quiet: bool,
    prompt: Option<&'a str>,
    file: Option<&'a str>,
}

/// What ed's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut quiet = false;
    let mut prompt = None;
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
        let letters = &word[1..];
        for (at, letter) in letters.char_indices() {
            match letter {
                's' => quiet = true,
                'p' => {
                    // The prompt is the rest of the word, or the next.
                    let attached = &letters[at + 1..];
                    if attached.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| String::from("ed: -p wants a prompt\n"))?;
                        prompt = Some(value.as_str());
                        words = rest;
                    } else {
                        prompt = Some(attached);
                    }
                    break;
                }
                _ => return Err(format!("ed: -{letter} is no option\n")),
            }
        }
    }
    let file = match words {
        [] => None,
        [file] => Some(file.as_str()),
        _ => return Err(String::from("ed: edits one file\n")),
    };
    Ok(Asked {
        quiet,
        prompt,
        file,
    })
}

/// What ed says about `e`.
fn says(e: program::Error) -> String {
    let said = text::trouble("", "", e);
    String::from(said.trim_start_matches(": ").trim_end())
}

/// Starts `command` in the sh dialect on `input`, writing on `output`,
/// and its errors on ed's.
fn start(command: &[u8], input: Handle, output: Option<Sink>) -> Result<Handle, String> {
    let given = Given {
        input: Some(input),
        output,
        error: child::handed(program::start::given().error),
        line: None,
    };
    child::shell(&String::from_utf8_lossy(command), given).map_err(says)
}

/// Waits for `process` to end, and lets it go.
fn finish(process: Handle) {
    let _ = program::wait(process);
    let _ = program::close(process);
}

/// Meridian's files and programs, and ed's own input and output.
struct System {
    input: LineReader<Source>,
    out: text::Out<text::Write>,
}

impl World for System {
    fn line(&mut self) -> Option<Vec<u8>> {
        // What ed wrote, a prompt too, shows before it waits for more.
        self.out.flush();
        self.input.line().ok().flatten()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.out.put(bytes);
    }

    fn read(&mut self, path: &[u8]) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        text::pieces(Some(&String::from_utf8_lossy(path)), |piece| {
            bytes.extend_from_slice(piece);
            true
        })
        .map_err(says)?;
        Ok(bytes)
    }

    fn write_file(&mut self, path: &[u8], bytes: &[u8], append: bool) -> Result<(), String> {
        let how = if append { OPEN_ALWAYS } else { CREATE_ALWAYS };
        let file = program::open_with(&String::from_utf8_lossy(path), WRITE, how).map_err(says)?;
        let end = if append {
            program::info(file).map(|info| info.size)
        } else {
            Ok(0)
        };
        let written = end.and_then(|at| program::write_file(file, at, bytes));
        let closed = program::close(file);
        written.and(closed).map_err(says)
    }

    fn run(&mut self, command: &[u8]) -> Result<Vec<u8>, String> {
        let (mine, theirs) = program::channel().map_err(says)?;
        let (input, ended) = program::channel().map_err(says)?;
        let _ = program::close(ended);
        let process = start(command, input, Some(Sink::Channel(theirs))).inspect_err(|_| {
            let _ = program::close(mine);
        })?;
        let mut bytes = Vec::new();
        let mut room = [0u8; program::abi::MESSAGE_BYTES];
        while let Ok(len @ 1..) = program::stream(mine, &mut room) {
            bytes.extend_from_slice(&room[..len]);
        }
        let _ = program::close(mine);
        finish(process);
        Ok(bytes)
    }

    fn pipe(&mut self, command: &[u8], input: &[u8]) -> Result<(), String> {
        // What ed wrote goes out before what the command writes.
        self.out.flush();
        let (mine, theirs) = program::channel().map_err(says)?;
        let output = child::handed(program::start::given().output);
        let process = start(command, theirs, output).inspect_err(|_| {
            let _ = program::close(mine);
        })?;
        let sent =
            program::start::pieces(input).try_for_each(|piece| program::send_waiting(mine, piece));
        let _ = program::close(mine);
        finish(process);
        sent.map_err(says)
    }

    fn shell(&mut self, command: &[u8]) {
        self.out.flush();
        let _ = child::run("shell", &["--sh", "-c", &String::from_utf8_lossy(command)]);
    }

    fn interactive(&mut self) -> bool {
        // What a program was not given stays the console's.
        program::start::given().input.is_none()
    }
}

#[cfg_attr(test, allow(dead_code))]
fn ed(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let mut system = System {
        input: LineReader::new(Source::Input),
        out: text::standard_output(),
    };
    let mut ed = Ed::new(asked.quiet, asked.prompt.map(str::as_bytes));
    let status = ed.run(asked.file.map(str::as_bytes), &mut system);
    let written = system.out.finish();
    u64::from(status != 0 || !written)
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

    fn taken(
        quiet: bool,
        prompt: Option<&'static str>,
        file: Option<&'static str>,
    ) -> Asked<'static> {
        Asked {
            quiet,
            prompt,
            file,
        }
    }

    #[test]
    fn its_words_are_options_then_one_file() {
        for (text, wanted) in [
            ("", taken(false, None, None)),
            ("f", taken(false, None, Some("f"))),
            ("-s f", taken(true, None, Some("f"))),
            ("-p * f", taken(false, Some("*"), Some("f"))),
            ("-p> f", taken(false, Some(">"), Some("f"))),
            ("-sp: f", taken(true, Some(":"), Some("f"))),
            ("-ps f", taken(false, Some("s"), Some("f"))),
            ("-p -s", taken(false, Some("-s"), None)),
            ("-s -s", taken(true, None, None)),
            ("-- -s", taken(false, None, Some("-s"))),
            ("-", taken(false, None, Some("-"))),
        ] {
            assert_eq!(asked(&words(text)), Ok(wanted), "{text:?}");
        }
    }

    #[test]
    fn words_it_cannot_take_are_refused() {
        assert_eq!(asked(&words("-x f")), Err("ed: -x is no option\n".into()));
        assert_eq!(
            asked(&words("-sx")),
            Err("ed: -x is no option\n".into()),
            "a letter after another"
        );
        assert_eq!(asked(&words("-p")), Err("ed: -p wants a prompt\n".into()));
        assert_eq!(
            asked(&words("-s -p")),
            Err("ed: -p wants a prompt\n".into())
        );
        assert_eq!(asked(&words("a b")), Err("ed: edits one file\n".into()));
        assert_eq!(
            asked(&words("f -s")),
            Err("ed: edits one file\n".into()),
            "no option comes after the file"
        );
        assert_eq!(asked(&words("-- a b")), Err("ed: edits one file\n".into()));
    }

    #[test]
    fn trouble_is_said_without_a_name() {
        assert_eq!(says(program::Error::NotFound), "no such file");
    }
}
