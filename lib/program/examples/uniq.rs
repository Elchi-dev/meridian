//! uniq: writes the lines of a file, or of its standard input, each run
//! of lines that compare the same once, as POSIX's uniq does: with `-c`
//! after how many times it came, with `-d` only runs of more than one
//! line and with `-u` only lines alone. `-f` leaves out so many fields of
//! each line from the comparison, a field being blanks and what is not
//! blank after them, and `-s` so many characters after those. The first
//! line of a run is written, into the output file when one follows the
//! input, and `-` for the input is the standard input.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::file::{CREATE_ALWAYS, WRITE};
use program::text::{self, Lines, Pieces, Source};
use program::{Error, Handle};

program::main!(uniq);

/// What uniq's words ask for.
#[derive(Debug, PartialEq)]
struct Asked<'a> {
    counting: bool,
    /// Whether only runs of more than one line are written, and whether
    /// only lines alone.
    repeated: bool,
    alone: bool,
    fields: usize,
    chars: usize,
    /// The input, none for the standard input, and the output, none for
    /// the standard output.
    input: Option<&'a str>,
    output: Option<&'a str>,
}

/// A count as -f and -s take it.
fn count(letter: char, value: &str) -> Result<usize, String> {
    Some(value)
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .map(|v| v.parse::<usize>().unwrap_or(usize::MAX))
        .ok_or_else(|| format!("uniq: -{letter} wants a number, not {value}\n"))
}

/// What uniq's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        counting: false,
        repeated: false,
        alone: false,
        fields: 0,
        chars: 0,
        input: None,
        output: None,
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
                'c' => asked.counting = true,
                'd' => asked.repeated = true,
                'u' => asked.alone = true,
                'f' | 's' => {
                    let after = &letters[at + 1..];
                    let value = if after.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("uniq: -{letter} wants its word\n"))?;
                        words = rest;
                        value.as_str()
                    } else {
                        after
                    };
                    if letter == 'f' {
                        asked.fields = count('f', value)?;
                    } else {
                        asked.chars = count('s', value)?;
                    }
                    break;
                }
                _ => return Err(format!("uniq: -{letter} is no option\n")),
            }
        }
    }
    match words {
        [] => {}
        [input] => asked.input = Some(input.as_str()),
        [input, output] => {
            asked.input = Some(input.as_str());
            asked.output = Some(output.as_str());
        }
        _ => return Err(String::from("uniq: takes an input and an output at most\n")),
    }
    asked.input = asked.input.filter(|&input| input != "-");
    Ok(asked)
}

/// What of `line` is compared: what is left after the first `fields`
/// fields, each blanks and then what is not blank, and after the first
/// `chars` characters past them.
fn compared(line: &[u8], fields: usize, chars: usize) -> &[u8] {
    let blank = |b: &u8| matches!(b, b' ' | b'\t');
    let mut rest = line;
    for _ in 0..fields {
        if rest.is_empty() {
            break;
        }
        let blanks = rest.iter().take_while(|b| blank(b)).count();
        let word = rest[blanks..].iter().take_while(|b| !blank(b)).count();
        rest = &rest[blanks + word..];
    }
    let mut at = 0;
    for _ in 0..chars {
        if at == rest.len() {
            break;
        }
        at += 1;
        // A byte that goes on with the character is part of it.
        while rest.get(at).is_some_and(|b| b & 0xc0 == 0x80) {
            at += 1;
        }
    }
    &rest[at..]
}

/// The lines of a text that comes in pieces, each run of lines that
/// compare the same once.
struct Runs<'a> {
    asked: &'a Asked<'a>,
    /// The first line of the run, and how many lines it has so far.
    last: Option<(Vec<u8>, usize)>,
    lines: Lines,
}

impl<'a> Runs<'a> {
    fn new(asked: &'a Asked<'a>) -> Runs<'a> {
        Runs {
            asked,
            last: None,
            lines: Lines::default(),
        }
    }

    /// Takes the next `piece` of the text, and gives `out` each run it
    /// ends.
    fn feed(&mut self, piece: &[u8], out: &mut impl FnMut(&[u8])) {
        let mut lines = core::mem::take(&mut self.lines);
        lines.feed(piece, |line| {
            self.take(line, out);
            true
        });
        self.lines = lines;
    }

    fn take(&mut self, line: &[u8], out: &mut impl FnMut(&[u8])) {
        let (fields, chars) = (self.asked.fields, self.asked.chars);
        match &mut self.last {
            Some((first, times))
                if compared(first, fields, chars) == compared(line, fields, chars) =>
            {
                *times += 1;
            }
            _ => {
                self.end_run(out);
                self.last = Some((line.to_vec(), 1));
            }
        }
    }

    /// Gives `out` the run that ended, when it is one to write.
    fn end_run(&mut self, out: &mut impl FnMut(&[u8])) {
        let Some((line, times)) = self.last.take() else {
            return;
        };
        if self.asked.repeated && times == 1 || self.asked.alone && times > 1 {
            return;
        }
        if self.asked.counting {
            out(format!("{times} ").as_bytes());
        }
        out(&line);
        out(b"\n");
    }

    /// Ends the text, and gives `out` the last run.
    fn finish(mut self, out: &mut impl FnMut(&[u8])) {
        let lines = core::mem::take(&mut self.lines);
        lines.finish(|line| self.take(line, out));
        self.end_run(out);
    }
}

/// Runs uniq asked as `asked` on what `input` reads, writing with
/// `write`; answers whether every write went through.
///
/// # Errors
///
/// When the input cannot be read.
fn written<P: Pieces>(
    asked: &Asked,
    input: &mut P,
    write: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<bool, Error> {
    let mut out = text::Out::new(write);
    let mut runs = Runs::new(asked);
    let mut piece = Vec::new();
    while input.more(&mut piece)? {
        runs.feed(&piece, &mut |bytes| out.put(bytes));
        piece.clear();
    }
    runs.finish(&mut |bytes| out.put(bytes));
    Ok(out.finish())
}

#[cfg_attr(test, allow(dead_code))]
fn uniq(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    // The input opens first, so an output file is left as it was when
    // there is nothing to read.
    let named = asked.input.unwrap_or("-");
    let mut input = match Source::open(named) {
        Ok(input) => input,
        Err(e) => {
            let _ = program::write_error(&text::trouble("uniq", named, e));
            return 2;
        }
    };
    let ran = match asked.output {
        Some(path) => match program::open_with(path, WRITE, CREATE_ALWAYS) {
            Ok(file) => {
                let mut at = 0;
                let ran = written(&asked, &mut input, |bytes| {
                    program::write_file(file, at, bytes)?;
                    at += bytes.len() as u64;
                    Ok(())
                });
                let _ = program::close(file);
                ran
            }
            Err(e) => {
                let _ = program::write_error(&text::trouble("uniq", path, e));
                input.close();
                return 2;
            }
        },
        None => written(&asked, &mut input, program::write_bytes),
    };
    input.close();
    match ran {
        Ok(whole) => u64::from(!whole),
        Err(e) => {
            let _ = program::write_error(&text::trouble("uniq", named, e));
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    /// What uniq asked with `options` writes for `pieces`, one after
    /// another.
    fn uniq_bytes(options: &str, pieces: &[&[u8]]) -> Vec<u8> {
        let given = words(options);
        let asked = asked(&given).unwrap();
        let mut runs = Runs::new(&asked);
        let mut out = Vec::new();
        for piece in pieces {
            runs.feed(piece, &mut |bytes| out.extend_from_slice(bytes));
        }
        runs.finish(&mut |bytes| out.extend_from_slice(bytes));
        out
    }

    fn uniq_of(options: &str, pieces: &[&str]) -> String {
        let pieces: Vec<&[u8]> = pieces.iter().map(|p| p.as_bytes()).collect();
        String::from_utf8(uniq_bytes(options, &pieces)).unwrap()
    }

    fn runs(counting: bool, pieces: &[&str]) -> String {
        uniq_of(if counting { "-c" } else { "" }, pieces)
    }

    const RUNS: &str = "a\na\nb\nc\nc\nc\nd\n";

    #[test]
    fn options_and_the_input_and_output() {
        let given = words("-cdu -f 2 -s3 in out");
        assert_eq!(
            asked(&given).unwrap(),
            Asked {
                counting: true,
                repeated: true,
                alone: true,
                fields: 2,
                chars: 3,
                input: Some("in"),
                output: Some("out"),
            }
        );
        let given = words("");
        let plain = asked(&given).unwrap();
        assert!(!plain.counting && !plain.repeated && !plain.alone);
        assert_eq!(
            (plain.fields, plain.chars, plain.input, plain.output),
            (0, 0, None, None)
        );
        let given = words("- out");
        assert_eq!(
            asked(&given).unwrap().input,
            None,
            "a dash is the standard input"
        );
        let given = words("-- -c");
        assert_eq!(asked(&given).unwrap().input, Some("-c"));
        let given = words("-f 99999999999999999999999");
        assert_eq!(asked(&given).unwrap().fields, usize::MAX);
        let wrong = |options: &str| asked(&words(options)).unwrap_err();
        assert_eq!(wrong("-x"), "uniq: -x is no option\n");
        assert_eq!(wrong("-cx"), "uniq: -x is no option\n");
        assert_eq!(wrong("-f"), "uniq: -f wants its word\n");
        assert_eq!(wrong("-s a"), "uniq: -s wants a number, not a\n");
        assert_eq!(wrong("-f -1"), "uniq: -f wants a number, not -1\n");
        assert_eq!(
            wrong("a b c"),
            "uniq: takes an input and an output at most\n"
        );
    }

    #[test]
    fn a_line_that_comes_again_right_after_itself_comes_once() {
        assert_eq!(runs(false, &["a\na\nb\na\n"]), "a\nb\na\n");
        assert_eq!(runs(false, &["a\nb\nc\n"]), "a\nb\nc\n");
        assert_eq!(runs(false, &["x\nx"]), "x\n", "the last without its end");
        assert_eq!(runs(false, &["x\n", "x\n", "y"]), "x\ny\n");
        assert_eq!(runs(false, &["\n\n\na\n"]), "\na\n", "empty lines too");
        assert_eq!(runs(false, &[]), "");
        assert_eq!(runs(false, &["A\na\n"]), "A\na\n", "capitals differ");
    }

    #[test]
    fn a_run_goes_on_however_the_pieces_cut_it() {
        let text = "aa\naa\naa\nb\n";
        let one_by_one: Vec<String> = text.chars().map(String::from).collect();
        let pieces: Vec<&str> = one_by_one.iter().map(String::as_str).collect();
        assert_eq!(runs(true, &pieces), "3 aa\n1 b\n");
        assert_eq!(
            uniq_bytes("", &[&[0xff, b'\n', 0xff, b'\n', 0xfe, b'\n']]),
            [0xff, b'\n', 0xfe, b'\n'],
            "lines of bytes, not text"
        );
        let many = "x\n".repeat(1000);
        assert_eq!(runs(true, &[&many]), "1000 x\n");
    }

    #[test]
    fn with_a_count_each_line_says_how_often_it_came() {
        assert_eq!(runs(true, &["a\na\nb\na\n"]), "2 a\n1 b\n1 a\n");
        assert_eq!(runs(true, &["z\n", "z\nz"]), "3 z\n");
        assert_eq!(runs(true, &[]), "");
    }

    #[test]
    fn d_writes_the_runs_and_u_the_lines_alone() {
        assert_eq!(uniq_of("-d", &[RUNS]), "a\nc\n");
        assert_eq!(uniq_of("-u", &[RUNS]), "b\nd\n");
        assert_eq!(uniq_of("-c -d", &[RUNS]), "2 a\n3 c\n");
        assert_eq!(uniq_of("-cu", &[RUNS]), "1 b\n1 d\n");
        assert_eq!(uniq_of("-d -u", &[RUNS]), "", "nothing is both");
        assert_eq!(uniq_of("-u", &["x"]), "x\n");
        assert_eq!(uniq_of("-d", &["x"]), "");
    }

    #[test]
    fn f_and_s_leave_the_start_of_each_line_out_of_the_comparison() {
        assert_eq!(
            uniq_of("-f 2", &["1 x a\n2 y a\n3 z b\n"]),
            "1 x a\n3 z b\n"
        );
        assert_eq!(uniq_of("-s 1", &["xa\nya\nzb\n"]), "xa\nzb\n");
        assert_eq!(
            uniq_of("-f 1", &[" 1 a\n  2 a\n"]),
            " 1 a\n",
            "blanks belong to the field after them"
        );
        assert_eq!(uniq_of("-f 1 -s 1", &["k ab\nk ac\n"]), "k ab\nk ac\n");
        assert_eq!(
            uniq_of("-s 1", &["\u{e9}a\n\u{fc}a\n"]),
            "\u{e9}a\n",
            "a character, not a byte"
        );
        assert_eq!(
            uniq_of("-c -f 5", &["a b\nc\n"]),
            "2 a b\n",
            "past the last field all compare the same"
        );
        assert_eq!(uniq_of("-s 9", &["abc\nxyz\n"]), "abc\n");
    }

    #[test]
    fn what_is_compared_of_a_line() {
        assert_eq!(compared(b"a b c", 0, 0), b"a b c");
        assert_eq!(compared(b"a b c", 1, 0), b" b c");
        assert_eq!(compared(b"a b c", 2, 0), b" c");
        assert_eq!(compared(b"a b c", 3, 0), b"");
        assert_eq!(compared(b"a\tb", 1, 0), b"\tb");
        assert_eq!(compared(b"  a  b", 1, 0), b"  b");
        assert_eq!(compared(b"a b", 1, 1), b"b");
        assert_eq!(compared(b"abc", 0, 2), b"c");
        assert_eq!(compared("\u{1f600}\u{e9}z".as_bytes(), 0, 2), b"z");
        assert_eq!(compared(b"ab", usize::MAX, usize::MAX), b"");
        assert_eq!(
            compared(b"\xbf\xbfa", 0, 1),
            b"a",
            "bytes going on with nothing ride along"
        );
    }

    #[test]
    fn random_lines_come_out_as_their_runs_whatever_the_pieces() {
        let mut seed = 0x3c6e_f372_fe94_f82b_u64;
        for _ in 0..300 {
            let mut lines = Vec::new();
            for _ in 0..seed % 30 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                lines.push(["a", "b", "", "aa"][usize::try_from(seed % 4).unwrap()]);
            }
            let text: String = lines.iter().fold(String::new(), |all, l| all + l + "\n");
            let mut runs_of: Vec<(usize, &str)> = Vec::new();
            for line in &lines {
                match runs_of.last_mut() {
                    Some((count, last)) if last == line => *count += 1,
                    _ => runs_of.push((1, line)),
                }
            }
            let plain = runs_of
                .iter()
                .fold(String::new(), |all, (_, l)| all + l + "\n");
            let counted = runs_of.iter().fold(String::new(), |all, (n, l)| {
                all + &n.to_string() + " " + l + "\n"
            });
            let size = 1 + usize::try_from(seed % 5).unwrap();
            let pieces: Vec<&str> = text
                .as_bytes()
                .chunks(size)
                .map(|c| core::str::from_utf8(c).unwrap())
                .collect();
            assert_eq!(runs(false, &pieces), plain, "{text:?}");
            assert_eq!(runs(true, &pieces), counted, "{text:?}");
            assert_eq!(runs_of.iter().map(|(n, _)| n).sum::<usize>(), lines.len());
        }
    }

    /// Pieces of a text, and then an error if one is given.
    struct Cut(alloc::collections::VecDeque<Vec<u8>>, Option<Error>);

    impl Pieces for Cut {
        fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
            match self.0.pop_front() {
                Some(piece) => {
                    held.extend_from_slice(&piece);
                    Ok(true)
                }
                None => self.1.map_or(Ok(false), Err),
            }
        }
    }

    #[test]
    fn a_whole_run_reads_its_input_and_writes_through_the_writer() {
        let given = words("-c");
        let asked = asked(&given).unwrap();
        let mut input = Cut(
            [b"a\na".to_vec(), b"\nb\n".to_vec()].into_iter().collect(),
            None,
        );
        let mut writes: Vec<Vec<u8>> = Vec::new();
        let whole = written(&asked, &mut input, |bytes| {
            writes.push(bytes.to_vec());
            Ok(())
        });
        assert_eq!(whole, Ok(true));
        assert_eq!(writes.concat(), b"2 a\n1 b\n");
        let mut input = Cut([b"x\n".to_vec()].into_iter().collect(), None);
        let refused = written(&asked, &mut input, |_| Err(Error::Closed));
        assert_eq!(refused, Ok(false), "a write that went wrong");
        let mut input = Cut(
            [b"x\n".to_vec()].into_iter().collect(),
            Some(Error::NotAFile),
        );
        let unread = written(&asked, &mut input, |_| Ok(()));
        assert_eq!(unread, Err(Error::NotAFile));
    }

    #[test]
    fn every_run_is_either_repeated_or_alone() {
        let mut seed = 0x1405_7b7e_f767_814f_u64;
        for _ in 0..300 {
            let mut text = String::new();
            for _ in 0..seed % 25 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push_str(
                    ["1 a\n", "2 a\n", "1 b\n", "x\n"][usize::try_from(seed % 4).unwrap()],
                );
            }
            for skip in ["", "-f 1", "-s 1"] {
                let plain = uniq_of(skip, &[&text]);
                let repeated = uniq_of(&alloc::format!("{skip} -d"), &[&text]);
                let alone = uniq_of(&alloc::format!("{skip} -u"), &[&text]);
                assert_eq!(
                    plain.lines().count(),
                    repeated.lines().count() + alone.lines().count(),
                    "{skip} {text:?}"
                );
                let counted = uniq_of(&alloc::format!("{skip} -c -d"), &[&text]);
                assert!(
                    counted.lines().all(|line| line
                        .split(' ')
                        .next()
                        .unwrap()
                        .parse::<usize>()
                        .unwrap()
                        > 1)
                );
                let total: usize = uniq_of(&alloc::format!("{skip} -c"), &[&text])
                    .lines()
                    .map(|line| line.split(' ').next().unwrap().parse::<usize>().unwrap())
                    .sum();
                assert_eq!(total, text.lines().count());
            }
        }
    }
}
