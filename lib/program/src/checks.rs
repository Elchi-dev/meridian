//! Checks of every call from ring 3, for the QEMU test image, where the
//! test build of hello runs them. Each logs "ok" or "FAIL" with its name,
//! and hello ends with the number that failed, which the kernel's check
//! wants to be 0.

use abi::LOG_BYTES;

use crate::{Error, Handle, channel, close, log, receive, send};

type Outcome = Result<(), &'static str>;

type Check = fn() -> Outcome;

const CHECKS: [(&str, Check); 11] = [
    ("log", lines),
    ("log.format", formatted),
    ("ticks", time_goes_on),
    ("channel", round_trip),
    ("channel.empty", nothing_waits),
    ("channel.room", room_too_small),
    ("channel.carry", handle_carried),
    ("channel.closed", other_end_closed),
    ("channel.full", full),
    ("close", closed_twice),
    ("handle", not_a_handle),
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
