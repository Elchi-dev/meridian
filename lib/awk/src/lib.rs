//! POSIX's awk (decision record 0026): its values and how one becomes
//! another, numbers and strings written as C's printf writes them, and
//! the functions of a double awk has.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod format;
pub mod math;
pub mod value;
