//! POSIX's bc (decision record 0027): whole numbers of any size
//! ([`big`]), the decimal numbers with a scale bc works out with them
//! ([`number`]), and how they are read and written in other bases
//! ([`base`]).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod base;
pub mod big;
pub mod number;
