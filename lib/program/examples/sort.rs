//! sort: the lines of the files it is given, or of its standard input
//! without one, all together, in the order of their bytes, or the other way
//! round with `-r`, as POSIX's sort writes them in the C locale.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Lines};

program::main!(sort);

/// What sort was asked for: whether the other way round, and the files.
fn asked(arguments: &[String]) -> Result<(bool, &[String]), String> {
    match arguments.split_first() {
        Some((flag, files)) if flag == "-r" => Ok((true, files)),
        Some((flag, _)) if flag.starts_with('-') && flag.len() > 1 => {
            Err(format!("sort: {flag} is no option\n"))
        }
        _ => Ok((false, arguments)),
    }
}

/// The lines of every text given, each line's last one too.
#[derive(Default)]
struct All {
    lines: Vec<Vec<u8>>,
}

impl All {
    /// Takes in the whole of one text, piece by piece from `pieces`.
    fn take<E>(
        &mut self,
        pieces: impl FnOnce(&mut dyn FnMut(&[u8]) -> bool) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut cut = Lines::default();
        let all = &mut self.lines;
        pieces(&mut |piece| {
            cut.feed(piece, |line| {
                all.push(line.to_vec());
                true
            })
        })?;
        cut.finish(|line| all.push(line.to_vec()));
        Ok(())
    }

    /// The lines in order, each with its end.
    fn sorted(mut self, reversed: bool) -> Vec<u8> {
        self.lines.sort();
        if reversed {
            self.lines.reverse();
        }
        let mut out = Vec::new();
        for line in self.lines {
            out.extend_from_slice(&line);
            out.push(b'\n');
        }
        out
    }
}

#[cfg_attr(test, allow(dead_code))]
fn sort(_: Handle) -> u64 {
    let (reversed, files) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let sources = text::sources(files);
    let mut all = All::default();
    for path in sources {
        if let Err(e) = all.take(|each| text::pieces(path, each)) {
            let _ = program::write_error(&text::trouble("sort", path.unwrap_or("-"), e));
            return 2;
        }
    }
    u64::from(program::write_bytes(&all.sorted(reversed)).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    /// The texts `texts`, each in its pieces, sorted.
    fn sorted(texts: &[&[&str]], reversed: bool) -> String {
        let mut all = All::default();
        for pieces in texts {
            let fed: Result<(), ()> = all.take(|each| {
                for piece in *pieces {
                    if !each(piece.as_bytes()) {
                        break;
                    }
                }
                Ok(())
            });
            assert_eq!(fed, Ok(()));
        }
        String::from_utf8(all.sorted(reversed)).unwrap()
    }

    #[test]
    fn it_sorts_forward_unless_told_the_other_way() {
        let none: [String; 0] = [];
        assert_eq!(asked(&[]), Ok((false, &none[..])));
        let r = words("-r /a /b");
        assert_eq!(asked(&r), Ok((true, &r[1..])));
        let files = words("/a -r");
        assert_eq!(
            asked(&files),
            Ok((false, &files[..])),
            "an option comes first"
        );
        assert_eq!(asked(&words("-x")), Err("sort: -x is no option\n".into()));
        let dash = words("-");
        assert_eq!(asked(&dash), Ok((false, &dash[..])));
    }

    #[test]
    fn lines_come_in_the_order_of_their_bytes() {
        assert_eq!(sorted(&[&["b\na\nc\n"]], false), "a\nb\nc\n");
        assert_eq!(sorted(&[&["b\na\nc\n"]], true), "c\nb\na\n");
        assert_eq!(
            sorted(&[&["B\na\nA\n"]], false),
            "A\nB\na\n",
            "capitals first"
        );
        assert_eq!(
            sorted(&[&["two\none"], &["three\n"]], false),
            "one\nthree\ntwo\n"
        );
        assert_eq!(sorted(&[&["x", "y\nw"]], false), "w\nxy\n");
        assert_eq!(
            sorted(&[&["a\n\nb\n"]], false),
            "\na\nb\n",
            "an empty line first"
        );
        assert_eq!(sorted(&[&["ab\na\n"]], false), "a\nab\n");
        assert_eq!(sorted(&[], false), "");
    }

    #[test]
    fn equal_lines_stay_and_bytes_beyond_ascii_come_after_it() {
        assert_eq!(sorted(&[&["b\na\nb\na\n"]], false), "a\na\nb\nb\n");
        assert_eq!(sorted(&[&["b\na\nb\n"]], true), "b\nb\na\n");
        assert_eq!(
            sorted(&[&["\u{e9}\nz\ne\n"]], false),
            "e\nz\n\u{e9}\n",
            "by their bytes, not their letters"
        );
        assert_eq!(
            sorted(&[&["last"], &["first\n"]], false),
            "first\nlast\n",
            "a file's last line without its end"
        );
    }

    #[test]
    fn a_text_that_stops_early_keeps_what_came() {
        let mut all = All::default();
        let fed: Result<(), &str> = all.take(|each| {
            let _ = each(b"b\na");
            Err("the disk could not be read")
        });
        assert_eq!(fed, Err("the disk could not be read"));
        assert_eq!(String::from_utf8(all.sorted(false)).unwrap(), "b\n");
    }
}
