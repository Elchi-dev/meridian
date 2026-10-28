//! unlink: removes the one file it names, as POSIX's unlink does, with
//! no options and nothing more: a directory is no file it removes. It
//! ends with 0 when the file went, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::{Error, Handle, text};

program::main!(unlink);

/// The file unlink's words name, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<&str, String> {
    let words = match arguments.split_first() {
        Some((first, rest)) if first == "--" => rest,
        _ => arguments,
    };
    match words {
        [path] if path.starts_with('-') && path != "-" && words.len() == arguments.len() => {
            Err(format!("unlink: {path}: takes no options\n"))
        }
        [path] => Ok(path),
        _ => Err(String::from("unlink: removes one file\n")),
    }
}

/// What unlink does with files.
trait Files {
    /// Whether something is at `path`, and whether it is a directory.
    fn kind(&mut self, path: &str) -> Option<bool>;

    /// Removes the file at `path`.
    fn remove(&mut self, path: &str) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// Removes the file at `path`, saying what went wrong; whether it went.
fn unlink_one(files: &mut impl Files, path: &str) -> bool {
    let removed = match files.kind(path) {
        None => Err(Error::NotFound),
        Some(true) => {
            files.say(&format!(
                "unlink: {path}: a directory, which unlink does not remove\n"
            ));
            return false;
        }
        Some(false) => files.remove(path),
    };
    removed
        .inspect_err(|&e| files.say(&text::trouble("unlink", path, e)))
        .is_ok()
}

/// Meridian's files.
struct System;

impl Files for System {
    fn kind(&mut self, path: &str) -> Option<bool> {
        let file = program::open(path, program::abi::file::READ).ok()?;
        let info = program::info(file);
        let _ = program::close(file);
        Some(info.ok()?.directory)
    }

    fn remove(&mut self, path: &str) -> Result<(), Error> {
        program::remove(path)
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn unlink(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(path) => u64::from(!unlink_one(&mut System, path)),
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
    /// test, so that no mistake in unlink reaches the host's own files.
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
    /// unlink said.
    struct Host {
        root: std::path::PathBuf,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-unlink-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("d")).unwrap();
            std::fs::write(root.join("f"), "x").unwrap();
            std::fs::write(root.join("-dash"), "x").unwrap();
            Host {
                root,
                said: String::new(),
            }
        }

        fn there(&self, path: &str) -> bool {
            self.root.join(path).exists()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Files for Host {
        fn kind(&mut self, path: &str) -> Option<bool> {
            std::fs::metadata(confined(&self.root, path))
                .ok()
                .map(|m| m.is_dir())
        }

        fn remove(&mut self, path: &str) -> Result<(), Error> {
            std::fs::remove_file(confined(&self.root, path)).map_err(|_| Error::Unwritable)
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    fn ran(host: &mut Host, text: &str) -> (bool, String) {
        let given = words(text);
        let path = asked(&given).unwrap();
        let went = unlink_one(host, path);
        (went, core::mem::take(&mut host.said))
    }

    #[test]
    fn its_word_is_one_file() {
        assert_eq!(asked(&words("f")), Ok("f"));
        assert_eq!(
            asked(&words("-- -dash")),
            Ok("-dash"),
            "-- lets a name start with -"
        );
        assert_eq!(asked(&words("-")), Ok("-"), "- alone is a name");
        assert_eq!(
            asked(&words("-f")),
            Err("unlink: -f: takes no options\n".into())
        );
        for text in ["", "a b", "--", "-- a b"] {
            assert_eq!(
                asked(&words(text)),
                Err("unlink: removes one file\n".into()),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_file_goes_and_a_directory_stays() {
        let mut host = Host::new("go");
        assert_eq!(ran(&mut host, "f"), (true, String::new()));
        assert!(!host.there("f"));
        assert_eq!(ran(&mut host, "-- -dash"), (true, String::new()));
        assert!(!host.there("-dash"));
        assert_eq!(
            ran(&mut host, "d"),
            (
                false,
                "unlink: d: a directory, which unlink does not remove\n".into()
            )
        );
        assert!(host.there("d"));
        assert_eq!(
            ran(&mut host, "f"),
            (false, "unlink: f: no such file\n".into())
        );
    }
}
