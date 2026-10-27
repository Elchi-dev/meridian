//! grep: the lines of each file it is given, or of its standard input
//! without one, that hold the text it is given, as it is, with no
//! patterns. `-i` looks for it in either case, `-v` takes the lines that
//! do not hold it, `-c` says only how many, and `-n` puts each line's
//! number before it; with more than one file, each line comes after its
//! file's path. It ends with 0 when it found a line, 1 when it found none,
//! and 2 when something went wrong, as POSIX's grep does.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Lines};

program::main!(grep);

/// What grep writes for the lines it finds: each, each with its number,
/// or how many there are, which wins over numbers as in POSIX.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Show {
    #[default]
    Lines,
    Numbered,
    Count,
}

/// What grep was asked for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Asked<'a> {
    text: &'a str,
    files: &'a [String],
    either_case: bool,
    without: bool,
    show: Show,
}

fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked::default();
    let mut rest = arguments;
    while let Some((flag, after)) = rest.split_first() {
        if flag == "--" {
            rest = after;
            break;
        }
        let Some(letters) = flag.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        for letter in letters.chars() {
            match letter {
                'i' => asked.either_case = true,
                'v' => asked.without = true,
                'c' => asked.show = Show::Count,
                'n' if asked.show == Show::Lines => asked.show = Show::Numbered,
                'n' => {}
                _ => return Err(format!("grep: -{letter} is no option\n")),
            }
        }
        rest = after;
    }
    let (text, files) = rest
        .split_first()
        .ok_or_else(|| String::from("grep: needs the text to look for\n"))?;
    asked.text = text;
    asked.files = files;
    Ok(asked)
}

/// Whether `line` holds `text`, in either case when `either_case`.
fn holds(line: &[u8], text: &str, either_case: bool) -> bool {
    if !either_case {
        return text.is_empty() || line.windows(text.len()).any(|w| w == text.as_bytes());
    }
    let line = String::from_utf8_lossy(line).to_uppercase();
    line.contains(&text.to_uppercase())
}

/// The lines grep finds in a text that comes in pieces.
struct Found<'a> {
    asked: &'a Asked<'a>,
    name: Option<&'a str>,
    number: usize,
    matched: usize,
    out: Vec<u8>,
    lines: Lines,
}

impl<'a> Found<'a> {
    fn new(asked: &'a Asked<'a>, name: Option<&'a str>) -> Found<'a> {
        Found {
            asked,
            name,
            number: 0,
            matched: 0,
            out: Vec::new(),
            lines: Lines::default(),
        }
    }

    fn feed(&mut self, piece: &[u8]) {
        let mut lines = core::mem::take(&mut self.lines);
        lines.feed(piece, |line| {
            self.take(line);
            true
        });
        self.lines = lines;
    }

    fn take(&mut self, line: &[u8]) {
        self.number += 1;
        if holds(line, self.asked.text, self.asked.either_case) == self.asked.without {
            return;
        }
        self.matched += 1;
        if self.asked.show == Show::Count {
            return;
        }
        if let Some(name) = self.name {
            self.out.extend_from_slice(name.as_bytes());
            self.out.push(b':');
        }
        if self.asked.show == Show::Numbered {
            self.out
                .extend_from_slice(format!("{}:", self.number).as_bytes());
        }
        self.out.extend_from_slice(line);
        self.out.push(b'\n');
    }

    /// What grep writes for the text, and how many lines it found.
    fn finish(mut self) -> (Vec<u8>, usize) {
        let lines = core::mem::take(&mut self.lines);
        lines.finish(|line| self.take(line));
        if self.asked.show == Show::Count {
            let name = self.name.map(|n| format!("{n}:")).unwrap_or_default();
            self.out = format!("{name}{}\n", self.matched).into_bytes();
        }
        (self.out, self.matched)
    }
}

#[cfg_attr(test, allow(dead_code))]
fn grep(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let sources = text::sources(asked.files);
    let named = sources.len() > 1;
    let (mut matched, mut failed) = (0, false);
    for path in sources {
        let mut found = Found::new(&asked, path.filter(|_| named));
        if let Err(e) = text::pieces(path, |piece| {
            found.feed(piece);
            true
        }) {
            failed = true;
            let _ = program::write_error(&text::trouble("grep", path.unwrap_or("-"), e));
            continue;
        }
        let (said, count) = found.finish();
        matched += count;
        failed |= program::write_bytes(&said).is_err();
    }
    ending(failed, matched)
}

/// What grep ends with: 2 when something `failed`, else 0 when it
/// `matched` a line and 1 when it found none.
fn ending(failed: bool, matched: usize) -> u64 {
    match (failed, matched) {
        (true, _) => 2,
        (false, 0) => 1,
        (false, _) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    /// What grep asked with `arguments` finds in `pieces`, as `name`.
    fn found(arguments: &str, name: Option<&str>, pieces: &[&str]) -> (String, usize) {
        let words = words(arguments);
        let asked = asked(&words).unwrap();
        let mut found = Found::new(&asked, name);
        for piece in pieces {
            found.feed(piece.as_bytes());
        }
        let (out, count) = found.finish();
        (String::from_utf8(out).unwrap(), count)
    }

    #[test]
    fn options_come_before_the_text_and_the_files() {
        let w = words("-in -c two /a /b");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.text, a.files, a.either_case, a.show, a.without),
            ("two", &w[3..], true, Show::Count, false)
        );
        assert_eq!(asked(&words("-n x")).unwrap().show, Show::Numbered);
        assert_eq!(
            asked(&words("-cn x")).unwrap().show,
            Show::Count,
            "a count wins"
        );
        let w = words("-v x");
        assert!(asked(&w).unwrap().without);
        let w = words("-- -v /a");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.text, a.files, a.without),
            ("-v", &w[2..], false),
            "after --"
        );
        let w = words("- /a");
        assert_eq!(asked(&w).unwrap().text, "-", "a dash alone is text");
        assert_eq!(asked(&words("-x a")), Err("grep: -x is no option\n".into()));
        assert_eq!(
            asked(&words("-i")),
            Err("grep: needs the text to look for\n".into())
        );
        assert_eq!(asked(&[]), Err("grep: needs the text to look for\n".into()));
    }

    #[test]
    fn a_line_holds_the_text_as_it_is_or_in_either_case() {
        assert!(holds(b"one two", "two", false));
        assert!(!holds(b"one TWO", "two", false));
        assert!(holds(b"one TWO", "two", true));
        assert!(holds("\u{c9}t\u{e9}".as_bytes(), "\u{e9}T\u{c9}", true));
        assert!(holds(b"anything", "", false), "nothing is in every line");
        assert!(!holds(b"tw", "two", false));
        assert!(
            holds(&[0xff, b'a'], "A", true),
            "lines that are no text too"
        );
    }

    #[test]
    fn the_lines_found_come_as_asked() {
        let text = ["one\ntwo\nthr", "ee two\nfour"];
        assert_eq!(found("two", None, &text), ("two\nthree two\n".into(), 2));
        assert_eq!(found("-v two", None, &text), ("one\nfour\n".into(), 2));
        assert_eq!(
            found("-n two", None, &text),
            ("2:two\n3:three two\n".into(), 2)
        );
        assert_eq!(found("-c two", None, &text), ("2\n".into(), 2));
        assert_eq!(found("-c two", Some("/a"), &text), ("/a:2\n".into(), 2));
        assert_eq!(
            found("-n o", Some("/a"), &text),
            ("/a:1:one\n/a:2:two\n/a:3:three two\n/a:4:four\n".into(), 4)
        );
        assert_eq!(
            found("four", None, &text),
            ("four\n".into(), 1),
            "the last too"
        );
        assert_eq!(found("five", None, &text), (String::new(), 0));
        assert_eq!(found("-c five", None, &text), ("0\n".into(), 0));
        assert_eq!(found("-i TWO", None, &text), ("two\nthree two\n".into(), 2));
    }

    #[test]
    fn counts_and_numbers_go_on_across_pieces_and_files() {
        let text = ["a\nb", "\nc\n", "a b\n"];
        assert_eq!(found("-vc a", None, &text), ("2\n".into(), 2), "b and c");
        assert_eq!(found("-n a", None, &text), ("1:a\n4:a b\n".into(), 2));
        assert_eq!(found("-c a", Some("/x"), &[]), ("/x:0\n".into(), 0));
        assert_eq!(found("-c z", Some("/x"), &text), ("/x:0\n".into(), 0));
        assert_eq!(
            found("-v z", Some("/x"), &text),
            ("/x:a\n/x:b\n/x:c\n/x:a b\n".into(), 4)
        );
    }

    #[test]
    fn it_ends_with_what_it_found_unless_something_went_wrong() {
        assert_eq!(ending(false, 3), 0);
        assert_eq!(ending(false, 0), 1);
        assert_eq!(ending(true, 0), 2);
        assert_eq!(ending(true, 5), 2, "even with lines found");
    }

    #[test]
    fn nothing_to_look_for_is_in_every_line() {
        let w = [String::new()];
        let a = asked(&w).unwrap();
        assert_eq!(a.text, "");
        let mut found = Found::new(&a, None);
        found.feed(b"x\n\ny");
        assert_eq!(found.finish(), (b"x\n\ny\n".to_vec(), 3));
        let without = [String::from("-v"), String::new()];
        let a = asked(&without).unwrap();
        let mut none = Found::new(&a, None);
        none.feed(b"x\ny\n");
        assert_eq!(none.finish(), (Vec::new(), 0));
    }

    #[test]
    fn a_character_cut_between_pieces_is_found_in_either_case() {
        let e = "\u{e9}".as_bytes();
        let w = [String::from("-i"), String::from("\u{c9}T")];
        let a = asked(&w).unwrap();
        let mut found = Found::new(&a, None);
        found.feed(&[b'x', e[0]]);
        found.feed(&[e[1], b't', b'\n']);
        assert_eq!(found.finish(), ("x\u{e9}t\n".as_bytes().to_vec(), 1));
        assert!(!holds(b"short", "longer than it", false));
        assert!(holds(b"same", "same", false));
    }
}
