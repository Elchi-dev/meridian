//! sed's scripts, as POSIX has them: read into their commands
//! ([`script`]) and run on lines ([`run`]).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod run;
pub mod script;
