//! UEFI types and firmware calls, written from the UEFI 2.10 specification.
//!
//! Plain data such as status codes, GUIDs and memory descriptors lives in the
//! safe modules. Everything that touches firmware tables directly is in
//! [`raw`], which wraps it behind safe functions.

#![cfg_attr(not(test), no_std)]

pub mod device_path;
pub mod guid;
pub mod load_option;
pub mod memory;
#[cfg(feature = "qemu-test")]
pub mod qemu_test;
pub mod status;
pub mod text;
pub mod variable;

#[allow(unsafe_code)]
pub mod raw;

pub use guid::Guid;
pub use status::Status;
