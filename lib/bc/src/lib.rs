//! POSIX's bc (decision record 0027): whole numbers of any size
//! ([`big`]), the decimal numbers with a scale bc works out with them
//! ([`number`]), how they are read and written in other bases
//! ([`base`]), and programs read into their tokens ([`lex`]) and items
//! ([`parse`]) and run as they are read ([`run`]).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod base;
pub mod big;
pub mod lex;
pub mod number;
pub mod parse;
pub mod run;
