//! A program for the test image: it sends its own security token, as
//! TOKEN gives it, through the channel end it is handed, and exits with 0,
//! or with 1 when it could not.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use program::Handle;
use program::abi::MESSAGE_BYTES;

program::main!(whoami);

#[cfg_attr(test, allow(dead_code))]
fn whoami(end: Handle) -> u64 {
    let mut message = [0u8; MESSAGE_BYTES];
    let sent = program::token()
        .and_then(|token| token.write(&mut message))
        .is_some_and(|len| program::send(end, &message[..len], None).is_ok());
    u64::from(!sent)
}
