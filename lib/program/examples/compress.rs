//! compress: writes each file it is given compressed with LZW into the
//! same name with `.Z`, the file going once that is whole, or with none
//! its standard input onto its standard output, as POSIX's compress does,
//! with lib/lzw. `-c` writes onto the standard output and keeps the
//! files, `-f` goes over a file there and compresses one that would grow,
//! `-v` says how much smaller each became, and `-b` gives the most bits
//! a code has. A file it cannot make smaller is kept. It ends with 0, with
//! 2 when it kept a file, and with 1 when one went wrong.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use program::Handle;
use program::squeeze::{self, System};

program::main!(compress);

#[cfg_attr(test, allow(dead_code))]
fn compress(_: Handle) -> u64 {
    let asked = match squeeze::asked("compress", program::arguments(), "cfvb") {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut system = System::default();
    let went = squeeze::each(asked.files, |path| {
        squeeze::compress(&mut system, path, &asked)
    });
    if system.finish() {
        squeeze::status(went)
    } else {
        1
    }
}
