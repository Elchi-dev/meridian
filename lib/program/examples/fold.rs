//! fold: breaks the lines of each file it is given, or of its standard
//! input, that are wider than a width, as POSIX's fold does: 80 columns,
//! or `-w`'s. A tab takes the column on to the next multiple of 8, a
//! backspace takes it back one and a carriage return back to the start;
//! with `-b` every byte is a column. With `-s` a line breaks after the
//! last blank before the width when it has one.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::abi::MESSAGE_BYTES;
use program::text::{self, LineReader, Source};
use program::{Error, Handle};

program::main!(fold);

/// What fold was asked for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    width: usize,
    bytes: bool,
    at_blanks: bool,
    files: &'a [String],
}

fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        width: 80,
        bytes: false,
        at_blanks: false,
        files: &[],
    };
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
                'b' => asked.bytes = true,
                's' => asked.at_blanks = true,
                'w' => {
                    // The rest of the word is the width, or else the next
                    // word.
                    let after = &letters[at + 1..];
                    let width = if after.is_empty() {
                        let (width, rest) = words
                            .split_first()
                            .ok_or_else(|| String::from("fold: -w wants a width\n"))?;
                        words = rest;
                        width.as_str()
                    } else {
                        after
                    };
                    asked.width = width
                        .parse()
                        .ok()
                        .filter(|&width| width > 0)
                        .ok_or_else(|| format!("fold: {width} is no width\n"))?;
                    break;
                }
                _ => return Err(format!("fold: -{letter} is no option\n")),
            }
        }
    }
    asked.files = words;
    Ok(asked)
}

/// The column after `c` at `column`: one more, or as a tab, a backspace
/// or a carriage return take it, unless every byte is a column; none for
/// a byte that is no UTF-8, which is a column.
fn after(column: usize, c: Option<char>, bytes: bool) -> usize {
    match c {
        Some(c) if bytes => column + c.len_utf8(),
        Some('\t') => (column / 8 + 1) * 8,
        Some('\u{8}') => column.saturating_sub(1),
        Some('\r') => 0,
        _ => column + 1,
    }
}

/// A line being folded.
struct Folding<'a> {
    asked: &'a Asked<'a>,
    out: Vec<u8>,
    /// The piece being made, how wide it is, and where in it the last
    /// blank ends.
    piece: Vec<u8>,
    column: usize,
    blank: Option<usize>,
}

impl Folding<'_> {
    /// Takes in the character `c`, whose bytes are `bytes`, breaking the
    /// line before it when it would make it too wide.
    fn take(&mut self, c: Option<char>, bytes: &[u8]) {
        // After a break at a blank what is left may still be too wide
        // for `c`, so it is asked again.
        while after(self.column, c, self.asked.bytes) > self.asked.width && !self.piece.is_empty() {
            match self.blank.take() {
                Some(end) => {
                    // What came after the blank starts the next piece.
                    let rest = self.piece.split_off(end);
                    self.out.append(&mut self.piece);
                    self.piece = rest;
                }
                None => self.out.append(&mut self.piece),
            }
            self.out.push(b'\n');
            self.column = recount(&self.piece, self.asked.bytes);
        }
        self.piece.extend_from_slice(bytes);
        self.column = after(self.column, c, self.asked.bytes);
        if self.asked.at_blanks && matches!(c, Some(' ' | '\t')) {
            self.blank = Some(self.piece.len());
        }
    }
}

/// `line` broken into pieces no wider than `asked` says, each followed by
/// a line feed.
fn folded(line: &[u8], asked: &Asked) -> Vec<u8> {
    let mut folding = Folding {
        asked,
        out: Vec::new(),
        piece: Vec::new(),
        column: 0,
        blank: None,
    };
    for chunk in line.utf8_chunks() {
        for c in chunk.valid().chars() {
            let mut room = [0; 4];
            folding.take(Some(c), c.encode_utf8(&mut room).as_bytes());
        }
        for byte in chunk.invalid() {
            folding.take(None, core::slice::from_ref(byte));
        }
    }
    let mut out = folding.out;
    out.append(&mut folding.piece);
    out.push(b'\n');
    out
}

/// How wide `piece` is from the start of a line.
fn recount(piece: &[u8], bytes: bool) -> usize {
    let mut column = 0;
    for chunk in piece.utf8_chunks() {
        for c in chunk.valid().chars() {
            column = after(column, Some(c), bytes);
        }
        column += chunk.invalid().len();
    }
    column
}

#[cfg_attr(test, allow(dead_code))]
fn fold(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut failed = false;
    for path in text::sources(asked.files) {
        let path = path.unwrap_or("-");
        if let Err(e) = fold_file(path, &asked) {
            failed = true;
            let _ = program::write_error(&text::trouble("fold", path, e));
        }
    }
    u64::from(failed)
}

/// Folds the lines of the file at `path`, or of the standard input for
/// `-`, writing them as they are folded.
#[cfg_attr(test, allow(dead_code))]
fn fold_file(path: &str, asked: &Asked) -> Result<(), Error> {
    let mut reader = LineReader::new(Source::open(path)?);
    let mut held = Vec::new();
    let read = loop {
        match reader.line() {
            Ok(Some(line)) => held.extend(folded(&line, asked)),
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        }
        if held.len() >= MESSAGE_BYTES {
            let _ = program::write_bytes(&held);
            held.clear();
        }
    };
    let _ = program::write_bytes(&held);
    reader.into_pieces().close();
    read
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

    fn fold_with(options: &str, line: &str) -> String {
        let words = words(options);
        let asked = asked(&words).unwrap();
        String::from_utf8(folded(line.as_bytes(), &asked)).unwrap()
    }

    #[test]
    fn a_line_wider_than_the_width_breaks_at_it() {
        assert_eq!(fold_with("-w 4", "abcdefghij"), "abcd\nefgh\nij\n");
        assert_eq!(
            fold_with("-w 4", "abcd"),
            "abcd\n",
            "as wide as the width stays"
        );
        assert_eq!(fold_with("-w 4", ""), "\n");
        assert_eq!(fold_with("-w1", "abc"), "a\nb\nc\n");
        assert_eq!(
            fold_with("", &"x".repeat(81)),
            "x".repeat(80) + "\nx\n",
            "80 by default"
        );
        assert_eq!(
            fold_with("-w 3", "äöüß"),
            "äöü\nß\n",
            "characters are columns"
        );
    }

    #[test]
    fn tabs_backspaces_and_carriage_returns_move_the_column() {
        assert_eq!(
            fold_with("-w 10", "ab\tcd"),
            "ab\tcd\n",
            "the tab takes it to 8"
        );
        assert_eq!(fold_with("-w 10", "ab\tcdefg"), "ab\tcd\nefg\n");
        assert_eq!(
            fold_with("-w 4", "abc\u{8}\u{8}de"),
            "abc\u{8}\u{8}de\n",
            "back two, on two"
        );
        assert_eq!(
            fold_with("-w 4", "abc\rdefg"),
            "abc\rdefg\n",
            "back to the start"
        );
        assert_eq!(
            fold_with("-w 4", "\tab"),
            "\t\nab\n",
            "a tab alone wider than the width"
        );
    }

    #[test]
    fn b_counts_bytes() {
        assert_eq!(fold_with("-b -w 4", "äöü"), "äö\nü\n");
        assert_eq!(fold_with("-bw 3", "ab\tc"), "ab\t\nc\n", "a tab is a byte");
        assert_eq!(fold_with("-b -w 2", "a\rbc"), "a\r\nbc\n");
    }

    #[test]
    fn s_breaks_after_the_last_blank() {
        assert_eq!(
            fold_with("-s -w 10", "the quick brown fox"),
            "the quick \nbrown fox\n"
        );
        assert_eq!(fold_with("-s -w 4", "ab cd ef"), "ab \ncd \nef\n");
        assert_eq!(
            fold_with("-s -w 4", "abcdefgh"),
            "abcd\nefgh\n",
            "no blank, where the width says"
        );
        assert_eq!(
            fold_with("-s -w 6", "a b cdefgh"),
            "a b \ncdefgh\n",
            "the last of two"
        );
        assert_eq!(
            fold_with("-w 4", "ab cd ef"),
            "ab c\nd ef\n",
            "without -s the width alone"
        );
    }

    #[test]
    fn a_byte_that_is_no_utf8_is_a_column() {
        let narrow = words("-w 2");
        let asked = asked(&narrow).unwrap();
        assert_eq!(
            folded(&[0xff, 0xfe, 0xfd], &asked),
            [0xff, 0xfe, b'\n', 0xfd, b'\n']
        );
        assert_eq!(recount(&[0xff, b'a'], false), 2);
    }

    #[test]
    fn its_words_are_options_and_files() {
        let w = words("-bs -w 20 /a /b");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.width, a.bytes, a.at_blanks, a.files),
            (20, true, true, &w[3..])
        );
        assert_eq!(asked(&words("-w5")).unwrap().width, 5);
        assert_eq!(asked(&words("-- -w")).unwrap().files, ["-w"]);
        assert_eq!(
            asked(&words("-w 0")),
            Err(String::from("fold: 0 is no width\n"))
        );
        assert_eq!(
            asked(&words("-w x")),
            Err(String::from("fold: x is no width\n"))
        );
        assert_eq!(
            asked(&words("-w")),
            Err(String::from("fold: -w wants a width\n"))
        );
        assert_eq!(
            asked(&words("-q")),
            Err(String::from("fold: -q is no option\n"))
        );
    }

    #[test]
    fn a_column_moves_as_its_character_says() {
        assert_eq!(after(0, Some('a'), false), 1);
        assert_eq!(after(3, Some('\t'), false), 8);
        assert_eq!(
            after(8, Some('\t'), false),
            16,
            "a tab at a stop goes to the next"
        );
        assert_eq!(after(7, Some('\t'), false), 8);
        assert_eq!(
            after(0, Some('\u{8}'), false),
            0,
            "no further back than the start"
        );
        assert_eq!(after(5, Some('\u{8}'), false), 4);
        assert_eq!(after(5, Some('\r'), false), 0);
        assert_eq!(after(5, None, false), 6);
        assert_eq!(after(0, Some('ä'), true), 2, "with -b its bytes");
        assert_eq!(after(4, Some('\t'), true), 5);
        assert_eq!(after(4, None, true), 5);
    }

    #[test]
    fn folding_only_adds_line_feeds_and_keeps_within_the_width() {
        let mut seed = 0x1405_7b7e_f767_814f_u64;
        let mut roll = |sides: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % 1024).unwrap() % sides
        };
        for _ in 0..300 {
            let line: String = (0..roll(30))
                .map(|_| ['a', 'b', ' ', 'ä', '\t', 'x'][roll(6)])
                .collect();
            let width = 1 + roll(10);
            for options in ["", "-s", "-b", "-bs"] {
                let words = words(&alloc::format!("{options} -w {width}"));
                let asked = asked(&words).unwrap();
                let out = String::from_utf8(folded(line.as_bytes(), &asked)).unwrap();
                let pieces: Vec<&str> = out.strip_suffix('\n').unwrap().split('\n').collect();
                assert_eq!(pieces.concat(), line, "{options} -w {width} on {line:?}");
                for piece in &pieces {
                    let wide = recount(piece.as_bytes(), asked.bytes);
                    assert!(
                        wide <= width || piece.chars().count() == 1,
                        "{piece:?} of {line:?} wider than {width} with {options:?}"
                    );
                }
                if asked.at_blanks {
                    for (piece, next) in pieces.iter().zip(&pieces[1..]) {
                        let blank = piece.contains([' ', '\t']);
                        assert!(
                            !blank || piece.ends_with([' ', '\t']) || next.is_empty(),
                            "{piece:?} breaks after its last blank in {line:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_character_too_wide_after_a_break_at_a_blank_breaks_again() {
        assert_eq!(fold_with("-s -w 6", " b\tbxxb"), " \nb\n\t\nbxxb\n");
        assert_eq!(fold_with("-s -w 3", "a abc"), "a \nabc\n");
    }

    #[test]
    fn with_b_a_tab_is_one_byte_wide_after_a_break_too() {
        assert_eq!(
            fold_with("-bs -w 6", " b\tbxxb"),
            " b\t\nbxxb\n",
            "a byte each, and after the tab, the last blank"
        );
        assert_eq!(fold_with("-b -w 2", "\t\t\t"), "\t\t\n\t\n");
        assert_eq!(
            fold_with("-w 9", "\t\t"),
            "\t\n\t\n",
            "eight columns each without -b"
        );
    }
}
