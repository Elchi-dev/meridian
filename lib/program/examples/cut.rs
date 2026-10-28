//! cut: parts of each line of files or of its standard input, as POSIX's
//! cut has them: `-b` the bytes, `-c` the characters and `-f` the fields
//! a list names, fields split at a tab or at the character `-d` gives,
//! and with `-s` the lines that hold no delimiter left out.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Lines};

program::main!(cut);

/// What a line is cut into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum By {
    Bytes,
    Characters,
    /// Fields, split at `delimiter`, `only` those lines that hold one.
    Fields {
        delimiter: char,
        only: bool,
    },
}

/// What cut was asked for: how to cut, the parts by their numbers from 1,
/// as ranges, and the files, none for the standard input.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    by: By,
    list: Vec<(usize, usize)>,
    files: &'a [String],
}

/// The ranges a list names: numbers and ranges `N-M`, `N-` and `-M`, apart
/// at commas or blanks, counted from 1.
///
/// # Errors
///
/// For a list that names nothing, a 0, a range that goes down, or what is
/// no number.
fn list(text: &str) -> Result<Vec<(usize, usize)>, String> {
    let number = |n: &str| match n.parse::<usize>() {
        Ok(0) => Err(String::from("parts are counted from 1")),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("{text}: no list of numbers")),
    };
    let mut ranges = Vec::new();
    for part in text.split([',', ' ', '\t']).filter(|p| !p.is_empty()) {
        let range = match part.split_once('-') {
            None => {
                let n = number(part)?;
                (n, n)
            }
            Some(("", "")) => return Err(format!("{text}: no list of numbers")),
            Some(("", last)) => (1, number(last)?),
            Some((first, "")) => (number(first)?, usize::MAX),
            Some((first, last)) => (number(first)?, number(last)?),
        };
        if range.1 < range.0 {
            return Err(format!("{part} goes down"));
        }
        ranges.push(range);
    }
    if ranges.is_empty() {
        return Err(String::from("the list names nothing"));
    }
    Ok(ranges)
}

/// Whether part `n`, counted from 1, is among `list`.
fn chosen(list: &[(usize, usize)], n: usize) -> bool {
    list.iter()
        .any(|&(first, last)| (first..=last).contains(&n))
}

/// What cut's words ask of it.
///
/// # Errors
///
/// For an option it does not know or that wants its word, more than one
/// way to cut or none, a list that does not read, or `-d` or `-s` without
/// fields.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut by = None;
    let mut list_text = None;
    let (mut delimiter, mut only) = (None, false);
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
            let taking = matches!(letter, 'b' | 'c' | 'f' | 'd');
            if !taking {
                match letter {
                    's' => only = true,
                    'n' => {}
                    _ => return Err(format!("-{letter} is no option")),
                }
                continue;
            }
            // The rest of the word is what the option takes, or else the
            // next word.
            let after = &word[1 + at + 1..];
            let value = if after.is_empty() {
                let (value, rest) = words
                    .split_first()
                    .ok_or_else(|| format!("-{letter} wants its word"))?;
                words = rest;
                value.as_str()
            } else {
                after
            };
            if letter == 'd' {
                let mut chars = value.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => delimiter = Some(c),
                    _ => return Err(String::from("-d takes one character")),
                }
            } else {
                if by.is_some() {
                    return Err(String::from("cuts by one of -b, -c and -f"));
                }
                by = Some(letter);
                list_text = Some(value);
            }
            break;
        }
    }
    let by = match by {
        Some('b') => By::Bytes,
        Some('c') => By::Characters,
        Some(_) => By::Fields {
            delimiter: delimiter.unwrap_or('\t'),
            only,
        },
        None => return Err(String::from("cuts by one of -b, -c and -f")),
    };
    if !matches!(by, By::Fields { .. }) && (delimiter.is_some() || only) {
        return Err(String::from("-d and -s go with -f"));
    }
    Ok(Asked {
        by,
        list: list(list_text.unwrap_or_default())?,
        files: words,
    })
}

/// The parts of `line` `asked` names, in their order; none for a line
/// `-s` leaves out.
fn cut_line(asked: &Asked<'_>, line: &[u8]) -> Option<Vec<u8>> {
    let bytes = || {
        line.iter()
            .enumerate()
            .filter(|&(i, _)| chosen(&asked.list, i + 1))
            .map(|(_, &b)| b)
            .collect()
    };
    match asked.by {
        By::Bytes => Some(bytes()),
        // A line that is no UTF-8 is cut by its bytes.
        By::Characters => {
            let Ok(text) = core::str::from_utf8(line) else {
                return Some(bytes());
            };
            let kept: String = text
                .chars()
                .enumerate()
                .filter(|&(i, _)| chosen(&asked.list, i + 1))
                .map(|(_, c)| c)
                .collect();
            Some(kept.into_bytes())
        }
        By::Fields { delimiter, only } => {
            let mut room = [0u8; 4];
            let delimiter = delimiter.encode_utf8(&mut room).as_bytes();
            if !line.windows(delimiter.len()).any(|w| w == delimiter) {
                return (!only).then(|| line.to_vec());
            }
            let fields = split(line, delimiter);
            let kept: Vec<&[u8]> = fields
                .into_iter()
                .enumerate()
                .filter(|&(i, _)| chosen(&asked.list, i + 1))
                .map(|(_, field)| field)
                .collect();
            Some(kept.join(delimiter))
        }
    }
}

/// `line` split at each `delimiter`.
fn split<'l>(line: &'l [u8], delimiter: &[u8]) -> Vec<&'l [u8]> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + delimiter.len() <= line.len() {
        if &line[i..i + delimiter.len()] == delimiter {
            fields.push(&line[start..i]);
            i += delimiter.len();
            start = i;
        } else {
            i += 1;
        }
    }
    fields.push(&line[start..]);
    fields
}

/// cut at work on a text that comes in pieces.
struct Cutter<'a> {
    asked: &'a Asked<'a>,
    lines: Lines,
}

impl Cutter<'_> {
    /// What cut writes for the lines `piece` ends.
    fn feed(&mut self, piece: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let asked = self.asked;
        self.lines.feed(piece, |line| {
            put(asked, line, &mut out);
            true
        });
        out
    }

    /// What cut writes for the last line, ended or not.
    fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        self.lines.finish(|line| put(self.asked, line, &mut out));
        out
    }
}

/// Puts what cut writes for `line` onto `out`.
fn put(asked: &Asked<'_>, line: &[u8], out: &mut Vec<u8>) {
    if let Some(kept) = cut_line(asked, line) {
        out.extend_from_slice(&kept);
        out.push(b'\n');
    }
}

#[cfg_attr(test, allow(dead_code))]
fn cut(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&format!("cut: {why}\n"));
            return 2;
        }
    };
    let mut failed = false;
    for path in text::sources(asked.files) {
        let mut cutter = Cutter {
            asked: &asked,
            lines: Lines::default(),
        };
        let read = text::pieces(path, |piece| {
            program::write_bytes(&cutter.feed(piece)).is_ok()
        });
        failed |= program::write_bytes(&cutter.finish()).is_err();
        if let Err(e) = read {
            failed = true;
            let _ = program::write_error(&text::trouble("cut", path.unwrap_or("-"), e));
        }
    }
    u64::from(failed)
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

    /// What cut writes for `pieces`, as `asked` asks.
    fn cut_text(asked: &Asked<'_>, pieces: &[&[u8]]) -> Vec<u8> {
        let mut cutter = Cutter {
            asked,
            lines: Lines::default(),
        };
        let mut out = Vec::new();
        for piece in pieces {
            out.extend(cutter.feed(piece));
        }
        out.extend(cutter.finish());
        out
    }

    /// What cut with the words `args` writes for `input`.
    fn run(args: &str, input: &str) -> String {
        let args = words(args);
        let asked = asked(&args).unwrap();
        String::from_utf8(cut_text(&asked, &[input.as_bytes()])).unwrap()
    }

    #[test]
    fn a_list_names_numbers_and_ranges() {
        assert_eq!(list("1,3-4"), Ok(Vec::from([(1, 1), (3, 4)])));
        assert_eq!(list("2-"), Ok(Vec::from([(2, usize::MAX)])));
        assert_eq!(list("-3"), Ok(Vec::from([(1, 3)])));
        assert_eq!(list("5 1"), Ok(Vec::from([(5, 5), (1, 1)])));
        assert_eq!(list("0"), Err(String::from("parts are counted from 1")));
        assert_eq!(list("3-1"), Err(String::from("3-1 goes down")));
        assert_eq!(list("a"), Err(String::from("a: no list of numbers")));
        assert_eq!(list("-"), Err(String::from("-: no list of numbers")));
        assert_eq!(list(""), Err(String::from("the list names nothing")));
        assert_eq!(list(",,"), Err(String::from("the list names nothing")));
    }

    #[test]
    fn a_part_is_chosen_by_any_range() {
        let ranges = [(1, 1), (3, 4), (9, usize::MAX)];
        let kept: Vec<usize> = (1..=12).filter(|&n| chosen(&ranges, n)).collect();
        assert_eq!(kept, [1, 3, 4, 9, 10, 11, 12]);
        assert!(!chosen(&[], 1));
    }

    #[test]
    fn its_words_say_how_to_cut() {
        let args = words("-f 2 -d : /a");
        let a = asked(&args).unwrap();
        assert_eq!(
            a.by,
            By::Fields {
                delimiter: ':',
                only: false
            }
        );
        assert_eq!(a.files, ["/a"]);
        let args = words("-sd, -f1");
        assert_eq!(
            asked(&args).unwrap().by,
            By::Fields {
                delimiter: ',',
                only: true
            }
        );
        assert_eq!(asked(&words("-c1-3")).unwrap().by, By::Characters);
        assert_eq!(asked(&words("-n -b 1")).unwrap().by, By::Bytes);
        assert_eq!(asked(&words("-f 1 -- -x")).unwrap().files, ["-x"]);
        assert_eq!(
            asked(&words("-f1")).unwrap().by,
            By::Fields {
                delimiter: '\t',
                only: false
            }
        );
        for (args, why) in [
            ("", "cuts by one of -b, -c and -f"),
            ("-b 1 -c 1", "cuts by one of -b, -c and -f"),
            ("-x", "-x is no option"),
            ("-f", "-f wants its word"),
            ("-d", "-d wants its word"),
            ("-f 1 -d ab", "-d takes one character"),
            ("-c 1 -d :", "-d and -s go with -f"),
            ("-b 1 -s", "-d and -s go with -f"),
            ("-f 0", "parts are counted from 1"),
        ] {
            assert_eq!(asked(&words(args)), Err(String::from(why)), "{args:?}");
        }
    }

    #[test]
    fn b_and_c_keep_the_bytes_and_characters_named() {
        assert_eq!(run("-b 1,3", "abcdef\nxyz\n"), "ac\nxz\n");
        assert_eq!(run("-b 4-", "abcdef\nab\n"), "def\n\n");
        assert_eq!(run("-c -2", "日本語\n"), "日本\n");
        assert_eq!(
            run("-b -3", "日本語\n"),
            "日\n",
            "three bytes are one character there"
        );
        assert_eq!(run("-c 2", "aé\n"), "é\n");
        assert_eq!(run("-c 1-", ""), "");
    }

    #[test]
    fn f_keeps_the_fields_named() {
        assert_eq!(run("-f 2 -d :", "a:b:c\nx:y\n"), "b\ny\n");
        assert_eq!(run("-f 1,3 -d :", "a:b:c\n"), "a:c\n");
        assert_eq!(run("-f 2-", "a\tb\tc\n"), "b\tc\n");
        assert_eq!(run("-f 5 -d :", "a:b\n"), "\n", "a field that is not there");
        assert_eq!(run("-f 2 -d :", "a::c\n"), "\n", "an empty field");
        assert_eq!(run("-f 1 -d é", "aébéc\n"), "a\n");
    }

    #[test]
    fn a_line_with_no_delimiter_stays_unless_s() {
        assert_eq!(run("-f 2 -d :", "whole\na:b\n"), "whole\nb\n");
        assert_eq!(run("-s -f 2 -d :", "whole\na:b\n"), "b\n");
    }

    #[test]
    fn a_line_that_is_no_text_is_cut_by_its_bytes() {
        let args = words("-c 2");
        let asked = asked(&args).unwrap();
        assert_eq!(
            cut_line(&asked, &[b'a', 0xff, b'b']),
            Some(Vec::from([0xff]))
        );
    }

    #[test]
    fn a_line_cut_between_pieces_comes_whole() {
        let args = words("-f 2 -d :");
        let asked = asked(&args).unwrap();
        let out = cut_text(&asked, &[b"a:b", b"b:c\nx", b":y"]);
        assert_eq!(out, b"bb\ny\n");
    }

    #[test]
    fn a_field_split_keeps_empty_ones() {
        assert_eq!(split(b"a::b", b":"), [&b"a"[..], b"", b"b"]);
        assert_eq!(split(b":", b":"), [&b""[..], b""]);
        assert_eq!(split(b"abc", b":"), [&b"abc"[..]]);
        assert_eq!(
            split("aébéc".as_bytes(), "é".as_bytes()),
            [&b"a"[..], b"b", b"c"]
        );
    }

    #[test]
    fn a_list_takes_ranges_that_overlap_or_come_in_any_order() {
        assert_eq!(
            run("-b 3,1", "abc\n"),
            "ac\n",
            "in the line's order, not the list's"
        );
        assert_eq!(run("-b 1-3,2-4", "abcde\n"), "abcd\n", "each byte once");
        assert_eq!(run("-c 2,2,2", "xyz\n"), "y\n");
        assert_eq!(run("-f 3,1 -d ,", "a,b,c\n"), "a,c\n");
        assert_eq!(run("-b 1-1", "q\n"), "q\n");
    }

    #[test]
    fn options_take_their_word_joined_or_apart() {
        for args in ["-f2 -d:", "-f 2 -d :", "-d: -f2", "-d : -f 2", "-sd: -f2"] {
            let parsed = words(args);
            let a = asked(&parsed).unwrap();
            assert_eq!(a.list, [(2, 2)], "{args}");
            assert!(matches!(a.by, By::Fields { delimiter: ':', .. }), "{args}");
        }
        let args = words("-b1-2 a b");
        assert_eq!(asked(&args).unwrap().files, ["a", "b"]);
        let args = words("-nb 1");
        assert_eq!(asked(&args).unwrap().by, By::Bytes, "-n is let be");
        let args = words("-c 1 -");
        assert_eq!(asked(&args).unwrap().files, ["-"], "a dash alone is a file");
    }

    #[test]
    fn a_delimiter_may_be_any_one_character() {
        assert_eq!(run("-f 2 -d .", "a.b.c\n"), "b\n");
        assert_eq!(run("-f 1,2 -d |", "x|y|z\n"), "x|y\n");
        assert_eq!(run("-f 2 -d -", "a-b\n"), "b\n");
        assert_eq!(run("-f 2 -d 日", "a日b\n"), "b\n");
        let args = vec![
            String::from("-f"),
            String::from("2"),
            String::from("-d"),
            String::from(" "),
        ];
        let a = asked(&args).unwrap();
        assert_eq!(
            cut_line(&a, b"a b c"),
            Some(b"b".to_vec()),
            "a blank, in a word of its own"
        );
        let args = vec![String::from("-f1"), String::from("-d")];
        assert_eq!(asked(&args), Err(String::from("-d wants its word")));
    }

    #[test]
    fn fields_at_the_ends_of_a_line_are_empty() {
        assert_eq!(run("-f 1 -d :", ":b\n"), "\n");
        assert_eq!(run("-f 2 -d :", "a:\n"), "\n");
        assert_eq!(
            run("-f 1- -d :", ":a:\n"),
            ":a:\n",
            "all of them, and the delimiters between"
        );
        assert_eq!(
            run("-f 2,3 -d :", "a:b\n"),
            "b\n",
            "what is not there is left out"
        );
    }

    #[test]
    fn empty_lines_and_a_last_line_without_its_end() {
        assert_eq!(run("-c 1", "\n\n"), "\n\n");
        assert_eq!(
            run("-f 1 -d : -s", "\n"),
            "",
            "an empty line holds no delimiter"
        );
        assert_eq!(run("-c 1-2", "abc"), "ab\n", "the last line gets its end");
        assert_eq!(run("-f 2 -d :", "a:b"), "b\n");
    }

    #[test]
    fn many_lines_in_many_pieces() {
        let line = "name:x:1000:1000:Somebody:/home/name:/bin/sh\n";
        let text = line.repeat(50);
        let bytes = text.as_bytes();
        let pieces: Vec<&[u8]> = bytes.chunks(11).collect();
        let args = words("-f 1,7 -d :");
        let asked = asked(&args).unwrap();
        let out = String::from_utf8(cut_text(&asked, &pieces)).unwrap();
        assert_eq!(out, "name:/bin/sh\n".repeat(50));
    }

    #[test]
    fn b_keeps_bytes_even_where_they_cut_a_character() {
        let args = words("-b 2");
        let cut = asked(&args).unwrap();
        assert_eq!(cut_line(&cut, "é".as_bytes()), Some(Vec::from([0xa9])));
        assert_eq!(cut_line(&cut, b""), Some(Vec::new()));
        assert_eq!(cut_line(&cut, b"a"), Some(Vec::new()), "past its end");
        let args = words("-b 1,3-");
        let cut = asked(&args).unwrap();
        assert_eq!(cut_line(&cut, b"abcdef"), Some(b"acdef".to_vec()));
    }

    #[test]
    fn s_leaves_out_only_lines_with_no_delimiter() {
        let args = words("-s -d , -f 1");
        let cut = asked(&args).unwrap();
        assert_eq!(cut_line(&cut, b"a,b"), Some(b"a".to_vec()));
        assert_eq!(
            cut_line(&cut, b","),
            Some(Vec::new()),
            "a delimiter alone is two fields"
        );
        assert_eq!(cut_line(&cut, b"ab"), None);
        assert_eq!(cut_line(&cut, b""), None);
        let args = words("-d , -f 1");
        let cut = asked(&args).unwrap();
        assert_eq!(
            cut_line(&cut, b"ab"),
            Some(b"ab".to_vec()),
            "the whole line without -s"
        );
    }

    #[test]
    fn its_words_take_files_after_the_options() {
        let args = words("-f 1 /a /b /c");
        assert_eq!(asked(&args).unwrap().files, ["/a", "/b", "/c"]);
        let args = words("-c1 -- -f");
        assert_eq!(asked(&args).unwrap().files, ["-f"]);
        let args = words("-c 1 /a -b 2");
        assert_eq!(
            asked(&args).unwrap().files,
            ["/a", "-b", "2"],
            "options come first"
        );
        let args = words("-c");
        assert_eq!(asked(&args), Err(String::from("-c wants its word")));
        let args = words("-f 1 -f 2");
        assert_eq!(
            asked(&args),
            Err(String::from("cuts by one of -b, -c and -f"))
        );
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn random_lines_keep_the_fields_a_list_names_in_their_order() {
        let mut seed = 0x6a09_e667_f3bc_c908_u64;
        for _ in 0..300 {
            let lines: Vec<String> = (0..shuffled(&mut seed) % 8)
                .map(|_| {
                    (0..shuffled(&mut seed) % 5)
                        .map(|f| alloc::format!("f{f}"))
                        .collect::<Vec<_>>()
                        .join(":")
                })
                .collect();
            let text = lines.iter().fold(String::new(), |all, l| all + l + "\n");
            let first = 1 + usize::try_from(shuffled(&mut seed) % 4).unwrap();
            let last = first + usize::try_from(shuffled(&mut seed) % 3).unwrap();
            let want = lines.iter().fold(String::new(), |all, line| {
                if !line.contains(':') {
                    return all + line + "\n";
                }
                let kept: Vec<&str> = line
                    .split(':')
                    .enumerate()
                    .filter(|(at, _)| (first..=last).contains(&(at + 1)))
                    .map(|(_, f)| f)
                    .collect();
                all + &kept.join(":") + "\n"
            });
            assert_eq!(
                run(&alloc::format!("-d : -f {first}-{last}"), &text),
                want,
                "{text:?}"
            );
            let delimited =
                lines
                    .iter()
                    .filter(|l| l.contains(':'))
                    .fold(String::new(), |all, l| {
                        let kept: Vec<&str> = l
                            .split(':')
                            .skip(first - 1)
                            .take(last - first + 1)
                            .collect();
                        all + &kept.join(":") + "\n"
                    });
            assert_eq!(
                run(&alloc::format!("-s -d : -f {first}-{last}"), &text),
                delimited
            );
        }
    }

    #[test]
    fn random_lines_keep_the_bytes_a_list_names() {
        let mut seed = 0xbb67_ae85_84ca_a73b_u64;
        for _ in 0..300 {
            let lines: Vec<String> = (0..shuffled(&mut seed) % 8)
                .map(|_| {
                    (0..shuffled(&mut seed) % 9)
                        .map(|n| char::from(b'a' + u8::try_from(n).unwrap()))
                        .collect()
                })
                .collect();
            let text = lines.iter().fold(String::new(), |all, l| all + l + "\n");
            let from = 1 + usize::try_from(shuffled(&mut seed) % 5).unwrap();
            let want = lines.iter().fold(String::new(), |all, l| {
                all + l.get(from - 1..).unwrap_or("") + "\n"
            });
            assert_eq!(run(&alloc::format!("-b {from}-"), &text), want);
            let first = lines.iter().fold(String::new(), |all, l| {
                all + l.get(..1).unwrap_or("") + "\n"
            });
            assert_eq!(run("-c 1", &text), first);
        }
    }
}
