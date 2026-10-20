//! The first program: it starts the services that
//! `/System/Boot/meridian/services.meri` lists, each in a thread of its own
//! that starts it again whenever it ended, and waits longer before that
//! each time it ended soon after it started. Without that file, as on a
//! CD, the shell is the one service. Handed a channel end, it ends every
//! service and then itself once the other end is closed.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};

use meri::{Event, Reader, Value};
use program::{Ended, Error, Handle, abi::file, log};

program::main!(init);

/// Where the services are listed.
const LIST: &str = "/System/Boot/meridian/services.meri";

/// The list when there is none to read.
const ALONE: &[u8] = b"service shell {\n    program = shell\n}\n";

/// The most services there are.
const MOST: usize = 16;

/// The process each service runs now, as the number of init's handle to
/// it, 0 for none.
static RUNNING: [AtomicU64; MOST] = [const { AtomicU64::new(0) }; MOST];

/// Whether init is ending its services.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// A service that ended sooner than this many ticks after it started
/// ended soon, and waits longer before it starts again.
const SOON: u64 = 100;

/// A service: its name, and the program it runs, by its name among the
/// loader's programs or by a path from the root.
#[derive(Debug, PartialEq, Eq)]
struct Service {
    name: String,
    program: String,
}

/// The services `list` names, in order: each `service` block with a label
/// at its root that holds a `program`, a word or a string.
fn services(list: &[u8]) -> Result<Vec<Service>, meri::Error> {
    meri::Document::parse(list)?;
    let mut found = Vec::new();
    let mut depth = 0;
    let mut open: Option<(String, Option<String>)> = None;
    for event in Reader::new(list) {
        match event? {
            Event::Block { name, label, .. } => {
                depth += 1;
                if depth == 1 && name == "service" {
                    open = label.map(|l| (l.chars().collect(), None));
                }
            }
            Event::End => {
                if depth == 1
                    && let Some((name, Some(program))) = open.take()
                {
                    found.push(Service { name, program });
                }
                depth -= 1;
            }
            Event::Entry {
                key: "program",
                value,
                ..
            } if depth == 1 => {
                if let Some((_, program)) = open.as_mut() {
                    *program = match value {
                        Value::Word(word) => Some(word.into()),
                        Value::Str(text) => Some(text.chars().collect()),
                        _ => None,
                    };
                }
            }
            _ => {}
        }
    }
    Ok(found)
}

/// What SPAWN takes for `program`: a path as it is, or a name as the file
/// the loader read.
fn file_of(program: &str) -> String {
    if program.starts_with('/') {
        program.into()
    } else {
        format!("{program}.mexec")
    }
}

/// How many times in a row a service ended soon after it started, given
/// how often it did before and how many ticks it ran this time.
fn soon_again(soon: u32, ran: u64) -> u32 {
    if ran < SOON {
        soon.saturating_add(1)
    } else {
        0
    }
}

/// How many ticks a service waits before it starts again, having ended
/// soon after it started `soon` times in a row: none the first time, then
/// twice as long each time, up to 128 ticks.
fn pause(soon: u32) -> u64 {
    match soon {
        0 => 0,
        n => 1 << (n - 1).min(7),
    }
}

/// How the log tells that a service ended.
fn ended(name: &str, how: Ended) -> String {
    match how {
        Ended::Exited(code) => format!("init: {name} ended with {code}"),
        Ended::Faulted(vector) => format!("init: {name} stopped by exception {vector}"),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn init(control: Handle) -> u64 {
    let list = read(LIST).unwrap_or_else(|| ALONE.into());
    let services = services(&list).unwrap_or_else(|e| {
        let _ = log!("init: {LIST}: {e}");
        services(ALONE).unwrap_or_default()
    });
    let mut started = 0;
    for (slot, service) in services.into_iter().take(MOST).enumerate() {
        if program::thread(move || keep(slot, &service)).is_ok() {
            started += 1;
        }
    }
    if started == 0 {
        return 1;
    }
    let mut room = [0u8; 1];
    loop {
        match program::receive(control, &mut room) {
            Err(Error::Closed) => break,
            Err(Error::Empty) | Ok(_) => program::sleep(10),
            Err(_) => program::end_thread(0),
        }
    }
    STOPPING.store(true, SeqCst);
    for running in &RUNNING {
        let child = running.load(SeqCst);
        if child != 0 {
            let _ = program::terminate(Handle(child), 0);
        }
    }
    0
}

/// The whole file at `path`, when it can be read.
#[cfg_attr(test, allow(dead_code))]
fn read(path: &str) -> Option<Vec<u8>> {
    let handle = program::open(path, file::READ).ok()?;
    let size = program::info(handle).ok()?.size;
    let mut bytes = alloc::vec![0; usize::try_from(size).ok()?];
    let mut done = 0;
    while done < bytes.len() {
        match program::read(handle, done as u64, &mut bytes[done..]) {
            Ok(0) | Err(_) => break,
            Ok(len) => done += len,
        }
    }
    let _ = program::close(handle);
    (done == bytes.len()).then_some(bytes)
}

/// Starts `service`, and again whenever it ended, keeping what it runs
/// in `slot` of [`RUNNING`], until init ends its services.
#[cfg_attr(test, allow(dead_code))]
fn keep(slot: usize, service: &Service) -> u64 {
    let file = file_of(&service.program);
    let mut soon = 0;
    while !STOPPING.load(SeqCst) {
        let start = program::ticks();
        let _ = match program::spawn(&file, None) {
            Ok(child) => {
                // Either this sees init stopping, or init sees the child.
                RUNNING[slot].store(child.0, SeqCst);
                if STOPPING.load(SeqCst) {
                    let _ = program::terminate(child, 0);
                }
                let how = program::wait(child);
                RUNNING[slot].store(0, SeqCst);
                let _ = program::close(child);
                match how {
                    Ok(how) => program::log(&ended(&service.name, how)),
                    Err(e) => log!("init: {} could not be waited for: {e:?}", service.name),
                }
            }
            Err(e) => log!("init: {} did not start: {e:?}", service.name),
        };
        soon = soon_again(soon, program::ticks() - start);
        program::sleep(pause(soon));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, program: &str) -> Service {
        Service {
            name: name.into(),
            program: program.into(),
        }
    }

    #[test]
    fn the_list_names_each_service_and_its_program() {
        let list = b"# What init starts.\nservice shell {\n    program = shell\n}\n\nservice tools {\n    program = \"/System/Boot/meridian/programs/tools.mexec\"\n}\n";
        assert_eq!(
            services(list).unwrap(),
            [
                service("shell", "shell"),
                service("tools", "/System/Boot/meridian/programs/tools.mexec")
            ]
        );
        assert_eq!(services(ALONE).unwrap(), [service("shell", "shell")]);
    }

    #[test]
    fn only_labelled_services_with_a_program_count() {
        let list = b"service {\n    program = a\n}\nservice b {\n    note = x\n}\nservice c {\n    program = 3\n}\nother d {\n    program = d\n}\nservice e {\n    inner {\n        program = f\n    }\n}\nprogram = g\nservice h {\n    program = h\n}\n";
        assert_eq!(services(list).unwrap(), [service("h", "h")]);
        assert_eq!(services(b"").unwrap(), []);
    }

    #[test]
    fn names_and_programs_may_be_strings_with_escapes() {
        let list = b"service \"two words\" {\n    program = \"/a/\\u{62}.mexec\"\n}\nservice \"tab\\there\" {\n    program = \"x\"\n}\n";
        assert_eq!(
            services(list).unwrap(),
            [
                service("two words", "/a/b.mexec"),
                service("tab\there", "x")
            ]
        );
    }

    #[test]
    fn services_come_in_the_order_they_are_listed() {
        let list = b"service c {\n    program = c\n}\nservice a {\n    program = a\n}\nservice b {\n    program = b\n}\n";
        let names: Vec<String> = services(list)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["c", "a", "b"]);
    }

    #[test]
    fn a_service_listed_twice_is_a_broken_list() {
        let e = services(b"service a {\n    program = a\n}\nservice a {\n    program = b\n}\n")
            .unwrap_err();
        assert_eq!(e.code, meri::Code::DuplicateLabel);
        let twice = b"service a {\n    program = a\n    program = b\n}\n";
        assert_eq!(services(twice).unwrap_err().code, meri::Code::DuplicateKey);
    }

    #[test]
    fn a_service_that_ran_long_starts_counting_again() {
        assert_eq!(soon_again(0, 0), 1);
        assert_eq!(soon_again(3, 99), 4);
        assert_eq!(soon_again(3, 100), 0, "a second is long enough");
        assert_eq!(soon_again(7, u64::MAX), 0);
        assert_eq!(soon_again(u32::MAX, 0), u32::MAX);
        let mut soon = 0;
        let waits: Vec<u64> = [1, 1, 1, 500, 1]
            .iter()
            .map(|&ran| {
                soon = soon_again(soon, ran);
                pause(soon)
            })
            .collect();
        assert_eq!(waits, [1, 2, 4, 0, 1]);
    }

    #[test]
    fn a_broken_list_says_where() {
        let e = services(b"service shell {\n    program = shell\n").unwrap_err();
        assert_eq!(e.code, meri::Code::UnclosedBlock);
        assert!(services(b"service a {\n    program\n}\n").is_err());
    }

    #[test]
    fn a_program_is_started_by_its_path_or_its_name() {
        assert_eq!(file_of("shell"), "shell.mexec");
        assert_eq!(file_of("/a/b.mexec"), "/a/b.mexec");
    }

    #[test]
    fn a_service_that_ends_soon_waits_longer_each_time() {
        let pauses: Vec<u64> = (0..10).map(pause).collect();
        assert_eq!(pauses, [0, 1, 2, 4, 8, 16, 32, 64, 128, 128]);
        assert_eq!(pause(u32::MAX), 128);
        assert_eq!(SOON, 100);
    }

    #[test]
    fn the_log_says_how_a_service_ended() {
        assert_eq!(ended("shell", Ended::Exited(0)), "init: shell ended with 0");
        assert_eq!(
            ended("tools", Ended::Faulted(14)),
            "init: tools stopped by exception 14"
        );
    }
}
