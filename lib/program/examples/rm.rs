//! rm: removes files, as POSIX's rm does: a directory only with `-R` or
//! `-r`, and then all it holds first; with `-i` it asks at the console
//! before each, and with `-f` it asks nothing and says nothing of a file
//! that is not there. A path ending in `.` or `..` is refused, and so is
//! the root. It ends with 0 when everything named went, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Error, Handle, text};

program::main!(rm);

/// What rm's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    /// Whether nothing is asked and a file not there is no trouble.
    force: bool,
    /// Whether the one at the console is asked before each removal.
    ask: bool,
    /// Whether directories go too, with all they hold.
    deep: bool,
    paths: &'w [String],
}

/// What rm's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let (mut force, mut ask, mut deep) = (false, false, false);
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
            // Of -f and -i, the one given last counts.
            match letter {
                'f' => (force, ask) = (true, false),
                'i' => (force, ask) = (false, true),
                'R' | 'r' => deep = true,
                _ => return Err(format!("rm: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    if words.is_empty() && !force {
        return Err(String::from("rm: removes a file at least\n"));
    }
    Ok(Asked {
        force,
        ask,
        deep,
        paths: words,
    })
}

/// Whether `path` is one rm refuses: one whose last name is `.` or
/// `..`, or the root.
fn refused(path: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    (trimmed.is_empty() && !path.is_empty()) || matches!(last, "." | "..")
}

/// What rm does with files.
trait Files {
    /// Whether the file at `path` is a directory.
    fn directory(&mut self, path: &str) -> Result<bool, Error>;

    /// The names in the directory at `path`.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// Removes the file or the empty directory at `path`.
    fn remove(&mut self, path: &str) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);

    /// Asks `question` at the console, and answers whether the answer is
    /// yes.
    fn yes(&mut self, question: &str) -> bool;
}

/// `name` in the directory at `directory`.
fn joined(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// Removes what is at `path` as `asked` says, saying what went wrong;
/// whether it went.
fn remove(files: &mut impl Files, asked: &Asked, path: &str) -> bool {
    let directory = match files.directory(path) {
        Ok(directory) => directory,
        Err(Error::NotFound) if asked.force => return true,
        Err(e) => {
            files.say(&text::trouble("rm", path, e));
            return false;
        }
    };
    if directory && !asked.deep {
        files.say(&format!(
            "rm: {path}: a directory, which goes only with -R\n"
        ));
        return false;
    }
    if directory {
        if asked.ask && !files.yes(&format!("rm: go into {path}? ")) {
            return true;
        }
        let names = match files.names(path) {
            Ok(names) => names,
            Err(e) => {
                files.say(&text::trouble("rm", path, e));
                return false;
            }
        };
        // Each goes, and what could not go keeps the directory too.
        let mut all = true;
        for name in names {
            all &= remove(files, asked, &joined(path, &name));
        }
        if !all {
            return false;
        }
    }
    if asked.ask && !files.yes(&format!("rm: remove {path}? ")) {
        return true;
    }
    match files.remove(path) {
        Ok(()) => true,
        Err(Error::NotEmpty) => {
            files.say(&format!("rm: {path}: still holds what was kept\n"));
            false
        }
        Err(e) => {
            files.say(&text::trouble("rm", path, e));
            false
        }
    }
}

/// Removes every path `asked` names, and answers the status rm ends
/// with.
fn remove_all(files: &mut impl Files, asked: &Asked) -> u64 {
    let mut all = true;
    for path in asked.paths {
        if refused(path) {
            files.say(&format!("rm: {path}: refused\n"));
            all = false;
            continue;
        }
        all &= remove(files, asked, path);
    }
    u64::from(!all)
}

/// Meridian's files, and the one at its console.
struct System;

impl Files for System {
    fn directory(&mut self, path: &str) -> Result<bool, Error> {
        let file = program::open(path, program::abi::file::READ)?;
        let info = program::info(file);
        let _ = program::close(file);
        Ok(info?.directory)
    }

    fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
        let directory = program::open(path, program::abi::file::READ)?;
        let mut names = Vec::new();
        let mut room = [0u8; 1024];
        let mut index = 0;
        let listed = loop {
            match program::list(directory, index, &mut room) {
                Ok((len, _)) => names.push(String::from_utf8_lossy(&room[..len]).into_owned()),
                Err(Error::NotFound) => break Ok(names),
                Err(e) => break Err(e),
            }
            index += 1;
        };
        let _ = program::close(directory);
        listed
    }

    fn remove(&mut self, path: &str) -> Result<(), Error> {
        program::remove(path)
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }

    fn yes(&mut self, question: &str) -> bool {
        let _ = program::write_error(question);
        program::typed_line().is_some_and(|answer| answer.starts_with(['y', 'Y']))
    }
}

#[cfg_attr(test, allow(dead_code))]
fn rm(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => remove_all(&mut System, &asked),
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
    fn its_words_are_options_and_paths() {
        let given = words("-rf a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                force: true,
                ask: false,
                deep: true,
                paths: &given[1..]
            })
        );
        let given = words("-f -i -R a");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                force: false,
                ask: true,
                deep: true,
                paths: &given[3..]
            }),
            "the last of -f and -i"
        );
        let given = words("-if a");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                force: true,
                ask: false,
                deep: false,
                paths: &given[1..]
            })
        );
        let given = words("-- -f");
        assert_eq!(asked(&given).unwrap().paths, ["-f"]);
        let given = words("-f");
        assert_eq!(
            asked(&given).unwrap().paths,
            Vec::<String>::new(),
            "-f alone removes nothing, and is fine"
        );
        assert_eq!(
            asked(&words("")),
            Err("rm: removes a file at least\n".into())
        );
        assert_eq!(asked(&words("-x a")), Err("rm: -x is no option\n".into()));
    }

    #[test]
    fn the_root_and_paths_ending_in_dots_are_refused() {
        for path in ["/", "//", ".", "..", "a/.", "a/..", "a/../", "./"] {
            assert!(refused(path), "{path}");
        }
        for path in ["a", "a/b", ".a", "a..", "...", "a/.b", ""] {
            assert!(!refused(path), "{path}");
        }
        assert_eq!(joined("a", "b"), "a/b");
        assert_eq!(joined("a/", "b"), "a/b");
    }

    /// Files among the host's temporary files, gone once dropped, the
    /// answers typed to rm, and what it asked and said.
    struct Host {
        root: std::path::PathBuf,
        answers: Vec<bool>,
        asked: Vec<String>,
        said: String,
    }

    impl Host {
        /// The files `paths` name, those ending in `/` directories.
        fn new(name: &str, paths: &[&str]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-rm-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for path in paths {
                let at = root.join(path.trim_end_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, "x").unwrap();
                }
            }
            Host {
                root,
                answers: Vec::new(),
                asked: Vec::new(),
                said: String::new(),
            }
        }

        /// What is left, every path under the root in order.
        fn left(&self) -> Vec<String> {
            fn walk(at: &std::path::Path, from: &std::path::Path, all: &mut Vec<String>) {
                for entry in std::fs::read_dir(at).unwrap() {
                    let path = entry.unwrap().path();
                    let name = path
                        .strip_prefix(from)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    if path.is_dir() {
                        all.push(name + "/");
                        walk(&path, from, all);
                    } else {
                        all.push(name);
                    }
                }
            }
            let mut all = Vec::new();
            walk(&self.root, &self.root, &mut all);
            all.sort();
            all
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            std::io::ErrorKind::DirectoryNotEmpty => Error::NotEmpty,
            std::io::ErrorKind::PermissionDenied => Error::Denied,
            _ => Error::Unwritable,
        }
    }

    impl Files for Host {
        fn directory(&mut self, path: &str) -> Result<bool, Error> {
            std::fs::metadata(confined(&self.root, path))
                .map(|m| m.is_dir())
                .map_err(|e| error(&e))
        }

        fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
            let listed = std::fs::read_dir(confined(&self.root, path)).map_err(|e| error(&e))?;
            let mut names: Vec<String> = listed
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            Ok(names)
        }

        fn remove(&mut self, path: &str) -> Result<(), Error> {
            let at = confined(&self.root, path);
            if at.is_dir() {
                std::fs::remove_dir(at).map_err(|e| error(&e))
            } else {
                std::fs::remove_file(at).map_err(|e| error(&e))
            }
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }

        fn yes(&mut self, question: &str) -> bool {
            self.asked.push(question.into());
            !self.answers.is_empty() && self.answers.remove(0)
        }
    }

    /// How rm with the words `text` ran on `host`: its status and what it
    /// said.
    fn ran(host: &mut Host, text: &str) -> (u64, String) {
        let given = words(text);
        let status = remove_all(host, &asked(&given).unwrap());
        (status, core::mem::take(&mut host.said))
    }

    #[test]
    fn files_go_and_directories_only_with_r() {
        let mut host = Host::new("plain", &["a", "b", "d/x", "d/e/y", "f/"]);
        assert_eq!(ran(&mut host, "a b"), (0, String::new()));
        assert_eq!(ran(&mut host, "d f"), (1, "rm: d: a directory, which goes only with -R\nrm: f: a directory, which goes only with -R\n".into()));
        assert_eq!(host.left(), ["d/", "d/e/", "d/e/y", "d/x", "f/"]);
        assert_eq!(
            ran(&mut host, "-r d"),
            (0, String::new()),
            "and all it holds"
        );
        assert_eq!(ran(&mut host, "-R f"), (0, String::new()));
        assert_eq!(host.left(), Vec::<String>::new());
    }

    #[test]
    fn f_says_nothing_of_what_is_not_there() {
        let mut host = Host::new("force", &["a"]);
        assert_eq!(
            ran(&mut host, "nope a"),
            (1, "rm: nope: no such file\n".into())
        );
        assert_eq!(ran(&mut host, "-f nope a"), (0, String::new()));
        assert_eq!(ran(&mut host, "-f"), (0, String::new()));
    }

    #[test]
    fn i_asks_before_each_and_keeps_what_it_is_told_to() {
        let mut host = Host::new("ask", &["a", "b", "d/x", "d/y"]);
        host.answers = Vec::from([true, false]);
        assert_eq!(ran(&mut host, "-i a b"), (0, String::new()));
        assert_eq!(host.asked, ["rm: remove a? ", "rm: remove b? "]);
        assert_eq!(host.left(), ["b", "d/", "d/x", "d/y"]);
        host.asked.clear();
        host.answers = Vec::from([true, true, false]);
        assert_eq!(
            ran(&mut host, "-ri d"),
            (0, String::new()),
            "a file kept is no trouble"
        );
        assert_eq!(
            host.asked,
            [
                "rm: go into d? ",
                "rm: remove d/x? ",
                "rm: remove d/y? ",
                "rm: remove d? "
            ]
        );
        assert_eq!(host.left(), ["b", "d/", "d/y"]);
        host.asked.clear();
        host.answers = Vec::from([true, false, true]);
        assert_eq!(
            ran(&mut host, "-ri d"),
            (1, "rm: d: still holds what was kept\n".into()),
            "a directory that keeps a file does not go"
        );
        assert_eq!(host.left(), ["b", "d/", "d/y"]);
        host.asked.clear();
        host.answers = Vec::from([false]);
        assert_eq!(
            ran(&mut host, "-ri d"),
            (0, String::new()),
            "not gone into, kept whole"
        );
        assert_eq!(host.asked, ["rm: go into d? "]);
    }

    #[test]
    fn refused_paths_are_said_and_the_rest_go() {
        let mut host = Host::new("refused", &["a", "d/x"]);
        assert_eq!(
            ran(&mut host, "-rf d/.. a"),
            (1, "rm: d/..: refused\n".into())
        );
        assert_eq!(host.left(), ["d/", "d/x"]);
        assert_eq!(ran(&mut host, "-rf /").0, 1);
    }

    #[test]
    fn what_cannot_go_is_said_and_keeps_what_holds_it() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut host = Host::new("denied", &["d/held/y", "d/sealed/x"]);
        let set = |host: &Host, path: &str, mode| {
            let mode = std::fs::Permissions::from_mode(mode);
            std::fs::set_permissions(host.root.join(path), mode).unwrap();
        };
        // A directory that cannot be listed, and one whose file cannot go.
        set(&host, "d/sealed", 0o000);
        set(&host, "d/held", 0o555);
        assert!(
            std::fs::read_dir(host.root.join("d/sealed")).is_err(),
            "run as root, nothing is sealed"
        );
        assert_eq!(
            ran(&mut host, "-r d/sealed"),
            (1, "rm: d/sealed: not allowed\n".into())
        );
        assert_eq!(
            ran(&mut host, "-r d/held"),
            (1, "rm: d/held/y: not allowed\n".into()),
            "and what holds a file kept is not tried"
        );
        assert_eq!(
            ran(&mut host, "-r d"),
            (
                1,
                "rm: d/held/y: not allowed\nrm: d/sealed: not allowed\n".into()
            ),
            "nor what holds those"
        );
        set(&host, "d/sealed", 0o755);
        set(&host, "d/held", 0o755);
        assert_eq!(
            host.left(),
            ["d/", "d/held/", "d/held/y", "d/sealed/", "d/sealed/x"]
        );
    }
}
