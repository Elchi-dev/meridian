//! uniq: the lines of the file it is given, or of its standard input
//! without one, each line that comes again right after itself once; with
//! `-c` each with how many times it came, as POSIX's uniq writes them.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Lines};

program::main!(uniq);

/// What uniq was asked for: whether to count, and the file, none for the
/// standard input.
fn asked(arguments: &[String]) -> Result<(bool, Option<&str>), String> {
    let (counting, rest) = match arguments.split_first() {
        Some((flag, rest)) if flag == "-c" => (true, rest),
        Some((flag, _)) if flag.starts_with('-') && flag.len() > 1 => {
            return Err(format!("uniq: {flag} is no option\n"));
        }
        _ => (false, arguments),
    };
    match rest {
        [] => Ok((counting, None)),
        [path] => Ok((counting, Some(path.as_str()))),
        _ => Err("uniq: reads one file at most\n".into()),
    }
}

/// The lines of a text that comes in pieces, each run of the same line
/// once.
struct Runs {
    counting: bool,
    last: Option<(Vec<u8>, usize)>,
    out: Vec<u8>,
    lines: Lines,
}

impl Runs {
    fn new(counting: bool) -> Runs {
        Runs {
            counting,
            last: None,
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
        match &mut self.last {
            Some((last, times)) if last.as_slice() == line => *times += 1,
            _ => {
                self.end_run();
                self.last = Some((line.to_vec(), 1));
            }
        }
    }

    /// Writes the run that ended.
    fn end_run(&mut self) {
        let Some((line, times)) = self.last.take() else {
            return;
        };
        if self.counting {
            self.out.extend_from_slice(format!("{times} ").as_bytes());
        }
        self.out.extend_from_slice(&line);
        self.out.push(b'\n');
    }

    fn finish(mut self) -> Vec<u8> {
        let lines = core::mem::take(&mut self.lines);
        lines.finish(|line| self.take(line));
        self.end_run();
        self.out
    }
}

#[cfg_attr(test, allow(dead_code))]
fn uniq(_: Handle) -> u64 {
    let (counting, path) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let mut runs = Runs::new(counting);
    if let Err(e) = text::pieces(path, |piece| {
        runs.feed(piece);
        true
    }) {
        let _ = program::write_error(&text::trouble("uniq", path.unwrap_or("-"), e));
        return 2;
    }
    u64::from(program::write_bytes(&runs.finish()).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    fn runs(counting: bool, pieces: &[&str]) -> String {
        let mut runs = Runs::new(counting);
        for piece in pieces {
            runs.feed(piece.as_bytes());
        }
        String::from_utf8(runs.finish()).unwrap()
    }

    #[test]
    fn it_reads_one_file_or_its_input_and_counts_when_told() {
        assert_eq!(asked(&[]), Ok((false, None)));
        assert_eq!(asked(&words("-c")), Ok((true, None)));
        assert_eq!(asked(&words("-c /a")), Ok((true, Some("/a"))));
        assert_eq!(asked(&words("/a")), Ok((false, Some("/a"))));
        assert_eq!(
            asked(&words("/a /b")),
            Err("uniq: reads one file at most\n".into())
        );
        assert_eq!(asked(&words("-x")), Err("uniq: -x is no option\n".into()));
        assert_eq!(
            asked(&words("-")),
            Ok((false, Some("-"))),
            "a dash is a path"
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
        let mut bytes = Runs::new(false);
        bytes.feed(&[0xff, b'\n', 0xff, b'\n', 0xfe, b'\n']);
        assert_eq!(
            bytes.finish(),
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
}
