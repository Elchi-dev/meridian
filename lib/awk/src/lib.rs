//! POSIX's awk (decision record 0026): a program read into its tokens
//! and its tree, its values and how one becomes another, printf's
//! formats filled with values, numbers and strings written as C's printf
//! writes them, and the functions of a double awk has.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod format;
pub mod lex;
pub mod math;
pub mod parse;
pub mod printf;
pub mod value;
