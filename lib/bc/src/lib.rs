//! POSIX's bc (decision record 0027): whole numbers of any size
//! ([`big`]) and the decimal numbers with a scale bc works out with them
//! ([`number`]).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod big;
pub mod number;
