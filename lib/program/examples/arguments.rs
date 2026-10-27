//! A program for the test image: it sends its arguments back over the
//! channel it started with, each in the messages a stream takes and an
//! empty message after it, so the test image sees what a program is
//! handed (decision record 0018).

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use program::Handle;
use program::start::pieces;

program::main!(arguments);

#[cfg_attr(test, allow(dead_code))]
fn arguments(back: Handle) -> u64 {
    for argument in program::arguments() {
        for piece in pieces(argument.as_bytes()).chain([&[][..]]) {
            if program::send_waiting(back, piece).is_err() {
                return 1;
            }
        }
    }
    0
}
