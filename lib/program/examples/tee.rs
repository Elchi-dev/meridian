//! tee: its standard input to its standard output and into each file it
//! names, as POSIX's tee has it; `-a` adds to the end of the files rather
//! than writing them anew, and `-i` changes nothing, as Meridian sends no
//! signals to ignore.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::file::{CREATE_ALWAYS, OPEN_ALWAYS, WRITE};
use program::{Handle, text};

program::main!(tee);

/// What tee's words ask: whether to add to the files' ends, and the files.
///
/// # Errors
///
/// For an option it does not know.
fn asked(arguments: &[String]) -> Result<(bool, &[String]), String> {
    let mut adding = false;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            return Ok((adding, rest));
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        for letter in letters.chars() {
            match letter {
                'a' => adding = true,
                'i' => {}
                _ => return Err(format!("-{letter} is no option")),
            }
        }
        words = rest;
    }
    Ok((adding, words))
}

/// A file tee writes into, and where the next piece goes in it.
struct Into {
    path: String,
    file: Handle,
    at: u64,
}

#[cfg_attr(test, allow(dead_code))]
fn tee(_: Handle) -> u64 {
    let (adding, paths) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&format!("tee: {why}\n"));
            return 2;
        }
    };
    let mut failed = false;
    let mut into: Vec<Into> = Vec::new();
    for path in paths {
        let how = if adding { OPEN_ALWAYS } else { CREATE_ALWAYS };
        let opened = program::open_with(path, WRITE, how).and_then(|file| {
            let at = if adding { program::info(file)?.size } else { 0 };
            Ok(Into {
                path: path.clone(),
                file,
                at,
            })
        });
        match opened {
            Ok(file) => into.push(file),
            Err(e) => {
                failed = true;
                let _ = program::write_error(&text::trouble("tee", path, e));
            }
        }
    }
    let read = text::pieces(None, |piece| {
        failed |= program::write_bytes(piece).is_err();
        // A file that could not be written is written no more, but the
        // others and the output go on.
        into.retain_mut(
            |file| match program::write_file(file.file, file.at, piece) {
                Ok(()) => {
                    file.at += piece.len() as u64;
                    true
                }
                Err(e) => {
                    failed = true;
                    let _ = program::write_error(&text::trouble("tee", &file.path, e));
                    let _ = program::close(file.file);
                    false
                }
            },
        );
        true
    });
    for file in into {
        let _ = program::close(file.file);
    }
    if let Err(e) = read {
        failed = true;
        let _ = program::write_error(&text::trouble("tee", "-", e));
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

    #[test]
    fn its_words_are_files_after_its_options() {
        let args = words("/a /b");
        assert_eq!(asked(&args), Ok((false, &args[..])));
        let args = words("-a /a");
        assert_eq!(asked(&args), Ok((true, &args[1..])));
        let args = words("-ai -i /a");
        assert_eq!(asked(&args), Ok((true, &args[2..])));
        let args = words("-- -a");
        assert_eq!(
            asked(&args),
            Ok((false, &args[1..])),
            "after -- a dash is a file"
        );
        let args = words("/a -a");
        assert_eq!(asked(&args), Ok((false, &args[..])), "options come first");
        let args = words("-");
        assert_eq!(
            asked(&args),
            Ok((false, &args[..])),
            "a dash alone is a file"
        );
        assert_eq!(asked(&[]), Ok((false, &[][..])));
        assert_eq!(asked(&words("-x")), Err(String::from("-x is no option")));
        assert_eq!(asked(&words("-ax")), Err(String::from("-x is no option")));
    }

    #[test]
    fn a_dash_dash_ends_its_options_once() {
        let args = words("-a -- -a");
        assert_eq!(
            asked(&args),
            Ok((true, &args[2..])),
            "a -a after -- is a file"
        );
        let args = words("--");
        assert_eq!(asked(&args), Ok((false, &[][..])), "no file at all");
        let args = words("-- --");
        assert_eq!(
            asked(&args),
            Ok((false, &args[1..])),
            "the second is a file"
        );
        let args = words("-i");
        assert_eq!(asked(&args), Ok((false, &[][..])), "-i alone adds nothing");
        let args = words("-i -- -x");
        assert_eq!(asked(&args), Ok((false, &args[2..])));
        let args = words("-a -");
        assert_eq!(asked(&args), Ok((true, &args[1..])), "- is a file");
    }
}
