//! uncompress: writes each file compress wrote that it is given back as
//! it was, the name without `.Z` (given with or without it), the file
//! going once that is whole, or with none its standard input onto its
//! standard output, as POSIX's uncompress does, with lib/lzw. `-c` writes
//! onto the standard output and keeps the files, `-f` goes over a file
//! there, and `-v` says each file done. It ends with 0, and with 1 when a
//! file went wrong.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use program::Handle;
use program::squeeze::{self, System};

program::main!(uncompress);

#[cfg_attr(test, allow(dead_code))]
fn uncompress(_: Handle) -> u64 {
    let asked = match squeeze::asked("uncompress", program::arguments(), "cfv") {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut system = System::default();
    let went = squeeze::each(asked.files, |path| {
        squeeze::uncompress(&mut system, "uncompress", path, &asked)
    });
    if system.finish() {
        squeeze::status(went)
    } else {
        1
    }
}
