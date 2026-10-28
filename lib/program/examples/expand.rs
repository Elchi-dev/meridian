//! expand: writes its files, or its standard input, with each tab turned
//! into the spaces up to the next tab stop, as POSIX's expand does: every
//! eight columns, or as `-t` says, one number for every so many columns or
//! a list of the columns, past whose last a tab is one space. A backspace
//! takes a column back, a line feed starts again at the first, and a
//! character of more than one byte takes one column. `-` is the standard
//! input.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::Handle;
use program::tabs::Tabs;
use program::text;

program::main!(expand);

/// The most spaces one piece of output takes.
const SPACES: [u8; 64] = [b' '; 64];

/// What expand's words ask for: the tab stops, and the files.
fn asked(arguments: &[String]) -> Result<(Tabs, &[String]), String> {
    let mut tabs = Tabs::default();
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
        let Some(after) = letters.strip_prefix('t') else {
            let first = letters.chars().take(1).collect::<String>();
            return Err(format!("expand: -{first} is no option\n"));
        };
        let value = if after.is_empty() {
            let (value, rest) = words
                .split_first()
                .ok_or_else(|| String::from("expand: -t wants its word\n"))?;
            words = rest;
            value.as_str()
        } else {
            after
        };
        tabs = Tabs::parse("expand", value)?;
    }
    Ok((tabs, words))
}

/// Gives `put` `count` spaces, a few at a time.
fn spaces(count: u64, put: &mut impl FnMut(&[u8])) {
    let mut left = count;
    while left > 0 {
        let now = usize::try_from(left).map_or(SPACES.len(), |left| left.min(SPACES.len()));
        put(&SPACES[..now]);
        left -= now as u64;
    }
}

/// Turns tabs into spaces as the text comes, keeping the column it is at.
struct Expander {
    tabs: Tabs,
    column: u64,
}

impl Expander {
    /// Gives `put` what `bytes`, the next of the text, become.
    fn feed(&mut self, bytes: &[u8], put: &mut impl FnMut(&[u8])) {
        let mut from = 0;
        for (at, &byte) in bytes.iter().enumerate() {
            match byte {
                b'\t' => {
                    put(&bytes[from..at]);
                    from = at + 1;
                    let to = self
                        .tabs
                        .after(self.column)
                        .unwrap_or(self.column.saturating_add(1));
                    spaces(to - self.column, put);
                    self.column = to;
                }
                8 => self.column = self.column.saturating_sub(1),
                b'\n' => self.column = 0,
                // A byte that goes on with a character takes no column.
                _ if byte & 0xc0 == 0x80 => {}
                _ => self.column = self.column.saturating_add(1),
            }
        }
        put(&bytes[from..]);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn expand(_: Handle) -> u64 {
    let (tabs, files) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut expander = Expander { tabs, column: 0 };
    let mut out = text::standard_output();
    let whole = text::each_piece("expand", files, |piece| {
        expander.feed(piece, &mut |bytes| out.put(bytes));
    });
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

    /// What expand asked with `options` writes for `input`, read in pieces
    /// of `piece` bytes.
    fn expanded_in(options: &str, input: &[u8], piece: usize) -> Vec<u8> {
        let given = words(options);
        let (tabs, _) = asked(&given).unwrap();
        let mut expander = Expander { tabs, column: 0 };
        let mut out = Vec::new();
        for piece in input.chunks(piece.max(1)) {
            expander.feed(piece, &mut |bytes| out.extend_from_slice(bytes));
        }
        out
    }

    fn expanded(options: &str, input: &str) -> String {
        String::from_utf8(expanded_in(options, input.as_bytes(), 128)).unwrap()
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn tabs_become_spaces_up_to_every_eighth_column() {
        assert_eq!(expanded("", "\tx\n"), "        x\n");
        assert_eq!(expanded("", "abc\tx\n"), "abc     x\n");
        assert_eq!(expanded("", "abcdefgh\tx\n"), "abcdefgh        x\n");
        assert_eq!(expanded("", "a\t\tb"), "a               b");
        assert_eq!(expanded("", "no tabs\n"), "no tabs\n");
        assert_eq!(expanded("", ""), "");
    }

    #[test]
    fn t_sets_the_distance_or_the_columns() {
        assert_eq!(expanded("-t 4", "ab\tc\n"), "ab  c\n");
        assert_eq!(expanded("-t4", "\t\ta\n"), "        a\n");
        assert_eq!(expanded("-t 3", "\t\ta\n"), "      a\n");
        assert_eq!(expanded("-t 1", "a\tb\n"), "a b\n");
        assert_eq!(
            expanded("-t 2,5", "a\tb\tc\td\n"),
            "a b  c d\n",
            "past the last stop a tab is one space"
        );
        assert_eq!(expanded("-t 2,4", "\t\t\tx\n"), "     x\n");
        let far = expanded("-t 1000", "\ta");
        assert_eq!(far.len(), 1001);
        assert!(far.starts_with(&" ".repeat(1000)));
    }

    #[test]
    fn lines_backspaces_and_characters_move_the_column() {
        assert_eq!(expanded("-t 4", "abc\tx\nab\ty\n"), "abc x\nab  y\n");
        assert_eq!(expanded("", "ab\x08\tc\n"), "ab\x08       c\n");
        assert_eq!(expanded("", "\x08\x08\tc"), "\x08\x08        c");
        assert_eq!(expanded("-t 4", "\u{e9}\tx\n"), "\u{e9}   x\n");
        assert_eq!(
            expanded("-t 4", "\u{20ac}\u{1f600}\tx"),
            "\u{20ac}\u{1f600}  x"
        );
    }

    #[test]
    fn options_and_the_files_after_them() {
        let given = words("-t 2 -- -t a");
        let (tabs, files) = asked(&given).unwrap();
        assert_eq!(tabs, Tabs::Every(2));
        assert_eq!(files, ["-t", "a"]);
        let given = words("- -t 3");
        let (tabs, files) = asked(&given).unwrap();
        assert_eq!(tabs, Tabs::Every(8));
        assert_eq!(files, ["-", "-t", "3"]);
        let given = words("-t 9 -t 2,3");
        assert_eq!(
            asked(&given).unwrap().0,
            Tabs::At(alloc::vec![2, 3]),
            "the last -t"
        );
        let wrong = |options: &str| asked(&words(options)).unwrap_err();
        assert_eq!(wrong("-x"), "expand: -x is no option\n");
        assert_eq!(wrong("-at 4"), "expand: -a is no option\n");
        assert_eq!(wrong("-t"), "expand: -t wants its word\n");
        assert_eq!(wrong("-t 0"), "expand: 0 is no tab stop\n");
        assert_eq!(wrong("-t 3,2"), "expand: tab stops go up, and 2 does not\n");
    }

    #[test]
    fn many_spaces_come_out_in_pieces_and_all_of_them() {
        let mut pieces = Vec::new();
        spaces(150, &mut |bytes: &[u8]| pieces.push(bytes.to_vec()));
        assert_eq!(
            pieces.iter().map(Vec::len).collect::<Vec<_>>(),
            [64, 64, 22]
        );
        assert!(pieces.concat().iter().all(|&b| b == b' '));
        let mut none = 0;
        spaces(0, &mut |_: &[u8]| none += 1);
        assert_eq!(none, 0);
    }

    #[test]
    fn random_texts_lose_their_tabs_and_nothing_else_whatever_the_pieces() {
        let mut seed = 0x1234_5678_9abc_def1_u64;
        let alphabet: [&[u8]; 6] = [b"a", b" ", b"\t", b"\n", "\u{e9}".as_bytes(), b"\x08"];
        for _ in 0..300 {
            let mut input = Vec::new();
            for _ in 0..shuffled(&mut seed) % 60 {
                let at = usize::try_from(shuffled(&mut seed) % 6).unwrap();
                input.extend_from_slice(alphabet[at]);
            }
            let options =
                ["", "-t 3", "-t 2,7,11"][usize::try_from(shuffled(&mut seed) % 3).unwrap()];
            let whole = expanded_in(options, &input, input.len());
            for piece in [1, 2, 5] {
                assert_eq!(expanded_in(options, &input, piece), whole);
            }
            assert!(!whole.contains(&b'\t'));
            let without = |bytes: &[u8]| -> Vec<u8> {
                bytes
                    .iter()
                    .copied()
                    .filter(|&b| b != b' ' && b != b'\t')
                    .collect()
            };
            assert_eq!(without(&whole), without(&input), "only blanks change");
            assert_eq!(
                expanded_in(options, &whole, whole.len()),
                whole,
                "a text without tabs stays as it is"
            );
            if options.is_empty() && !input.contains(&8) {
                // Each line comes out as wide as its last column, counted
                // a character at a time, a tab going to the next eighth.
                let lines_in = input.split(|&b| b == b'\n');
                for (line, out) in lines_in.zip(whole.split(|&b| b == b'\n')) {
                    let mut column = 0;
                    for c in core::str::from_utf8(line).unwrap().chars() {
                        column = if c == '\t' {
                            column / 8 * 8 + 8
                        } else {
                            column + 1
                        };
                    }
                    assert_eq!(core::str::from_utf8(out).unwrap().chars().count(), column);
                }
            }
        }
    }
}
