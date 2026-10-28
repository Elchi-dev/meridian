//! rmdir: removes empty directories, as POSIX's rmdir does; with `-p`
//! then each directory above it in its path too, as long as each goes.
//! A file is no directory and stays. It ends with 0 when every directory
//! named went, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Error, Handle, text};

program::main!(rmdir);

/// What rmdir's words ask for: whether the directories above go too,
/// and the directories.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    parents: bool,
    paths: &'w [String],
}

/// What rmdir's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut parents = false;
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
                'p' => parents = true,
                _ => return Err(format!("rmdir: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    if words.is_empty() {
        return Err(String::from("rmdir: removes a directory at least\n"));
    }
    Ok(Asked {
        parents,
        paths: words,
    })
}

/// What rmdir does with files.
trait Files {
    /// Whether something is at `path`, and whether it is a directory.
    fn kind(&mut self, path: &str) -> Option<bool>;

    /// Removes the empty directory `path`.
    fn remove(&mut self, path: &str) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// The directories `-p` removes for `path`: its own, then each above it
/// in the path, the lowest first.
fn ways(path: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut at = path.trim_end_matches('/');
    while !at.is_empty() {
        found.push(String::from(at));
        at = match at.rfind('/') {
            Some(slash) => at[..slash].trim_end_matches('/'),
            None => "",
        };
    }
    found
}

/// Removes the directory `path` as `asked` says, saying what went
/// wrong; whether it went.
fn remove(files: &mut impl Files, asked: &Asked, path: &str) -> bool {
    let wanted = if asked.parents {
        ways(path)
    } else {
        Vec::from([String::from(path)])
    };
    for way in &wanted {
        let removed = match files.kind(way) {
            None => Err(Error::NotFound),
            Some(false) => Err(Error::NotADirectory),
            Some(true) => files.remove(way),
        };
        let why = match removed {
            Ok(()) => continue,
            Err(Error::NotEmpty) => format!("rmdir: {way}: holds something\n"),
            Err(Error::NotADirectory) => format!("rmdir: {way}: no directory\n"),
            Err(e) => text::trouble("rmdir", way, e),
        };
        files.say(&why);
        return false;
    }
    true
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
fn rmdir(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => {
            let mut all = true;
            for path in asked.paths {
                all &= remove(&mut System, &asked, path);
            }
            u64::from(!all)
        }
        Err(why) => {
            let _ = program::write_error(&why);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where `path` is under `root`, a path from the top taken from
    /// `root`. One that climbs out of it stops the test, so that no
    /// mistake in the program reaches the host's own files.
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

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    #[test]
    fn its_words_are_p_and_directories() {
        let given = words("-p a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                parents: true,
                paths: &given[1..]
            })
        );
        let given = words("-- -p");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                parents: false,
                paths: &given[1..]
            })
        );
        assert_eq!(
            asked(&words("-x a")),
            Err("rmdir: -x is no option\n".into())
        );
        assert_eq!(
            asked(&words("")),
            Err("rmdir: removes a directory at least\n".into())
        );
    }

    #[test]
    fn p_goes_up_the_path_from_its_end() {
        assert_eq!(ways("a/b/c"), ["a/b/c", "a/b", "a"]);
        assert_eq!(ways("a//b/"), ["a//b", "a"]);
        assert_eq!(ways("/a/b"), ["/a/b", "/a"], "never the root");
        assert_eq!(ways("a"), ["a"]);
    }

    /// Directories among the host's temporary files, gone once dropped,
    /// and what rmdir said.
    struct Host {
        root: std::path::PathBuf,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-rmdir-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("a/b/c")).unwrap();
            std::fs::create_dir_all(root.join("full/empty")).unwrap();
            std::fs::write(root.join("full/file"), "x").unwrap();
            Host {
                root,
                said: String::new(),
            }
        }

        fn there(&self, path: &str) -> bool {
            confined(&self.root, path).exists()
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
            std::fs::remove_dir(confined(&self.root, path)).map_err(|e| match e.kind() {
                std::io::ErrorKind::DirectoryNotEmpty => Error::NotEmpty,
                _ => Error::Unwritable,
            })
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How rmdir with the words `text` ran on `host`: whether all went,
    /// and what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String) {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let mut all = true;
        for path in taken.paths {
            all &= remove(host, &taken, path);
        }
        (all, core::mem::take(&mut host.said))
    }

    #[test]
    fn empty_directories_go_and_with_p_those_above() {
        let mut host = Host::new("gone");
        assert_eq!(
            ran(&mut host, "a/b"),
            (false, "rmdir: a/b: holds something\n".into())
        );
        assert_eq!(ran(&mut host, "a/b/c"), (true, String::new()));
        assert!(host.there("a/b") && !host.there("a/b/c"));
        assert_eq!(ran(&mut host, "-p a/b"), (true, String::new()));
        assert!(!host.there("a"));
        assert_eq!(
            ran(&mut host, "-p full/empty"),
            (false, "rmdir: full: holds something\n".into()),
            "it goes up as long as each goes"
        );
        assert!(!host.there("full/empty") && host.there("full"));
        assert_eq!(
            ran(&mut host, "full/file"),
            (false, "rmdir: full/file: no directory\n".into())
        );
        assert!(host.there("full/file"));
        assert_eq!(
            ran(&mut host, "nope"),
            (false, "rmdir: nope: no such file\n".into())
        );
    }
}
