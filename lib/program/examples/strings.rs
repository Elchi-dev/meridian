//! strings: writes the runs of printable characters in files, or in its
//! standard input without one, as POSIX's strings does: each run of at
//! least four, or as many as `-n` says, that a line feed, a NUL, the end
//! or a character that does not print ends, on a line of its own; with
//! `-t` after its place in the file, in decimal, octal or hexadecimal as
//! `d`, `o` or `x` say. A printable character is one of ASCII's from the
//! blank to the tilde, or a tab. Every file is read whole, as `-a` asks.
//! It ends with 0, or 1 when a file could not be read.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Handle, text};

program::main!(strings);

/// How a run's place in the file is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Places {
    None,
    Decimal,
    Octal,
    Hexadecimal,
}

/// What strings's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    least: usize,
    places: Places,
    paths: &'w [String],
}

/// What strings's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut least = 4;
    let mut places = Places::None;
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
            if letter == 'a' {
                continue;
            }
            if letter != 'n' && letter != 't' {
                return Err(format!("strings: -{letter} is no option\n"));
            }
            // The value is the rest of the word, or the next word.
            let attached = &letters[at + 1..];
            let value = if attached.is_empty() {
                let Some((value, rest)) = words.split_first() else {
                    return Err(format!("strings: -{letter} wants a value\n"));
                };
                words = rest;
                value.as_str()
            } else {
                attached
            };
            if letter == 'n' {
                least =
                    value.parse().ok().filter(|&n| n > 0).ok_or_else(|| {
                        format!("strings: -n {value}: wants a count of characters\n")
                    })?;
            } else {
                places = match value {
                    "d" => Places::Decimal,
                    "o" => Places::Octal,
                    "x" => Places::Hexadecimal,
                    _ => return Err(format!("strings: -t {value}: wants d, o or x\n")),
                };
            }
            break;
        }
    }
    Ok(Asked {
        least,
        places,
        paths: words,
    })
}

/// Whether `byte` prints.
fn prints(byte: u8) -> bool {
    byte == b'\t' || (b' '..=b'~').contains(&byte)
}

/// The runs of printable bytes as they come in pieces, kept across them.
struct Runs<'a> {
    asked: &'a Asked<'a>,
    /// The run so far, and where in the file it started.
    run: Vec<u8>,
    start: u64,
    at: u64,
}

impl Runs<'_> {
    /// Takes `piece`, writing each run it ends with `write`.
    fn feed(&mut self, piece: &[u8], write: &mut impl FnMut(&str)) {
        for &byte in piece {
            if prints(byte) {
                if self.run.is_empty() {
                    self.start = self.at;
                }
                self.run.push(byte);
            } else {
                self.end(write);
            }
            self.at += 1;
        }
    }

    /// Ends the run in hand, writing it when it is long enough.
    fn end(&mut self, write: &mut impl FnMut(&str)) {
        if self.run.len() >= self.asked.least {
            let run = String::from_utf8_lossy(&self.run);
            let line = match self.asked.places {
                Places::None => format!("{run}\n"),
                Places::Decimal => format!("{} {run}\n", self.start),
                Places::Octal => format!("{:o} {run}\n", self.start),
                Places::Hexadecimal => format!("{:x} {run}\n", self.start),
            };
            write(&line);
        }
        self.run.clear();
    }
}

#[cfg_attr(test, allow(dead_code))]
fn strings(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut out = text::standard_output();
    let mut failed = false;
    for path in text::sources(asked.paths) {
        let mut runs = Runs {
            asked: &asked,
            run: Vec::new(),
            start: 0,
            at: 0,
        };
        let mut write = |line: &str| out.put(line.as_bytes());
        let read = text::pieces(path.filter(|&p| p != "-"), |piece| {
            runs.feed(piece, &mut write);
            true
        });
        runs.end(&mut write);
        if let Err(e) = read {
            out.flush();
            let _ = program::write_error(&text::trouble("strings", path.unwrap_or("-"), e));
            failed = true;
        }
    }
    u64::from(!out.finish() || failed)
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

    /// What strings with the words `text` writes of `bytes`, fed in
    /// pieces of `piece` bytes.
    fn found(text: &str, bytes: &[u8], piece: usize) -> String {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let mut runs = Runs {
            asked: &taken,
            run: Vec::new(),
            start: 0,
            at: 0,
        };
        let mut written = String::new();
        let mut write = |line: &str| written.push_str(line);
        for chunk in bytes.chunks(piece.max(1)) {
            runs.feed(chunk, &mut write);
        }
        runs.end(&mut write);
        written
    }

    #[test]
    fn its_words_are_options_then_files() {
        let given = words("-a -n 6 -t x f g");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                least: 6,
                places: Places::Hexadecimal,
                paths: &given[5..],
            })
        );
        let given = words("-n2 -td");
        assert_eq!(
            asked(&given).map(|a| (a.least, a.places)),
            Ok((2, Places::Decimal)),
            "values attached"
        );
        assert_eq!(
            asked(&words("-an3")).map(|a| a.least),
            Ok(3),
            "after -a in one word"
        );
        assert_eq!(asked(&words("-t o")).map(|a| a.places), Ok(Places::Octal));
        assert_eq!(
            asked(&words("")).map(|a| (a.least, a.places)),
            Ok((4, Places::None))
        );
        let given = words("-- -n");
        assert_eq!(asked(&given).map(|a| a.paths.len()), Ok(1));
        for (text, why) in [
            ("-x", "strings: -x is no option\n"),
            ("-n", "strings: -n wants a value\n"),
            ("-t", "strings: -t wants a value\n"),
            ("-n 0", "strings: -n 0: wants a count of characters\n"),
            ("-n x", "strings: -n x: wants a count of characters\n"),
            ("-t b", "strings: -t b: wants d, o or x\n"),
        ] {
            assert_eq!(asked(&words(text)), Err(String::from(why)), "{text}");
        }
    }

    #[test]
    fn runs_of_four_printable_characters_or_more_are_written() {
        let bytes = b"\x00\x01abc\x00abcd\x02hello world\nxy\tzw\x7f\x80last";
        assert_eq!(found("", bytes, 1000), "abcd\nhello world\nxy\tzw\nlast\n");
        assert_eq!(
            found("-n 2", bytes, 1000),
            "abc\nabcd\nhello world\nxy\tzw\nlast\n"
        );
        assert_eq!(found("-n 6", bytes, 1000), "hello world\n");
        assert_eq!(found("", b"", 1000), "");
        assert_eq!(found("", b"abc", 1000), "", "three is too few");
        assert_eq!(
            found("", "näme".as_bytes(), 1000),
            "",
            "a byte past ASCII does not print"
        );
    }

    #[test]
    fn places_are_where_each_run_starts() {
        let bytes = b"\x00\x00abcde\x00\x00\x00\x00\x00\x00\x00\x00fghij";
        assert_eq!(found("-t d", bytes, 1000), "2 abcde\n15 fghij\n");
        assert_eq!(found("-t o", bytes, 1000), "2 abcde\n17 fghij\n");
        assert_eq!(found("-t x", bytes, 1000), "2 abcde\nf fghij\n");
    }

    #[test]
    fn runs_go_on_across_pieces() {
        let bytes = b"\x00abcdefgh\x00ij\x00klmnop";
        let whole = found("-t d", bytes, 1000);
        for piece in 1..bytes.len() {
            assert_eq!(found("-t d", bytes, piece), whole, "{piece}");
        }
        assert_eq!(whole, "1 abcdefgh\n13 klmnop\n");
    }

    #[test]
    fn printable_is_ascii_from_blank_to_tilde_and_tab() {
        for byte in 0..=255u8 {
            let printable = byte == b'\t' || (32..=126).contains(&byte);
            assert_eq!(prints(byte), printable, "{byte}");
        }
    }

    #[test]
    fn what_ends_a_run_is_not_written() {
        assert_eq!(
            found("", b"abcd\nefgh\x00ijkl\x7fmnop", 1000),
            "abcd\nefgh\nijkl\nmnop\n"
        );
        assert_eq!(
            found("-n 1", b"a\nb", 1000),
            "a\nb\n",
            "one is enough with -n 1"
        );
        assert_eq!(found("", b"\t\t\t\t", 1000), "\t\t\t\t\n", "tabs print");
        let long = [b'x'; 5000];
        assert_eq!(
            found("", &long, 7),
            format!("{}\n", "x".repeat(5000)),
            "a run may be long"
        );
        assert_eq!(
            found("-t x", &[&[0u8; 300][..], b"abcd"].concat(), 64),
            "12c abcd\n"
        );
        assert_eq!(found("-n 5 -t o", b"abcd\x00abcde", 3), "5 abcde\n");
    }
}
