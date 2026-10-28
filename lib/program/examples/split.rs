//! split: cuts a file, or its standard input, into files of so many
//! lines, a thousand unless `-l` says otherwise, or with `-b` of so many
//! bytes, times 1024 after a k and 1048576 after an m, as POSIX's split
//! does. The files are named after the name it is given, `x` without one,
//! and a suffix of `-a` letters, two unless told otherwise, from aa on.
//! The last file holds what is left. When the suffixes run out before the
//! input does, split ends with 1 after the last name's file. `-` is the
//! standard input, and an empty input makes no file.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use abi::file::{CREATE_ALWAYS, WRITE};
use program::{Error, Handle, text};

program::main!(split);

/// How much each file takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Size {
    Lines(u64),
    Bytes(u64),
}

/// What split's words ask for.
#[derive(Debug, PartialEq)]
struct Asked<'a> {
    size: Size,
    suffix: usize,
    input: &'a str,
    name: &'a str,
}

/// A positive number as `-l` and `-a` take it.
fn count(letter: char, value: &str) -> Result<u64, String> {
    Some(value)
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("split: -{letter} wants a number from 1, not {value}\n"))
}

/// A size in bytes as `-b` takes it: a number, times 1024 after a k and
/// 1048576 after an m.
fn bytes(value: &str) -> Result<u64, String> {
    let (number, unit) = match value.as_bytes().last() {
        Some(b'k') => (&value[..value.len() - 1], 1024),
        Some(b'm') => (&value[..value.len() - 1], 1 << 20),
        _ => (value, 1),
    };
    count('b', number)
        .ok()
        .and_then(|n| n.checked_mul(unit))
        .ok_or_else(|| format!("split: -b wants a size from 1, not {value}\n"))
}

/// What split's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut lines = None;
    let mut size = None;
    let mut suffix = 2;
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
        let letter = letters.chars().next().unwrap_or('-');
        if !"lba".contains(letter) {
            return Err(format!("split: -{letter} is no option\n"));
        }
        let after = &letters[letter.len_utf8()..];
        let value = if after.is_empty() {
            let (value, rest) = words
                .split_first()
                .ok_or_else(|| format!("split: -{letter} wants its word\n"))?;
            words = rest;
            value.as_str()
        } else {
            after
        };
        match letter {
            'l' => lines = Some(count('l', value)?),
            'b' => size = Some(bytes(value)?),
            _ => suffix = usize::try_from(count('a', value)?).unwrap_or(usize::MAX),
        }
    }
    let size = match (lines, size) {
        (Some(_), Some(_)) => return Err(String::from("split: cuts by -l or by -b, not both\n")),
        (_, Some(bytes)) => Size::Bytes(bytes),
        (lines, None) => Size::Lines(lines.unwrap_or(1000)),
    };
    let (input, name) = match words {
        [] => ("-", "x"),
        [input] => (input.as_str(), "x"),
        [input, name] => (input.as_str(), name.as_str()),
        _ => return Err(String::from("split: takes a file and a name\n")),
    };
    // A file's name is at most 255 characters long.
    let last = name.rsplit('/').next().unwrap_or(name);
    if last.chars().count().saturating_add(suffix) > 255 {
        return Err(format!(
            "split: names of {last} and {suffix} letters would be longer than 255 characters\n"
        ));
    }
    Ok(Asked {
        size,
        suffix,
        input,
        name,
    })
}

/// The names of the files, one after another: the name with a suffix of
/// so many letters, from all a on, as long as there are suffixes left.
struct Names<'a> {
    name: &'a str,
    letters: usize,
    next: u64,
    /// How many suffixes there are, none for more than a u64 counts.
    all: Option<u64>,
}

impl<'a> Names<'a> {
    fn new(name: &'a str, letters: usize) -> Names<'a> {
        let all = u32::try_from(letters)
            .ok()
            .and_then(|l| 26u64.checked_pow(l));
        Names {
            name,
            letters,
            next: 0,
            all,
        }
    }

    /// The next name, none once the suffixes ran out.
    fn next(&mut self) -> Option<String> {
        if self.all.is_some_and(|all| self.next >= all) {
            return None;
        }
        let mut suffix = alloc::vec![b'a'; self.letters];
        let mut left = self.next;
        for letter in suffix.iter_mut().rev() {
            if left == 0 {
                break;
            }
            *letter = b'a' + u8::try_from(left % 26).unwrap_or(0);
            left /= 26;
        }
        self.next += 1;
        Some(format!("{}{}", self.name, String::from_utf8_lossy(&suffix)))
    }
}

/// What split does with the files it makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event<'b> {
    /// A file of this name starts, and the one before is done.
    Start(&'b str),
    /// These bytes go at the end of the file that started last.
    Bytes(&'b [u8]),
}

/// Why split stopped.
#[derive(Debug, PartialEq)]
enum Stop {
    /// The suffixes ran out, after the file of this name.
    NoMoreNames(String),
    /// The file of this name could not be made or written.
    Unwritten(String, Error),
}

/// Cuts the input as it comes into files of a size.
struct Splitter<'a> {
    size: Size,
    names: Names<'a>,
    /// The name of the file being written, and how much it still takes.
    current: Option<String>,
    left: u64,
}

impl<'a> Splitter<'a> {
    fn new(asked: &'a Asked) -> Splitter<'a> {
        Splitter {
            size: asked.size,
            names: Names::new(asked.name, asked.suffix),
            current: None,
            left: 0,
        }
    }

    /// Hands `out` what `bytes`, the next of the input, make: a new file
    /// whenever the last is full and more comes.
    ///
    /// # Errors
    ///
    /// When the suffixes ran out, or a file could not be made or written.
    fn feed(
        &mut self,
        bytes: &[u8],
        out: &mut impl FnMut(Event) -> Result<(), Error>,
    ) -> Result<(), Stop> {
        let mut rest = bytes;
        while !rest.is_empty() {
            if self.left == 0 {
                let Some(name) = self.names.next() else {
                    return Err(Stop::NoMoreNames(self.current.take().unwrap_or_default()));
                };
                out(Event::Start(&name)).map_err(|e| Stop::Unwritten(name.clone(), e))?;
                self.left = match self.size {
                    Size::Lines(n) | Size::Bytes(n) => n,
                };
                self.current = Some(name);
            }
            let (take, used) = match self.size {
                Size::Bytes(_) => {
                    let take = usize::try_from(self.left).map_or(rest.len(), |l| l.min(rest.len()));
                    (take, take as u64)
                }
                Size::Lines(_) => {
                    let mut ended = 0;
                    let mut take = rest.len();
                    for (at, _) in rest.iter().enumerate().filter(|(_, b)| **b == b'\n') {
                        ended += 1;
                        if ended == self.left {
                            take = at + 1;
                            break;
                        }
                    }
                    (take, ended)
                }
            };
            out(Event::Bytes(&rest[..take]))
                .map_err(|e| Stop::Unwritten(self.current.clone().unwrap_or_default(), e))?;
            self.left -= used;
            rest = &rest[take..];
        }
        Ok(())
    }
}

/// The file being written, and where its next byte goes.
struct Writing {
    file: Option<(Handle, u64)>,
}

impl Writing {
    #[cfg_attr(test, allow(dead_code))]
    fn handle(&mut self, event: Event) -> Result<(), Error> {
        match event {
            Event::Start(name) => {
                self.close();
                self.file = Some((program::open_with(name, WRITE, CREATE_ALWAYS)?, 0));
                Ok(())
            }
            Event::Bytes(bytes) => {
                let Some((file, at)) = self.file.as_mut() else {
                    return Ok(());
                };
                program::write_file(*file, *at, bytes)?;
                *at += bytes.len() as u64;
                Ok(())
            }
        }
    }

    fn close(&mut self) {
        if let Some((file, _)) = self.file.take() {
            let _ = program::close(file);
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn split(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut splitter = Splitter::new(&asked);
    let mut writing = Writing { file: None };
    let mut stopped = None;
    let input = Some(asked.input).filter(|&path| path != "-");
    let read = text::pieces(input, |piece| {
        if let Err(stop) = splitter.feed(piece, &mut |event| writing.handle(event)) {
            stopped = Some(stop);
        }
        stopped.is_none()
    });
    writing.close();
    let said = match (read, stopped) {
        (Err(e), _) => text::trouble("split", asked.input, e),
        (Ok(()), Some(Stop::Unwritten(name, e))) => text::trouble("split", &name, e),
        (Ok(()), Some(Stop::NoMoreNames(last))) => {
            format!("split: no more names after {last}\n")
        }
        (Ok(()), None) => return 0,
    };
    let _ = program::write_error(&said);
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// The files split asked with `options` makes of `input`, read in
    /// pieces of `piece` bytes, and why it stopped, if it did.
    fn split_in(
        options: &str,
        input: &[u8],
        piece: usize,
    ) -> (Vec<(String, Vec<u8>)>, Option<Stop>) {
        let given = words(options);
        let asked = asked(&given).unwrap();
        let mut splitter = Splitter::new(&asked);
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut out = |event: Event| {
            match event {
                Event::Start(name) => files.push((String::from(name), Vec::new())),
                Event::Bytes(bytes) => files.last_mut().unwrap().1.extend_from_slice(bytes),
            }
            Ok(())
        };
        for piece in input.chunks(piece.max(1)) {
            if let Err(stop) = splitter.feed(piece, &mut out) {
                return (files, Some(stop));
            }
        }
        (files, None)
    }

    /// The files' names and texts.
    fn split_of(options: &str, input: &str) -> Vec<(String, String)> {
        let (files, stop) = split_in(options, input.as_bytes(), 3);
        assert_eq!(stop, None);
        files
            .into_iter()
            .map(|(name, bytes)| (name, String::from_utf8(bytes).unwrap()))
            .collect()
    }

    /// The numbers of `numbers`, a line each.
    fn numbered(numbers: core::ops::Range<u32>) -> String {
        numbers.fold(String::new(), |all, n| all + &n.to_string() + "\n")
    }

    fn file(name: &str, text: &str) -> (String, String) {
        (String::from(name), String::from(text))
    }

    #[test]
    fn so_many_lines_a_file() {
        assert_eq!(
            split_of("-l 2", "1\n2\n3\n4\n5\n"),
            [
                file("xaa", "1\n2\n"),
                file("xab", "3\n4\n"),
                file("xac", "5\n")
            ]
        );
        assert_eq!(
            split_of("-l2", "1\n2\n3\n4\n"),
            [file("xaa", "1\n2\n"), file("xab", "3\n4\n")],
            "no empty file after the last"
        );
        assert_eq!(split_of("-l 3", "a\nb"), [file("xaa", "a\nb")]);
        assert_eq!(split_of("", ""), [], "an empty input makes no file");
        let thousand = numbered(0..1500);
        let made = split_of("", &thousand);
        assert_eq!(made.len(), 2);
        assert_eq!(made[0].1.lines().count(), 1000);
        assert_eq!(made[1].1.lines().next(), Some("1000"));
    }

    #[test]
    fn so_many_bytes_a_file() {
        assert_eq!(
            split_of("-b 3 - p", "abcdefg"),
            [file("paa", "abc"), file("pab", "def"), file("pac", "g")]
        );
        assert_eq!(split_of("-b 1k", "a"), [file("xaa", "a")]);
        let big = "x".repeat(2049);
        let made = split_of("-b1k", &big);
        assert_eq!(
            made.iter().map(|(_, text)| text.len()).collect::<Vec<_>>(),
            [1024, 1024, 1]
        );
        assert_eq!(bytes("2m"), Ok(2 << 20));
        assert_eq!(
            split_of("-b 2", "a\nb\n"),
            [file("xaa", "a\n"), file("xab", "b\n")]
        );
    }

    #[test]
    fn suffixes_count_up_and_run_out() {
        let mut names = Names::new("x", 2);
        let first: Vec<String> = (0..28).filter_map(|_| names.next()).collect();
        assert_eq!(&first[..3], ["xaa", "xab", "xac"]);
        assert_eq!(first[25], "xaz");
        assert_eq!(first[26], "xba");
        let mut one = Names::new("n/", 1);
        assert_eq!((0..27).filter_map(|_| one.next()).count(), 26);
        assert_eq!(one.next(), None);
        let mut many = Names::new("", 20);
        assert_eq!(many.next().as_deref(), Some("aaaaaaaaaaaaaaaaaaaa"));
        assert_eq!(many.next().as_deref(), Some("aaaaaaaaaaaaaaaaaaab"));
        let lines = numbered(1..31);
        let (made, stop) = split_in("-l 1 -a 1", lines.as_bytes(), 7);
        assert_eq!(made.len(), 26);
        assert_eq!(made[25], (String::from("xz"), b"26\n".to_vec()));
        assert_eq!(stop, Some(Stop::NoMoreNames(String::from("xz"))));
    }

    #[test]
    fn a_file_that_cannot_be_made_or_written_stops_it() {
        let given = words("-l 1");
        let asked = asked(&given).unwrap();
        let mut splitter = Splitter::new(&asked);
        let refused = splitter.feed(b"a\n", &mut |event: Event| match event {
            Event::Start(_) => Err(Error::ReadOnly),
            Event::Bytes(_) => Ok(()),
        });
        assert_eq!(
            refused,
            Err(Stop::Unwritten(String::from("xaa"), Error::ReadOnly))
        );
        let mut splitter = Splitter::new(&asked);
        let full = splitter.feed(b"a\n", &mut |event: Event| match event {
            Event::Start(_) => Ok(()),
            Event::Bytes(_) => Err(Error::DiskFull),
        });
        assert_eq!(
            full,
            Err(Stop::Unwritten(String::from("xaa"), Error::DiskFull))
        );
    }

    #[test]
    fn options_and_the_file_and_name() {
        let given = words("-l 5 -a 3 in out");
        assert_eq!(
            asked(&given).unwrap(),
            Asked {
                size: Size::Lines(5),
                suffix: 3,
                input: "in",
                name: "out"
            }
        );
        let given = words("");
        assert_eq!(
            asked(&given).unwrap(),
            Asked {
                size: Size::Lines(1000),
                suffix: 2,
                input: "-",
                name: "x"
            }
        );
        let given = words("-b 10 -- -l");
        let asked_b = asked(&given).unwrap();
        assert_eq!((asked_b.size, asked_b.input), (Size::Bytes(10), "-l"));
        let wrong = |options: &str| asked(&words(options)).unwrap_err();
        assert_eq!(wrong("-x"), "split: -x is no option\n");
        assert_eq!(wrong("-l"), "split: -l wants its word\n");
        assert_eq!(wrong("-l 0"), "split: -l wants a number from 1, not 0\n");
        assert_eq!(wrong("-a x"), "split: -a wants a number from 1, not x\n");
        assert_eq!(wrong("-b 0"), "split: -b wants a size from 1, not 0\n");
        assert_eq!(wrong("-b 1q"), "split: -b wants a size from 1, not 1q\n");
        assert_eq!(wrong("-b k"), "split: -b wants a size from 1, not k\n");
        assert_eq!(
            wrong("-b 99999999999999999m"),
            "split: -b wants a size from 1, not 99999999999999999m\n"
        );
        assert_eq!(wrong("-l 1 -b 1"), "split: cuts by -l or by -b, not both\n");
        assert_eq!(wrong("a b c"), "split: takes a file and a name\n");
        assert_eq!(
            wrong("-a 250 - dir/abcdef"),
            "split: names of abcdef and 250 letters would be longer than 255 characters\n"
        );
        assert!(asked(&words("-a 250 - dir/abcde")).is_ok());
    }

    #[test]
    fn random_inputs_come_back_whole_from_their_files() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let input: Vec<u8> = (0..seed % 200)
                .map(|n| {
                    if (seed >> (n % 60)).trailing_zeros() >= 3 {
                        b'\n'
                    } else {
                        b'a'
                    }
                })
                .collect();
            let n = 1 + seed % 9;
            for (options, by_lines) in [
                (alloc::format!("-l {n}"), true),
                (alloc::format!("-b {n}"), false),
            ] {
                let (made, stop) =
                    split_in(&options, &input, 1 + usize::try_from(seed % 13).unwrap());
                assert_eq!(stop, None);
                let joined: Vec<u8> = made.iter().flat_map(|(_, bytes)| bytes.clone()).collect();
                assert_eq!(joined, input, "{options}");
                for (at, (_, bytes)) in made.iter().enumerate() {
                    let size = if by_lines {
                        bytes.iter().map(|&b| u64::from(b == b'\n')).sum::<u64>()
                    } else {
                        bytes.len() as u64
                    };
                    if at + 1 < made.len() {
                        assert_eq!(size, n, "{options}: every file but the last is full");
                    } else {
                        assert!(size <= n && !bytes.is_empty());
                    }
                }
            }
        }
    }

    /// The events split asked with `options` hands out for `input`, in
    /// pieces of `piece` bytes, written as text.
    fn events(options: &str, input: &[u8], piece: usize) -> Vec<String> {
        let given = words(options);
        let asked = asked(&given).unwrap();
        let mut splitter = Splitter::new(&asked);
        let mut seen = Vec::new();
        for piece in input.chunks(piece) {
            splitter
                .feed(piece, &mut |event: Event| {
                    seen.push(match event {
                        Event::Start(name) => alloc::format!("start {name}"),
                        Event::Bytes(bytes) => {
                            alloc::format!("bytes {}", String::from_utf8_lossy(bytes))
                        }
                    });
                    Ok(())
                })
                .unwrap();
        }
        seen
    }

    #[test]
    fn each_file_starts_before_its_bytes_and_no_write_is_empty() {
        assert_eq!(
            events("-l 1", b"a\nb\n", 4),
            ["start xaa", "bytes a\n", "start xab", "bytes b\n"]
        );
        assert_eq!(
            events("-b 2", b"abcde", 3),
            [
                "start xaa",
                "bytes ab",
                "start xab",
                "bytes c",
                "bytes d",
                "start xac",
                "bytes e"
            ]
        );
        for piece in 1..6 {
            let seen = events("-b 2", b"abcde", piece);
            assert!(seen.iter().all(|e| e != "bytes "), "pieces of {piece}");
            assert_eq!(seen.iter().filter(|e| e.starts_with("start")).count(), 3);
        }
    }

    #[test]
    fn a_line_longer_than_any_piece_stays_in_one_file() {
        let long = alloc::format!("{}\nshort\n", "x".repeat(300));
        for piece in [1, 7, 128] {
            let (made, stop) = split_in("-l 1", long.as_bytes(), piece);
            assert_eq!(stop, None);
            assert_eq!(made.len(), 2, "pieces of {piece}");
            assert_eq!(made[0].1.len(), 301);
            assert_eq!(made[1].1, b"short\n");
        }
    }

    #[test]
    fn names_run_out_by_bytes_too() {
        let input = [b'q'; 27];
        let (made, stop) = split_in("-b 1 -a 1 - out/n", &input, 5);
        assert_eq!(made.len(), 26);
        assert_eq!(made[0].0, "out/na");
        assert_eq!(made[25], (String::from("out/nz"), b"q".to_vec()));
        assert_eq!(stop, Some(Stop::NoMoreNames(String::from("out/nz"))));
        let (made, stop) = split_in("-b 1 -a 1", &input[..26], 5);
        assert_eq!(
            (made.len(), stop),
            (26, None),
            "exactly as many as there are names"
        );
    }

    #[test]
    fn a_file_refused_later_stops_after_the_ones_before() {
        let given = words("-l 1");
        let asked = asked(&given).unwrap();
        let mut splitter = Splitter::new(&asked);
        let mut made = Vec::new();
        let stopped = splitter.feed(b"a\nb\nc\n", &mut |event: Event| match event {
            Event::Start("xab") => Err(Error::DiskFull),
            Event::Start(name) => {
                made.push(String::from(name));
                Ok(())
            }
            Event::Bytes(_) => Ok(()),
        });
        assert_eq!(
            stopped,
            Err(Stop::Unwritten(String::from("xab"), Error::DiskFull))
        );
        assert_eq!(made, ["xaa"]);
    }

    #[test]
    fn numbers_as_the_options_take_them() {
        assert_eq!(count('l', "007"), Ok(7));
        assert_eq!(count('l', "18446744073709551615"), Ok(u64::MAX));
        for not in ["", "+1", "-1", "1.5", "18446744073709551616", "0"] {
            assert_eq!(
                count('a', not),
                Err(alloc::format!(
                    "split: -a wants a number from 1, not {not}\n"
                ))
            );
        }
        assert_eq!(bytes("1"), Ok(1));
        assert_eq!(bytes("3k"), Ok(3072));
        assert_eq!(bytes("16777215m"), Ok(16_777_215 << 20));
        assert!(bytes("17592186044416m").is_err(), "past what a u64 holds");
        assert!(bytes("m").is_err() && bytes("0k").is_err() && bytes("1g").is_err());
    }
}
