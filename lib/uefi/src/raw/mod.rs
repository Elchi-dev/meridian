//! Firmware tables and protocols exactly as the specification lays them out,
//! and the safe calls built on top of them.

pub mod console;
pub mod file;
pub mod system;
pub mod table;

pub use file::File;
pub use system::{Console, Pages, System};

pub type Handle = *mut core::ffi::c_void;
pub type Event = *mut core::ffi::c_void;
