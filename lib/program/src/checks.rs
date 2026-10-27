//! Checks of every call from ring 3, for the QEMU test image, where the
//! test build of hello runs them. Each logs "ok" or "FAIL" with its name,
//! and hello ends with the number that failed, which the kernel's check
//! wants to be 0.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use abi::{LOG_BYTES, MESSAGE_BYTES};

use security::{Integrity, Privileges, Restriction, Token};

use crate::{
    Ended, Error, Handle, Info, channel, close, duplicate, info, key, list, log, make_directory,
    map, open, open_with, raw, read, receive, remove, rename, send, sleep, spawn, spawn_handing,
    spawn_restricted, spawn_with, start, stream, terminate, token, unmap, wait, write, write_file,
};

type Outcome = Result<(), &'static str>;

type Check = fn() -> Outcome;

const CHECKS: [(&str, Check); 70] = [
    ("log", lines),
    ("log.format", formatted),
    ("ticks", time_goes_on),
    ("channel", round_trip),
    ("channel.empty", nothing_waits),
    ("channel.room", room_too_small),
    ("channel.longest", longest_message),
    ("channel.carry", handle_carried),
    ("channel.closed", other_end_closed),
    ("channel.full", full),
    ("close", closed_twice),
    ("handle", not_a_handle),
    ("memory", pages_come_and_go),
    ("memory.sizes", sizes_there_are),
    ("alloc", heap_in_use),
    ("spawn", child_exits_with_what_it_was_sent),
    ("spawn.fault", child_runs_out_of_stack),
    ("spawn.missing", no_such_program),
    ("terminate", child_ended_from_outside),
    ("thread", threads_of_a_child),
    ("thread.here", a_thread_of_its_own),
    ("thread.moves", a_thread_takes_what_it_was_given),
    ("thread.stack", a_thread_has_all_its_stack),
    ("wait.other", wait_for_no_process),
    ("token", own_token),
    ("spawn.token", child_gets_no_more),
    ("file", settings_read_back),
    ("file.refused", files_that_do_not_open),
    ("file.list", directories_list_their_entries),
    ("file.write", files_written),
    ("file.write_refused", writes_refused),
    ("file.rename", files_renamed),
    ("directory", working_directory),
    ("spawn.file", child_from_its_file),
    ("handle.duplicate", duplicates_stand_for_the_same_object),
    ("spawn.arguments", arguments_arrive),
    ("spawn.output", output_goes_where_it_is_handed),
    ("spawn.input", input_is_read_to_its_end),
    ("spawn.error", errors_go_apart),
    ("stream", streams_end_and_close_what_they_bring),
    ("shell.script", shell_runs_a_script),
    ("shell.builtins", the_shells_own_commands),
    ("shell.status", shell_ends_with_the_status),
    ("shell.programs", shell_starts_programs),
    ("shell.files", shell_writes_files),
    ("shell.patterns", shell_matches_patterns),
    ("shell.file", shell_runs_a_file),
    ("shell.redirections", shell_redirects),
    ("shell.pipes", shell_pipes),
    ("shell.pipes.trouble", pipes_that_go_wrong),
    ("shell.pipes.files", pipes_and_files),
    ("shell.pipes.long", a_long_pipeline),
    ("shell.input", shell_hands_its_input_on),
    ("shell.redirections.both", reading_and_writing_files_at_once),
    ("shell.output.file", programs_write_into_the_shells_file),
    ("shell.bytes", bytes_go_through_as_they_are),
    ("shell.carry", files_copied_and_moved),
    ("shell.directory", the_working_directory),
    ("shell.errors", errors_go_where_2_sends_them),
    ("shell.errors.output", errors_go_with_the_output),
    ("tools", head_grep_and_sort),
    ("tools.files", tools_on_files),
    ("tools.input", tools_read_the_shells_input),
    ("tools.long", long_lines_and_many),
    ("tools.empty", tools_given_nothing),
    ("shell.pipes.own", the_shells_own_commands_in_pipes),
    ("shell.carry.itself", no_copy_onto_itself),
    ("shell.carry.capitals", a_move_to_other_capitals),
    ("tools.refused", tools_refuse_what_is_no_text),
    ("console", writes_on_the_console),
];

/// Runs every check and answers how many failed.
pub fn run() -> u64 {
    let mut failed = 0;
    for (name, check) in CHECKS {
        let _ = match check() {
            Ok(()) => crate::log!("ok program.{name}"),
            Err(why) => {
                failed += 1;
                crate::log!("FAIL program.{name}: {why}")
            }
        };
    }
    failed
}

fn expect(ok: bool, why: &'static str) -> Outcome {
    if ok { Ok(()) } else { Err(why) }
}

fn pair() -> Result<(Handle, Handle), &'static str> {
    channel().map_err(|_| "a channel should be made")
}

fn lines() -> Outcome {
    expect(log("a line") == Ok(()), "a short line should be taken")?;
    let long = [b'x'; LOG_BYTES + 1];
    let long = core::str::from_utf8(&long).map_err(|_| "x is text")?;
    expect(
        log(long) == Err(Error::TooLong),
        "a line longer than the log takes should be refused",
    )?;
    expect(
        log(&long[..LOG_BYTES]) == Ok(()),
        "the longest line should be taken",
    )
}

fn formatted() -> Outcome {
    let (two, four) = (2, 4);
    expect(
        crate::log!("{two} and {two} make {four}") == Ok(()),
        "a formatted line should be taken",
    )?;
    let mut line = crate::Line::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("{two}{four}"));
    expect(line.as_str() == "24", "a line should hold what was written")
}

fn time_goes_on() -> Outcome {
    let before = crate::ticks();
    crate::sleep(2);
    let after = crate::ticks();
    expect(
        after >= before + 2,
        "two ticks should pass in a sleep of two",
    )?;
    crate::yield_now();
    expect(crate::ticks() >= after, "time should not go back")
}

fn round_trip() -> Outcome {
    let (a, b) = pair()?;
    expect(
        a != b && a.0 != 0 && b.0 != 0,
        "the ends should be two handles",
    )?;
    let mut room = [0u8; 16];
    expect(send(a, b"ping", None) == Ok(()), "a message should go out")?;
    expect(
        receive(b, &mut room) == Ok((4, None)) && room[..4] == *b"ping",
        "the message should arrive whole and alone",
    )?;
    expect(send(b, b"pong!", None) == Ok(()), "the other way too")?;
    expect(
        receive(a, &mut room) == Ok((5, None)) && room[..5] == *b"pong!",
        "the answer should arrive",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn nothing_waits() -> Outcome {
    let (a, b) = pair()?;
    let mut room = [0u8; 4];
    expect(
        receive(b, &mut room) == Err(Error::Empty),
        "an empty channel should say so",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn room_too_small() -> Outcome {
    let (a, b) = pair()?;
    expect(
        send(a, b"12345678", None) == Ok(()),
        "a message should go out",
    )?;
    let mut small = [0u8; 4];
    expect(
        receive(b, &mut small) == Err(Error::TooSmall),
        "four bytes should not hold eight",
    )?;
    let mut room = [0u8; 8];
    expect(
        receive(b, &mut room) == Ok((8, None)) && room == *b"12345678",
        "the message should wait until there is room",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn longest_message() -> Outcome {
    let (a, b) = pair()?;
    let longest = [b'm'; MESSAGE_BYTES + 1];
    expect(
        send(a, &longest, None) == Err(Error::TooLong),
        "a message longer than a channel takes should be refused",
    )?;
    expect(
        send(a, &longest[..MESSAGE_BYTES], None) == Ok(()),
        "the longest message should go",
    )?;
    let mut room = [0u8; MESSAGE_BYTES];
    expect(
        receive(b, &mut room) == Ok((MESSAGE_BYTES, None)) && room == longest[..MESSAGE_BYTES],
        "and arrive whole",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn handle_carried() -> Outcome {
    let (sender, receiver) = pair()?;
    let (kept, far) = pair()?;
    expect(
        send(sender, b"x", Some(kept)) == Ok(()),
        "a handle should go along",
    )?;
    let mut room = [0u8; 8];
    let Ok((1, Some(arrived))) = receive(receiver, &mut room) else {
        return Err("the handle should arrive with its message");
    };
    expect(
        close(kept) == Err(Error::BadHandle),
        "the handle sent should be gone from the sender",
    )?;
    expect(
        send(arrived, b"through", None) == Ok(()),
        "the handle received should work",
    )?;
    expect(
        receive(far, &mut room) == Ok((7, None)) && room[..7] == *b"through",
        "and reach the channel it stands for",
    )?;
    expect(
        [sender, receiver, far, arrived]
            .into_iter()
            .all(|h| close(h) == Ok(())),
        "every end should close",
    )
}

fn other_end_closed() -> Outcome {
    let (a, b) = pair()?;
    expect(close(a) == Ok(()), "one end should close")?;
    expect(
        send(b, b"anyone?", None) == Err(Error::Closed),
        "a message to a closed end should be refused",
    )?;
    let mut room = [0u8; 8];
    expect(
        receive(b, &mut room) == Err(Error::Closed),
        "nothing should come from a closed end",
    )?;
    expect(close(b) == Ok(()), "the other end should close")
}

fn full() -> Outcome {
    let (a, b) = pair()?;
    let mut sent = 0;
    while send(a, b"more", None) == Ok(()) {
        sent += 1;
        if sent > 1000 {
            return Err("a channel should fill up");
        }
    }
    expect(
        send(a, b"more", None) == Err(Error::Full),
        "a full channel should say so",
    )?;
    let mut room = [0u8; 4];
    expect(
        receive(b, &mut room) == Ok((4, None)),
        "a message should leave",
    )?;
    expect(
        send(a, b"more", None) == Ok(()),
        "and make room for one more",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn closed_twice() -> Outcome {
    let (a, b) = pair()?;
    expect(close(a) == Ok(()), "a handle should close")?;
    expect(
        close(a) == Err(Error::BadHandle),
        "a closed handle should be gone",
    )?;
    expect(close(b) == Ok(()), "the other end should close")
}

fn pages_come_and_go() -> Outcome {
    let first = map(2).map_err(|_| "two pages should be given")?;
    let second = map(1).map_err(|_| "and one more")?;
    expect(
        first % 4096 == 0 && first >= 0x10_0000_0000 && second >= first + 2 * 4096,
        "pages should come on whole pages, from the heap region, one piece after another",
    )?;
    expect(unmap(first + 4096, 1) == Ok(()), "a part should go back")?;
    expect(
        unmap(first, 2) == Err(Error::BadAddress),
        "pages that went back already should not go again",
    )?;
    expect(unmap(first, 1) == Ok(()), "the rest should go back")?;
    expect(unmap(second, 1) == Ok(()), "and the other piece")?;
    let three = map(3).map_err(|_| "three pages should be given")?;
    expect(unmap(three, 3) == Ok(()), "three should go back at once")?;
    expect(
        (0..3).all(|i| unmap(three + i * 4096, 1) == Err(Error::BadAddress)),
        "and every one of them be gone",
    )?;
    expect(
        map(1).is_ok_and(|again| again > second),
        "what went back should not be given again",
    )
}

fn sizes_there_are() -> Outcome {
    expect(map(0) == Err(Error::BadSize), "no pages are no request")?;
    expect(
        map(16385) == Err(Error::BadSize),
        "one call gives at most 64 MiB",
    )?;
    expect(
        unmap(0x40_0000, 1) == Err(Error::BadAddress),
        "the program's image is not the heap's to give back",
    )?;
    expect(
        unmap(0x10_0000_0000, 0) == Err(Error::BadAddress),
        "nothing cannot go back either",
    )
}

fn heap_in_use() -> Outcome {
    let numbers: Vec<u64> = (1..=1000).collect();
    expect(
        numbers.iter().sum::<u64>() == 500_500,
        "a vector should hold what was pushed",
    )?;
    let text = alloc::format!("{} numbers, the last {}", numbers.len(), numbers[999]);
    expect(
        text == "1000 numbers, the last 1000",
        "a string should be formatted",
    )?;
    drop(numbers);
    let page = Box::new([7u8; 4096]);
    expect(page.iter().all(|&b| b == 7), "a box should hold a page")?;
    let mut big: Vec<u8> = Vec::new();
    expect(
        big.try_reserve(2 << 20).is_err(),
        "more than the heap holds should be refused, not crash",
    )?;
    let mut grown = String::new();
    for _ in 0..1000 {
        grown.push_str("room ");
    }
    expect(grown.len() == 5000, "a string should grow")?;
    for round in 0..64u8 {
        let block = alloc::vec![round; 64 * 1024];
        expect(
            block[65_535] == round,
            "a block should hold what it was made of",
        )?;
    }
    Ok(())
}

/// Ends the start messages over `mine` to the child holding the other
/// end, sending none before the end, as a parent may (decision record
/// 0017).
fn begin(mine: Handle) -> Outcome {
    expect(
        send(mine, &[crate::start::Kind::End.byte()], None) == Ok(()),
        "the start messages should end",
    )
}

/// Starts exitwith with one end of a fresh channel, and returns the
/// process and the other end.
fn exitwith() -> Result<(Handle, Handle), &'static str> {
    let (mine, theirs) = pair()?;
    let child = spawn("exitwith.mexec", Some(theirs)).map_err(|_| "exitwith should start")?;
    begin(mine)?;
    expect(
        close(theirs) == Err(Error::BadHandle),
        "the end handed over should be gone from the parent",
    )?;
    Ok((child, mine))
}

fn child_exits_with_what_it_was_sent() -> Outcome {
    let (child, end) = exitwith()?;
    expect(
        send(end, &42u64.to_le_bytes(), None) == Ok(()),
        "the number should go to the child",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(42)),
        "the child should exit with the number it was sent",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(42)),
        "and say so again while the handle is held",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

fn child_runs_out_of_stack() -> Outcome {
    let (child, end) = exitwith()?;
    expect(send(end, b"!", None) == Ok(()), "the message should go")?;
    expect(
        wait(child) == Ok(Ended::Faulted(14)),
        "a child that runs out of stack should end by a page fault",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

fn child_ended_from_outside() -> Outcome {
    let (child, end) = exitwith()?;
    expect(
        terminate(child, 77) == Ok(()),
        "a child should be asked to end",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(77)),
        "and end with the code it was asked to end with",
    )?;
    expect(
        terminate(child, 78) == Ok(()) && wait(child) == Ok(Ended::Exited(77)),
        "a child that ended should stay as it ended",
    )?;
    expect(
        send(end, b"late", None) == Err(Error::Closed),
        "and its end of the channel should be closed",
    )?;
    let (spinning, its) = exitwith()?;
    expect(
        send(its, b"", None) == Ok(()),
        "the child should be told to spin",
    )?;
    sleep(2);
    expect(
        terminate(spinning, 79) == Ok(()) && wait(spinning) == Ok(Ended::Exited(79)),
        "a child that never calls the kernel should still end at the next interrupt",
    )?;
    expect(
        close(spinning) == Ok(()) && close(its) == Ok(()),
        "its handles should close",
    )?;
    let (later, its) = exitwith()?;
    let (waiting, theirs) = exitwith()?;
    expect(
        send(theirs, b"wait", Some(later)) == Ok(()),
        "a child should be handed another to wait for",
    )?;
    sleep(2);
    expect(
        terminate(waiting, 81) == Ok(()) && wait(waiting) == Ok(Ended::Exited(81)),
        "a child that waits for another should still end once asked to",
    )?;
    expect(
        send(its, &5u64.to_le_bytes(), None) == Ok(()),
        "the other one should still be there to end on its own",
    )?;
    expect(
        close(waiting) == Ok(()) && close(theirs) == Ok(()) && close(its) == Ok(()),
        "their handles should close",
    )?;
    expect(
        terminate(end, 1) == Err(Error::Denied),
        "a channel is no process to end",
    )?;
    expect(
        terminate(Handle(0x7777_0042), 1) == Err(Error::BadHandle),
        "nor is a handle never given",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

/// Starts the threads program, tells it `word`, and answers its process
/// and the end of the channel it sends on.
fn threads(word: &[u8]) -> Result<(Handle, Handle), &'static str> {
    let (mine, theirs) = pair()?;
    let child = spawn("threads.mexec", Some(theirs)).map_err(|_| "threads should start")?;
    begin(mine)?;
    expect(send(mine, word, None) == Ok(()), "the word should go")?;
    Ok((child, mine))
}

fn threads_of_a_child() -> Outcome {
    let (child, end) = threads(b"sum")?;
    let mut got = 0;
    let mut room = [0u8; 8];
    for _ in 0..1000 {
        match receive(end, &mut room) {
            Ok((8, _)) => got |= 1 << u64::from_le_bytes(room),
            Ok(_) => return Err("each thread should send its number"),
            Err(_) => sleep(1),
        }
        if got == 0b11110 {
            break;
        }
    }
    expect(got == 0b11110, "four threads should each send their number")?;
    expect(
        wait(child) == Ok(Ended::Exited(0)),
        "and the program exit with 0 once they all did",
    )?;
    let ended = |word: &[u8], how: Ended| {
        threads(word).is_ok_and(|(child, end)| {
            wait(child) == Ok(how) && close(child).is_ok() && close(end).is_ok()
        })
    };
    expect(
        ended(b"last", Ended::Exited(7)),
        "a program should end with its last thread, with that one's code",
    )?;
    expect(
        ended(b"exit", Ended::Exited(9)),
        "an exit should end every thread of a program",
    )?;
    expect(
        ended(b"fault", Ended::Faulted(14)),
        "a fault in one thread should end the whole program",
    )?;
    expect(
        close(child) == Ok(()) && close(end) == Ok(()),
        "the handles should close",
    )
}

/// What hello's own second thread found.
static FOUND: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn found(n: u64) -> u64 {
    FOUND.store(n * 2, core::sync::atomic::Ordering::Release);
    0
}

fn a_thread_of_its_own() -> Outcome {
    expect(
        crate::thread(|| found(21)) == Ok(()),
        "hello should start a thread of its own",
    )?;
    let seen = (0..100).any(|_| {
        sleep(1);
        FOUND.load(core::sync::atomic::Ordering::Acquire) == 42
    });
    expect(seen, "which should run beside it, in its memory")
}

/// What the thread with the deep stack read back from both ends of it.
static DEEPEST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Fills all but 4 KiB of a thread's stack, a byte on every page, and
/// says what it reads back at both ends. A stack smaller than promised
/// runs into the page below it, and the fault ends hello.
fn deep() -> u64 {
    let mut room = [0u8; 60 << 10];
    let room = core::hint::black_box(&mut room);
    for page in room.chunks_mut(4096) {
        page[0] = 7;
    }
    let ends = u64::from(room[0]) + u64::from(room[room.len() - 4096]);
    DEEPEST.store(ends, core::sync::atomic::Ordering::Release);
    0
}

fn a_thread_has_all_its_stack() -> Outcome {
    expect(
        crate::thread(deep) == Ok(()),
        "a thread should start for a deep stack",
    )?;
    let seen = (0..100).any(|_| {
        sleep(1);
        DEEPEST.load(core::sync::atomic::Ordering::Acquire) == 14
    });
    expect(
        seen,
        "and have the 64 KiB its stack is promised, all but 4 KiB of it in use",
    )
}

/// What the thread that was given a list found in it.
static SUMMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn a_thread_takes_what_it_was_given() -> Outcome {
    let numbers: Vec<u64> = (1..=100).collect();
    let (mine, theirs) = pair()?;
    let started = crate::thread(move || {
        let sum: u64 = numbers.iter().sum();
        SUMMED.store(sum, core::sync::atomic::Ordering::Release);
        let sent = send(theirs, &numbers.len().to_le_bytes(), None);
        u64::from(sent.is_err())
    });
    expect(
        started == Ok(()),
        "a thread should start with a list of its own",
    )?;
    let mut room = [0u8; 8];
    let got = (0..100).find_map(|_| {
        if let Ok((8, _)) = receive(mine, &mut room) {
            return Some(u64::from_le_bytes(room));
        }
        sleep(1);
        None
    });
    expect(
        got == Some(100) && SUMMED.load(core::sync::atomic::Ordering::Acquire) == 5050,
        "it should find every number it was given, in the memory it was moved to",
    )?;
    expect(
        close(mine) == Ok(()) && close(theirs) == Ok(()),
        "the channel's ends should close",
    )
}

fn no_such_program() -> Outcome {
    expect(
        spawn("nothing.mexec", None) == Err(Error::NoSuchProgram),
        "a name no file has should start nothing",
    )?;
    expect(
        spawn("", None) == Err(Error::NoSuchProgram),
        "neither should no name",
    )?;
    expect(
        spawn("a-name-much-longer-than-a-module-keeps.mexec", None) == Err(Error::NoSuchProgram),
        "nor one longer than any file's",
    )?;
    expect(
        spawn("exitwith.mexec", Some(Handle(0x7777_0042))) == Err(Error::BadHandle),
        "a handle the parent does not hold cannot be handed over",
    )
}

fn own_token() -> Outcome {
    expect(
        token() == Some(Token::system()),
        "hello, started by the kernel, should run as SYSTEM with every privilege",
    )?;
    let mut room = [0u8; Token::MAX_SIZE];
    let size = Token::system().size();
    let short = abi::Call::Token {
        room: size as u64 - 1,
        at: room.as_mut_ptr() as u64,
    };
    expect(
        abi::answer(raw::call(short).0) == Err(Error::TooSmall),
        "a byte too little room should take nothing",
    )?;
    let exact = abi::Call::Token {
        room: size as u64,
        at: room.as_mut_ptr() as u64,
    };
    expect(
        abi::answer(raw::call(exact).0) == Ok(size as u64)
            && Token::read(&room[..size]) == Some(Token::system()),
        "room for the token and no more should do",
    )?;
    let kernels = abi::Call::Token {
        room: size as u64,
        at: abi::USER_END,
    };
    expect(
        abi::answer(raw::call(kernels).0) == Err(Error::BadAddress),
        "the kernel's half is no room of the program's",
    )
}

/// What whoami, started with `restriction`, sends as its token.
fn whoami(restriction: Option<Restriction>) -> Result<Token, &'static str> {
    let (mine, theirs) = pair()?;
    let child = match restriction {
        None => spawn("whoami.mexec", Some(theirs)),
        Some(r) => spawn_restricted("whoami.mexec", Some(theirs), r),
    }
    .map_err(|_| "whoami should start")?;
    begin(mine)?;
    let ended = wait(child);
    let mut room = [0u8; MESSAGE_BYTES];
    let got = receive(mine, &mut room);
    let closed = close(child) == Ok(()) && close(mine) == Ok(());
    match (ended, got, closed) {
        (Ok(Ended::Exited(0)), Ok((len, None)), true) => {
            Token::read(&room[..len]).ok_or("whoami's token should read back")
        }
        _ => Err("whoami should send its token and exit"),
    }
}

fn child_gets_no_more() -> Outcome {
    expect(
        whoami(None) == Ok(Token::system()),
        "a child should get its parent's token as it is",
    )?;
    let low = Restriction {
        integrity: Integrity::Low,
        keep: Privileges::SHUTDOWN,
    };
    let less = whoami(Some(low))?;
    expect(
        less.integrity == Integrity::Low
            && less.privileges == Privileges::SHUTDOWN
            && less == Token::system().restricted(low),
        "a restricted child should run low, with only the privilege it kept",
    )?;
    let bad = abi::Call::Spawn {
        len: 12,
        at: "whoami.mexec".as_ptr() as u64,
        carry: 0,
        restriction: 1,
    };
    expect(
        abi::answer(raw::call(bad).0) == Err(Error::BadArgument),
        "a word that holds no restriction should start nothing",
    )
}

/// The settings sextant writes into test images.
const SETTINGS: &[u8] =
    b"boot {\n    timeout = 2500ms\n    default = example\n    verbose = true\n}\n";

/// Whether the boot volume hangs in the tree, as it does whenever the
/// machine started from a disk; the kernel's own check wants it then.
fn booted_from_disk() -> bool {
    open("/System/Boot", 0).is_ok_and(|boot| close(boot).is_ok())
}

fn child_from_its_file() -> Outcome {
    let exitwith = "/System/Boot/meridian/programs/exitwith.mexec";
    if !booted_from_disk() {
        return expect(
            spawn(exitwith, None) == Err(Error::NotFound),
            "booted from a CD, there should be no file to start",
        );
    }
    let (mine, theirs) = pair()?;
    let child = spawn(exitwith, Some(theirs)).map_err(|_| "exitwith should start from its file")?;
    begin(mine)?;
    expect(
        send(mine, &7u64.to_le_bytes(), None) == Ok(()),
        "the number should go to the child",
    )?;
    expect(
        wait(child) == Ok(Ended::Exited(7)),
        "the child started from its file should exit with the number it was sent",
    )?;
    expect(
        close(child) == Ok(()) && close(mine) == Ok(()),
        "the handles should close",
    )?;
    expect(
        spawn("/System/Boot/meridian/programs", None) == Err(Error::NotAFile),
        "a directory is no program",
    )?;
    expect(
        spawn("/System/Boot/meridian/boot.meri", None) == Err(Error::NotAProgram),
        "nor are the loader's settings",
    )?;
    expect(
        spawn("/System/Boot/meridian/programs/nothing.mexec", None) == Err(Error::NotFound),
        "a path to no file starts nothing",
    )?;
    let longest = alloc::format!("/{}", "a".repeat(abi::PATH_BYTES - 1));
    expect(
        spawn(&longest, None) == Err(Error::NotFound)
            && spawn(&alloc::format!("{longest}b"), None) == Err(Error::TooLong),
        "a path as long as a path may be is looked for, and one longer refused",
    )
}

fn settings_read_back() -> Outcome {
    if !booted_from_disk() {
        return expect(
            open("/System/Boot/meridian/boot.meri", abi::file::READ) == Err(Error::NotFound),
            "booted from a CD, there should be no boot.meri to open",
        );
    }
    let file = open("/System/Boot/meridian/boot.meri", abi::file::READ)
        .map_err(|_| "boot.meri should open from the EFI system partition")?;
    expect(
        info(file)
            == Ok(Info {
                size: SETTINGS.len() as u64,
                directory: false,
            }),
        "its size should be the one sextant wrote",
    )?;
    let mut room = [0u8; 128];
    expect(
        read(file, 0, &mut room) == Ok(SETTINGS.len()) && room[..SETTINGS.len()] == *SETTINGS,
        "it should read as sextant wrote it",
    )?;
    expect(
        read(file, 11, &mut room[..7]) == Ok(7) && room[..7] == *b"timeout",
        "from a byte in the middle on",
    )?;
    expect(
        read(file, SETTINGS.len() as u64, &mut room) == Ok(0),
        "and nothing from its end on",
    )?;
    expect(close(file) == Ok(()), "the file should close")?;
    let again = open("/system/BOOT/Meridian/BOOT.MERI", abi::file::READ)
        .map_err(|_| "a path in other capitals should name the same file")?;
    expect(
        read(again, 0, &mut room) == Ok(SETTINGS.len()) && close(again) == Ok(()),
        "and read the same",
    )
}

fn files_that_do_not_open() -> Outcome {
    expect(
        open("/System/Boot/meridian/nothing", abi::file::READ) == Err(Error::NotFound),
        "a name no file has should open nothing",
    )?;
    expect(
        open("/System/Boot/meridian/boot.meri/inside", abi::file::READ) == Err(Error::NotFound),
        "nor a name inside a file",
    )?;
    expect(
        open("/Nowhere/x", abi::file::READ) == Err(Error::NotFound),
        "nor a path no volume hangs at",
    )?;
    let opened = |path: &str| {
        abi::answer(
            raw::call(abi::Call::Open {
                len: path.len() as u64,
                at: path.as_ptr() as u64,
                wanted: u64::from(abi::file::READ),
                how: 0,
            })
            .0,
        )
    };
    expect(
        opened("meridian/boot.meri") == Err(Error::BadArgument)
            && opened("/System/../..") == Err(Error::BadArgument),
        "the kernel should take a path not from the root, or above it, as none",
    )?;
    expect(
        open("meridian/boot.meri", abi::file::READ) == Err(Error::NotFound)
            && crate::full_path("meridian/boot.meri") == "/meridian/boot.meri",
        "the runtime should start a path not from the root at the working directory, the root here",
    )?;
    let long = [b'/'; abi::PATH_BYTES + 1];
    let too_long = abi::Call::Open {
        len: long.len() as u64,
        at: long.as_ptr() as u64,
        wanted: u64::from(abi::file::READ),
        how: 0,
    };
    expect(
        abi::answer(raw::call(too_long).0) == Err(Error::TooLong),
        "a path longer than PATH_BYTES",
    )?;
    let mut longest = [b'a'; abi::PATH_BYTES];
    longest[0] = b'/';
    let just = abi::Call::Open {
        len: longest.len() as u64,
        at: longest.as_ptr() as u64,
        wanted: u64::from(abi::file::READ),
        how: 0,
    };
    expect(
        abi::answer(raw::call(just).0) == Err(Error::NotFound),
        "a path of PATH_BYTES is still one, if of no file",
    )?;
    let stranger = Handle(0x7777_0042);
    let mut room = [0u8; 8];
    expect(
        read(stranger, 0, &mut room) == Err(Error::BadHandle)
            && info(stranger) == Err(Error::BadHandle)
            && list(stranger, 0, &mut room) == Err(Error::BadHandle),
        "a made up handle is no file to read, ask about or list",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        open("/System/Boot/meridian/boot.meri", abi::file::WRITE) == Err(Error::ReadOnly),
        "nothing writes to a volume mounted read only, and it says so",
    )?;
    let directory =
        open("/System/Boot/meridian", abi::file::READ).map_err(|_| "a directory should open")?;
    expect(
        info(directory).is_ok_and(|i| i.directory)
            && read(directory, 0, &mut room) == Err(Error::NotAFile)
            && close(directory) == Ok(()),
        "a directory should say it is one, and have no bytes to read",
    )?;
    let blind =
        open("/System/Boot/meridian/boot.meri", 0).map_err(|_| "a file should open for nothing")?;
    expect(
        read(blind, 0, &mut room) == Err(Error::Denied) && close(blind) == Ok(()),
        "a handle opened without the right to read should not read",
    )
}

fn directories_list_their_entries() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let meridian = open("/System/Boot/meridian", abi::file::READ)
        .map_err(|_| "the meridian directory should open")?;
    let mut names = Vec::new();
    let mut room = [0u8; 64];
    for index in 0.. {
        match list(meridian, index, &mut room) {
            Ok((len, directory)) => names.push((
                String::from_utf8_lossy(&room[..len]).into_owned(),
                directory,
            )),
            Err(Error::NotFound) => break,
            Err(_) => return Err("each entry should list until the last"),
        }
    }
    let has = |name: &str, directory: bool| names.iter().any(|(n, d)| n == name && *d == directory);
    expect(
        has("boot.meri", false) && has("kernel", false) && has("programs", true),
        "the meridian directory should list boot.meri, the kernel and the programs' directory",
    )?;
    expect(
        list(meridian, 0, &mut room[..1]) == Err(Error::TooSmall),
        "a name should not be cut to fit",
    )?;
    let settings = open("/System/Boot/meridian/boot.meri", abi::file::READ)
        .map_err(|_| "boot.meri should open")?;
    expect(
        list(settings, 0, &mut room) == Err(Error::NotADirectory),
        "a file has no entries",
    )?;
    let blind =
        open("/System/Boot/meridian", 0).map_err(|_| "a directory should open for nothing")?;
    expect(
        list(blind, 0, &mut room) == Err(Error::Denied),
        "a handle opened without the right to read should not list",
    )?;
    expect(
        close(meridian) == Ok(()) && close(settings) == Ok(()) && close(blind) == Ok(()),
        "the handles should close",
    )
}

/// On /System: a file made, written in more pieces than one call takes,
/// read back, and removed once closed; a directory made and removed.
/// Where /System hangs read only, as on a disk that only reads, nothing is
/// made or removed there.
fn files_written() -> Outcome {
    use abi::file::{CREATE_NEW, READ, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/ring3.txt";
    let file = match open_with(path, READ | WRITE, CREATE_NEW) {
        Err(Error::ReadOnly) => {
            return expect(
                make_directory("/System/Ring3") == Err(Error::ReadOnly)
                    && remove("/System/made.meri") == Err(Error::ReadOnly),
                "on /System read only nothing should be made or removed",
            );
        }
        other => other.map_err(|_| "a file should be made on /System")?,
    };
    let other = open("/System/made.meri", READ).map_err(|_| "made.meri should open")?;
    let size = info(other).map(|i| i.size);
    let bytes: Vec<u8> = (0..abi::WRITE_BYTES + 100)
        .map(|i| b'a' + (i % 26).to_le_bytes()[0])
        .collect();
    expect(
        write_file(file, 0, &bytes) == Ok(()),
        "a file should take more than one call writes",
    )?;
    let mut back = alloc::vec![0u8; bytes.len()];
    let mut done = 0;
    while let Ok(n @ 1..) = read(file, done as u64, &mut back[done..]) {
        done += n;
    }
    expect(
        back == bytes && info(file).map(|i| i.size) == Ok(bytes.len() as u64),
        "the file should read back as written",
    )?;
    expect(
        info(other).map(|i| i.size) == size && close(other) == Ok(()),
        "another file open on the volume should keep its size",
    )?;
    let too_long = abi::Call::WriteFile {
        handle: file.0,
        at: bytes.as_ptr() as u64,
        len: (abi::WRITE_BYTES + 1) as u64,
        offset: 0,
    };
    expect(
        abi::answer(raw::call(too_long).0) == Err(Error::TooLong),
        "one call should take at most WRITE_BYTES",
    )?;
    let reader = open(path, READ).map_err(|_| "the file should open again")?;
    let too_long = abi::Call::WriteFile {
        handle: reader.0,
        at: bytes.as_ptr() as u64,
        len: (abi::WRITE_BYTES + 1) as u64,
        offset: 0,
    };
    expect(
        write_file(reader, 0, b"x") == Err(Error::Denied)
            && abi::answer(raw::call(too_long).0) == Err(Error::Denied)
            && remove(path) == Err(Error::InUse),
        "a handle that may only read should not write, whatever it hands over, and an open file should stay",
    )?;
    expect(
        close(reader) == Ok(()) && close(file) == Ok(()),
        "the handles should close",
    )?;
    expect(
        remove(path) == Ok(()) && open(path, READ) == Err(Error::NotFound),
        "a closed file should go",
    )?;
    expect(
        make_directory("/System/Ring3") == Ok(())
            && make_directory("/System/Ring3") == Err(Error::Exists)
            && remove("/System/Ring3") == Ok(()),
        "a directory should be made once and removed",
    )
}

/// What takes no writes wherever the program runs: /System/Boot, a way of
/// opening that is none, a handle that is not the program's, and a path
/// longer than a call takes.
fn writes_refused() -> Outcome {
    use abi::file::{CREATE_NEW, READ, WRITE};
    if booted_from_disk() {
        expect(
            open_with("/System/Boot/new.txt", READ | WRITE, CREATE_NEW) == Err(Error::ReadOnly)
                && remove("/System/Boot/meridian/boot.meri") == Err(Error::ReadOnly)
                && make_directory("/System/Boot/New") == Err(Error::ReadOnly),
            "/System/Boot should take no writes",
        )?;
        expect(
            open_with("/System/Boot/meridian/boot.meri", READ, 6) == Err(Error::BadArgument),
            "there is no sixth way to open a file",
        )?;
    }
    let long = alloc::format!("/{}", "a".repeat(abi::PATH_BYTES));
    expect(
        write_file(Handle(0x7777_0042), 0, b"x") == Err(Error::BadHandle)
            && remove(&long) == Err(Error::TooLong)
            && make_directory(&long) == Err(Error::TooLong),
        "a stranger's handle should not write, nor a path past PATH_BYTES name anything",
    )
}

/// A duplicate stands for what its handle stands for, which closes with
/// the last of them: a channel end, an open file and a process.
fn duplicates_stand_for_the_same_object() -> Outcome {
    let (mine, theirs) = pair()?;
    let copy = duplicate(mine).map_err(|_| "a channel end should be duplicated")?;
    let mut room = [0u8; 8];
    expect(
        copy != mine
            && send(copy, b"a", None) == Ok(())
            && send(mine, b"b", None) == Ok(())
            && receive(theirs, &mut room) == Ok((1, None))
            && room[0] == b'a'
            && receive(theirs, &mut room) == Ok((1, None))
            && room[0] == b'b',
        "both handles should send through the one end",
    )?;
    expect(
        close(mine) == Ok(())
            && send(theirs, b"c", None) == Ok(())
            && receive(copy, &mut room) == Ok((1, None))
            && room[0] == b'c',
        "the end should stay open while its duplicate does",
    )?;
    expect(
        close(copy) == Ok(())
            && send(theirs, b"d", None) == Err(Error::Closed)
            && receive(theirs, &mut room) == Err(Error::Closed)
            && close(theirs) == Ok(()),
        "the end should close with its last handle",
    )?;
    let (child, line) = exitwith()?;
    let watcher = duplicate(child).map_err(|_| "a process should be duplicated")?;
    expect(
        send(line, &5u64.to_le_bytes(), None) == Ok(())
            && wait(child) == Ok(Ended::Exited(5))
            && close(child) == Ok(())
            && wait(watcher) == Ok(Ended::Exited(5))
            && close(watcher) == Ok(())
            && close(line) == Ok(()),
        "a duplicate should wait for the process after the first handle closed",
    )?;
    expect(
        duplicate(Handle(0x7777_0042)) == Err(Error::BadHandle),
        "a stranger's handle should not be duplicated",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let file = open("/System/Boot/meridian/boot.meri", abi::file::READ)
        .map_err(|_| "boot.meri should open")?;
    let again = duplicate(file).map_err(|_| "a file should be duplicated")?;
    let size = info(file).map(|i| i.size);
    expect(
        close(file) == Ok(())
            && info(again).map(|i| i.size) == size
            && read(again, 0, &mut room).is_ok_and(|n| n > 0)
            && close(again) == Ok(())
            && info(again) == Err(Error::BadHandle),
        "an open file should read through its duplicate after the first handle closed",
    )
}

/// A file or a directory moves within its volume, and keeps what it holds;
/// what cannot move says why.
fn files_renamed() -> Outcome {
    use abi::file::{CREATE_NEW, READ, WRITE};
    let long = alloc::format!("/{}", "a".repeat(abi::PATH_BYTES));
    expect(
        rename(&long, "/System/x") == Err(Error::TooLong)
            && rename("/System/x", &long) == Err(Error::TooLong)
            && rename("/System/a\0b", "/System/y") == Err(Error::BadArgument),
        "paths past PATH_BYTES or that are none should move nothing",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        rename("/System/Boot/meridian/boot.meri", "/System/boot.meri") == Err(Error::NotSameVolume)
            && rename("/System/Boot/meridian/boot.meri", "/System/Boot/b.meri")
                == Err(Error::ReadOnly),
        "a file should move neither to another volume nor on one only read",
    )?;
    let file = match open_with("/System/r.txt", READ | WRITE, CREATE_NEW) {
        Err(Error::ReadOnly) => {
            return expect(
                rename("/System/made.meri", "/System/m.meri") == Err(Error::ReadOnly),
                "on /System read only nothing should move",
            );
        }
        other => other.map_err(|_| "a file should be made to move")?,
    };
    let written = write_file(file, 0, b"moved");
    expect(
        written == Ok(()) && rename("/System/r.txt", "/System/r2.txt") == Err(Error::InUse),
        "an open file should stay where it is",
    )?;
    let _ = close(file);
    expect(
        make_directory("/System/RDir") == Ok(())
            && rename("/System/r.txt", "/System/RDir/r2.txt") == Ok(())
            && open("/System/r.txt", READ) == Err(Error::NotFound),
        "a closed file should move into a directory",
    )?;
    let moved = open("/System/rdir/R2.TXT", READ).map_err(|_| "the moved file should open")?;
    let mut room = [0u8; 8];
    let len = read(moved, 0, &mut room).unwrap_or(0);
    let _ = close(moved);
    expect(
        room[..len] == *b"moved",
        "the moved file should hold what it held",
    )?;
    expect(
        rename("/System/RDir", "/System/RDir/Inner") == Err(Error::Inside)
            && rename("/System/nope", "/System/x") == Err(Error::NotFound)
            && rename("/System/RDir", "/System/made.meri") == Err(Error::Exists),
        "a directory should not go inside itself, nor anything onto what is there",
    )?;
    expect(
        rename("/System/RDir", "/System/rdir") == Ok(())
            && remove("/System/rdir/r2.txt") == Ok(())
            && remove("/System/rdir") == Ok(()),
        "a directory should take other capitals, and go once empty",
    )
}

/// The working directory changes to a directory that is there and to
/// nothing else, paths start there, and a child starts in its parent's.
fn working_directory() -> Outcome {
    // Whatever went wrong, the checks after start at the root again.
    let checked = directory_moves();
    let back = crate::change_directory("/");
    checked?;
    expect(
        back == Ok(()) && crate::directory() == "/",
        "the program should go back to the root",
    )
}

fn directory_moves() -> Outcome {
    use abi::file::READ;
    expect(
        crate::directory() == "/"
            && crate::change_directory("/nope") == Err(Error::NotFound)
            && crate::change_directory("/") == Ok(())
            && crate::directory() == "/",
        "a program should start at the root, and stay there when told to go nowhere",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        crate::change_directory("/System/Boot") == Ok(())
            && crate::directory() == "/System/Boot"
            && open("meridian/boot.meri", READ).is_ok_and(|h| close(h) == Ok(()))
            && crate::change_directory("meridian/boot.meri") == Err(Error::NotADirectory)
            && crate::directory() == "/System/Boot",
        "paths not from the root should start at the working directory, which takes no file",
    )?;
    let inherited = shell_on("pwd")?;
    expect(
        crate::change_directory("..") == Ok(())
            && crate::directory() == "/System"
            && crate::change_directory("/") == Ok(()),
        "the directory above should be a step away, and the root one more",
    )?;
    expect(
        inherited == ran(0, b"/System/Boot\n"),
        "a child should start in its parent's working directory",
    )
}

/// A child gets the words it is started with as they were, empty ones
/// and ones longer than a message too, and sends them back.
fn arguments_arrive() -> Outcome {
    let long = "\u{e9}".repeat(150);
    let sent = ["one", "", long.as_str(), "two words"];
    let (child, mine) =
        spawn_with("arguments.mexec", &sent).map_err(|_| "arguments should start")?;
    let mut got: Vec<Vec<u8>> = Vec::from([Vec::new()]);
    let mut room = [0u8; MESSAGE_BYTES];
    while got.len() <= sent.len() {
        match receive(mine, &mut room) {
            Ok((0, _)) => got.push(Vec::new()),
            Ok((len, _)) => got
                .last_mut()
                .into_iter()
                .for_each(|a| a.extend_from_slice(&room[..len])),
            Err(Error::Empty) => sleep(1),
            Err(_) => break,
        }
    }
    got.pop();
    let ended = wait(child);
    let closed = close(child) == Ok(()) && close(mine) == Ok(());
    expect(
        ended == Ok(Ended::Exited(0)) && closed && got.iter().eq(sent.iter().map(|a| a.as_bytes())),
        "the child should send back each word it was started with, as it was",
    )
}

/// What a child is given with `sink` as its standard output.
fn output(sink: start::Sink) -> start::Given {
    start::Given {
        output: Some(sink),
        ..start::Given::default()
    }
}

/// Takes every message waiting at `end` once the child that sent them
/// ended, and answers their bytes.
fn drained(end: Handle) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut room = [0u8; MESSAGE_BYTES];
    while let Ok((len, _)) = receive(end, &mut room) {
        bytes.extend_from_slice(&room[..len]);
    }
    bytes
}

/// What a child writes goes to the standard output it was handed: a
/// channel, and a file where /System takes one, at its end.
fn output_goes_where_it_is_handed() -> Outcome {
    let words = ["first", "second line"];
    let want = b"first\nsecond line\n";
    let (mine, theirs) = pair()?;
    let (child, back) = spawn_handing(
        "arguments.mexec",
        &words,
        output(start::Sink::Channel(theirs)),
    )
    .map_err(|_| "arguments should start with an output")?;
    let ended = wait(child);
    let _ = drained(back);
    expect(
        ended == Ok(Ended::Exited(0)) && drained(mine) == want,
        "what the child writes should come through the channel it was handed",
    )?;
    expect(
        close(child) == Ok(()) && close(back) == Ok(()) && close(mine) == Ok(()),
        "the handles should close",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/output.txt";
    let file = match open_with(path, abi::file::WRITE, abi::file::CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "a file should be made for the output")?,
    };
    write_file(file, 0, b"before\n").map_err(|_| "the file should take a line")?;
    let (child, back) = spawn_handing("arguments.mexec", &words, output(start::Sink::File(file)))
        .map_err(|_| "arguments should start with a file")?;
    let ended = wait(child);
    let _ = drained(back);
    let _ = close(child);
    let _ = close(back);
    let reader = open(path, abi::file::READ).map_err(|_| "the output should open")?;
    let mut room = [0u8; 64];
    let len = read(reader, 0, &mut room).unwrap_or(0);
    let _ = close(reader);
    expect(
        ended == Ok(Ended::Exited(0)) && room[..len] == *b"before\nfirst\nsecond line\n",
        "what the child writes should go after what the file held",
    )?;
    expect(remove(path) == Ok(()), "the output should go")
}

/// How the shell ended and what it wrote.
type Ran = (Result<Ended, Error>, Vec<u8>);

/// Starts the shell with `words` and a channel as its standard output,
/// and answers how it ended and what it wrote, which has to fit in the
/// messages a channel holds, since nothing takes them until it ended.
fn shell_with(words: &[&str]) -> Result<Ran, &'static str> {
    shell_reading(words, None)
}

/// Starts the shell with `words` as [`shell_with`] does, and with a
/// standard input that brings `input` and ends, when that is some. What
/// the shell says went wrong comes through the same channel, in its order.
fn shell_reading(words: &[&str], input: Option<&[u8]>) -> Result<Ran, &'static str> {
    let (mine, theirs) = pair()?;
    let mut given = output(start::Sink::Channel(theirs));
    given.error = Some(start::Sink::Channel(
        duplicate(theirs).map_err(|_| "the shell's output should be duplicated")?,
    ));
    let into = match input {
        Some(_) => {
            let (into, from) = pair()?;
            given.input = Some(from);
            Some(into)
        }
        None => None,
    };
    let (child, back) =
        spawn_handing("shell.mexec", words, given).map_err(|_| "the shell should start")?;
    if let (Some(into), Some(input)) = (into, input) {
        for piece in start::pieces(input) {
            let _ = crate::send_waiting(into, piece);
        }
        let _ = close(into);
    }
    let ended = wait(child);
    let said = drained(mine);
    expect(
        close(child) == Ok(()) && close(back) == Ok(()) && close(mine) == Ok(()),
        "the shell's handles should close",
    )?;
    Ok((ended, said))
}

/// The shell run on the script `text`.
fn shell_on(text: &str) -> Result<Ran, &'static str> {
    shell_with(&["-c", text])
}

/// What a shell that ended with `status` after writing `said` answers.
fn ran(status: u64, said: &[u8]) -> Ran {
    (Ok(Ended::Exited(status)), said.to_vec())
}

/// A child reads the standard input it is handed until an empty message
/// comes, or until nothing can write to it any more: wc, which counts it.
fn input_is_read_to_its_end() -> Outcome {
    for empty_at_the_end in [true, false] {
        let (into, from) = pair()?;
        let (mine, theirs) = pair()?;
        let given = start::Given {
            input: Some(from),
            output: Some(start::Sink::Channel(theirs)),
            ..start::Given::default()
        };
        let (child, back) = spawn_handing("wc.mexec", &[], given).map_err(|_| "wc should start")?;
        let sent = send(into, b"one two\n", None) == Ok(()) && send(into, b"three", None) == Ok(());
        let ended = if empty_at_the_end {
            let sent = send(into, b"", None) == Ok(());
            let ended = wait(child);
            sent.then_some(ended)
        } else {
            let closed = close(into) == Ok(());
            closed.then(|| wait(child))
        };
        expect(
            sent && ended == Some(Ok(Ended::Exited(0))) && drained(mine) == b"1 3 13\n",
            "wc should count its standard input to the empty message or to the closed end",
        )?;
        if empty_at_the_end {
            let _ = close(into);
        }
        let _ = (close(child), close(back), close(mine));
    }
    Ok(())
}

/// What a child says went wrong goes to the standard error it was handed,
/// apart from what it writes.
fn errors_go_apart() -> Outcome {
    let (out, theirs) = pair()?;
    let (errors, error_end) = pair()?;
    let given = start::Given {
        output: Some(start::Sink::Channel(theirs)),
        error: Some(start::Sink::Channel(error_end)),
        ..start::Given::default()
    };
    let (child, back) = spawn_handing("wc.mexec", &["/nope", "/nope/either"], given)
        .map_err(|_| "wc should start")?;
    let ended = wait(child);
    let (said, complained) = (drained(out), drained(errors));
    let _ = (close(child), close(back), close(out), close(errors));
    expect(
        ended == Ok(Ended::Exited(1))
            && said == b"0 0 0 total\n"
            && complained == b"wc: /nope: no such file\nwc: /nope/either: no such file\n",
        "a child should write its total to its output and what went wrong to its error",
    )
}

/// A stream's pieces come as sent, an empty one ends it as the other end
/// closing does, a handle one brings is closed, and a piece longer than
/// the room stays.
fn streams_end_and_close_what_they_bring() -> Outcome {
    let (mine, theirs) = pair()?;
    let (kept, brought) = pair()?;
    let mut room = [0u8; MESSAGE_BYTES];
    expect(
        send(theirs, b"one", None) == Ok(())
            && send(theirs, b"two", Some(brought)) == Ok(())
            && send(theirs, b"", None) == Ok(())
            && stream(mine, &mut room) == Ok(3)
            && room[..3] == *b"one"
            && stream(mine, &mut room) == Ok(3)
            && room[..3] == *b"two"
            && stream(mine, &mut room) == Ok(0),
        "pieces should come as sent, and an empty one end the stream",
    )?;
    expect(
        send(kept, b"x", None) == Err(Error::Closed) && close(kept) == Ok(()),
        "a handle a piece brought should be closed",
    )?;
    let mut small = [0u8; 2];
    expect(
        send(theirs, b"long", None) == Ok(())
            && stream(mine, &mut small) == Err(Error::TooSmall)
            && stream(mine, &mut room) == Ok(4),
        "a piece longer than the room should stay for a read with room enough",
    )?;
    expect(
        close(theirs) == Ok(()) && stream(mine, &mut room) == Ok(0) && close(mine) == Ok(()),
        "a stream should end once its other end closed",
    )?;
    expect(
        stream(Handle(0x7777_0042), &mut room) == Err(Error::BadHandle),
        "a stranger's handle should bring no stream",
    )
}

fn shell_runs_a_script() -> Outcome {
    let script = "let a = hi; echo $a there\nif false { echo no } else { echo yes }\n\
                  for x in 1 2 { echo $x }\nfn f { return 3 }\nf; echo $?";
    expect(
        shell_on(script)? == ran(0, b"hi there\nyes\n1\n2\n3\n"),
        "the shell should run the script after -c, its variables, blocks and functions",
    )
}

/// The shell's own commands, each as `help` names it, and what they say
/// when they are missing what they need.
fn the_shells_own_commands() -> Outcome {
    expect(
        shell_on("echo; echo  a   b")? == ran(0, b"\na b\n"),
        "echo should write its words with one space between",
    )?;
    expect(
        shell_on("md; rm")? == ran(1, b"md needs a path\nrm needs a path\n"),
        "md and rm without a path should say they need one",
    )?;
    expect(
        shell_on("services")? == ran(1, b"services: init did not start this shell\n"),
        "services should say init did not start a shell hello started",
    )?;
    expect(
        shell_on("clear | wc")? == ran(0, b"0 0 1\n"),
        "clear should write one form feed",
    )?;
    let listed = shell_on("cd /System/Boot; ls")?;
    let want: &[u8] = if booted_from_disk() {
        b"meridian/\n"
    } else {
        b"/System/Boot: no such file\n"
    };
    expect(
        listed.1.windows(want.len()).any(|w| w == want),
        "ls without a path should list the working directory",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        shell_on("ls /System/Boot/meridian/programs/wh*; dir /System/Boot/meridian/programs/wc*")?
            == ran(
                0,
                b"/System/Boot/meridian/programs/whoami.mexec\n\
                  /System/Boot/meridian/programs/whoami_again.mexec\n\
                  /System/Boot/meridian/programs/wc.mexec\n\
                  /System/Boot/meridian/programs/wc_again.mexec\n",
            ),
        "ls and dir of files should show their paths",
    )?;
    expect(
        shell_on(
            "type /System/Boot/meridian/boot.meri | wc; cat /System/Boot/meridian/boot.meri | wc",
        )?
        .1
        .split(|&b| b == b'\n')
        .take(2)
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[0] == w[1] && !w[0].is_empty()),
        "type should show a file as cat does",
    )
}

fn shell_ends_with_the_status() -> Outcome {
    expect(
        shell_on("echo a; exit 7; echo b")? == ran(7, b"a\n"),
        "exit should end the script with its status",
    )?;
    expect(
        shell_on("true; false")? == ran(1, b""),
        "a script should end with the status of its last pipeline",
    )?;
    expect(
        shell_on("echo a\n}")?
            == ran(
                2,
                b"2:1: unexpected-close: remove the }, or open its block before it\n",
            ),
        "a script that does not read should run nothing and say where it is wrong",
    )?;
    expect(
        shell_on("echo a; echo $nope")?
            == ran(
                2,
                b"a\n1:14: unset-variable: set the variable with let before this\n",
            ),
        "a script should stop at a variable never set",
    )
}

fn shell_starts_programs() -> Outcome {
    expect(
        shell_on("exitwith; echo $?")? == ran(0, b"exitwith ended with 2989\n255\n"),
        "the shell should start a program by its name, from the loader's on a CD, and keep how it ended in $?",
    )?;
    expect(
        shell_on("nope")? == ran(127, b"nope: no such command\n"),
        "a command that is nowhere should end with 127",
    )?;
    expect(
        shell_on("arguments a 'b c'; echo $?")? == ran(0, b"a\nb c\n0\n"),
        "a program the shell starts should write to the shell's own output",
    )?;
    expect(
        shell_on("/System/Boot/meridian/programs")?.0 == Ok(Ended::Exited(126)),
        "a path that is no program should end with 126",
    )
}

fn shell_writes_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/Sh; echo one > /System/Sh/a; echo two >> /System/Sh/a\n\
         arguments x 'y z' >> /System/Sh/a; cat /System/Sh/a; ls /System/Sh/*\n\
         wc /System/Sh/a /System/Sh/a; rm /System/Sh/a /System/Sh",
    )?;
    if said.starts_with(b"/System/Sh: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(2)),
            "on /System read only the script should stop where nothing matches",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"one\ntwo\nx\ny z\n/System/Sh/a\n\
                     4 5 14 /System/Sh/a\n4 5 14 /System/Sh/a\n8 10 28 total\n",
        "the shell should make a directory, write a file and add to it, from a program too, show it, match it, count it and remove both",
    )?;
    expect(
        open("/System/Sh", abi::file::READ) == Err(Error::NotFound),
        "the directory should be gone",
    )
}

fn shell_matches_patterns() -> Outcome {
    let found = if booted_from_disk() {
        ran(
            0,
            b"/System/Boot/meridian/programs/whoami.mexec /System/Boot/meridian/programs/whoami_again.mexec\n",
        )
    } else {
        ran(
            2,
            b"1:6: no-match: no file matches the pattern; quote it to mean the word itself\n",
        )
    };
    expect(
        shell_on("echo /System/Boot/meridian/programs/wh*")? == found,
        "a pattern should become the paths it matches, in order",
    )?;
    expect(
        shell_on("echo '/System/*' /System/Boot/meridian/programs/whoami.me?ec")?
            == if booted_from_disk() {
                ran(
                    0,
                    b"/System/* /System/Boot/meridian/programs/whoami.mexec\n",
                )
            } else {
                ran(
                    2,
                    b"1:18: no-match: no file matches the pattern; quote it to mean the word itself\n",
                )
            },
        "a quoted pattern should stay the word it is",
    )
}

fn shell_runs_a_file() -> Outcome {
    use abi::file::{CREATE_ALWAYS, WRITE};
    expect(
        shell_with(&["/System/none.sh"])? == ran(2, b"/System/none.sh: no such file\n"),
        "a script that is not there should end the shell with 2",
    )?;
    expect(
        shell_with(&["-c"])?
            == ran(
                2,
                b"shell: start it with no words, -c SCRIPT, or a script's path\n",
            ),
        "words the shell does not know should end it with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/script.sh";
    let file = match open_with(path, WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "the script should be made")?,
    };
    let written = write_file(
        file,
        0,
        b"# a script\nfor x in a b {\n  echo $x\n}\nexit 4\n",
    );
    let _ = close(file);
    let ended = shell_with(&[path])?;
    expect(
        written == Ok(()) && remove(path) == Ok(()) && ended == ran(4, b"a\nb\n"),
        "the shell should run the script in a file",
    )
}

fn shell_redirects() -> Outcome {
    expect(
        shell_on("cat < /System/x")? == ran(1, b"/System/x: no such file\n"),
        "a redirection from a file that is not there should say so",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    expect(
        shell_on("echo a > /System/Boot/x")?
            == ran(1, b"/System/Boot/x: on a volume that is only read\n"),
        "a redirection into a file that cannot be written should say why",
    )?;
    let script = "/System/Boot/meridian/boot.meri";
    let (ended, said) = shell_on(&alloc::format!("cat < {script} | wc; wc < {script}"))?;
    let (first, second) = said.split_at(said.len() / 2);
    expect(
        ended == Ok(Ended::Exited(0)) && first == second && first.ends_with(b"\n"),
        "< should hand the shell's own commands and programs the file to read",
    )?;
    let (ended, said) = shell_on(
        "echo a > /System/r1 > /System/r2; cat /System/r1 /System/r2; rm /System/r1 /System/r2",
    )?;
    if said.starts_with(b"/System/r1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only nothing should be written or removed",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"a\n",
        "of two redirections the shell should make both files and write into the last",
    )
}

fn shell_pipes() -> Outcome {
    expect(
        shell_on("echo one two | wc")? == ran(0, b"1 2 8\n"),
        "what one command writes the next should read",
    )?;
    expect(
        shell_on("echo x | cat | cat | wc | cat")? == ran(0, b"1 1 2\n"),
        "the shell's own commands should read and write pipes too",
    )?;
    expect(
        shell_on("arguments a 'b c' | wc; echo a | arguments")? == ran(0, b"2 3 6\n"),
        "a program should write into a pipe, and one that reads nothing should end",
    )?;
    expect(
        shell_on("true | false")? == ran(1, b"")
            && shell_on("false | true")? == ran(0, b"")
            && shell_on("nope | wc")? == ran(0, b"nope: no such command\n0 0 0\n"),
        "a pipeline should end as its last command, and one that never started write nothing",
    )?;
    expect(
        shell_on("help | wc")?
            == ran(
                0,
                alloc::format!("2 {HELP_WORDS} {HELP_BYTES}\n").as_bytes(),
            ),
        "a builtin writing more than a message holds should go whole through the pipe",
    )
}

/// A command of a pipeline that does not start, or reads what is not
/// there, leaves the others to end as they would.
fn pipes_that_go_wrong() -> Outcome {
    expect(
        shell_on("cat /nope | wc")? == ran(0, b"/nope: no such file\n0 0 0\n"),
        "a command that writes nothing into a pipe should leave the next one an empty stream",
    )?;
    expect(
        shell_on("echo a | cat /nope")? == ran(1, b"/nope: no such file\n"),
        "a pipeline should end as its last command when that went wrong",
    )?;
    expect(
        shell_on("echo a | nope")? == ran(127, b"nope: no such command\n"),
        "a program that is nowhere should end a pipeline with 127",
    )?;
    expect(
        shell_on("echo a | /System/Boot/meridian/programs")?.0 == Ok(Ended::Exited(126)),
        "a path that is no program should end a pipeline with 126",
    )?;
    expect(
        shell_on("exitwith | wc")? == ran(0, b"0 0 0\nexitwith ended with 2989\n"),
        "a program that ends without writing should close its end of the pipe",
    )
}

/// Redirections go before pipes: a command that writes into a file
/// writes nothing into its pipe, and one that reads a file reads nothing
/// from its pipe.
fn pipes_and_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo a b > /System/p1 | wc; echo c d | wc < /System/p1\n\
         wc < /System/p1 | cat > /System/p2 | wc; cat /System/p2 /System/p1\n\
         rm /System/p1 /System/p2",
    )?;
    if said.starts_with(b"/System/p1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made for a pipeline",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"0 0 0\n1 2 4\n0 0 0\n1 2 4\na b\n",
        "a redirection should take the place of the pipe on its side",
    )
}

/// Seven of the shell's own commands and a program, each reading what the
/// one before wrote, all at once.
fn a_long_pipeline() -> Outcome {
    expect(
        shell_on("echo x y | cat | cat | cat | cat | cat | cat | wc")? == ran(0, b"1 2 4\n"),
        "a long pipeline should go through every command",
    )?;
    expect(
        shell_on("echo x | wc | wc | wc")? == ran(0, b"1 3 6\n"),
        "programs one after another should each read what the last wrote",
    )
}

/// The shell's own standard input goes to the first command of a
/// pipeline, and to a command alone, a program or one of the shell's own.
fn shell_hands_its_input_on() -> Outcome {
    let input = b"one two\nthree\n";
    expect(
        shell_reading(&["-c", "wc"], Some(input))? == ran(0, b"2 3 14\n"),
        "a program should read the shell's own input",
    )?;
    expect(
        shell_reading(&["-c", "cat | wc"], Some(input))? == ran(0, b"2 3 14\n"),
        "the first command of a pipeline should read it",
    )?;
    expect(
        shell_reading(&["-c", "cat"], Some(input))? == ran(0, input),
        "cat should show what it reads",
    )?;
    expect(
        shell_reading(&["-c", "echo x | wc; wc"], Some(b"a\n"))? == ran(0, b"1 1 2\n1 1 2\n"),
        "a command after a pipeline should still read the shell's own input",
    )?;
    expect(
        shell_reading(&["-c", "wc; wc"], Some(b"a b\n"))? == ran(0, b"1 2 4\n0 0 0\n"),
        "what one command read the next should not read again",
    )
}

/// A command that reads one file and writes another, the last of two
/// files to read winning, as in sh.
fn reading_and_writing_files_at_once() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo one > /System/f1; echo two three > /System/f2\n\
         cat < /System/f1 > /System/f3; wc < /System/f1 < /System/f2 >> /System/f3\n\
         cat /System/f3; rm /System/f1 /System/f2 /System/f3",
    )?;
    if said.starts_with(b"/System/f1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"one\n1 2 10\n",
        "a command should read one file and write another, and read the last it is given",
    )
}

/// Started with a file as its standard output, the shell writes there,
/// and so do the programs it starts, each after what was there.
fn programs_write_into_the_shells_file() -> Outcome {
    use abi::file::{CREATE_ALWAYS, READ, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let path = "/System/shell.out";
    let file = match open_with(path, WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "the shell's output should be made")?,
    };
    let (child, back) = spawn_handing(
        "shell.mexec",
        &[
            "-c",
            "echo first; arguments a 'b c'; echo x y | wc; echo last",
        ],
        output(start::Sink::File(file)),
    )
    .map_err(|_| "the shell should start")?;
    let ended = wait(child);
    let _ = (close(child), close(back));
    let reader = open(path, READ).map_err(|_| "the shell's output should open")?;
    let mut room = [0u8; 64];
    let len = read(reader, 0, &mut room).unwrap_or(0);
    let _ = close(reader);
    expect(
        ended == Ok(Ended::Exited(0))
            && room[..len] == *b"first\na\nb c\n1 2 4\nlast\n"
            && remove(path) == Ok(()),
        "the shell and its programs should write one after another into the shell's file",
    )
}

/// Bytes that are no text go through files and pipes as they are: cat
/// into a file, a file fed to a program, and a pipe between them.
fn bytes_go_through_as_they_are() -> Outcome {
    use abi::file::{CREATE_ALWAYS, READ, WRITE};
    if !booted_from_disk() {
        return Ok(());
    }
    let bytes = [0xff, 0, 0xc3, b'\n', 0x80, b' ', b'x'];
    let file = match open_with("/System/b1", WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "a file should be made for the bytes")?,
    };
    let written = write_file(file, 0, &bytes);
    let _ = close(file);
    let (ended, said) =
        shell_on("cat /System/b1 > /System/b2; cat < /System/b1 | wc; wc < /System/b2")?;
    let copy = open("/System/b2", READ).map_err(|_| "the copy should open")?;
    let mut room = [0u8; 16];
    let len = read(copy, 0, &mut room).unwrap_or(0);
    let _ = close(copy);
    let removed = remove("/System/b1") == Ok(()) && remove("/System/b2") == Ok(());
    expect(
        written == Ok(())
            && ended == Ok(Ended::Exited(0))
            && room[..len] == bytes
            && said == b"1 3 7\n1 3 7\n"
            && removed,
        "cat, a redirection and a pipe should pass on bytes that are no text as they are",
    )
}

/// cp and mv carry files where they are told, into a directory by their
/// names, mv a directory too and a file across volumes, and rd removes
/// only empty directories.
fn files_copied_and_moved() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "md /System/C; echo one > /System/C/a; cp /System/C/a /System/C/b\n\
         copy /System/C/a /System/C/b /System; mv /System/C/b /System/C/c\n\
         ls /System/C/*; cat /System/a /System/b /System/C/c",
    )?;
    if said.starts_with(b"/System/C: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(2)),
            "on /System read only nothing should be copied or moved",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"/System/C/a\n/System/C/c\none\none\none\n",
        "cp should copy a file to a name and into a directory, and mv move one",
    )?;
    expect(
        shell_on("move /System/C /System/D; ls /System/D/*; rd /System/D/a; rmdir /System/D")?
            == ran(
                1,
                b"/System/D/a\n/System/D/c\n/System/D/a: not a directory\n\
                  /System/D: a directory that holds something\n",
            ),
        "move should carry a directory and what it holds, and rd keep what is no empty directory",
    )?;
    expect(
        shell_on(
            "cp /System/a /System/b /System/x.txt; mv /System/Boot/meridian/boot.meri /System/D",
        )? == ran(
            1,
            b"/System/x.txt: not a directory, so one path alone goes there\n\
              /System/Boot/meridian/boot.meri: on a volume that is only read\n",
        ),
        "two paths should go into no file, and a file across volumes be copied though it stays",
    )?;
    expect(
        shell_on(
            "cat /System/D/boot.meri | wc; wc /System/Boot/meridian/boot.meri\n\
             rm /System/a /System/b /System/D/a /System/D/c /System/D/boot.meri; rd /System/D",
        )?
        .1
        .split(|&b| b == b'\n')
        .take(2)
        .map(|line| line.split(|&b| b == b' ').take(3).collect::<Vec<_>>())
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[0] == w[1]),
        "the copy across volumes should hold what the file holds",
    )?;
    expect(
        open("/System/D", abi::file::READ) == Err(Error::NotFound),
        "rd should remove the empty directory",
    )
}

/// cd changes where paths not from the root start, for the shell's own
/// commands, its patterns and the programs it starts.
fn the_working_directory() -> Outcome {
    expect(
        shell_on("pwd; cd; cd /nope; cd a b")?
            == ran(1, b"/\n/\n/nope: no such file\ncd takes one path\n"),
        "the shell should start where its parent is, and stay when cd goes nowhere",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "cd /System/Boot; pwd; chdir meridian; pwd; wc boot.meri; cat boot.meri | wc\n\
         cd ..; pwd; cd meridian/boot.meri",
    )?;
    let lines: Vec<&[u8]> = said.split(|&b| b == b'\n').collect();
    let counts = |line: &[u8]| {
        line.split(|&b| b == b' ')
            .take(3)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
    };
    expect(
        ended == Ok(Ended::Exited(1))
            && lines.len() == 7
            && lines[0] == b"/System/Boot"
            && lines[1] == b"/System/Boot/meridian"
            && lines[2].ends_with(b" boot.meri")
            && counts(lines[2]) == counts(lines[3])
            && lines[4] == b"/System/Boot"
            && lines[5] == b"meridian/boot.meri: not a directory",
        "cd should go into a directory and out, and a program find a file from the shell's",
    )?;
    expect(
        shell_on("cd /System/Boot/meridian/programs; echo wh*; ls wc*")?
            == ran(
                0,
                b"whoami.mexec whoami_again.mexec\nwc.mexec\nwc_again.mexec\n",
            ),
        "patterns not from the root should match in the working directory",
    )
}

/// What a command says went wrong goes to the shell's standard error,
/// and with `2>` and `2>>` into a file instead, for the shell's own
/// commands and its programs alike.
fn errors_go_where_2_sends_them() -> Outcome {
    let (mine, theirs) = pair()?;
    let (errors, error_end) = pair()?;
    let given = start::Given {
        output: Some(start::Sink::Channel(theirs)),
        error: Some(start::Sink::Channel(error_end)),
        ..start::Given::default()
    };
    let (child, back) = spawn_handing(
        "shell.mexec",
        &["-c", "echo a; cat /nope; wc /nope; nope"],
        given,
    )
    .map_err(|_| "the shell should start")?;
    let ended = wait(child);
    let (said, complained) = (drained(mine), drained(errors));
    let _ = (close(child), close(back), close(mine), close(errors));
    expect(
        ended == Ok(Ended::Exited(127))
            && said == b"a\n"
            && complained
                == b"/nope: no such file\nwc: /nope: no such file\nwc ended with 1\nnope: no such command\n",
        "the shell and its programs should say what went wrong on the shell's standard error",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo a 2> /System/e1; cat /nope 2> /System/e1; wc /nope 2>> /System/e1\n\
         cat /System/e1; rm /System/e1",
    )?;
    if said.starts_with(b"/System/e1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should take what went wrong",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"a\nwc ended with 1\n/nope: no such file\nwc: /nope: no such file\n",
        "2> and 2>> should put what went wrong into a file, from a program too",
    )
}

/// `2>&1` sends what goes wrong where the output goes when it comes: into
/// a pipe, into the file a `>` before it named, or where the output went
/// before a `>` after it.
fn errors_go_with_the_output() -> Outcome {
    expect(
        shell_on("cat /nope 2>&1 | wc; wc /nope 2>&1 | wc")?
            == ran(0, b"1 4 20\n1 5 24\nwc ended with 1\n"),
        "2>&1 should send what goes wrong into the pipe",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo hi > /System/o 2>&1; cat /nope >> /System/o 2>&1; wc /nope >> /System/o 2>&1\n\
         cat /System/o; wc /nope 2>&1 > /System/o; cat /System/o; rm /System/o",
    )?;
    if said.starts_with(b"/System/o: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should take the output",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said
                == b"wc ended with 1\nhi\n/nope: no such file\nwc: /nope: no such file\n\
                     wc: /nope: no such file\nwc ended with 1\n",
        "2>&1 should share a file with the output, and keep where the output went before a > after it",
    )?;
    let (ended, said) = shell_on("cat /nope 2>&1 > /System/o | wc; cat /System/o; rm /System/o")?;
    expect(
        ended == Ok(Ended::Exited(0)) && said == b"1 4 20\n",
        "2>&1 before a > should keep the pipe for what goes wrong while the output goes to the file",
    )
}

/// head, grep and sort, each in a pipeline, and what they say when they
/// are given what they cannot take.
fn head_grep_and_sort() -> Outcome {
    expect(
        shell_on(
            "arguments c a b | sort; arguments c a b | sort -r; arguments one two three | head -n 2",
        )? == ran(0, b"a\nb\nc\nc\nb\na\none\ntwo\n"),
        "sort should put lines in order either way, and head take the first",
    )?;
    expect(
        shell_on(
            "arguments apple Banana cherry | grep an; arguments apple Banana | grep -ic A\n\
                  arguments x y | grep -vn x; arguments x | grep z; echo $?",
        )? == ran(0, b"Banana\n2\n2:y\ngrep ended with 1\n1\n"),
        "grep should find the lines that hold its text, count them, take the others, and end with 1 without any",
    )?;
    expect(
        shell_on("head -n x; grep; sort -x; grep a /nope")?
            == ran(
                2,
                b"head: x is no count of lines\nhead ended with 2\n\
                  grep: needs the text to look for\ngrep ended with 2\n\
                  sort: -x is no option\nsort ended with 2\n\
                  grep: /nope: no such file\ngrep ended with 2\n",
            ),
        "head, grep and sort should say what they cannot take and end with 2",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "cd /System/Boot/meridian; head -n 1 boot.meri boot.meri | grep -c ==; head -n 1 boot.meri | wc",
    )?;
    expect(
        ended == Ok(Ended::Exited(0)) && said.starts_with(b"2\n1 "),
        "head should name each of several files, and take one line of one",
    )
}

/// head, grep and sort on files: several at once, named where they say
/// so, a file given with `<`, and one that is not there.
fn tools_on_files() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) =
        shell_on("echo two > /System/t1; echo one > /System/t2; echo two two >> /System/t2")?;
    if said.starts_with(b"/System/t1: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only no file should be made",
        );
    }
    let checked = shell_on("grep -n two /System/t1 /System/t2; sort /System/t1 /System/t2")?
        == ran(
            0,
            b"/System/t1:1:two\n/System/t2:2:two two\none\ntwo\ntwo two\n",
        )
        && shell_on("head -n 1 /System/t1 /System/t2")?
            == ran(0, b"==> /System/t1 <==\ntwo\n\n==> /System/t2 <==\none\n")
        && shell_on("sort -r < /System/t2; grep -c two < /System/t2")?
            == ran(0, b"two two\none\n1\n")
        && shell_on("grep -c x /System/t1 /nope")?
            == ran(
                2,
                b"/System/t1:0\ngrep: /nope: no such file\ngrep ended with 2\n",
            )
        && shell_on("head /nope; sort /System/t1 /nope")?
            == ran(
                2,
                b"head: /nope: no such file\nhead ended with 1\n\
                  sort: /nope: no such file\nsort ended with 2\n",
            );
    let removed = shell_on("rm /System/t1 /System/t2")? == ran(0, b"");
    expect(
        checked && removed,
        "head, grep and sort should read several files, name them, take one given with <, and say which is not there",
    )
}

/// head, grep and sort alone at the shell read what the shell reads.
fn tools_read_the_shells_input() -> Outcome {
    let input = b"pear\napple\nfig\n";
    expect(
        shell_reading(&["-c", "sort"], Some(input))? == ran(0, b"apple\nfig\npear\n")
            && shell_reading(&["-c", "grep -v p"], Some(input))? == ran(0, b"fig\n")
            && shell_reading(&["-c", "head -n 1; head"], Some(input))?
                == ran(0, b"pear\napple\nfig\n"),
        "the tools should read the shell's input, what one left for the next",
    )
}

/// A line longer than a message holds goes through a pipe whole, and a
/// hundred lines come out of sort in order.
fn long_lines_and_many() -> Outcome {
    let long = "x".repeat(300);
    expect(
        shell_on(&alloc::format!(
            "echo {long} | grep x | wc; echo {long}y | grep -c y"
        ))? == ran(0, b"1 1 301\n1\n"),
        "a line of 300 bytes should go through two pipes whole",
    )?;
    // 37 and 100 share no factor, so this is each of 0 to 99 once, out of
    // order.
    let numbers: Vec<String> = (0..100u32)
        .map(|n| alloc::format!("{:03}", (n * 37) % 100))
        .collect();
    let script = alloc::format!("arguments {} | sort", numbers.join(" "));
    let want = (0..100u32).fold(String::new(), |mut want, n| {
        let _ = core::fmt::Write::write_fmt(&mut want, format_args!("{n:03}\n"));
        want
    });
    expect(
        shell_on(&script)? == ran(0, want.as_bytes()),
        "a hundred lines out of order should come out of sort in order",
    )
}

/// The tools given nothing to read write nothing, and grep ends with 1 as
/// it found nothing; head takes no line when told none.
fn tools_given_nothing() -> Outcome {
    expect(
        shell_on("true | sort; true | head; true | wc; true | grep x; echo $?")?
            == ran(0, b"0 0 0\ngrep ended with 1\n1\n"),
        "nothing read should give nothing written, and grep's 1",
    )?;
    expect(
        shell_on("help | head -n 0 | wc; help | head -n 1 | wc")? == ran(0, b"0 0 0\n1 35 158\n"),
        "head should take no line when told none, and one when told one",
    )
}

/// The shell's own commands write into pipes for the tools, and say what
/// went wrong on the shell's standard error meanwhile.
fn the_shells_own_commands_in_pipes() -> Outcome {
    let listed: &[u8] = if booted_from_disk() {
        b"22\n"
    } else {
        b"/System/Boot/meridian/programs: no such file\n0\ngrep ended with 1\n"
    };
    expect(
        shell_on("ls /System/Boot/meridian/programs | grep -c mexec")?
            == ran(u64::from(!booted_from_disk()), listed),
        "ls should write the programs' names into the pipe, eleven twice",
    )?;
    expect(
        shell_on("help | grep -c or; pwd | wc; true | wc")? == ran(0, b"2\n1 1 2\n0 0 0\n"),
        "help, pwd and true should write into a pipe, or not at all",
    )?;
    expect(
        shell_on("cat /nope | cat | wc; services | wc")?
            == ran(
                0,
                b"/nope: no such file\n0 0 0\nservices: init did not start this shell\n0 0 0\n",
            ),
        "the shell's own commands in a pipeline should say what went wrong on the shell's error",
    )
}

/// cp never copies a file onto itself, which would empty it first, even
/// written in other capitals.
fn no_copy_onto_itself() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo kept > /System/s; cp /System/s /System/S; cd /System; copy s ./s\n\
         cat /System/s; rm /System/s",
    )?;
    if said.starts_with(b"/System/s: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only nothing should be copied",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"/System/s: copied onto itself\ns: copied onto itself\nkept\n",
        "cp should refuse to copy a file onto itself and leave what it holds",
    )
}

/// mv to the same name in other capitals renames the file, and grep counts
/// in each of several files by its name.
fn a_move_to_other_capitals() -> Outcome {
    if !booted_from_disk() {
        return Ok(());
    }
    let (ended, said) = shell_on(
        "echo one > /System/m; mv /System/m /System/M; ls /System/M*; cat /System/m\n\
         grep -c o /System/M /System/m /System/M; rm /System/M",
    )?;
    if said.starts_with(b"/System/m: on a volume that is only read\n") {
        return expect(
            ended == Ok(Ended::Exited(1)),
            "on /System read only nothing should be renamed",
        );
    }
    expect(
        ended == Ok(Ended::Exited(0))
            && said == b"/System/M\none\n/System/M:1\n/System/m:1\n/System/M:1\n",
        "mv should rename a file to other capitals, which a path in any capitals then finds",
    )
}

/// The tools say a directory is no file, and pass lines that are no text
/// on as they are.
fn tools_refuse_what_is_no_text() -> Outcome {
    use abi::file::{CREATE_ALWAYS, WRITE};
    let directory: &[u8] = if booted_from_disk() {
        b"grep: /System/Boot: a directory, not a file\ngrep ended with 2\n\
          head: /System/Boot: a directory, not a file\nhead ended with 1\n"
    } else {
        b"grep: /System/Boot: no such file\ngrep ended with 2\n\
          head: /System/Boot: no such file\nhead ended with 1\n"
    };
    expect(
        shell_on("grep x /System/Boot; head /System/Boot")? == ran(1, directory),
        "a directory should be no file to read",
    )?;
    if !booted_from_disk() {
        return Ok(());
    }
    let file = match open_with("/System/raw", WRITE, CREATE_ALWAYS) {
        Err(Error::ReadOnly) => return Ok(()),
        other => other.map_err(|_| "a file should be made for the bytes")?,
    };
    let written = write_file(file, 0, &[0xff, b'x', b'\n', b'y', 0xfe, b'\n']);
    let _ = close(file);
    let ran_it = shell_on("grep x /System/raw; sort -r /System/raw; head -n 1 /System/raw")?;
    let removed = remove("/System/raw") == Ok(());
    expect(
        written == Ok(())
            && removed
            && ran_it
                == ran(
                    0,
                    &[
                        0xff, b'x', b'\n', 0xff, b'x', b'\n', b'y', 0xfe, b'\n', 0xff, b'x', b'\n',
                    ],
                ),
        "grep, sort and head should pass on lines that are no text as they are",
    )
}

/// What `help` shows, counted as wc counts.
const HELP_WORDS: usize = 60;
const HELP_BYTES: usize = 273;

fn writes_on_the_console() -> Outcome {
    // The first piece would end inside the last character. Spaces, since
    // they take the console no time to draw.
    let long = alloc::format!("{}\u{e4}\n", " ".repeat(abi::WRITE_BYTES - 1));
    expect(
        write(&long) == Ok(()),
        "a text longer than one call takes should go in pieces cut between characters",
    )?;
    expect(
        write("hello on the console\n") == Ok(()),
        "a line should go to the console",
    )?;
    let broken = [0xffu8, 0xfe];
    let not_text = abi::Call::Write {
        len: broken.len() as u64,
        at: broken.as_ptr() as u64,
    };
    let too_long = abi::Call::Write {
        len: abi::WRITE_BYTES as u64 + 1,
        at: broken.as_ptr() as u64,
    };
    let kernels = abi::Call::Write {
        len: 1,
        at: abi::USER_END,
    };
    expect(
        abi::answer(raw::call(not_text).0) == Err(Error::BadArgument)
            && abi::answer(raw::call(too_long).0) == Err(Error::TooLong)
            && abi::answer(raw::call(kernels).0) == Err(Error::BadAddress),
        "bytes that are no UTF-8, more than WRITE_BYTES and the kernel's half should not be written",
    )?;
    expect(
        key().is_none(),
        "no key should wait before anyone pressed one",
    )
}

fn wait_for_no_process() -> Outcome {
    let (a, b) = pair()?;
    expect(
        wait(a) == Err(Error::Denied),
        "a channel end is no process to wait for",
    )?;
    expect(
        wait(Handle(0x7777_0042)) == Err(Error::BadHandle),
        "nor is a made up handle",
    )?;
    expect(
        close(a) == Ok(()) && close(b) == Ok(()),
        "both ends should close",
    )
}

fn not_a_handle() -> Outcome {
    let mut room = [0u8; 4];
    let stranger = Handle(0x7777_0042);
    expect(
        send(stranger, b"x", None) == Err(Error::BadHandle)
            && receive(stranger, &mut room) == Err(Error::BadHandle)
            && close(stranger) == Err(Error::BadHandle),
        "a handle the program never got should reach nothing",
    )
}
