//! A program for the test image: it takes eight bytes from the channel end
//! it is handed and exits with the number they hold. A message that
//! carries a process's handle makes it wait for that process and exit with
//! its code, an empty one makes it spin without calling the kernel again,
//! until it is ended, and any other runs it out of stack, so it ends by a
//! page fault instead.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use program::{Ended, Error, Handle};

program::main!(exitwith);

#[cfg_attr(test, allow(dead_code))]
fn exitwith(end: Handle) -> u64 {
    let mut room = [0u8; 8];
    loop {
        match program::receive(end, &mut room) {
            Ok((_, Some(process))) => {
                return match program::wait(process) {
                    Ok(Ended::Exited(code)) => code,
                    _ => 0xbad,
                };
            }
            Ok((8, _)) => return u64::from_le_bytes(room),
            Ok((0, _)) => loop {
                core::hint::spin_loop();
            },
            Ok(_) => return deeper(0),
            Err(Error::Empty) => program::sleep(1),
            Err(_) => return 0xbad,
        }
    }
}

/// Calls itself until the stack runs out, long before the depth could
/// reach its end.
#[cfg_attr(test, allow(dead_code))]
fn deeper(depth: u64) -> u64 {
    let frame = core::hint::black_box([depth; 64]);
    if core::hint::black_box(depth) == u64::MAX {
        return frame[0];
    }
    deeper(depth + 1) + frame[7]
}
