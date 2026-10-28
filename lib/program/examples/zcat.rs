//! zcat: writes each file compress wrote that it is given, named with
//! `.Z` or without, back as it was onto its standard output, or with none
//! its standard input, as POSIX's zcat does, with lib/lzw: uncompress -c.
//! The files are kept. It ends with 0, and with 1 when a file went wrong.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use program::Handle;
use program::squeeze::{self, System};

program::main!(zcat);

#[cfg_attr(test, allow(dead_code))]
fn zcat(_: Handle) -> u64 {
    let mut asked = match squeeze::asked("zcat", program::arguments(), "") {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    asked.to_output = true;
    let mut system = System::default();
    let went = squeeze::each(asked.files, |path| {
        squeeze::uncompress(&mut system, "zcat", path, &asked)
    });
    if system.finish() {
        squeeze::status(went)
    } else {
        1
    }
}
