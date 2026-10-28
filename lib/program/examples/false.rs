//! false: the shell's false as a program of its own, run on the streams
//! it was handed (decision record 0024).

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use program::Handle;

program::main!(command);

#[cfg_attr(test, allow(dead_code))]
fn command(_: Handle) -> u64 {
    program::commands::run("false")
}
