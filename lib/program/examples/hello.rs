//! The first program: it greets from ring 3 and ends. Built for the test
//! image, it checks every system call first and ends with the number of
//! checks that failed.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

use program::Handle;

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
