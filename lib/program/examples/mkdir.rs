//! mkdir: makes directories, as POSIX's mkdir does; with `-p` first the
//! directories above each that are not there yet, and nothing for one
//! that is there already. Meridian's files have no modes, so `-m` is
//! refused. It ends with 0 when every directory was made, and 1
//! otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Error, Handle, text};

program::main!(mkdir);

/// What mkdir's words ask for: whether the directories above come too,
/// and the directories.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    parents: bool,
    paths: &'w [String],
}

/// What mkdir's words ask for, or why they cannot be taken.
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
                'm' => return Err(String::from("mkdir: -m: Meridian's files have no modes\n")),
                _ => return Err(format!("mkdir: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    if words.is_empty() {
        return Err(String::from("mkdir: makes a directory at least\n"));
    }
    Ok(Asked {
        parents,
        paths: words,
    })
}

/// What mkdir does with files.
trait Files {
    /// Whether something is at `path`, and whether it is a directory.
    fn kind(&mut self, path: &str) -> Option<bool>;

    /// Makes the directory `path`.
    fn make(&mut self, path: &str) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// The paths of the directories `path` passes through, and its own, the
/// highest first.
fn ways(path: &str) -> Vec<String> {
    let mut at = String::from(if path.starts_with('/') { "/" } else { "" });
    let mut found = Vec::new();
    for name in path.split('/').filter(|n| !n.is_empty()) {
        if !at.is_empty() && !at.ends_with('/') {
            at.push('/');
        }
        at.push_str(name);
        found.push(at.clone());
    }
    found
}

/// Makes the directory `path` as `asked` says, saying what went wrong;
/// whether it went.
fn make(files: &mut impl Files, asked: &Asked, path: &str) -> bool {
    let wanted = if asked.parents {
        ways(path)
    } else {
        Vec::from([String::from(path)])
    };
    for way in &wanted {
        let made = match (files.kind(way), asked.parents) {
            (Some(true), true) => continue,
            (Some(false), true) => Err(Error::NotADirectory),
            (Some(_), false) => Err(Error::Exists),
            (None, _) => files.make(way),
        };
        let why = match made {
            Ok(()) => continue,
            Err(Error::Exists) => format!("mkdir: {way}: there already\n"),
            Err(Error::NotADirectory) => format!("mkdir: {way}: a file, no directory\n"),
            Err(e) => text::trouble("mkdir", way, e),
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

    fn make(&mut self, path: &str) -> Result<(), Error> {
        program::make_directory(path)
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn mkdir(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => {
            let mut all = true;
            for path in asked.paths {
                all &= make(&mut System, &asked, path);
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
            asked(&words("-m 755 a")),
            Err("mkdir: -m: Meridian's files have no modes\n".into())
        );
        assert_eq!(
            asked(&words("-x a")),
            Err("mkdir: -x is no option\n".into())
        );
        assert_eq!(
            asked(&words("-p")),
            Err("mkdir: makes a directory at least\n".into())
        );
    }

    #[test]
    fn the_ways_to_a_path_come_highest_first() {
        assert_eq!(ways("a/b/c"), ["a", "a/b", "a/b/c"]);
        assert_eq!(ways("/a//b/"), ["/a", "/a/b"]);
        assert_eq!(ways("/"), Vec::<String>::new());
    }

    /// Directories among the host's temporary files, gone once dropped,
    /// and what mkdir said.
    struct Host {
        root: std::path::PathBuf,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-mkdir-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("file"), "x").unwrap();
            Host {
                root,
                said: String::new(),
            }
        }

        fn directory(&self, path: &str) -> bool {
            confined(&self.root, path).is_dir()
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

        fn make(&mut self, path: &str) -> Result<(), Error> {
            std::fs::create_dir(confined(&self.root, path)).map_err(|e| match e.kind() {
                std::io::ErrorKind::AlreadyExists => Error::Exists,
                std::io::ErrorKind::NotFound => Error::NotFound,
                _ => Error::Unwritable,
            })
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How mkdir with the words `text` ran on `host`: whether all went,
    /// and what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String) {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let mut all = true;
        for path in taken.paths {
            all &= make(host, &taken, path);
        }
        (all, core::mem::take(&mut host.said))
    }

    #[test]
    fn a_directory_is_made_and_with_p_those_above_it() {
        let mut host = Host::new("made");
        assert_eq!(ran(&mut host, "a"), (true, String::new()));
        assert!(host.directory("a"));
        assert_eq!(
            ran(&mut host, "b/c"),
            (false, "mkdir: b/c: no such file\n".into()),
            "nothing above it"
        );
        assert_eq!(
            ran(&mut host, "-p b/c/d a"),
            (true, String::new()),
            "and nothing for one there already"
        );
        assert!(host.directory("b/c/d"));
        assert_eq!(
            ran(&mut host, "a"),
            (false, "mkdir: a: there already\n".into())
        );
        assert_eq!(
            ran(&mut host, "file"),
            (false, "mkdir: file: there already\n".into()),
            "a file is there too"
        );
        assert_eq!(
            ran(&mut host, "-p file/x"),
            (false, "mkdir: file: a file, no directory\n".into())
        );
        assert_eq!(
            ran(&mut host, "-p /e/f"),
            (true, String::new()),
            "from the root"
        );
        assert!(host.directory("e/f"));
    }
}
