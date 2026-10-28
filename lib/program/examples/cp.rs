//! cp: copies files, as POSIX's cp does: one file onto another, made new
//! or emptied, or each of several into a directory under its own name,
//! and with `-R` or `-r` a directory and all it holds. `-i` asks at the
//! console before a file is written over, and `-f` removes a file that
//! cannot be opened to be written over and tries again. Meridian's files
//! have no modes, owners or times to keep, so `-p` keeps what there is,
//! and with no symbolic links `-H`, `-L` and `-P` change nothing. A file
//! is never copied onto itself, nor a directory into itself. It ends
//! with 0 when everything was copied, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use program::abi::file::{CREATE_ALWAYS, READ, WRITE};
use program::{Error, Handle, text};

program::main!(cp);

/// What cp's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    deep: bool,
    /// Whether the one at the console is asked before a file is written
    /// over.
    ask: bool,
    /// Whether a file that cannot be written over is removed first.
    force: bool,
    sources: &'w [String],
    target: &'w str,
}

/// What cp's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let (mut deep, mut ask, mut force) = (false, false, false);
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
                'R' | 'r' => deep = true,
                'i' => ask = true,
                'f' => force = true,
                'p' | 'H' | 'L' | 'P' => {}
                _ => return Err(format!("cp: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    let Some((target, sources)) = words
        .split_last()
        .filter(|(_, sources)| !sources.is_empty())
    else {
        return Err(String::from(
            "cp: copies a file to another, or files into a directory\n",
        ));
    };
    Ok(Asked {
        deep,
        ask,
        force,
        sources,
        target,
    })
}

/// What cp does with files.
trait Files {
    /// Whether the file at `path` is a directory.
    fn directory(&mut self, path: &str) -> Result<bool, Error>;

    /// The names in the directory at `path`.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// Copies the file at `from` to `to`, made new or emptied.
    fn copy(&mut self, from: &str, to: &str) -> Result<(), Error>;

    /// Makes a directory at `path`.
    fn make_directory(&mut self, path: &str) -> Result<(), Error>;

    /// Removes the file at `path`.
    fn remove(&mut self, path: &str) -> Result<(), Error>;

    /// `path` from the root, as the tree of files compares paths.
    fn compared(&mut self, path: &str) -> String;

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

/// The last name of `path`.
fn last_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Copies the file at `from` to `to` as `asked` says, saying what went
/// wrong; whether it was copied, or rightly left.
fn copy_file(files: &mut impl Files, asked: &Asked, from: &str, to: &str) -> bool {
    if files.compared(from) == files.compared(to) {
        files.say(&format!("cp: {from} and {to} are one file\n"));
        return false;
    }
    let there = files.directory(to).is_ok();
    if there && asked.ask && !files.yes(&format!("cp: write over {to}? ")) {
        return true;
    }
    let copied = match files.copy(from, to) {
        // One that cannot be opened goes, and a new one is made.
        Err(Error::Denied | Error::Unwritable) if there && asked.force => {
            files.remove(to).and_then(|()| files.copy(from, to))
        }
        other => other,
    };
    copied
        .inspect_err(|&e| files.say(&text::trouble("cp", &format!("{from} to {to}"), e)))
        .is_ok()
}

/// Copies what is at `from` to `to`, a directory and all it holds when
/// `asked` goes deep; whether everything was.
fn copy(files: &mut impl Files, asked: &Asked, from: &str, to: &str) -> bool {
    let directory = match files.directory(from) {
        Ok(directory) => directory,
        Err(e) => {
            files.say(&text::trouble("cp", from, e));
            return false;
        }
    };
    if !directory {
        return copy_file(files, asked, from, to);
    }
    if !asked.deep {
        files.say(&format!(
            "cp: {from}: a directory, which is copied only with -R\n"
        ));
        return false;
    }
    let (source, target) = (files.compared(from), files.compared(to));
    if target.starts_with(&joined(&source, "")) {
        files.say(&format!(
            "cp: {from}: a directory cannot be copied into itself\n"
        ));
        return false;
    }
    match files.directory(to) {
        Ok(true) => {}
        Ok(false) => {
            files.say(&format!(
                "cp: {to}: a file, which a directory cannot be copied onto\n"
            ));
            return false;
        }
        Err(_) => {
            if let Err(e) = files.make_directory(to) {
                files.say(&text::trouble("cp", to, e));
                return false;
            }
        }
    }
    let names = match files.names(from) {
        Ok(names) => names,
        Err(e) => {
            files.say(&text::trouble("cp", from, e));
            return false;
        }
    };
    let mut all = true;
    for name in names {
        all &= copy(files, asked, &joined(from, &name), &joined(to, &name));
    }
    all
}

/// Copies what `asked` names: onto the target, or into it when it is a
/// directory or there is more than one source; whether everything was.
fn copy_all(files: &mut impl Files, asked: &Asked) -> bool {
    let into = files.directory(asked.target) == Ok(true);
    if !into && asked.sources.len() > 1 {
        files.say(&format!(
            "cp: {}: no directory to copy files into\n",
            asked.target
        ));
        return false;
    }
    let mut all = true;
    for source in asked.sources {
        let to = if into {
            joined(asked.target, last_name(source))
        } else {
            asked.target.to_string()
        };
        all &= copy(files, asked, source, &to);
    }
    all
}

/// Meridian's files.
struct System;

impl Files for System {
    fn directory(&mut self, path: &str) -> Result<bool, Error> {
        let file = program::open(path, READ)?;
        let info = program::info(file);
        let _ = program::close(file);
        Ok(info?.directory)
    }

    fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
        let directory = program::open(path, READ)?;
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

    fn copy(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let source = program::open(from, READ)?;
        let copied = program::open_with(to, WRITE, CREATE_ALWAYS).and_then(|target| {
            let mut room = [0u8; 4096];
            let mut at = 0;
            let written = loop {
                let len = match program::read(source, at, &mut room) {
                    Ok(0) => break Ok(()),
                    Ok(len) => len,
                    Err(e) => break Err(e),
                };
                if let Err(e) = program::write_file(target, at, &room[..len]) {
                    break Err(e);
                }
                at += len as u64;
            };
            let _ = program::close(target);
            written
        });
        let _ = program::close(source);
        copied
    }

    fn make_directory(&mut self, path: &str) -> Result<(), Error> {
        program::make_directory(path)
    }

    fn remove(&mut self, path: &str) -> Result<(), Error> {
        program::remove(path)
    }

    fn compared(&mut self, path: &str) -> String {
        program::full_path(path).to_uppercase()
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
fn cp(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => u64::from(!copy_all(&mut System, &asked)),
        Err(why) => {
            let _ = program::write_error(&why);
            1
        }
    }
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

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in cp reaches the host's own files.
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

    /// Files among the host's temporary ones, gone once dropped, the
    /// answers to give at the console, and what cp asked and said.
    struct Host {
        root: std::path::PathBuf,
        answers: Vec<bool>,
        asked: Vec<String>,
        said: String,
    }

    impl Host {
        /// The files `paths` name, those ending in `/` directories, each
        /// file holding its own name.
        fn new(name: &str, paths: &[&str]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-cp-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for path in paths {
                let at = root.join(path.trim_end_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, path.as_bytes()).unwrap();
                }
            }
            Host {
                root,
                answers: Vec::new(),
                asked: Vec::new(),
                said: String::new(),
            }
        }

        fn holds(&self, path: &str) -> Option<String> {
            std::fs::read_to_string(self.root.join(path)).ok()
        }

        /// Every path under the root, in order, directories with a `/`.
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
            std::io::ErrorKind::PermissionDenied => Error::Denied,
            std::io::ErrorKind::IsADirectory => Error::NotAFile,
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

        fn copy(&mut self, from: &str, to: &str) -> Result<(), Error> {
            let bytes = std::fs::read(confined(&self.root, from)).map_err(|e| error(&e))?;
            std::fs::write(confined(&self.root, to), bytes).map_err(|e| error(&e))
        }

        fn make_directory(&mut self, path: &str) -> Result<(), Error> {
            std::fs::create_dir(confined(&self.root, path)).map_err(|e| error(&e))
        }

        fn remove(&mut self, path: &str) -> Result<(), Error> {
            std::fs::remove_file(confined(&self.root, path)).map_err(|e| error(&e))
        }

        fn compared(&mut self, path: &str) -> String {
            // The path from the root of the test's directory, its . and
            // .. worked out.
            let mut names: Vec<&str> = Vec::new();
            for name in path.split('/') {
                match name {
                    "" | "." => {}
                    ".." => {
                        names.pop();
                    }
                    name => names.push(name),
                }
            }
            format!("/{}", names.join("/"))
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }

        fn yes(&mut self, question: &str) -> bool {
            self.asked.push(question.into());
            !self.answers.is_empty() && self.answers.remove(0)
        }
    }

    /// How cp with the words `text` ran on `host`: whether all went, and
    /// what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String) {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let all = copy_all(host, &taken);
        (all, core::mem::take(&mut host.said))
    }

    fn went() -> (bool, String) {
        (true, String::new())
    }

    #[test]
    fn its_words_are_options_sources_and_a_target() {
        let given = words("-Rif a b c");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                deep: true,
                ask: true,
                force: true,
                sources: &given[1..3],
                target: "c",
            })
        );
        let given = words("-r -p -H -L -P a b");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (taken.deep, taken.ask, taken.force, taken.target),
            (true, false, false, "b")
        );
        let given = words("-- -a b");
        assert_eq!(
            asked(&given).map(|a| (a.sources.len(), a.target)),
            Ok((1, "b"))
        );
        assert_eq!(asked(&words("-x a b")), Err("cp: -x is no option\n".into()));
        for text in ["", "a", "-R a"] {
            assert_eq!(
                asked(&words(text)),
                Err("cp: copies a file to another, or files into a directory\n".into()),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_file_is_copied_onto_another_or_into_a_directory() {
        let mut host = Host::new("files", &["a", "b", "d/"]);
        assert_eq!(ran(&mut host, "a c"), went());
        assert_eq!(host.holds("c").as_deref(), Some("a"));
        assert_eq!(ran(&mut host, "a b"), went());
        assert_eq!(
            host.holds("b").as_deref(),
            Some("a"),
            "a file there is written over"
        );
        assert_eq!(ran(&mut host, "a c d"), went());
        assert_eq!(host.holds("d/a").as_deref(), Some("a"));
        assert_eq!(host.holds("d/c").as_deref(), Some("a"));
        assert_eq!(ran(&mut host, "a d/"), went(), "a directory with a slash");
        assert_eq!(
            ran(&mut host, "a c e"),
            (false, "cp: e: no directory to copy files into\n".into())
        );
        assert_eq!(
            ran(&mut host, "nope a d"),
            (false, "cp: nope: no such file\n".into()),
            "what is not there is said, the rest copied"
        );
        assert_eq!(
            ran(&mut host, "a nowhere/x"),
            (false, "cp: a to nowhere/x: no such file\n".into())
        );
    }

    #[test]
    fn a_file_is_never_copied_onto_itself() {
        let mut host = Host::new("itself", &["a", "d/x"]);
        assert_eq!(
            ran(&mut host, "a a"),
            (false, "cp: a and a are one file\n".into())
        );
        assert_eq!(
            ran(&mut host, "d/x d/../d/x"),
            (false, "cp: d/x and d/../d/x are one file\n".into())
        );
        assert_eq!(
            ran(&mut host, "d/x d"),
            (false, "cp: d/x and d/x are one file\n".into())
        );
        assert_eq!(
            host.holds("a").as_deref(),
            Some("a"),
            "and so stays as it was"
        );
    }

    #[test]
    fn r_copies_a_directory_and_all_it_holds() {
        let mut host = Host::new("deep", &["t/a", "t/u/b", "t/u/v/", "e/"]);
        assert_eq!(
            ran(&mut host, "t n"),
            (
                false,
                "cp: t: a directory, which is copied only with -R\n".into()
            )
        );
        assert_eq!(ran(&mut host, "-R t n"), went());
        assert_eq!(
            ran(&mut host, "-R t e"),
            went(),
            "into a directory there, under its name"
        );
        assert_eq!(
            host.left(),
            [
                "e/", "e/t/", "e/t/a", "e/t/u/", "e/t/u/b", "e/t/u/v/", "n/", "n/a", "n/u/",
                "n/u/b", "n/u/v/", "t/", "t/a", "t/u/", "t/u/b", "t/u/v/",
            ]
        );
        assert_eq!(
            ran(&mut host, "-R t/ e"),
            went(),
            "a trailing slash names the directory"
        );
        assert_eq!(ran(&mut host, "-R n e/t/u"), went());
        assert_eq!(host.holds("e/t/u/n/u/b").as_deref(), Some("t/u/b"));
    }

    #[test]
    fn a_directory_goes_neither_into_itself_nor_onto_a_file() {
        let mut host = Host::new("into", &["t/a", "f"]);
        assert_eq!(
            ran(&mut host, "-R t t/inner"),
            (
                false,
                "cp: t: a directory cannot be copied into itself\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "-R t t"),
            (
                false,
                "cp: t: a directory cannot be copied into itself\n".into()
            ),
            "t into t is t/t"
        );
        assert_eq!(
            ran(&mut host, "-R t f"),
            (
                false,
                "cp: f: a file, which a directory cannot be copied onto\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "-R t tt"),
            went(),
            "a name that only starts the same is another"
        );
        assert_eq!(host.left(), ["f", "t/", "t/a", "tt/", "tt/a"]);
    }

    #[test]
    fn i_asks_before_a_file_is_written_over() {
        let mut host = Host::new("ask", &["a", "b"]);
        host.answers = alloc::vec![false];
        assert_eq!(ran(&mut host, "-i a b"), went());
        assert_eq!(host.holds("b").as_deref(), Some("b"), "no keeps it");
        host.answers = alloc::vec![true];
        assert_eq!(ran(&mut host, "-i a b"), went());
        assert_eq!(host.holds("b").as_deref(), Some("a"));
        assert_eq!(
            ran(&mut host, "-i a c"),
            went(),
            "a new file is no question"
        );
        assert_eq!(host.asked, ["cp: write over b? ", "cp: write over b? "]);
    }

    #[test]
    fn f_removes_a_file_it_cannot_write_over() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut host = Host::new("force", &["a", "b"]);
        let target = host.root.join("b");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).unwrap();
        // One who may write anything writes it all the same.
        let writable = std::fs::OpenOptions::new()
            .write(true)
            .open(&target)
            .is_ok();
        if !writable {
            assert_eq!(
                ran(&mut host, "a b"),
                (false, "cp: a to b: not allowed\n".into())
            );
        }
        assert_eq!(ran(&mut host, "-f a b"), went());
        assert_eq!(host.holds("b").as_deref(), Some("a"));
    }

    #[test]
    fn last_names_and_joins() {
        assert_eq!(last_name("a/b/c"), "c");
        assert_eq!(last_name("a/b/"), "b");
        assert_eq!(last_name("c"), "c");
        assert_eq!(last_name("/"), "");
        assert_eq!(joined("d/", "x"), "d/x");
        assert_eq!(joined("d", "x"), "d/x");
    }

    #[test]
    fn what_fails_within_a_directory_fails_the_copy_and_the_rest_goes() {
        let mut host = Host::new("within", &["t/a", "t/b", "n/t/a/"]);
        assert_eq!(
            ran(&mut host, "-R t n"),
            (false, "cp: t/a to n/t/a: a directory, not a file\n".into())
        );
        assert_eq!(
            host.holds("n/t/b").as_deref(),
            Some("t/b"),
            "b is copied all the same"
        );
        assert_eq!(ran(&mut host, "-R t m"), went());
    }

    #[test]
    fn a_directory_that_cannot_be_made_or_listed_fails_the_copy() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut host = Host::new("denied", &["empty/", "shut/", "sealed/x"]);
        let set = |host: &Host, path: &str, mode| {
            let mode = std::fs::Permissions::from_mode(mode);
            std::fs::set_permissions(host.root.join(path), mode).unwrap();
        };
        set(&host, "shut", 0o555);
        set(&host, "sealed", 0o000);
        assert!(
            std::fs::read_dir(host.root.join("sealed")).is_err(),
            "run as root, nothing is sealed"
        );
        let made = ran(&mut host, "-R empty shut/new");
        let listed = ran(&mut host, "-R sealed copy");
        set(&host, "shut", 0o755);
        set(&host, "sealed", 0o755);
        assert_eq!(
            made,
            (false, "cp: shut/new: not allowed\n".into()),
            "even with nothing in it to copy"
        );
        assert_eq!(listed, (false, "cp: sealed: not allowed\n".into()));
        assert_eq!(
            host.left(),
            ["copy/", "empty/", "sealed/", "sealed/x", "shut/"],
            "the copy made before its source could not be listed"
        );
    }
}
