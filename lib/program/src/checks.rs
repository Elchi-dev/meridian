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
    Ended, Error, Handle, Info, channel, close, info, key, list, log, map, open, raw, read,
    receive, send, spawn, spawn_restricted, token, unmap, wait, write,
};

type Outcome = Result<(), &'static str>;

type Check = fn() -> Outcome;

const CHECKS: [(&str, Check); 26] = [
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
    ("wait.other", wait_for_no_process),
    ("token", own_token),
    ("spawn.token", child_gets_no_more),
    ("file", settings_read_back),
    ("file.refused", files_that_do_not_open),
    ("file.list", directories_list_their_entries),
    ("spawn.file", child_from_its_file),
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

/// Starts exitwith with one end of a fresh channel, and returns the
/// process and the other end.
fn exitwith() -> Result<(Handle, Handle), &'static str> {
    let (mine, theirs) = pair()?;
    let child = spawn("exitwith.mexec", Some(theirs)).map_err(|_| "exitwith should start")?;
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
    expect(
        open("meridian/boot.meri", abi::file::READ) == Err(Error::BadArgument)
            && open("/System/../..", abi::file::READ) == Err(Error::BadArgument),
        "a path not from the root, or above it, is none",
    )?;
    let long = [b'/'; abi::PATH_BYTES + 1];
    let too_long = abi::Call::Open {
        len: long.len() as u64,
        at: long.as_ptr() as u64,
        wanted: u64::from(abi::file::READ),
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
        open("/System/Boot/meridian/boot.meri", abi::file::WRITE) == Err(Error::Denied),
        "nothing writes to a volume mounted read only",
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
