//! The commands the shell shares with its sh dialect, run by a program of
//! their own (decision record 0024): on the streams the program was
//! handed, and with what the program may do with files.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use shell::commands::{Allowed, File, Kind, Streams};

use crate::abi::file;

/// Whether the file at `path` opens with the right `wanted`.
fn may(path: &str, wanted: u32) -> bool {
    crate::open(path, wanted).is_ok_and(|handle| {
        let _ = crate::close(handle);
        true
    })
}

/// What is at `path`, as `test` asks: a directory may always be gone
/// into, and a file run when it is a program's image and the right to run
/// it is there. None when nothing is.
#[must_use]
pub fn file(path: &str) -> Option<File> {
    let handle = crate::open(path, 0).ok()?;
    let info = crate::info(handle);
    let _ = crate::close(handle);
    let info = info.ok()?;
    let program = path
        .rsplit_once('.')
        .is_some_and(|(_, end)| end.eq_ignore_ascii_case("mexec"));
    Some(File {
        kind: if info.directory {
            Kind::Directory
        } else {
            Kind::File
        },
        size: info.size,
        allowed: Allowed {
            read: may(path, file::READ),
            write: may(path, file::WRITE),
            run: info.directory || program && may(path, file::RUN),
        },
    })
}

/// A program's own streams and files, as a shared command reaches them.
pub struct Own;

impl Streams for Own {
    fn out(&mut self, bytes: &[u8]) -> bool {
        crate::write_bytes(bytes).is_ok()
    }

    fn err(&mut self, bytes: &[u8]) -> bool {
        crate::write_error(&String::from_utf8_lossy(bytes)).is_ok()
    }

    fn file(&mut self, path: &str) -> Option<File> {
        file(path)
    }

    /// A stream is a terminal when the program was handed none for it,
    /// and so has the console's.
    fn terminal(&mut self, stream: u32) -> bool {
        let given = crate::start::given();
        match stream {
            0 => given.input.is_none(),
            1 => given.output.is_none(),
            2 => given.error.is_none(),
            _ => false,
        }
    }
}

/// Runs the shared command `name` on the program's arguments, its own
/// streams and its files, and answers its status.
#[must_use]
pub fn run(name: &str) -> u64 {
    let mut words = Vec::from([name.to_string()]);
    words.extend(crate::arguments().iter().cloned());
    u64::from(shell::commands::run(&words, &mut Own).unwrap_or(127))
}
