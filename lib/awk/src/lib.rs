//! POSIX's awk (decision record 0026): a program read into its tokens
//! ([`lex`]) and its tree ([`parse`]) and run over the records of its
//! input ([`run`]), with its values and how one becomes another
//! ([`value`]), the input's records and fields ([`record`]), printf's
//! formats filled with values ([`printf`]), numbers and strings written
//! as C's printf writes them ([`format`]), and the functions of a double
//! awk has ([`math`]).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod format;
pub mod lex;
pub mod math;
pub mod parse;
pub mod printf;
pub mod record;
pub mod run;
pub mod value;
