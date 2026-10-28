//! What differs between two texts, as POSIX's diff finds it: the fewest
//! lines to leave out of the first and put into it to make the second,
//! found by Myers' algorithm in as much room as the texts take, and those
//! differences written in the forms diff writes them in.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod compare;
pub mod write;

pub use compare::{Hunk, common, hunks};
pub use write::{Form, Text, keys, write};
