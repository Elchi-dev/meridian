//! paste: joins the lines of its files, as POSIX's paste does: the first
//! line of each, then the second of each, and so on, with a tab between,
//! or with `-s` all the lines of each file into one. `-d` gives the
//! delimiters to use one after another instead, in which `\n`, `\t`, `\\`
//! and `\0`, for none, stand for those. `-` is the standard input, and
//! each `-` takes its next line.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::text::{self, LineReader, Pieces, Source};
use program::{Error, Handle};

program::main!(paste);

/// What paste was asked for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    serial: bool,
    delimiters: Vec<Vec<u8>>,
    files: &'a [String],
}

/// The delimiters `list` gives, each as the bytes it puts in, none for
/// `\0`.
fn delimiters(list: &str) -> Result<Vec<Vec<u8>>, String> {
    let mut delimiters = Vec::new();
    let mut chars = list.chars();
    while let Some(c) = chars.next() {
        let delimiter = match c {
            '\\' => match chars.next() {
                Some('n') => String::from("\n"),
                Some('t') => String::from("\t"),
                Some('\\') => String::from("\\"),
                Some('0') => String::new(),
                Some(other) => return Err(format!("paste: \\{other} is no delimiter\n")),
                None => {
                    return Err(String::from(
                        "paste: a delimiter list ends in a backslash\n",
                    ));
                }
            },
            c => String::from(c),
        };
        delimiters.push(delimiter.into_bytes());
    }
    if delimiters.is_empty() {
        delimiters.push(Vec::new());
    }
    Ok(delimiters)
}

fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut serial = false;
    let mut list = None;
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
                's' => serial = true,
                'd' => {
                    // The rest of the word is the list, or else the next
                    // word.
                    let after = &letters[at + 1..];
                    list = Some(if after.is_empty() {
                        let (list, rest) = words
                            .split_first()
                            .ok_or_else(|| String::from("paste: -d wants a list\n"))?;
                        words = rest;
                        list.as_str()
                    } else {
                        after
                    });
                    break;
                }
                _ => return Err(format!("paste: -{letter} is no option\n")),
            }
        }
    }
    if words.is_empty() {
        return Err(String::from("paste: needs a file to join\n"));
    }
    Ok(Asked {
        serial,
        delimiters: delimiters(list.unwrap_or("\t"))?,
        files: words,
    })
}

/// For each file, the reader it is read through: one for the standard
/// input, which each `-` takes its next line from.
fn readers_of(files: &[String]) -> (Vec<&str>, Vec<usize>) {
    let mut sources: Vec<&str> = Vec::new();
    let mut of = Vec::new();
    for file in files {
        let shared = (file == "-")
            .then(|| sources.iter().position(|s| *s == "-"))
            .flatten();
        of.push(shared.unwrap_or_else(|| {
            sources.push(file);
            sources.len() - 1
        }));
    }
    (sources, of)
}

/// Gives `out` each line paste writes for the files that `of` sends to
/// `readers`, side by side, until all ended.
///
/// # Errors
///
/// Which reader could not be read, and why.
fn parallel<P: Pieces>(
    readers: &mut [LineReader<P>],
    of: &[usize],
    delimiters: &[Vec<u8>],
    mut out: impl FnMut(&[u8]),
) -> Result<(), (usize, Error)> {
    loop {
        let mut line = Vec::new();
        let mut any = false;
        for (column, &reader) in of.iter().enumerate() {
            if column > 0 {
                line.extend_from_slice(&delimiters[(column - 1) % delimiters.len()]);
            }
            if let Some(taken) = readers[reader].line().map_err(|e| (reader, e))? {
                line.extend(taken);
                any = true;
            }
        }
        if !any {
            return Ok(());
        }
        line.push(b'\n');
        out(&line);
    }
}

/// Gives `out` the line paste `-s` writes for each file, all of its lines
/// one after another.
///
/// # Errors
///
/// Which reader could not be read, and why.
fn serial<P: Pieces>(
    readers: &mut [LineReader<P>],
    of: &[usize],
    delimiters: &[Vec<u8>],
    mut out: impl FnMut(&[u8]),
) -> Result<(), (usize, Error)> {
    for &reader in of {
        let mut line = Vec::new();
        let mut taken = 0;
        while let Some(next) = readers[reader].line().map_err(|e| (reader, e))? {
            if taken > 0 {
                line.extend_from_slice(&delimiters[(taken - 1) % delimiters.len()]);
            }
            line.extend(next);
            taken += 1;
        }
        line.push(b'\n');
        out(&line);
    }
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
fn paste(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let (paths, of) = readers_of(asked.files);
    let mut readers = Vec::new();
    for path in &paths {
        match Source::open(path) {
            Ok(source) => readers.push(LineReader::new(source)),
            Err(e) => {
                let _ = program::write_error(&text::trouble("paste", path, e));
                for reader in readers {
                    reader.into_pieces().close();
                }
                return 1;
            }
        }
    }
    let mut out = text::standard_output();
    let joined = if asked.serial {
        serial(&mut readers, &of, &asked.delimiters, |line| out.put(line))
    } else {
        parallel(&mut readers, &of, &asked.delimiters, |line| out.put(line))
    };
    let written = out.finish();
    for reader in readers {
        reader.into_pieces().close();
    }
    if let Err((reader, e)) = joined {
        let _ = program::write_error(&text::trouble("paste", paths[reader], e));
        return 1;
    }
    u64::from(!written)
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

    /// A text in memory, given in the pieces it was cut into.
    struct Cut(alloc::collections::VecDeque<Vec<u8>>);

    impl Pieces for Cut {
        fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
            let Some(piece) = self.0.pop_front() else {
                return Ok(false);
            };
            held.extend_from_slice(&piece);
            Ok(true)
        }
    }

    /// What paste asked with `options` writes for `texts`, read in pieces
    /// of 2 bytes, each `-` among `files` the last of the texts.
    fn pasted(options: &str, files: &[&str], texts: &[&str]) -> String {
        let names: Vec<String> = files.iter().map(|f| String::from(*f)).collect();
        let mut all = words(options);
        all.extend(names);
        let asked = asked(&all).unwrap();
        let (sources, of) = readers_of(asked.files);
        assert_eq!(sources.len(), texts.len(), "a text for each source");
        let mut readers: Vec<LineReader<Cut>> = texts
            .iter()
            .map(|text| {
                LineReader::new(Cut(text.as_bytes().chunks(2).map(<[u8]>::to_vec).collect()))
            })
            .collect();
        let mut out = Vec::new();
        let put = |line: &[u8]| out.extend_from_slice(line);
        if asked.serial {
            serial(&mut readers, &of, &asked.delimiters, put).unwrap();
        } else {
            parallel(&mut readers, &of, &asked.delimiters, put).unwrap();
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn lines_of_each_file_come_side_by_side() {
        assert_eq!(
            pasted("", &["/a", "/b"], &["1\n2\n", "x\ny\n"]),
            "1\tx\n2\ty\n"
        );
        assert_eq!(
            pasted("", &["/a", "/b"], &["1\n2\n3\n", "x\n"]),
            "1\tx\n2\t\n3\t\n",
            "a file that ended gives nothing"
        );
        assert_eq!(pasted("", &["/a", "/b"], &["", "x\n"]), "\tx\n");
        assert_eq!(pasted("", &["/a"], &["only\n"]), "only\n");
        assert_eq!(pasted("", &["/a", "/b"], &["", ""]), "");
        assert_eq!(
            pasted("", &["/a", "/b"], &["a", "b"]),
            "a\tb\n",
            "last lines without their line feed"
        );
    }

    #[test]
    fn delimiters_take_turns_between_the_columns() {
        let texts = ["1\n", "2\n", "3\n", "4\n"];
        let files = ["/a", "/b", "/c", "/d"];
        assert_eq!(pasted("-d ,", &files, &texts), "1,2,3,4\n");
        assert_eq!(pasted("-d ,;", &files, &texts), "1,2;3,4\n", "round again");
        assert_eq!(
            pasted("-d ,;:", &files, &texts),
            "1,2;3:4\n",
            "the first between the first two columns"
        );
        assert_eq!(pasted("-d \\0", &files, &texts), "1234\n", "none");
        assert_eq!(pasted("-d \\n", &files[..2], &texts[..2]), "1\n2\n");
        assert_eq!(pasted("-d \\t\\\\", &files, &texts), "1\t2\\3\t4\n");
        assert_eq!(
            pasted("-d,", &files[..2], &["a\nb\n", "c\nd\n"]),
            "a,c\nb,d\n",
            "starting over on each line"
        );
    }

    #[test]
    fn s_joins_the_lines_of_each_file_into_one() {
        assert_eq!(pasted("-s", &["/a"], &["1\n2\n3\n"]), "1\t2\t3\n");
        assert_eq!(pasted("-s", &["/a", "/b"], &["1\n2\n", "x\n"]), "1\t2\nx\n");
        assert_eq!(pasted("-s -d ,;", &["/a"], &["1\n2\n3\n4\n"]), "1,2;3,4\n");
        assert_eq!(
            pasted("-s -d ,;:", &["/a"], &["1\n2\n3\n4\n5\n"]),
            "1,2;3:4,5\n"
        );
        assert_eq!(
            pasted("-s", &["/a"], &[""]),
            "\n",
            "an empty file is an empty line"
        );
        assert_eq!(
            pasted("-sd :", &["/a", "/b"], &["a\nb\n", "c\nd\n"]),
            "a:b\nc:d\n"
        );
    }

    #[test]
    fn each_dash_takes_the_next_line_of_the_standard_input() {
        assert_eq!(
            readers_of(&words("- /a - -")),
            (Vec::from(["-", "/a"]), Vec::from([0, 1, 0, 0]))
        );
        assert_eq!(pasted("", &["-", "-"], &["1\n2\n3\n4\n"]), "1\t2\n3\t4\n");
        assert_eq!(
            pasted("", &["-", "-", "-"], &["1\n2\n3\n4\n"]),
            "1\t2\t3\n4\t\t\n"
        );
        assert_eq!(
            pasted("-s", &["-", "-"], &["1\n2\n"]),
            "1\t2\n\n",
            "the second finds it read"
        );
    }

    #[test]
    fn its_words_are_options_and_at_least_one_file() {
        let w = words("-s -d ,: /a /b");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.serial, a.delimiters, a.files),
            (true, Vec::from([b",".to_vec(), b":".to_vec()]), &w[3..])
        );
        assert_eq!(asked(&words("/a")).unwrap().delimiters, [b"\t".to_vec()]);
        assert_eq!(asked(&words("-- -s")).unwrap().files, ["-s"]);
        assert_eq!(
            asked(&words("-s")),
            Err(String::from("paste: needs a file to join\n"))
        );
        assert_eq!(
            asked(&words("-d")),
            Err(String::from("paste: -d wants a list\n"))
        );
        assert_eq!(
            asked(&words("-x /a")),
            Err(String::from("paste: -x is no option\n"))
        );
        assert_eq!(
            delimiters("\\q"),
            Err(String::from("paste: \\q is no delimiter\n"))
        );
        assert_eq!(
            delimiters("a\\"),
            Err(String::from(
                "paste: a delimiter list ends in a backslash\n"
            ))
        );
        assert_eq!(
            delimiters(""),
            Ok(Vec::from([Vec::new()])),
            "an empty list is none"
        );
        assert_eq!(delimiters("ä"), Ok(Vec::from(["ä".as_bytes().to_vec()])));
    }

    #[test]
    fn random_files_paste_as_their_lines_zipped_say() {
        let mut seed = 0x8f1b_bcdc_bfa5_3e0b_u64;
        let mut roll = |sides: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % 1024).unwrap() % sides
        };
        for _ in 0..150 {
            let files: Vec<Vec<String>> = (0..=roll(3))
                .map(|_| {
                    (0..roll(4))
                        .map(|_| ["", "a", "bc", "dé"][roll(4)].into())
                        .collect()
                })
                .collect();
            let texts: Vec<String> = files
                .iter()
                .map(|lines| {
                    lines
                        .iter()
                        .fold(String::new(), |text, line| text + line + "\n")
                })
                .collect();
            let names: Vec<String> = (0..files.len()).map(|i| alloc::format!("/f{i}")).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            let longest = files.iter().map(Vec::len).max().unwrap_or(0);
            let zipped: String = (0..longest)
                .map(|row| {
                    let cells: Vec<&str> = files
                        .iter()
                        .map(|lines| lines.get(row).map_or("", String::as_str))
                        .collect();
                    cells.join(",") + "\n"
                })
                .collect();
            assert_eq!(pasted("-d ,", &names, &texts), zipped, "{files:?}");
            let joined: String = files.iter().map(|lines| lines.join(",") + "\n").collect();
            assert_eq!(
                pasted("-s -d ,", &names, &texts),
                joined,
                "{files:?} with -s"
            );
        }
    }

    #[test]
    fn the_last_d_and_an_empty_list_count() {
        let w = words("-d , -d ; /a");
        assert_eq!(
            asked(&w).unwrap().delimiters,
            [b";".to_vec()],
            "the last list"
        );
        let empty = [String::from("-d"), String::new(), String::from("/a")];
        assert_eq!(asked(&empty).unwrap().delimiters, [Vec::<u8>::new()]);
        assert_eq!(pasted("-d ä", &["/a", "/b"], &["1\n", "2\n"]), "1ä2\n");
        assert_eq!(pasted("-s -d \\0", &["/a"], &["x\ny\n"]), "xy\n");
    }

    #[test]
    fn files_named_twice_are_read_twice_but_the_input_once() {
        assert_eq!(
            readers_of(&words("- /a -")),
            (Vec::from(["-", "/a"]), Vec::from([0, 1, 0])),
            "a file between two dashes"
        );
        assert_eq!(
            readers_of(&words("/a /a")),
            (Vec::from(["/a", "/a"]), Vec::from([0, 1]))
        );
        assert_eq!(
            readers_of(&words("- -")),
            (Vec::from(["-"]), Vec::from([0, 0]))
        );
        assert_eq!(
            readers_of(&words("/a /b")),
            (Vec::from(["/a", "/b"]), Vec::from([0, 1]))
        );
        assert_eq!(
            pasted("", &["/a", "/a"], &["1\n2\n", "1\n2\n"]),
            "1\t1\n2\t2\n",
            "each its own reader of the same file"
        );
    }
}
