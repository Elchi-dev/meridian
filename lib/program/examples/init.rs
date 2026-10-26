//! The first program: it starts the services that
//! `/System/Boot/meridian/services.meri` lists, each in a thread of its own
//! that starts it again whenever it ended, and waits longer before that
//! each time it ended soon after it started. A service may run once
//! instead, and may wait for services listed above it: for one that runs
//! once until it ended, for any other until it started. Without that file,
//! as on a CD, the shell is the one service. Each program a service runs
//! starts with a line to init, which answers what it asks about the
//! services ([`services`]). Handed a channel end, init answers the same
//! questions there, and ends every service and then itself once the other
//! end is closed.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{
    AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst,
};

use meri::{Event, Reader, Value};
use program::{Ended, Error, Handle, abi::file, log};
use services::{About, MESSAGE, State};

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

/// Whether the services that wait for each one may start: once it ended
/// when it runs once, and once it started otherwise.
static READY: [AtomicBool; MOST] = [const { AtomicBool::new(false) }; MOST];

/// What each service is doing, as its place in [`STATES`], and how often
/// it started.
static STATE: [AtomicU8; MOST] = [const { AtomicU8::new(0) }; MOST];
static STARTS: [AtomicU32; MOST] = [const { AtomicU32::new(0) }; MOST];
const STATES: [State; 4] = [State::Waiting, State::Running, State::Resting, State::Ended];

/// init's end of the line each service's program runs with, 0 for none.
static LINES: [AtomicU64; MOST] = [const { AtomicU64::new(0) }; MOST];

/// Whether init is ending its services.
static STOPPING: AtomicBool = AtomicBool::new(false);

/// How many of init's threads still keep a service. init ends once none
/// does: its end ends the whole program, so a thread that just started a
/// service again would leave that service running past it.
static KEEPING: AtomicUsize = AtomicUsize::new(0);

/// A service that ended sooner than this many ticks after it started
/// ended soon, and waits longer before it starts again.
const SOON: u64 = 100;

/// A service: its name, the program it runs, by its name among the
/// loader's programs or by a path from the root, whether it runs once
/// instead of again whenever it ended, and the names of the services it
/// waits for.
#[derive(Debug, PartialEq, Eq)]
struct Service {
    name: String,
    program: String,
    once: bool,
    after: Vec<String>,
}

/// A word or a string as text; none for any other value.
fn text(value: Value<'_>) -> Option<String> {
    match value {
        Value::Word(word) => Some(word.into()),
        Value::Str(text) => Some(text.chars().collect()),
        _ => None,
    }
}

/// The services `list` names, in order: each `service` block with a label
/// at its root that holds a `program`, a word or a string. `once = true`
/// runs it once, and `after` names the services it waits for, one as a
/// word or a string, or several in a list.
fn services(list: &[u8]) -> Result<Vec<Service>, meri::Error> {
    meri::Document::parse(list)?;
    let mut found = Vec::new();
    let mut depth = 0;
    let mut open: Option<(Service, bool)> = None;
    // How deep the reader is in the list of an `after`, 0 outside one.
    let mut in_after: usize = 0;
    for event in Reader::new(list) {
        match event? {
            Event::Block { name, label, .. } => {
                depth += 1;
                if depth == 1 && name == "service" {
                    open = label.map(|l| {
                        let service = Service {
                            name: l.chars().collect(),
                            program: String::new(),
                            once: false,
                            after: Vec::new(),
                        };
                        (service, false)
                    });
                }
            }
            Event::End => {
                if depth == 1
                    && let Some((service, true)) = open.take()
                {
                    found.push(service);
                }
                depth -= 1;
            }
            Event::Entry { key, value, .. } if depth == 1 => {
                let Some((service, has_program)) = open.as_mut() else {
                    continue;
                };
                match (key, value) {
                    ("program", value) => {
                        *has_program = false;
                        if let Some(program) = text(value) {
                            service.program = program;
                            *has_program = true;
                        }
                    }
                    ("once", Value::Bool(once)) => service.once = once,
                    ("after", Value::List) => in_after = 1,
                    ("after", value) => service.after.extend(text(value)),
                    _ => {}
                }
            }
            Event::Element { value, .. } if in_after > 0 => match value {
                Value::List => in_after += 1,
                value if in_after == 1 => {
                    if let Some((service, _)) = open.as_mut() {
                        service.after.extend(text(value));
                    }
                }
                _ => {}
            },
            Event::ListEnd => in_after = in_after.saturating_sub(1),
            _ => {}
        }
    }
    Ok(found)
}

/// The places in `services` of those each one waits for: those of its
/// `after` names listed above it, which keeps any from waiting on itself
/// or in a ring. Each name that is no service above it comes back too, as
/// what the log says about it.
fn waits(services: &[Service]) -> (Vec<Vec<usize>>, Vec<String>) {
    let mut unknown = Vec::new();
    let waits = services
        .iter()
        .enumerate()
        .map(|(at, service)| {
            service
                .after
                .iter()
                .filter_map(|name| {
                    let place = services[..at].iter().position(|s| s.name == *name);
                    if place.is_none() {
                        unknown.push(format!(
                            "init: {} waits for {name}, which is no service above it",
                            service.name
                        ));
                    }
                    place
                })
                .collect()
        })
        .collect();
    (waits, unknown)
}

/// Whether a service that waits for the services in the places `waits`
/// may start, as `ready` says of each place.
fn may_start(waits: &[usize], ready: impl Fn(usize) -> bool) -> bool {
    waits.iter().all(|&at| ready(at))
}

/// What init says about the service at `place` among `names`, as `now`
/// gives its state and how often it started; none past the last.
fn about(names: &[String], place: usize, now: impl Fn(usize) -> (State, u32)) -> Option<About<'_>> {
    let name = names.get(place)?;
    let (state, starts) = now(place);
    Some(About {
        name,
        state,
        starts,
    })
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
    let mut services = services;
    services.truncate(MOST);
    let (waits, unknown) = waits(&services);
    for line in unknown {
        let _ = program::log(&line);
    }
    let names: Arc<Vec<String>> = Arc::new(services.iter().map(|s| s.name.clone()).collect());
    for (slot, (service, waits)) in services.into_iter().zip(waits).enumerate() {
        KEEPING.fetch_add(1, SeqCst);
        if program::thread(move || keep(slot, &service, &waits)).is_err() {
            done(slot);
        }
    }
    if KEEPING.load(SeqCst) == 0 {
        return 1;
    }
    let lines = Arc::clone(&names);
    if program::thread(move || serve(&lines)).is_err() {
        let _ = program::log("init: the services' lines have nobody to answer them");
    }
    let mut room = [0u8; MESSAGE];
    loop {
        match program::receive(control, &mut room) {
            Err(Error::Closed) => break,
            Ok((len, _)) => reply(control, &room[..len], &names),
            Err(Error::Empty) => program::sleep(1),
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
    while KEEPING.load(SeqCst) != 0 {
        program::sleep(1);
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
    while done != bytes.len() {
        match program::read(handle, done as u64, &mut bytes[done..]) {
            Ok(0) | Err(_) => break,
            Ok(len) => done += len,
        }
    }
    let _ = program::close(handle);
    (done == bytes.len()).then_some(bytes)
}

/// Starts `service` once the services in the places `waits` are ready,
/// and again whenever it ended unless it runs once, keeping what it runs
/// in `slot` of [`RUNNING`], until init ends its services.
#[cfg_attr(test, allow(dead_code))]
fn keep(slot: usize, service: &Service, waits: &[usize]) -> u64 {
    set_state(slot, State::Waiting);
    while !may_start(waits, |at| READY[at].load(SeqCst)) && !STOPPING.load(SeqCst) {
        program::sleep(1);
    }
    let file = file_of(&service.program);
    let mut soon = 0;
    while !STOPPING.load(SeqCst) {
        let start = program::ticks();
        let (ours, theirs) = program::channel().map_or((None, None), |(a, b)| (Some(a), Some(b)));
        let spawned = program::spawn(&file, theirs);
        if !service.once {
            READY[slot].store(true, SeqCst);
        }
        let _ = match spawned {
            Ok(child) => {
                LINES[slot].store(ours.map_or(0, |h| h.0), SeqCst);
                STARTS[slot].fetch_add(1, SeqCst);
                set_state(slot, State::Running);
                // Either this sees init stopping, or init sees the child.
                RUNNING[slot].store(child.0, SeqCst);
                if STOPPING.load(SeqCst) {
                    let _ = program::terminate(child, 0);
                }
                let how = program::wait(child);
                RUNNING[slot].store(0, SeqCst);
                let _ = program::close(child);
                let line = LINES[slot].swap(0, SeqCst);
                if line != 0 {
                    let _ = program::close(Handle(line));
                }
                match how {
                    Ok(how) => program::log(&ended(&service.name, how)),
                    Err(e) => log!("init: {} could not be waited for: {e:?}", service.name),
                }
            }
            Err(e) => {
                for end in [ours, theirs].into_iter().flatten() {
                    let _ = program::close(end);
                }
                log!("init: {} did not start: {e:?}", service.name)
            }
        };
        if service.once {
            break;
        }
        set_state(slot, State::Resting);
        soon = soon_again(soon, program::ticks() - start);
        rest(pause(soon));
    }
    set_state(slot, State::Ended);
    done(slot);
    0
}

/// Notes what the service in `slot` is doing.
#[cfg_attr(test, allow(dead_code))]
fn set_state(slot: usize, state: State) {
    STATE[slot].store(code(state), SeqCst);
}

/// The place in [`STATES`] of `state`.
fn code(state: State) -> u8 {
    let place = STATES.iter().position(|&s| s == state);
    place.and_then(|p| u8::try_from(p).ok()).unwrap_or(0)
}

/// The state at the place `code` in [`STATES`]; past them, the last.
fn state_of(code: u8) -> State {
    STATES[usize::from(code).min(STATES.len() - 1)]
}

/// Answers the questions programs ask over the lines of the services that
/// run them, looking at each line in turn and resting a tick when none
/// asked anything.
#[cfg_attr(test, allow(dead_code))]
fn serve(names: &[String]) -> u64 {
    let mut room = [0u8; MESSAGE];
    loop {
        let mut asked = false;
        for line in &LINES {
            let end = Handle(line.load(SeqCst));
            if end.0 != 0
                && let Ok((len, _)) = program::receive(end, &mut room)
            {
                reply(end, &room[..len], names);
                asked = true;
            }
        }
        if !asked {
            program::sleep(1);
        }
    }
}

/// Answers `message` on `end` when it asks about a service.
#[cfg_attr(test, allow(dead_code))]
fn reply(end: Handle, message: &[u8], names: &[String]) {
    let now = |at: usize| (state_of(STATE[at].load(SeqCst)), STARTS[at].load(SeqCst));
    if let Some((room, len)) = answer_to(message, names, now) {
        let _ = program::send(end, &room[..len], None);
    }
}

/// init's answer to `message`, knowing the services `names` and what `now`
/// says of each, and its length; none when `message` asks nothing.
fn answer_to(
    message: &[u8],
    names: &[String],
    now: impl Fn(usize) -> (State, u32),
) -> Option<([u8; MESSAGE], usize)> {
    let place = services::asked(message)?;
    let mut room = [0u8; MESSAGE];
    let len = services::answer(about(names, usize::from(place), now), &mut room);
    Some((room, len))
}

/// Counts the service in `slot` as kept no longer, and as ready for those
/// that wait for it, which no longer wait once it will not run again.
#[cfg_attr(test, allow(dead_code))]
fn done(slot: usize) {
    READY[slot].store(true, SeqCst);
    KEEPING.fetch_sub(1, SeqCst);
}

/// Sleeps `ticks`, or less once init ends its services.
#[cfg_attr(test, allow(dead_code))]
fn rest(ticks: u64) {
    for _ in 0..ticks {
        if STOPPING.load(SeqCst) {
            return;
        }
        program::sleep(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, program: &str) -> Service {
        Service {
            name: name.into(),
            program: program.into(),
            once: false,
            after: Vec::new(),
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
        let waits_twice = b"service a {\n    program = a\n}\nservice b {\n    program = b\n    after = a\n    after = [a]\n}\n";
        assert_eq!(
            services(waits_twice).unwrap_err().code,
            meri::Code::DuplicateKey,
            "after is said once, as a name or a list"
        );
    }

    #[test]
    fn a_service_may_run_once_and_wait_for_others() {
        let list = b"service setup {\n    program = setup\n    once = true\n}\nservice disk {\n    program = disk\n    after = setup\n}\nservice shell {\n    program = shell\n    after = [setup, \"disk\"]\n    once = false\n}\n";
        let found = services(list).unwrap();
        assert_eq!(
            found,
            [
                Service {
                    once: true,
                    ..service("setup", "setup")
                },
                Service {
                    after: alloc::vec!["setup".into()],
                    ..service("disk", "disk")
                },
                Service {
                    after: alloc::vec!["setup".into(), "disk".into()],
                    ..service("shell", "shell")
                },
            ]
        );
    }

    #[test]
    fn only_names_count_as_what_a_service_waits_for() {
        let list = b"service a {\n    program = a\n    once = yes\n    after = [b, 3, [c, d], \"e\"]\n    notes = [[f], g]\n    inner {\n        after = g\n    }\n}\nservice b {\n    after = [a]\n}\n";
        assert_eq!(
            services(list).unwrap(),
            [Service {
                after: alloc::vec!["b".into(), "e".into()],
                ..service("a", "a")
            }],
            "once takes true or false, and only words and strings at the top of the list are names"
        );
    }

    #[test]
    fn a_service_waits_only_for_those_listed_above_it() {
        let named = |name: &str, after: &[&str]| Service {
            after: after.iter().map(|&a| a.into()).collect(),
            ..service(name, name)
        };
        let list = [
            named("a", &["a"]),
            named("b", &["a"]),
            named("c", &["b", "a", "d", "nobody"]),
            named("d", &["c"]),
        ];
        let (each, unknown) = waits(&list);
        assert_eq!(
            each,
            [
                alloc::vec![],
                alloc::vec![0],
                alloc::vec![1, 0],
                alloc::vec![2]
            ]
        );
        assert_eq!(
            unknown,
            [
                "init: a waits for a, which is no service above it",
                "init: c waits for d, which is no service above it",
                "init: c waits for nobody, which is no service above it",
            ]
        );
        assert_eq!(waits(&[]), (alloc::vec![], alloc::vec![]));
    }

    #[test]
    fn a_service_starts_once_every_one_it_waits_for_is_ready() {
        let ready = [true, false, true];
        let at = |place: usize| ready[place];
        assert!(may_start(&[], at), "waiting for none");
        assert!(may_start(&[0, 2], at));
        assert!(!may_start(&[1], at));
        assert!(!may_start(&[0, 1, 2], at), "one not ready is enough");
        assert!(may_start(&[2, 2], at));
    }

    #[test]
    fn the_services_waited_for_may_be_named_as_strings() {
        let list = b"service \"two words\" {\n    program = a\n}\nservice b {\n    program = b\n    after = [\"two words\", \"t\\u{61}b\"]\n}\nservice tab {\n    program = t\n    after = \"b\"\n}\n";
        let found = services(list).unwrap();
        assert_eq!(found[1].after, ["two words", "tab"]);
        assert_eq!(found[2].after, ["b"]);
        let (each, unknown) = waits(&found);
        assert_eq!(each[1], [0], "tab comes after b, so b does not wait for it");
        assert_eq!(each[2], [1]);
        assert_eq!(
            unknown,
            ["init: b waits for tab, which is no service above it"]
        );
    }

    #[test]
    fn only_true_itself_runs_a_service_once() {
        let list = b"service a {\n    program = a\n    once = \"true\"\n}\nservice b {\n    program = b\n    once = 1\n}\nservice c {\n    program = c\n    once = false\n}\nservice d {\n    once = true\n    program = d\n}\n";
        let once: Vec<bool> = services(list).unwrap().iter().map(|s| s.once).collect();
        assert_eq!(once, [false, false, false, true]);
    }

    #[test]
    fn a_service_named_twice_is_waited_for_as_once() {
        let list = b"service a {\n    program = a\n}\nservice b {\n    program = b\n    after = [a, a]\n}\n";
        let found = services(list).unwrap();
        let (each, unknown) = waits(&found);
        assert_eq!(each[1], [0, 0]);
        assert_eq!(unknown, Vec::<String>::new());
        assert!(may_start(&each[1], |at| at == 0));
        assert!(!may_start(&each[1], |_| false));
    }

    #[test]
    fn init_tells_of_the_service_at_each_place_and_none_past_them() {
        let names: Vec<String> = ["exitwith", "whoami", "shell"].map(String::from).to_vec();
        let now = |at: usize| (STATES[at], u32::try_from(at).unwrap() * 3);
        assert_eq!(
            about(&names, 0, now),
            Some(About {
                name: "exitwith",
                state: State::Waiting,
                starts: 0
            })
        );
        assert_eq!(
            about(&names, 2, now),
            Some(About {
                name: "shell",
                state: State::Resting,
                starts: 6
            })
        );
        assert_eq!(about(&names, 3, now), None);
        assert_eq!(about(&[], 0, now), None);
        assert_eq!(
            STATES,
            [State::Waiting, State::Running, State::Resting, State::Ended]
        );
    }

    /// The services list sextant writes as `constant` in its image code.
    fn sextants_list(constant: &str) -> Vec<Service> {
        let source = include_str!("../../../tools/sextant/internal/image/image.go");
        let start = format!("const {constant} = `");
        let at = source.find(&start).expect("the list") + start.len();
        let list = &source[at..at + source[at..].find('`').expect("its end")];
        services(list.as_bytes()).expect("a list init reads")
    }

    #[test]
    fn init_reads_the_lists_sextant_writes_as_they_are_meant() {
        assert_eq!(sextants_list("Services"), [service("shell", "shell")]);
        let test = sextants_list("testServices");
        assert_eq!(
            test,
            [
                Service {
                    once: true,
                    ..service("setup", "whoami")
                },
                Service {
                    after: alloc::vec!["setup".into()],
                    ..service("whoami", "whoami")
                },
                Service {
                    after: alloc::vec!["whoami".into()],
                    ..service("shell", "shell")
                },
            ]
        );
        let (each, unknown) = waits(&test);
        assert_eq!(each, [alloc::vec![], alloc::vec![0], alloc::vec![1]]);
        assert_eq!(unknown, Vec::<String>::new());
    }

    #[test]
    fn each_state_keeps_its_own_place() {
        for state in STATES {
            assert_eq!(state_of(code(state)), state);
        }
        assert_eq!(STATES.map(code), [0, 1, 2, 3]);
        assert_eq!(state_of(4), State::Ended, "past the last");
        assert_eq!(state_of(u8::MAX), State::Ended);
    }

    #[test]
    fn init_answers_a_question_and_nothing_else() {
        let names: Vec<String> = ["exitwith", "whoami"].map(String::from).to_vec();
        let now = |at: usize| {
            let state = if at == 0 {
                State::Ended
            } else {
                State::Running
            };
            (state, 7)
        };
        let (room, len) = answer_to(&services::ask(1), &names, now).unwrap();
        assert_eq!(
            services::read(&room[..len]),
            Ok(Some(About {
                name: "whoami",
                state: State::Running,
                starts: 7
            }))
        );
        let (room, len) = answer_to(&services::ask(2), &names, now).unwrap();
        assert_eq!(services::read(&room[..len]), Ok(None), "none past the last");
        assert_eq!(answer_to(b"hello", &names, now), None);
        assert_eq!(answer_to(&[], &names, now), None);
        let (room, len) = answer_to(&services::ask(0), &[], now).unwrap();
        assert_eq!(&room[..len], b".");
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
