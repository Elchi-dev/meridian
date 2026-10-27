//! The sh dialect, POSIX sh as the Shell Command Language of IEEE Std
//! 1003.1 describes it: a script read into its tokens ([`lex`]), and those
//! into the commands they stand for ([`parse`]), and the patterns words are
//! matched against ([`pattern`]).

pub mod arith;
pub mod lex;
pub mod parse;
pub mod pattern;
