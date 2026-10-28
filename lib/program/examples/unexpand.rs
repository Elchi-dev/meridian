//! unexpand: writes its files, or its standard input, with the blanks at
//! the start of each line turned into as many tabs as reach a tab stop and
//! the fewest spaces after them, as POSIX's unexpand does; with `-a` also
//! every other run of two blanks or more. Tab stops are every eight
//! columns, or as `-t` says, which also means `-a`: one number for every so
//! many columns, or a list of the columns, past whose last nothing is
//! turned. A backspace takes a column back, a line feed starts again at
//! the first, and a character of more than one byte takes one column. `-`
//! is the standard input.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::tabs::Tabs;
use program::text;

program::main!(unexpand);

/// What unexpand's words ask for: the tab stops, whether every run of
/// blanks is turned or only those a line starts with, and the files.
fn asked(arguments: &[String]) -> Result<(Tabs, bool, &[String]), String> {
    let mut tabs = Tabs::default();
    let mut all = false;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        words = rest;
        for (at, letter) in letters.char_indices() {
            match letter {
                'a' => all = true,
                't' => {
                    let after = &letters[at + 1..];
                    let value = if after.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| String::from("unexpand: -t wants its word\n"))?;
                        words = rest;
                        value.as_str()
                    } else {
                        after
                    };
                    tabs = Tabs::parse("unexpand", value)?;
                    all = true;
                    break;
                }
                _ => return Err(format!("unexpand: -{letter} is no option\n")),
            }
        }
    }
    Ok((tabs, all, words))
}

/// Turns runs of blanks into tabs as the text comes, keeping the column
/// it is at and the run it is in.
struct Unexpander {
    tabs: Tabs,
    all: bool,
    column: u64,
    /// Whether nothing but blanks came since the line started.
    leading: bool,
    /// The blanks of the run so far, each with the column it starts at.
    run: Vec<(u8, u64)>,
}

impl Unexpander {
    fn new(tabs: Tabs, all: bool) -> Unexpander {
        Unexpander {
            tabs,
            all,
            column: 0,
            leading: true,
            run: Vec::new(),
        }
    }

    /// Gives `put` what `bytes`, the next of the text, become, holding
    /// back a run of blanks until something else ends it.
    fn feed(&mut self, bytes: &[u8], put: &mut impl FnMut(&[u8])) {
        let mut from = 0;
        for (at, &byte) in bytes.iter().enumerate() {
            if matches!(byte, b' ' | b'\t') {
                put(&bytes[from..at]);
                from = at + 1;
                self.run.push((byte, self.column));
                self.column = match byte {
                    b'\t' => self
                        .tabs
                        .after(self.column)
                        .unwrap_or(self.column.saturating_add(1)),
                    _ => self.column.saturating_add(1),
                };
                continue;
            }
            if !self.run.is_empty() {
                put(&bytes[from..at]);
                from = at;
                self.end_run(put);
            }
            match byte {
                8 => self.column = self.column.saturating_sub(1),
                b'\n' => self.column = 0,
                _ if byte & 0xc0 == 0x80 => {}
                _ => self.column = self.column.saturating_add(1),
            }
            self.leading = byte == b'\n';
        }
        put(&bytes[from..]);
    }

    /// Writes the run of blanks held back, when a line starts with it or
    /// every run of two blanks or more is turned, as tabs up to the last
    /// stop it reaches, and the blanks after that stop as they came; else
    /// all of it as it came. No blank starts before a stop and ends past
    /// it, since a tab ends at the next stop.
    fn end_run(&mut self, put: &mut impl FnMut(&[u8])) {
        let mut at = self.run.first().map_or(self.column, |&(_, start)| start);
        if self.leading || self.all && self.run.len() >= 2 {
            while let Some(stop) = self.tabs.after(at).filter(|&stop| stop <= self.column) {
                put(b"\t");
                at = stop;
            }
        }
        let kept: Vec<u8> = self
            .run
            .iter()
            .filter(|&&(_, start)| start >= at)
            .map(|&(blank, _)| blank)
            .collect();
        put(&kept);
        self.run.clear();
    }

    /// Writes a run of blanks the text ended in.
    fn finish(&mut self, put: &mut impl FnMut(&[u8])) {
        if !self.run.is_empty() {
            self.end_run(put);
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn unexpand(_: Handle) -> u64 {
    let (tabs, all, files) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut unexpander = Unexpander::new(tabs, all);
    let mut out = text::standard_output();
    let whole = text::each_piece("unexpand", files, |piece| {
        unexpander.feed(piece, &mut |bytes| out.put(bytes));
    });
    unexpander.finish(&mut |bytes| out.put(bytes));
    u64::from(!out.finish() || !whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// What unexpand asked with `options` writes for `input`, read in
    /// pieces of `piece` bytes.
    fn unexpanded_in(options: &str, input: &[u8], piece: usize) -> Vec<u8> {
        let given = words(options);
        let (tabs, all, _) = asked(&given).unwrap();
        let mut unexpander = Unexpander::new(tabs, all);
        let mut out = Vec::new();
        for piece in input.chunks(piece.max(1)) {
            unexpander.feed(piece, &mut |bytes| out.extend_from_slice(bytes));
        }
        unexpander.finish(&mut |bytes| out.extend_from_slice(bytes));
        out
    }

    fn unexpanded(options: &str, input: &str) -> String {
        String::from_utf8(unexpanded_in(options, input.as_bytes(), 128)).unwrap()
    }

    /// `text` with each tab turned into spaces up to its stop, past the
    /// last of a list into one: how it looks.
    fn looks(tabs: &Tabs, text: &[u8]) -> Vec<char> {
        let mut shown = Vec::new();
        let mut column = 0;
        for c in core::str::from_utf8(text).unwrap().chars() {
            if c == '\t' {
                let to = tabs.after(column).unwrap_or(column + 1);
                shown.extend((column..to).map(|_| ' '));
                column = to;
            } else {
                shown.push(c);
                column = if c == '\n' { 0 } else { column + 1 };
            }
        }
        shown
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn blanks_a_line_starts_with_become_tabs() {
        assert_eq!(unexpanded("", "        a  b       c\n"), "\ta  b       c\n");
        assert_eq!(unexpanded("", "                a\n"), "\t\ta\n");
        assert_eq!(unexpanded("", "          a\n"), "\t  a\n");
        assert_eq!(
            unexpanded("", "       a\n"),
            "       a\n",
            "no stop reached"
        );
        assert_eq!(unexpanded("", " \ta\n"), "\ta\n");
        assert_eq!(unexpanded("", "  \t  a\n"), "\t  a\n");
        assert_eq!(unexpanded("", "x\n        y\n"), "x\n\ty\n");
        assert_eq!(unexpanded("", "        "), "\t", "blanks the text ends in");
        assert_eq!(unexpanded("", ""), "");
    }

    #[test]
    fn a_turns_every_run_of_two_blanks_or_more() {
        assert_eq!(unexpanded("-a", "        a  b       c\n"), "\ta  b\t   c\n");
        assert_eq!(unexpanded("-a -t 4", "abc  d\n"), "abc\t d\n");
        assert_eq!(
            unexpanded("-a -t 4", "abc d\n"),
            "abc d\n",
            "one blank stays"
        );
        assert_eq!(unexpanded("-a -t 4", "a   b\n"), "a\tb\n");
        assert_eq!(unexpanded("-a -t 4", "abcd  e\n"), "abcd  e\n");
        assert_eq!(unexpanded("-a -t 4", "a\t  b\n"), "a\t  b\n");
        assert_eq!(unexpanded("-a", "x \t y\n"), "x\t y\n");
        assert_eq!(unexpanded("-a", "a    "), "a    ");
        assert_eq!(unexpanded("", "a        b\n"), "a        b\n", "without -a");
    }

    #[test]
    fn t_sets_the_stops_and_means_a() {
        assert_eq!(unexpanded("-t 2", "    a\n"), "\t\ta\n");
        assert_eq!(unexpanded("-t2,5", "  a   b\n"), "\ta\t b\n");
        assert_eq!(unexpanded("-t 4,8", "          a\n"), "\t\t  a\n");
        assert_eq!(unexpanded("-t 8,12", "  a\n"), "  a\n");
        assert_eq!(
            unexpanded("-t 2,4", "\t\t\tx\n"),
            "\t\t\tx\n",
            "past the last stop a blank stays as it came"
        );
        assert_eq!(unexpanded("-t 2,4", "      x\n"), "\t\t  x\n");
    }

    #[test]
    fn backspaces_lines_and_characters_move_the_column() {
        assert_eq!(unexpanded("-a", "ab\x08      c\n"), "ab\x08      c\n");
        assert_eq!(unexpanded("-a", "ab\x08       c\n"), "ab\x08\tc\n");
        assert_eq!(
            unexpanded("-a -t 4", "\u{e9}t\u{e9}  x\n"),
            "\u{e9}t\u{e9}\t x\n"
        );
        assert_eq!(unexpanded("-a -t 4", "\u{1f600}   x"), "\u{1f600}\tx");
    }

    #[test]
    fn options_and_the_files_after_them() {
        let given = words("-a -- -t b");
        let (tabs, all, files) = asked(&given).unwrap();
        assert_eq!((tabs, all), (Tabs::Every(8), true));
        assert_eq!(files, ["-t", "b"]);
        let given = words("-at 3 c");
        let (tabs, all, files) = asked(&given).unwrap();
        assert_eq!((tabs, all), (Tabs::Every(3), true));
        assert_eq!(files, ["c"]);
        let given = words("-t 2,6");
        assert!(asked(&given).unwrap().1, "-t means -a");
        let given = words("x -a");
        let (_, all, files) = asked(&given).unwrap();
        assert!(!all);
        assert_eq!(files, ["x", "-a"]);
        let wrong = |options: &str| asked(&words(options)).unwrap_err();
        assert_eq!(wrong("-x"), "unexpand: -x is no option\n");
        assert_eq!(wrong("-ax"), "unexpand: -x is no option\n");
        assert_eq!(wrong("-t"), "unexpand: -t wants its word\n");
        assert_eq!(wrong("-a -t 0"), "unexpand: 0 is no tab stop\n");
        assert_eq!(
            wrong("-t 5,5"),
            "unexpand: tab stops go up, and 5 does not\n"
        );
    }

    #[test]
    fn random_texts_look_the_same_and_come_out_the_same_whatever_the_pieces() {
        let mut seed = 0x0fed_cba9_8765_4321_u64;
        let alphabet: [&[u8]; 6] = [b"a", b" ", b" ", b"\t", b"\n", "\u{e9}".as_bytes()];
        for _ in 0..400 {
            let mut input = Vec::new();
            for _ in 0..shuffled(&mut seed) % 50 {
                let at = usize::try_from(shuffled(&mut seed) % 6).unwrap();
                input.extend_from_slice(alphabet[at]);
            }
            let options =
                ["", "-a", "-t 3", "-t 2,7,11"][usize::try_from(shuffled(&mut seed) % 4).unwrap()];
            let tabs = asked(&words(options)).unwrap().0;
            let whole = unexpanded_in(options, &input, input.len());
            for piece in [1, 2, 3, 7] {
                assert_eq!(unexpanded_in(options, &input, piece), whole);
            }
            assert_eq!(
                looks(&tabs, &whole),
                looks(&tabs, &input),
                "{options} {input:?}"
            );
            assert!(whole.len() <= input.len(), "never longer");
            assert_eq!(
                unexpanded_in(options, &whole, whole.len()),
                whole,
                "once is enough"
            );
        }
    }
}
