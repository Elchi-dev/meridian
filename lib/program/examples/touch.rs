//! touch: makes the files it names that are not there yet, as POSIX's
//! touch does, none with `-c`. Meridian's files have no times yet, which
//! is what touch otherwise changes, so for a file there it has nothing to
//! do, `-a` and `-m` choose among no times, and `-r`, `-t` and `-d`,
//! which give the time to set, are refused. It ends with 0 when every
//! file named is there, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::abi::file::{OPEN_ALWAYS, WRITE};
use program::{Error, Handle, text};

program::main!(touch);

/// What touch's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    /// Whether a file not there is made.
    make: bool,
    paths: &'w [String],
}

/// What touch's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut make = true;
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
                'c' => make = false,
                'a' | 'm' => {}
                'r' | 't' | 'd' => {
                    return Err(format!(
                        "touch: -{letter}: Meridian's files have no times yet\n"
                    ));
                }
                _ => return Err(format!("touch: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    if words.is_empty() {
        return Err(String::from("touch: names a file at least\n"));
    }
    Ok(Asked { make, paths: words })
}

/// What touch does with files.
trait Files {
    /// Whether something is at `path`.
    fn there(&mut self, path: &str) -> bool;

    /// Makes an empty file at `path`, or leaves the one there.
    fn make(&mut self, path: &str) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// Makes the files `asked` names, as far as it asks; whether every one
/// is there after.
fn touch_all(files: &mut impl Files, asked: &Asked) -> bool {
    let mut all = true;
    for path in asked.paths {
        if files.there(path) || !asked.make {
            continue;
        }
        if let Err(e) = files.make(path) {
            files.say(&text::trouble("touch", path, e));
            all = false;
        }
    }
    all
}

/// Meridian's files.
struct System;

impl Files for System {
    fn there(&mut self, path: &str) -> bool {
        program::open(path, program::abi::file::READ).is_ok_and(|file| {
            let _ = program::close(file);
            true
        })
    }

    fn make(&mut self, path: &str) -> Result<(), Error> {
        // Opened always, a file there is kept as it is.
        let file = program::open_with(path, WRITE, OPEN_ALWAYS)?;
        program::close(file)
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn touch(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => u64::from(!touch_all(&mut System, &asked)),
        Err(why) => {
            let _ = program::write_error(&why);
            1
        }
    }
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

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in touch reaches the host's own files.
    fn confined(root: &std::path::Path, path: &str) -> std::path::PathBuf {
        let mut depth = 0usize;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .unwrap_or_else(|| panic!("{path} climbs out of the test's directory"));
                }
                _ => depth += 1,
            }
        }
        root.join(path.trim_start_matches('/'))
    }

    /// Files among the host's temporary ones, gone once dropped, and what
    /// touch said.
    struct Host {
        root: std::path::PathBuf,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-touch-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("d")).unwrap();
            std::fs::write(root.join("kept"), "held").unwrap();
            Host {
                root,
                said: String::new(),
            }
        }

        fn holds(&self, path: &str) -> Option<String> {
            std::fs::read_to_string(self.root.join(path)).ok()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Files for Host {
        fn there(&mut self, path: &str) -> bool {
            confined(&self.root, path).exists()
        }

        fn make(&mut self, path: &str) -> Result<(), Error> {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(confined(&self.root, path))
                .map(|_| ())
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => Error::NotFound,
                    _ => Error::Unwritable,
                })
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How touch with the words `text` ran on `host`: whether every file
    /// is there, and what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String) {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let all = touch_all(host, &taken);
        (all, core::mem::take(&mut host.said))
    }

    #[test]
    fn its_words_are_options_then_files() {
        let given = words("-c a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                make: false,
                paths: &given[1..]
            })
        );
        let given = words("-am a");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                make: true,
                paths: &given[1..]
            })
        );
        let given = words("-- -c");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                make: true,
                paths: &given[1..]
            })
        );
        for letter in ["r", "t", "d"] {
            assert_eq!(
                asked(&words(&format!("-{letter} x f"))),
                Err(format!(
                    "touch: -{letter}: Meridian's files have no times yet\n"
                ))
            );
        }
        assert_eq!(
            asked(&words("-x a")),
            Err("touch: -x is no option\n".into())
        );
        assert_eq!(
            asked(&words("-c")),
            Err("touch: names a file at least\n".into())
        );
        assert_eq!(
            asked(&words("")),
            Err("touch: names a file at least\n".into())
        );
    }

    #[test]
    fn files_not_there_are_made_empty_and_those_there_kept() {
        let mut host = Host::new("make");
        assert_eq!(ran(&mut host, "new kept d/inner"), (true, String::new()));
        assert_eq!(host.holds("new").as_deref(), Some(""));
        assert_eq!(
            host.holds("kept").as_deref(),
            Some("held"),
            "a file there is left as it is"
        );
        assert_eq!(host.holds("d/inner").as_deref(), Some(""));
        assert_eq!(
            ran(&mut host, "d"),
            (true, String::new()),
            "a directory is there too"
        );
        assert_eq!(ran(&mut host, "-c other"), (true, String::new()));
        assert_eq!(host.holds("other"), None, "-c makes nothing");
        assert_eq!(
            ran(&mut host, "nowhere/x new2"),
            (false, "touch: nowhere/x: no such file\n".into())
        );
        assert_eq!(
            host.holds("new2").as_deref(),
            Some(""),
            "the rest are made all the same"
        );
    }
}
