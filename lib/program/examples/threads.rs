//! A program for the test image, for its checks of threads: it takes a
//! word from the channel end it is handed and does as it says. With `sum`
//! four threads each send their number back and it exits with 0 once all
//! did; with `last` its first thread ends with 3 while another sleeps and
//! then ends with 7, which the program ends with; with `exit` another
//! thread spins while the first exits with 9, which ends both; with
//! `fault` another thread runs out of stack while the first sleeps, and
//! the fault ends both.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use core::sync::atomic::{AtomicU64, Ordering};

use program::{Error, Handle};

program::main!(threads);

/// The channel end the program was handed, for the threads that send.
static END: AtomicU64 = AtomicU64::new(0);

/// How many of the threads that send did.
static SENT: AtomicU64 = AtomicU64::new(0);

#[cfg_attr(test, allow(dead_code))]
fn threads(end: Handle) -> u64 {
    END.store(end.0, Ordering::Release);
    let mut room = [0u8; 8];
    let word = loop {
        match program::receive(end, &mut room) {
            Ok((len, _)) => break &room[..len],
            Err(Error::Empty) => program::sleep(1),
            Err(_) => return 0xbad,
        }
    };
    match word {
        b"sum" => {
            let started = (1..=4).all(|n| program::thread(sender, n).is_ok());
            while started && SENT.load(Ordering::Acquire) < 4 {
                program::sleep(1);
            }
            u64::from(!started)
        }
        b"last" => match program::thread(sleeper, 0) {
            Ok(()) => program::end_thread(3),
            Err(_) => 0xbad,
        },
        b"exit" => {
            if program::thread(spinner, 0).is_err() {
                return 0xbad;
            }
            program::sleep(2);
            9
        }
        b"fault" => {
            if program::thread(deeper, 0).is_err() {
                return 0xbad;
            }
            loop {
                program::sleep(1);
            }
        }
        _ => 0xbad,
    }
}

#[cfg_attr(test, allow(dead_code))]
fn sender(n: u64) -> u64 {
    let end = Handle(END.load(Ordering::Acquire));
    let sent = program::send(end, &n.to_le_bytes(), None).is_ok();
    SENT.fetch_add(1, Ordering::AcqRel);
    u64::from(!sent)
}

#[cfg_attr(test, allow(dead_code))]
fn sleeper(_: u64) -> u64 {
    program::sleep(5);
    7
}

#[cfg_attr(test, allow(dead_code))]
fn spinner(_: u64) -> u64 {
    loop {
        core::hint::spin_loop();
    }
}

/// Calls itself until the stack runs out, into the page left unmapped
/// below it, long before the depth could reach its end.
#[cfg_attr(test, allow(dead_code))]
fn deeper(depth: u64) -> u64 {
    let frame = core::hint::black_box([depth; 64]);
    if core::hint::black_box(depth) == u64::MAX {
        return frame[0];
    }
    deeper(depth + 1) + frame[7]
}
