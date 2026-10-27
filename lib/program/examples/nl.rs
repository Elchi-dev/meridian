//! nl: numbers the lines of a file, or of its standard input, as POSIX's
//! nl does. A text is made of logical pages of a header, a body and a
//! footer, each starting at a line of the delimiter three, two or one
//! times over, `\:` unless `-d` says otherwise; a page starts the count
//! again unless `-p`. `-h`, `-b` and `-f` say which lines of each are
//! numbered: `a` all, `t` those not empty, `n` none, `pRE` those a basic
//! expression matches; by default only the body's not empty. `-v` gives
//! the first number, `-i` the step, `-w` the width, `-n` the format, `ln`,
//! `rn` or `rz`, `-s` what comes between number and line, and `-l` how
//! many empty lines in a row count as one.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::abi::MESSAGE_BYTES;
use program::text::{self, LineReader, Source};
use program::{Error, Handle};
use regex::{Regex, Syntax, Text};

// lib/regex keeps its deepest search within half of this.
program::main!(nl, stack = 256);

/// Which lines of a section are numbered.
#[derive(Debug)]
enum Kind {
    All,
    Text,
    None,
    Matching(Regex),
}

/// How numbers are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Left,
    Right,
    Zeros,
}

/// What nl was asked for.
#[derive(Debug)]
struct Asked<'a> {
    /// For the header, the body and the footer.
    kinds: [Kind; 3],
    delimiter: String,
    step: i64,
    blanks: usize,
    format: Format,
    keep_counting: bool,
    separator: String,
    first: i64,
    width: usize,
    file: &'a str,
}

/// The kind `-h`, `-b` or `-f` names with `text`.
fn kind(text: &str) -> Result<Kind, String> {
    match text {
        "a" => Ok(Kind::All),
        "t" => Ok(Kind::Text),
        "n" => Ok(Kind::None),
        _ => {
            let Some(pattern) = text.strip_prefix('p') else {
                return Err(format!("nl: {text} is no type of numbering\n"));
            };
            Regex::new(pattern, Syntax::Basic, false)
                .map(Kind::Matching)
                .map_err(|e| format!("nl: {pattern}: {}\n", e.code.says()))
        }
    }
}

/// `text` as a number for the option `letter`.
fn number<T: core::str::FromStr>(letter: char, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("nl: -{letter} wants a number, not {text}\n"))
}

fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        kinds: [Kind::None, Kind::Text, Kind::None],
        delimiter: String::from("\\:"),
        step: 1,
        blanks: 1,
        format: Format::Right,
        keep_counting: false,
        separator: String::from("\t"),
        first: 1,
        width: 6,
        file: "-",
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
            if letter == 'p' {
                asked.keep_counting = true;
                continue;
            }
            if !"hbfdilnsvw".contains(letter) {
                return Err(format!("nl: -{letter} is no option\n"));
            }
            // Every other option takes the rest of the word, or else the
            // next word.
            let after = &letters[at + 1..];
            let value = if after.is_empty() {
                let (value, rest) = words
                    .split_first()
                    .ok_or_else(|| format!("nl: -{letter} wants its word\n"))?;
                words = rest;
                value.as_str()
            } else {
                after
            };
            match letter {
                'h' => asked.kinds[0] = kind(value)?,
                'b' => asked.kinds[1] = kind(value)?,
                'f' => asked.kinds[2] = kind(value)?,
                'd' => {
                    // One character stands for the first, the second
                    // staying `:`.
                    let mut chars = value.chars();
                    asked.delimiter = match (chars.next(), chars.next(), chars.next()) {
                        (Some(one), None, None) => format!("{one}:"),
                        (Some(_), Some(_), None) => String::from(value),
                        _ => return Err(String::from("nl: -d takes one or two characters\n")),
                    };
                }
                'i' => asked.step = number(letter, value)?,
                'l' => {
                    asked.blanks = number(letter, value)?;
                    if asked.blanks == 0 {
                        return Err(String::from("nl: -l wants a number from 1\n"));
                    }
                }
                'n' => {
                    asked.format = match value {
                        "ln" => Format::Left,
                        "rn" => Format::Right,
                        "rz" => Format::Zeros,
                        _ => return Err(format!("nl: {value} is no format of numbers\n")),
                    };
                }
                's' => asked.separator = String::from(value),
                'v' => asked.first = number(letter, value)?,
                _ => {
                    asked.width = number(letter, value)?;
                    if asked.width == 0 {
                        return Err(String::from("nl: -w wants a number from 1\n"));
                    }
                }
            }
            break;
        }
    }
    match words {
        [] => {}
        [file] => asked.file = file,
        _ => return Err(String::from("nl: numbers one file\n")),
    }
    Ok(asked)
}

/// The numbering of a text, line by line.
struct Numbering<'a> {
    asked: &'a Asked<'a>,
    /// The section being read: the header, the body or the footer.
    section: usize,
    next: i64,
    /// How many empty lines came in a row.
    empties: usize,
}

impl<'a> Numbering<'a> {
    fn new(asked: &'a Asked<'a>) -> Numbering<'a> {
        Numbering {
            asked,
            section: 1,
            next: asked.first,
            empties: 0,
        }
    }

    /// The section a line of the delimiter that many times over starts.
    fn starts(&self, line: &[u8]) -> Option<usize> {
        let delimiter = self.asked.delimiter.as_bytes();
        let all = !line.is_empty()
            && line.len().is_multiple_of(delimiter.len())
            && line.chunks(delimiter.len()).all(|chunk| chunk == delimiter);
        match line.len() / delimiter.len() {
            3 if all => Some(0),
            2 if all => Some(1),
            1 if all => Some(2),
            _ => None,
        }
    }

    /// What nl writes for `line`.
    fn line(&mut self, line: &[u8]) -> Vec<u8> {
        if let Some(section) = self.starts(line) {
            if section == 0 && !self.asked.keep_counting {
                self.next = self.asked.first;
            }
            self.section = section;
            self.empties = 0;
            return b"\n".to_vec();
        }
        let numbered = match &self.asked.kinds[self.section] {
            Kind::All if line.is_empty() => {
                self.empties += 1;
                // Of empty lines in a row, every so manyth is numbered.
                self.empties.is_multiple_of(self.asked.blanks)
            }
            Kind::All => true,
            Kind::Text => !line.is_empty(),
            Kind::None => false,
            Kind::Matching(regex) => regex
                .find(&Text::new(line), 0)
                .is_ok_and(|found| found.is_some()),
        };
        if !line.is_empty() {
            self.empties = 0;
        }
        let mut out = if numbered {
            let number = self.next;
            self.next = self.next.saturating_add(self.asked.step);
            let width = self.asked.width;
            let written = match self.asked.format {
                Format::Left => format!("{number:<width$}"),
                Format::Right => format!("{number:>width$}"),
                Format::Zeros if number < 0 => format!(
                    "-{:0>w$}",
                    number.unsigned_abs(),
                    w = width.saturating_sub(1)
                ),
                Format::Zeros => format!("{number:0>width$}"),
            };
            written.into_bytes()
        } else {
            alloc::vec![b' '; self.asked.width]
        };
        out.extend_from_slice(self.asked.separator.as_bytes());
        out.extend_from_slice(line);
        out.push(b'\n');
        out
    }
}

#[cfg_attr(test, allow(dead_code))]
fn nl(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    match numbered(&asked) {
        Ok(()) => 0,
        Err(e) => {
            let _ = program::write_error(&text::trouble("nl", asked.file, e));
            1
        }
    }
}

/// Numbers the lines of `asked`'s file, writing them as it goes.
#[cfg_attr(test, allow(dead_code))]
fn numbered(asked: &Asked) -> Result<(), Error> {
    let mut reader = LineReader::new(Source::open(asked.file)?);
    let mut numbering = Numbering::new(asked);
    let mut held = Vec::new();
    let read = loop {
        match reader.line() {
            Ok(Some(line)) => held.extend(numbering.line(&line)),
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

    /// What nl asked with `options` writes for `text`.
    fn numbered(options: &str, text: &str) -> String {
        let words = words(options);
        let asked = asked(&words).unwrap();
        let mut numbering = Numbering::new(&asked);
        let mut out = Vec::new();
        for line in text.split_terminator('\n') {
            out.extend(numbering.line(line.as_bytes()));
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn lines_not_empty_are_numbered_by_default() {
        assert_eq!(numbered("", "a\n\nb\n"), "     1\ta\n      \t\n     2\tb\n");
        assert_eq!(numbered("", ""), "");
    }

    #[test]
    fn b_says_which_lines_of_the_body_are_numbered() {
        assert_eq!(
            numbered("-b a", "a\n\nb\n"),
            "     1\ta\n     2\t\n     3\tb\n"
        );
        assert_eq!(numbered("-b n", "a\nb\n"), "      \ta\n      \tb\n");
        assert_eq!(numbered("-b t", "a\n\n"), "     1\ta\n      \t\n");
        assert_eq!(
            numbered("-b p^x", "xa\nb\nxc\n"),
            "     1\txa\n      \tb\n     2\txc\n"
        );
        assert_eq!(numbered("-bp[0-9]", "a1\nb\n"), "     1\ta1\n      \tb\n");
    }

    #[test]
    fn numbers_are_written_as_format_width_step_and_start_say() {
        assert_eq!(numbered("-n ln", "a\n"), "1     \ta\n");
        assert_eq!(numbered("-n rz", "a\n"), "000001\ta\n");
        assert_eq!(numbered("-n rz -w 3 -v -5", "a\n"), "-05\ta\n");
        assert_eq!(numbered("-w 2 -v 9", "a\nb\nc\n"), " 9\ta\n10\tb\n11\tc\n");
        assert_eq!(
            numbered("-w 1 -v 98", "a\nb\nc\n"),
            "98\ta\n99\tb\n100\tc\n",
            "wider than the width"
        );
        assert_eq!(numbered("-i 10 -v 0 -w 3", "a\nb\n"), "  0\ta\n 10\tb\n");
        assert_eq!(
            numbered("-i -1 -w 2 -v 1", "a\nb\nc\n"),
            " 1\ta\n 0\tb\n-1\tc\n"
        );
        assert_eq!(numbered("-s : -w 1", "a\n"), "1:a\n");
        assert_eq!(numbered("-s ) -w 1", "a\n"), "1)a\n");
    }

    #[test]
    fn a_logical_page_starts_its_count_again_unless_p() {
        let text = "\\:\\:\\:\nh\n\\:\\:\nb\nc\n\\:\nf\n\\:\\:\\:\nh\n\\:\\:\nb\n";
        assert_eq!(
            numbered("-w 1", text),
            "\n \th\n\n1\tb\n2\tc\n\n \tf\n\n \th\n\n1\tb\n",
            "header and footer not numbered, the body from 1 on each page"
        );
        assert_eq!(
            numbered("-w 1 -p", text),
            "\n \th\n\n1\tb\n2\tc\n\n \tf\n\n \th\n\n3\tb\n"
        );
        assert_eq!(
            numbered("-w 1 -h a -f a", text),
            "\n1\th\n\n2\tb\n3\tc\n\n4\tf\n\n1\th\n\n2\tb\n"
        );
        assert_eq!(
            numbered("-w 1", "\\:\\:\\:\\:\n"),
            "1\t\\:\\:\\:\\:\n",
            "four times is a line"
        );
        assert_eq!(numbered("-w 1", "x\\:\n"), "1\tx\\:\n");
    }

    #[test]
    fn d_gives_other_delimiter_characters() {
        assert_eq!(numbered("-w 1 -d #@", "#@#@\na\n"), "\n1\ta\n");
        assert_eq!(
            numbered("-w 1 -d #", "#:\na\n#:#:\nb\n"),
            "\n \ta\n\n1\tb\n",
            "one is the first, : the second"
        );
    }

    #[test]
    fn l_counts_empty_lines_in_a_row_as_one() {
        assert_eq!(
            numbered("-b a -l 2 -w 1", "\n\n\n\n"),
            " \t\n1\t\n \t\n2\t\n"
        );
        assert_eq!(
            numbered("-b a -l 2 -w 1", "\na\n\n"),
            " \t\n1\ta\n \t\n",
            "a line between starts it again"
        );
        assert_eq!(
            numbered("-b t -l 2 -w 1", "\n\n"),
            " \t\n \t\n",
            "only for a"
        );
    }

    #[test]
    fn what_nl_cannot_take_it_says() {
        let wrong = |text: &str| asked(&words(text)).unwrap_err();
        assert_eq!(wrong("-b x"), "nl: x is no type of numbering\n");
        assert_eq!(
            wrong("-b p\\("),
            "nl: \\(: a parenthesis without its other half\n"
        );
        assert_eq!(wrong("-n zz"), "nl: zz is no format of numbers\n");
        assert_eq!(wrong("-w 0"), "nl: -w wants a number from 1\n");
        assert_eq!(wrong("-w x"), "nl: -w wants a number, not x\n");
        assert_eq!(wrong("-l 0"), "nl: -l wants a number from 1\n");
        assert_eq!(wrong("-d abc"), "nl: -d takes one or two characters\n");
        assert_eq!(wrong("-v"), "nl: -v wants its word\n");
        assert_eq!(wrong("-z"), "nl: -z is no option\n");
        assert_eq!(wrong("/a /b"), "nl: numbers one file\n");
        assert_eq!(asked(&words("/a")).unwrap().file, "/a");
        assert_eq!(asked(&[]).unwrap().file, "-");
        assert_eq!(asked(&words("-- -b")).unwrap().file, "-b");
    }

    /// The section the line `text` starts, with `-d` as `options` say.
    fn starts(options: &str, text: &str) -> Option<usize> {
        let words = words(options);
        let asked = asked(&words).unwrap();
        Numbering::new(&asked).starts(text.as_bytes())
    }

    #[test]
    fn a_delimiter_line_is_the_delimiter_one_to_three_times_and_nothing_else() {
        assert_eq!(starts("", "\\:\\:\\:"), Some(0));
        assert_eq!(starts("", "\\:\\:"), Some(1));
        assert_eq!(starts("", "\\:"), Some(2));
        assert_eq!(starts("", ""), None);
        assert_eq!(starts("", "\\:\\:\\:\\:"), None);
        assert_eq!(starts("", "\\:\\"), None, "a delimiter and a half");
        assert_eq!(starts("", "\\:x"), None);
        assert_eq!(
            starts("", ":\\"),
            None,
            "the right characters in the wrong order"
        );
        assert_eq!(starts("", " \\:"), None);
        assert_eq!(starts("-d ab", "ababab"), Some(0));
        assert_eq!(starts("-d ab", "\\:"), None, "the old one is no longer one");
        assert_eq!(
            starts("-d äö", "äöäö"),
            Some(1),
            "characters past ASCII too"
        );
    }

    #[test]
    fn each_section_numbers_as_its_own_type_says() {
        let text = "\\:\\:\\:\nh1\n\nh2\n\\:\\:\nb1\n\n\\:\nf1\n";
        assert_eq!(
            numbered("-w 1 -h t -b n -f a", text),
            "\n1\th1\n \t\n2\th2\n\n \tb1\n \t\n\n3\tf1\n"
        );
        assert_eq!(
            numbered("-w 1 -h pf -b ph -f pb", text),
            "\n \th1\n \t\n \th2\n\n \tb1\n \t\n\n \tf1\n",
            "an expression of another section's lines matches none"
        );
        assert_eq!(
            numbered("-w 1 -b p1$", "b1\nb2\nc1\n"),
            "1\tb1\n \tb2\n2\tc1\n",
            "anywhere in the line, its end anchored"
        );
    }

    #[test]
    fn lines_with_numbers_have_their_text_after_the_separator() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut roll = |sides: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % 1024).unwrap() % sides
        };
        for _ in 0..200 {
            let lines: Vec<&str> = (0..roll(12))
                .map(|_| ["", "a", "bc", "x y", "\\:\\:"][roll(5)])
                .collect();
            let text: String = lines
                .iter()
                .map(|line| String::from(*line) + "\n")
                .collect();
            let step = 1 + roll(4);
            let first = roll(5);
            let kind = ["a", "t", "pc"][roll(3)];
            let options = alloc::format!("-b {kind} -i {step} -v {first} -w 4 -s |");
            let out = numbered(&options, &text);
            let written: Vec<&str> = out.lines().collect();
            assert_eq!(written.len(), lines.len(), "{options} on {lines:?}");
            let mut want = i64::try_from(first).unwrap();
            for (line, written) in lines.iter().zip(&written) {
                if *line == "\\:\\:" {
                    assert_eq!(*written, "", "a delimiter is an empty line");
                    continue;
                }
                let (number, rest) = written.split_once('|').unwrap();
                assert_eq!(rest, *line, "{options} on {lines:?}");
                assert_eq!(number.len(), 4);
                let numbered = match kind {
                    "a" => true,
                    "t" => !line.is_empty(),
                    _ => line.contains('c'),
                };
                if numbered {
                    assert_eq!(
                        number.trim_start().parse::<i64>(),
                        Ok(want),
                        "{options} on {lines:?}"
                    );
                    want += i64::try_from(step).unwrap();
                } else {
                    assert_eq!(number, "    ");
                }
            }
        }
    }

    #[test]
    fn empty_lines_in_a_row_count_again_in_each_section() {
        assert_eq!(
            numbered("-b a -h a -l 2 -w 1", "\\:\\:\\:\n\n\\:\\:\n\n\n"),
            "\n \t\n\n \t\n1\t\n",
            "a section starts the run of empty lines again"
        );
        assert_eq!(
            numbered("-b a -l 3 -w 1", "\n\n\n\n\n\n"),
            " \t\n \t\n1\t\n \t\n \t\n2\t\n"
        );
    }

    #[test]
    fn options_take_their_word_from_the_rest_of_theirs_or_the_next() {
        let w = words("-ba -nln -w3 -v5 -i2 -s:");
        let a = asked(&w).unwrap();
        assert!(matches!(a.kinds, [Kind::None, Kind::All, Kind::None]));
        assert_eq!(
            (a.format, a.width, a.first, a.step, a.separator.as_str()),
            (Format::Left, 3, 5, 2, ":")
        );
        let w = words("-p -h a -f t /x");
        let a = asked(&w).unwrap();
        assert!(a.keep_counting);
        assert!(matches!(a.kinds, [Kind::All, Kind::Text, Kind::Text]));
        assert_eq!(a.file, "/x");
        let w = words("-pba");
        let a = asked(&w).unwrap();
        assert!(
            a.keep_counting && matches!(a.kinds[1], Kind::All),
            "-p then b with its word"
        );
        assert_eq!(asked(&words("-d x")).unwrap().delimiter, "x:");
        assert_eq!(asked(&words("-dxy")).unwrap().delimiter, "xy");
        assert_eq!(
            asked(&words("-v -3")).unwrap().first,
            -3,
            "a word with a dash as -v's"
        );
    }

    #[test]
    fn a_number_wider_than_its_width_keeps_every_digit() {
        assert_eq!(numbered("-w 2 -v 123 -n rz", "a\n"), "123\ta\n");
        assert_eq!(numbered("-w 2 -v 123 -n ln", "a\n"), "123\ta\n");
        assert_eq!(numbered("-w 4 -v -12 -n rz", "a\n"), "-012\ta\n");
        assert_eq!(numbered("-w 4 -v -12 -n ln", "a\n"), "-12 \ta\n");
        assert_eq!(numbered("-w 4 -v -12", "a\n"), " -12\ta\n");
    }

    #[test]
    fn a_line_too_complex_to_search_is_not_numbered() {
        let line = "a".repeat(40);
        assert_eq!(
            numbered("-w 1 -b p\\(\\(a*\\)*\\)*\\1x", &(line.clone() + "\n")),
            alloc::format!(" \t{line}\n")
        );
    }

    #[test]
    fn the_count_stays_at_the_largest_number_rather_than_wrap() {
        assert_eq!(
            numbered("-w 1 -v 9223372036854775806", "a\nb\nc\n"),
            "9223372036854775806\ta\n9223372036854775807\tb\n9223372036854775807\tc\n"
        );
        assert_eq!(
            numbered("-w 1 -n rz -v -5", "a\n"),
            "-5\ta\n",
            "a sign and no room for zeros"
        );
        assert_eq!(numbered("-w 3 -n rz -v 0", "a\n"), "000\ta\n");
    }

    #[test]
    fn an_empty_separator_puts_the_line_right_after_its_number() {
        let w = [
            String::from("-s"),
            String::new(),
            String::from("-w"),
            String::from("2"),
        ];
        let asked = asked(&w).unwrap();
        let mut numbering = Numbering::new(&asked);
        assert_eq!(numbering.line(b"a"), b" 1a\n");
        assert_eq!(numbering.line(b""), b"  \n");
    }

    #[test]
    fn numbers_an_option_takes_must_be_numbers() {
        let wrong = |text: &str| asked(&words(text)).unwrap_err();
        assert_eq!(wrong("-i x"), "nl: -i wants a number, not x\n");
        assert_eq!(wrong("-l -1"), "nl: -l wants a number, not -1\n");
        assert_eq!(wrong("-v 1.5"), "nl: -v wants a number, not 1.5\n");
        assert_eq!(wrong("-w -3"), "nl: -w wants a number, not -3\n");
        assert_eq!(wrong("-n"), "nl: -n wants its word\n");
        assert_eq!(
            wrong("-n RN"),
            "nl: RN is no format of numbers\n",
            "small letters"
        );
        assert_eq!(wrong("-d"), "nl: -d wants its word\n");
        assert_eq!(wrong("-s"), "nl: -s wants its word\n");
        assert_eq!(wrong("-h"), "nl: -h wants its word\n");
        assert_eq!(wrong("-f pa -h q"), "nl: q is no type of numbering\n");
    }
}
