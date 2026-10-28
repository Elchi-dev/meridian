//! mv: moves files and directories, as POSIX's mv does: one onto
//! another name, or each of several into a directory under its own name.
//! A file there already is written over, after a question at the console
//! with `-i`, none with `-f`, the last of the two counting; a directory
//! there takes the place of one only when that holds nothing. Across
//! volumes what moves is copied, and then removed. Nothing moves onto
//! itself, nor a directory into itself. It ends with 0 when everything
//! moved, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use program::abi::file::{CREATE_ALWAYS, READ, WRITE};
use program::{Error, Handle, text};

program::main!(mv);

/// What mv's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    /// Whether the one at the console is asked before a file is written
    /// over.
    ask: bool,
    sources: &'w [String],
    target: &'w str,
}

/// What mv's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut ask = false;
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
                'i' => ask = true,
                'f' => ask = false,
                _ => return Err(format!("mv: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    let Some((target, sources)) = words
        .split_last()
        .filter(|(_, sources)| !sources.is_empty())
    else {
        return Err(String::from(
            "mv: moves a file to another name, or files into a directory\n",
        ));
    };
    Ok(Asked {
        ask,
        sources,
        target,
    })
}

/// What mv does with files.
trait Files {
    /// Whether the file at `path` is a directory.
    fn directory(&mut self, path: &str) -> Result<bool, Error>;

    /// The names in the directory at `path`.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// Moves the file or directory at `from` to `to`, on one volume.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error>;

    /// Copies the file at `from` to `to`, made new.
    fn copy(&mut self, from: &str, to: &str) -> Result<(), Error>;

    /// Makes a directory at `path`.
    fn make_directory(&mut self, path: &str) -> Result<(), Error>;

    /// Removes the file or the empty directory at `path`.
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

/// Copies what is at `from` to `to`, a directory with all it holds.
fn copy_tree(files: &mut impl Files, from: &str, to: &str) -> Result<(), Error> {
    if !files.directory(from)? {
        return files.copy(from, to);
    }
    files.make_directory(to)?;
    for name in files.names(from)? {
        copy_tree(files, &joined(from, &name), &joined(to, &name))?;
    }
    Ok(())
}

/// Removes what is at `path`, a directory with all it holds.
fn remove_tree(files: &mut impl Files, path: &str) -> Result<(), Error> {
    if files.directory(path)? {
        for name in files.names(path)? {
            remove_tree(files, &joined(path, &name))?;
        }
    }
    files.remove(path)
}

/// Moves what is at `from` to `to` as `asked` says, saying what went
/// wrong; whether it moved, or was rightly left.
fn move_one(files: &mut impl Files, asked: &Asked, from: &str, to: &str) -> bool {
    let say = |files: &mut _, why: String| {
        Files::say(files, &why);
        false
    };
    let directory = match files.directory(from) {
        Ok(directory) => directory,
        Err(e) => return say(files, text::trouble("mv", from, e)),
    };
    let (source, target) = (files.compared(from), files.compared(to));
    if source == target {
        return say(files, format!("mv: {from} and {to} are one file\n"));
    }
    if directory && target.starts_with(&joined(&source, "")) {
        return say(
            files,
            format!("mv: {from}: a directory cannot be moved into itself\n"),
        );
    }
    match (directory, files.directory(to)) {
        (true, Ok(false)) => {
            return say(
                files,
                format!("mv: {to}: a file, which a directory cannot be moved onto\n"),
            );
        }
        (false, Ok(true)) => {
            return say(
                files,
                format!("mv: {to}: a directory, which a file cannot be moved onto\n"),
            );
        }
        (true, Ok(true)) => {
            // Only one that holds nothing makes room for another.
            if let Err(e) = files.remove(to) {
                return match e {
                    Error::NotEmpty => say(files, format!("mv: {to}: holds something\n")),
                    e => say(files, text::trouble("mv", to, e)),
                };
            }
        }
        (false, Ok(false)) => {
            if asked.ask && !files.yes(&format!("mv: write over {to}? ")) {
                return true;
            }
            if let Err(e) = files.remove(to) {
                return say(files, text::trouble("mv", to, e));
            }
        }
        (_, Err(_)) => {}
    }
    let moved = match files.rename(from, to) {
        Err(Error::NotSameVolume) => {
            copy_tree(files, from, to).and_then(|()| remove_tree(files, from))
        }
        other => other,
    };
    match moved {
        Ok(()) => true,
        Err(e) => say(files, text::trouble("mv", &format!("{from} to {to}"), e)),
    }
}

/// Moves what `asked` names: onto the target, or into it when it is a
/// directory or there is more than one source; whether everything moved.
fn move_all(files: &mut impl Files, asked: &Asked) -> bool {
    let into = files.directory(asked.target) == Ok(true);
    if !into && asked.sources.len() > 1 {
        files.say(&format!(
            "mv: {}: no directory to move files into\n",
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
        all &= move_one(files, asked, source, &to);
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

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        program::rename(from, to)
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
fn mv(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => u64::from(!move_all(&mut System, &asked)),
        Err(why) => {
            let _ = program::write_error(&why);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt as _;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// Where `path` is: under `shm` when it starts with `/shm`, which
    /// stands for another volume, else under `root`. One that climbs out
    /// stops the test, so that no mistake in mv reaches the host's files.
    fn confined(root: &std::path::Path, shm: &std::path::Path, path: &str) -> std::path::PathBuf {
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
        match path.strip_prefix("/shm") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                shm.join(rest.trim_start_matches('/'))
            }
            _ => root.join(path.trim_start_matches('/')),
        }
    }

    /// Files among the host's temporary ones, and under `/dev/shm` as a
    /// volume of their own, gone once dropped; the answers to give at the
    /// console, and what mv asked and said.
    struct Host {
        root: std::path::PathBuf,
        shm: std::path::PathBuf,
        answers: Vec<bool>,
        asked: Vec<String>,
        said: String,
    }

    impl Host {
        /// The files `paths` name, those ending in `/` directories, each
        /// file holding its own name.
        fn new(name: &str, paths: &[&str]) -> Host {
            let place = format!("meridian-mv-{name}-{}", std::process::id());
            let root = std::env::temp_dir().join(&place);
            let shm = std::path::Path::new("/dev/shm").join(&place);
            for top in [&root, &shm] {
                let _ = std::fs::remove_dir_all(top);
            }
            std::fs::create_dir_all(&root).unwrap();
            let _ = std::fs::create_dir_all(&shm);
            let host = Host {
                root,
                shm,
                answers: Vec::new(),
                asked: Vec::new(),
                said: String::new(),
            };
            for path in paths {
                let at = confined(&host.root, &host.shm, path.trim_end_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, path.as_bytes()).unwrap();
                }
            }
            host
        }

        /// Whether `/shm` is a volume other than the root's on this host.
        fn two_volumes(&self) -> bool {
            match (std::fs::metadata(&self.root), std::fs::metadata(&self.shm)) {
                (Ok(a), Ok(b)) => a.dev() != b.dev(),
                _ => false,
            }
        }

        fn holds(&self, path: &str) -> Option<String> {
            std::fs::read_to_string(confined(&self.root, &self.shm, path)).ok()
        }

        fn there(&self, path: &str) -> bool {
            confined(&self.root, &self.shm, path).exists()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_dir_all(&self.shm);
        }
    }

    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            std::io::ErrorKind::DirectoryNotEmpty => Error::NotEmpty,
            std::io::ErrorKind::CrossesDevices => Error::NotSameVolume,
            std::io::ErrorKind::PermissionDenied => Error::Denied,
            _ => Error::Unwritable,
        }
    }

    impl Files for Host {
        fn directory(&mut self, path: &str) -> Result<bool, Error> {
            std::fs::metadata(confined(&self.root, &self.shm, path))
                .map(|m| m.is_dir())
                .map_err(|e| error(&e))
        }

        fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
            let listed =
                std::fs::read_dir(confined(&self.root, &self.shm, path)).map_err(|e| error(&e))?;
            let mut names: Vec<String> = listed
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            Ok(names)
        }

        fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
            let (from, to) = (
                confined(&self.root, &self.shm, from),
                confined(&self.root, &self.shm, to),
            );
            std::fs::rename(from, to).map_err(|e| error(&e))
        }

        fn copy(&mut self, from: &str, to: &str) -> Result<(), Error> {
            let bytes =
                std::fs::read(confined(&self.root, &self.shm, from)).map_err(|e| error(&e))?;
            std::fs::write(confined(&self.root, &self.shm, to), bytes).map_err(|e| error(&e))
        }

        fn make_directory(&mut self, path: &str) -> Result<(), Error> {
            std::fs::create_dir(confined(&self.root, &self.shm, path)).map_err(|e| error(&e))
        }

        fn remove(&mut self, path: &str) -> Result<(), Error> {
            let at = confined(&self.root, &self.shm, path);
            if at.is_dir() {
                std::fs::remove_dir(at).map_err(|e| error(&e))
            } else {
                std::fs::remove_file(at).map_err(|e| error(&e))
            }
        }

        fn compared(&mut self, path: &str) -> String {
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

    /// How mv with the words `text` ran on `host`: whether all moved, and
    /// what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String) {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let all = move_all(host, &taken);
        (all, core::mem::take(&mut host.said))
    }

    fn went() -> (bool, String) {
        (true, String::new())
    }

    #[test]
    fn its_words_are_options_sources_and_a_target() {
        let given = words("-i a b c");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                ask: true,
                sources: &given[1..3],
                target: "c",
            })
        );
        assert_eq!(
            asked(&words("-if a b")).map(|a| a.ask),
            Ok(false),
            "the last of -i and -f counts"
        );
        assert_eq!(asked(&words("-fi a b")).map(|a| a.ask), Ok(true));
        let given = words("-- -a b");
        assert_eq!(
            asked(&given).map(|a| (a.sources.len(), a.target)),
            Ok((1, "b"))
        );
        assert_eq!(asked(&words("-x a b")), Err("mv: -x is no option\n".into()));
        for text in ["", "a", "-i a"] {
            assert_eq!(
                asked(&words(text)),
                Err("mv: moves a file to another name, or files into a directory\n".into()),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_file_moves_to_another_name_or_into_a_directory() {
        let mut host = Host::new("files", &["a", "b", "c", "d/"]);
        assert_eq!(ran(&mut host, "a e"), went());
        assert!(!host.there("a") && host.holds("e").as_deref() == Some("a"));
        assert_eq!(ran(&mut host, "e b"), went());
        assert_eq!(
            host.holds("b").as_deref(),
            Some("a"),
            "a file there is written over"
        );
        assert_eq!(ran(&mut host, "b c d"), went());
        assert_eq!(
            (host.holds("d/b").as_deref(), host.holds("d/c").as_deref()),
            (Some("a"), Some("c"))
        );
        assert_eq!(
            ran(&mut host, "d/b x y"),
            (false, "mv: y: no directory to move files into\n".into())
        );
        assert_eq!(
            ran(&mut host, "nope z"),
            (false, "mv: nope: no such file\n".into())
        );
        assert_eq!(
            ran(&mut host, "d/c nowhere/c"),
            (false, "mv: d/c to nowhere/c: no such file\n".into())
        );
        assert!(host.there("d/c"), "what could not move stays");
    }

    #[test]
    fn a_directory_moves_with_all_it_holds() {
        let mut host = Host::new("trees", &["t/a", "t/u/b", "e/", "full/x", "f"]);
        assert_eq!(ran(&mut host, "t n"), went());
        assert_eq!(host.holds("n/u/b").as_deref(), Some("t/u/b"));
        assert_eq!(
            ran(&mut host, "n e"),
            went(),
            "into a directory there, under its name"
        );
        assert!(host.there("e/n/a") && !host.there("n"));
        std::fs::create_dir(host.root.join("m")).unwrap();
        assert_eq!(
            ran(&mut host, "e/n m/"),
            went(),
            "into an empty one under its name"
        );
        assert!(host.there("m/n/u/b"));
        std::fs::create_dir_all(host.root.join("g/n")).unwrap();
        assert_eq!(
            ran(&mut host, "m/n g"),
            went(),
            "an empty directory there makes room"
        );
        assert!(host.there("g/n/a") && !host.there("m/n"));
        std::fs::create_dir_all(host.root.join("h/n")).unwrap();
        std::fs::write(host.root.join("h/n/kept"), "k").unwrap();
        assert_eq!(
            ran(&mut host, "g/n h"),
            (false, "mv: h/n: holds something\n".into())
        );
        assert!(host.there("g/n/a") && host.there("h/n/kept"));
    }

    #[test]
    fn nothing_moves_onto_itself_into_itself_or_across_kinds() {
        let mut host = Host::new("refused", &["a", "t/x", "d/a/", "f"]);
        assert_eq!(
            ran(&mut host, "a a"),
            (false, "mv: a and a are one file\n".into())
        );
        assert_eq!(
            ran(&mut host, "t/x t"),
            (false, "mv: t/x and t/x are one file\n".into())
        );
        assert_eq!(
            ran(&mut host, "t t/u"),
            (
                false,
                "mv: t: a directory cannot be moved into itself\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "t f"),
            (
                false,
                "mv: f: a file, which a directory cannot be moved onto\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "a d"),
            (
                false,
                "mv: d/a: a directory, which a file cannot be moved onto\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "t tt"),
            went(),
            "a name that only starts the same is another"
        );
        assert!(host.there("a") && host.there("tt/x") && host.there("f"));
    }

    #[test]
    fn i_asks_before_a_file_is_written_over() {
        let mut host = Host::new("ask", &["a", "b", "c"]);
        host.answers = alloc::vec![false, true];
        assert_eq!(ran(&mut host, "-i a b"), went());
        assert_eq!(
            (host.holds("a").as_deref(), host.holds("b").as_deref()),
            (Some("a"), Some("b")),
            "no keeps both"
        );
        assert_eq!(ran(&mut host, "-i a b"), went());
        assert_eq!(
            (host.there("a"), host.holds("b").as_deref()),
            (false, Some("a"))
        );
        assert_eq!(
            ran(&mut host, "-i c e"),
            went(),
            "a new name is no question"
        );
        assert_eq!(host.asked, ["mv: write over b? ", "mv: write over b? "]);
        assert_eq!(ran(&mut host, "-if e b"), went(), "-f last asks nothing");
        assert_eq!(host.asked.len(), 2);
    }

    #[test]
    fn across_volumes_what_moves_is_copied_and_then_removed() {
        let mut host = Host::new("volumes", &["t/a", "t/u/b", "f", "/shm/"]);
        if !host.two_volumes() {
            // The host has no second volume to move across.
            return;
        }
        assert_eq!(ran(&mut host, "f /shm/f"), went());
        assert_eq!(
            (host.there("f"), host.holds("/shm/f").as_deref()),
            (false, Some("f"))
        );
        assert_eq!(
            ran(&mut host, "t /shm"),
            went(),
            "a directory, into the other volume"
        );
        assert_eq!(host.holds("/shm/t/u/b").as_deref(), Some("t/u/b"));
        assert!(!host.there("t"), "the first is gone once it was copied");
        assert_eq!(ran(&mut host, "/shm/t back"), went());
        assert_eq!(host.holds("back/a").as_deref(), Some("t/a"));
    }

    #[test]
    fn trees_are_copied_and_removed_whole() {
        let mut host = Host::new("whole", &["t/a", "t/u/b", "t/u/v/"]);
        assert_eq!(copy_tree(&mut host, "t", "c"), Ok(()));
        assert_eq!(host.holds("c/u/b").as_deref(), Some("t/u/b"));
        assert!(host.there("c/u/v"));
        assert_eq!(remove_tree(&mut host, "c"), Ok(()));
        assert!(!host.there("c"));

        assert_eq!(remove_tree(&mut host, "nope"), Err(Error::NotFound));
        assert_eq!(copy_tree(&mut host, "nope", "x"), Err(Error::NotFound));
        assert_eq!(last_name("a/b/"), "b");
        assert_eq!(joined("d/", "x"), "d/x");
    }

    #[test]
    fn what_cannot_be_removed_after_a_copy_is_said_and_kept() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut host = Host::new("kept", &["t/u/b", "/shm/"]);
        if !host.two_volumes() {
            return;
        }
        let shut = host.root.join("t/u");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o555)).unwrap();
        // One who may remove anything removes it all the same.
        let removable = std::fs::File::create(shut.join("probe")).is_ok();
        let _ = std::fs::remove_file(shut.join("probe"));
        let found = ran(&mut host, "t /shm");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        if removable {
            assert_eq!(found, went());
            return;
        }
        assert_eq!(
            found,
            (false, "mv: t to /shm/t: not allowed\n".into()),
            "the file within could not go"
        );
        assert!(
            host.there("t/u/b") && host.there("/shm/t/u/b"),
            "both copies stay"
        );
    }
}
