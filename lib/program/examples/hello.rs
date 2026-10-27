//! The first program: it greets from ring 3 and ends. Built for the test
//! image, it checks every system call first and ends with the number of
//! checks that failed.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use program::Handle;

// Running the checks, one of which goes past the 64 KiB of stack every
// program gets, it asks for more.
#[cfg(feature = "qemu-test")]
program::main!(hello, stack = 32);
#[cfg(not(feature = "qemu-test"))]
program::main!(hello);

#[cfg_attr(test, allow(dead_code))]
fn hello(_: Handle) -> u64 {
    #[cfg(feature = "qemu-test")]
    let failed = program::checks::run();
    #[cfg(not(feature = "qemu-test"))]
    let failed = 0;
    let _ = program::log("Hello from ring 3, in Rust");
    failed
}
