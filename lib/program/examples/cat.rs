//! cat: writes files one after another on its standard output, as
//! POSIX's cat does, `-` or none at all being the standard input; with
//! `-u` each piece as soon as it is read. It ends with 0, or 1 when a
//! file could not be read, after going on with the rest.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::{Handle, text};

program::main!(cat);

/// What cat's words ask for: whether nothing waits before it is written,
/// and the files.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    unheld: bool,
    paths: &'w [String],
}

/// What cat's words ask for, or why they cannot be taken. A `-` alone
/// is a file.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut unheld = false;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        for letter in letters.chars() {
            match letter {
                'u' => unheld = true,
                _ => return Err(format!("cat: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    Ok(Asked {
        unheld,
        paths: words,
    })
}

#[cfg_attr(test, allow(dead_code))]
fn cat(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut out = text::standard_output();
    let read = text::each_piece("cat", asked.paths, |piece| {
        out.put(piece);
        if asked.unheld {
            out.flush();
        }
    });
    u64::from(!(out.finish() && read))
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

    #[test]
    fn its_words_are_u_and_files() {
        let given = words("-u a -");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                unheld: true,
                paths: &given[1..]
            })
        );
        let given = words("- -u");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                unheld: false,
                paths: &given[..]
            }),
            "a - alone is the standard input"
        );
        let given = words("-- -u");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                unheld: false,
                paths: &given[1..]
            })
        );
        let given = words("");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                unheld: false,
                paths: &given[..]
            })
        );
        assert_eq!(asked(&words("-x")), Err("cat: -x is no option\n".into()));
        assert_eq!(asked(&words("-uv")), Err("cat: -v is no option\n".into()));
    }
}
