//! The kernel's serial log, a whole line at a time: lines from different
//! cores and threads take turns at COM1 instead of running into each
//! other. A crash writes to COM1 directly, so it never waits for a line
//! another core may never finish.

use core::fmt::Write;

use kernel::log::Lines;
use x86::raw::Com1;

use super::memory::Spin;

/// Held while a line goes out.
static PORT: Spin<()> = Spin::new(());

/// The room a line gets before it goes out in pieces: a program's longest
/// line with the kernel's words around it.
const ROOM: usize = 512;

/// COM1, written a line at a time.
pub type Serial = Lines<ROOM, fn(&str)>;

fn send(line: &str) {
    let _held = PORT.lock();
    let _ = Com1::already_open().write_str(line);
}

/// Sets COM1 up and returns it, written a line at a time.
pub fn open() -> Serial {
    let _ = Com1::open();
    Lines::new(send)
}

/// COM1 as it was set up already, written a line at a time.
pub fn already_open() -> Serial {
    Lines::new(send)
}
