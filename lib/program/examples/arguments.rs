//! A program for the test image: it sends its arguments back over the
//! channel it started with, unless its parent closed that, each in the
//! messages a stream takes and an empty message after it, so the test
//! image sees what a program is handed (decision record 0018), and writes
//! them, a line each, to its standard output (decision record 0017).

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use program::start::pieces;
use program::{Error, Handle};

program::main!(arguments);

#[cfg_attr(test, allow(dead_code))]
fn arguments(back: Handle) -> u64 {
    let sent = program::arguments().iter().try_for_each(|argument| {
        pieces(argument.as_bytes())
            .chain([&[][..]])
            .try_for_each(|piece| program::send_waiting(back, piece))
    });
    // A parent that closed its end, as the shell does, takes none back.
    if !matches!(sent, Ok(()) | Err(Error::Closed)) {
        return 1;
    }
    for argument in program::arguments() {
        if program::write(&alloc::format!("{argument}\n")).is_err() {
            return 2;
        }
    }
    0
}
