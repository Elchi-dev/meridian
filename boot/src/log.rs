//! What the loader has said so far, kept without a heap so the boot screen
//! can show it all when V is pressed.

use core::fmt::{self, Write};

/// The longest line kept, in bytes. A longer line goes on in the next one.
pub const LINE: usize = 112;
/// The most lines kept. Once full, the oldest line makes room.
pub const LINES: usize = 64;

pub struct Log {
    text: [[u8; LINE]; LINES],
    lens: [usize; LINES],
    /// Where the oldest line is.
    first: usize,
    /// Lines in use, counting the one being written, which is the last.
    count: usize,
}

impl Log {
    #[must_use]
    pub const fn new() -> Log {
        Log {
            text: [[0; LINE]; LINES],
            lens: [0; LINES],
            first: 0,
            count: 1,
        }
    }

    fn current(&self) -> usize {
        (self.first + self.count - 1) % LINES
    }

    fn next_line(&mut self) {
        if self.count == LINES {
            self.first = (self.first + 1) % LINES;
        } else {
            self.count += 1;
        }
        let at = self.current();
        self.lens[at] = 0;
    }

    fn push(&mut self, c: char) {
        match c {
            '\n' => return self.next_line(),
            '\r' => return,
            _ => {}
        }
        let mut buf = [0u8; 4];
        let bytes = c.encode_utf8(&mut buf).as_bytes();
        if self.lens[self.current()] + bytes.len() > LINE {
            self.next_line();
        }
        let at = self.current();
        let len = self.lens[at];
        self.text[at][len..len + bytes.len()].copy_from_slice(bytes);
        self.lens[at] = len + bytes.len();
    }

    /// The lines kept, oldest first. A line still being written counts
    /// once it has text.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        let last = self.current();
        (0..self.count)
            .map(move |i| (self.first + i) % LINES)
            .filter(move |&at| at != last || self.lens[at] > 0)
            .map(|at| core::str::from_utf8(&self.text[at][..self.lens[at]]).unwrap_or(""))
    }

    /// A writer that puts everything into the log and into `other`.
    pub fn tee<'a, W: Write>(&'a mut self, other: &'a mut W) -> Tee<'a, W> {
        Tee { log: self, other }
    }
}

impl Default for Log {
    fn default() -> Self {
        Log::new()
    }
}

impl Write for Log {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        s.chars().for_each(|c| self.push(c));
        Ok(())
    }
}

/// Writes to a [`Log`] and to another writer, and fails when the other
/// writer does. The log takes everything either way.
pub struct Tee<'a, W: Write> {
    log: &'a mut Log,
    other: &'a mut W,
}

impl<W: Write> Write for Tee<'_, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let _ = self.log.write_str(s);
        self.other.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(log: &Log) -> Vec<String> {
        log.lines().map(String::from).collect()
    }

    #[test]
    fn lines_come_back_in_order() {
        let mut log = Log::new();
        assert_eq!(lines(&log), [""; 0]);
        write!(log, "Meridian boot 0.0.0\nFirmware: EDK II").unwrap();
        assert_eq!(lines(&log), ["Meridian boot 0.0.0", "Firmware: EDK II"]);
        writeln!(log, " (revision 0x10000)").unwrap();
        assert_eq!(
            lines(&log),
            ["Meridian boot 0.0.0", "Firmware: EDK II (revision 0x10000)"],
            "a finished line leaves no empty one behind"
        );
        writeln!(log).unwrap();
        writeln!(log, "after a gap").unwrap();
        assert_eq!(lines(&log)[2..], ["", "after a gap"]);
    }

    #[test]
    fn carriage_returns_are_dropped() {
        let mut log = Log::new();
        log.write_str("one\r\ntwo\r\n").unwrap();
        assert_eq!(lines(&log), ["one", "two"]);
    }

    #[test]
    fn long_lines_go_on_in_the_next() {
        let mut log = Log::new();
        let text = "x".repeat(LINE) + "yz";
        log.write_str(&text).unwrap();
        assert_eq!(lines(&log), ["x".repeat(LINE), "yz".to_string()]);
    }

    #[test]
    fn a_character_is_never_split() {
        let mut log = Log::new();
        let text = "a".repeat(LINE - 1) + "é";
        log.write_str(&text).unwrap();
        assert_eq!(lines(&log), ["a".repeat(LINE - 1), "é".to_string()]);
        let mut exact = Log::new();
        exact.write_str(&("a".repeat(LINE - 2) + "é")).unwrap();
        assert_eq!(lines(&exact).len(), 1, "two bytes fit in the last two");
    }

    #[test]
    fn the_oldest_lines_make_room() {
        let mut log = Log::new();
        for i in 0..LINES + 5 {
            writeln!(log, "line {i}").unwrap();
        }
        let kept = lines(&log);
        assert_eq!(
            kept.len(),
            LINES - 1,
            "the last slot holds the line being written"
        );
        assert_eq!(kept[0], "line 6");
        assert_eq!(kept[LINES - 2], format!("line {}", LINES + 4));
        write!(log, "partial").unwrap();
        let kept = lines(&log);
        assert_eq!(kept.len(), LINES);
        assert_eq!(kept.last().map(String::as_str), Some("partial"));
        writeln!(log).unwrap();
        assert_eq!(lines(&log)[0], "line 7");
    }

    struct Broken;

    impl Write for Broken {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[test]
    fn a_tee_writes_both_and_passes_on_failure() {
        let mut log = Log::new();
        let mut serial = String::new();
        writeln!(log.tee(&mut serial), "both").unwrap();
        assert_eq!(serial, "both\n");
        assert_eq!(lines(&log), ["both"]);
        assert!(write!(log.tee(&mut Broken), "kept anyway").is_err());
        assert_eq!(lines(&log), ["both", "kept anyway"]);
    }

    #[test]
    fn the_log_starts_empty_however_it_is_made() {
        static EARLY: Log = Log::new();
        assert_eq!(Log::default().lines().count(), 0);
        assert_eq!(EARLY.lines().count(), 0);
    }
}
