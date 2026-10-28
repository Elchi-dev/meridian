//! du: writes how much room the files under each of its paths take, as
//! POSIX's du does: each directory after what it holds, with `-a` each
//! file too, with `-s` only each path given, in blocks of 512 bytes or of
//! 1024 with `-k`. Meridian's INFO says how many bytes a file holds, not
//! how much its volume gave it, so a file takes the blocks its bytes
//! fill. With no symbolic links `-H` and `-L` change nothing, and since
//! volumes cannot be told apart yet `-x` is refused. It ends with 0, or
//! 1 when something could not be read.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Error, Handle, text};

program::main!(du);

/// Which files du writes a line for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lines {
    /// Each directory, the default.
    Directories,
    /// Each file and directory, as `-a` asks.
    All,
    /// Each path given, as `-s` asks.
    Given,
}

/// What du's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    lines: Lines,
    block: u64,
    paths: &'w [String],
}

/// What du's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut lines = Lines::Directories;
    let mut block = 512;
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
                'a' => lines = Lines::All,
                's' => lines = Lines::Given,
                'k' => block = 1024,
                'H' | 'L' => {}
                'x' => return Err(String::from("du: -x: volumes cannot be told apart yet\n")),
                _ => return Err(format!("du: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    Ok(Asked {
        lines,
        block,
        paths: words,
    })
}

/// What a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Found {
    directory: bool,
    size: u64,
}

/// What du does with files.
trait Files {
    /// What is at `path`.
    fn found(&mut self, path: &str) -> Result<Found, Error>;

    /// The names in the directory at `path`.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// Writes `text` on the standard output.
    fn write(&mut self, text: &str);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// `name` in the directory at `directory`.
fn joined(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// Walks what is at `path`, writing the lines `asked` wants for it and
/// what it holds, `top` being whether it is a path given: the blocks it
/// takes, and whether all of it could be read.
fn walk(files: &mut impl Files, asked: &Asked, path: &str, top: bool) -> (u64, bool) {
    let found = match files.found(path) {
        Ok(found) => found,
        Err(e) => {
            files.say(&text::trouble("du", path, e));
            return (0, false);
        }
    };
    let mut blocks = found.size.div_ceil(asked.block);
    let mut all = true;
    if found.directory {
        match files.names(path) {
            Ok(names) => {
                for name in names {
                    let (held, read) = walk(files, asked, &joined(path, &name), false);
                    blocks += held;
                    all &= read;
                }
            }
            Err(e) => {
                files.say(&text::trouble("du", path, e));
                all = false;
            }
        }
    }
    let written = match asked.lines {
        Lines::Given => top,
        Lines::All => true,
        // A file given is written though it is no directory.
        Lines::Directories => found.directory || top,
    };
    if written {
        files.write(&format!("{blocks} {path}\n"));
    }
    (blocks, all)
}

/// Walks each path `asked` names, or the working directory; whether all
/// could be read.
fn walk_all(files: &mut impl Files, asked: &Asked) -> bool {
    let dot = [String::from(".")];
    let paths = if asked.paths.is_empty() {
        &dot[..]
    } else {
        asked.paths
    };
    let mut all = true;
    for path in paths {
        all &= walk(files, asked, path, true).1;
    }
    all
}

/// Meridian's files, and du's own output.
struct System {
    out: text::Out<text::Write>,
}

impl Files for System {
    fn found(&mut self, path: &str) -> Result<Found, Error> {
        let file = program::open(path, program::abi::file::READ)?;
        let info = program::info(file);
        let _ = program::close(file);
        let info = info?;
        Ok(Found {
            directory: info.directory,
            size: info.size,
        })
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

    fn write(&mut self, text: &str) {
        self.out.put(text.as_bytes());
    }

    fn say(&mut self, text: &str) {
        self.out.flush();
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn du(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => {
            let mut system = System {
                out: text::standard_output(),
            };
            let all = walk_all(&mut system, &asked);
            let written = system.out.finish();
            u64::from(!(all && written))
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

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in du reaches the host's own files.
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
    /// du wrote and said.
    struct Host {
        root: std::path::PathBuf,
        out: String,
        said: String,
    }

    impl Host {
        /// The files `sizes` name with as many bytes as each says, those
        /// ending in `/` directories.
        fn new(name: &str, sizes: &[(&str, usize)]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-du-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for &(path, size) in sizes {
                let at = root.join(path.trim_end_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, alloc::vec![b'x'; size]).unwrap();
                }
            }
            Host {
                root,
                out: String::new(),
                said: String::new(),
            }
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
            _ => Error::Unreadable,
        }
    }

    impl Files for Host {
        fn found(&mut self, path: &str) -> Result<Found, Error> {
            let meta = std::fs::metadata(confined(&self.root, path)).map_err(|e| error(&e))?;
            Ok(Found {
                directory: meta.is_dir(),
                size: if meta.is_dir() { 0 } else { meta.len() },
            })
        }

        fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
            let listed = std::fs::read_dir(confined(&self.root, path)).map_err(|e| error(&e))?;
            let mut names: Vec<String> = listed
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            Ok(names)
        }

        fn write(&mut self, text: &str) {
            self.out.push_str(text);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How du with the words `text` ran on `host`: whether all could be
    /// read, what it wrote and what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String, String) {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let all = walk_all(host, &taken);
        (
            all,
            core::mem::take(&mut host.out),
            core::mem::take(&mut host.said),
        )
    }

    fn wrote(text: &str) -> (bool, String, String) {
        (true, String::from(text), String::new())
    }

    #[test]
    fn its_words_are_options_then_paths() {
        let given = words("-ak a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                lines: Lines::All,
                block: 1024,
                paths: &given[1..],
            })
        );
        assert_eq!(
            asked(&words("-a -s")).map(|a| a.lines),
            Ok(Lines::Given),
            "the last of -a and -s counts"
        );
        assert_eq!(
            asked(&words("-H -L")).map(|a| (a.lines, a.block)),
            Ok((Lines::Directories, 512))
        );
        let given = words("-- -s");
        assert_eq!(asked(&given).map(|a| a.paths.len()), Ok(1));
        assert_eq!(
            asked(&words("-x")),
            Err("du: -x: volumes cannot be told apart yet\n".into())
        );
        assert_eq!(asked(&words("-q")), Err("du: -q is no option\n".into()));
    }

    const TREE: &[(&str, usize)] = &[
        ("t/a", 100),
        ("t/b", 600),
        ("t/u/c", 1500),
        ("t/u/v/", 0),
        ("f", 2000),
    ];

    #[test]
    fn each_directory_is_written_after_what_it_holds() {
        let mut host = Host::new("tree", TREE);
        assert_eq!(ran(&mut host, "t"), wrote("0 t/u/v\n3 t/u\n6 t\n"));
        assert_eq!(
            ran(&mut host, "-a t"),
            wrote("1 t/a\n2 t/b\n3 t/u/c\n0 t/u/v\n3 t/u\n6 t\n")
        );
        assert_eq!(ran(&mut host, "-s t f"), wrote("6 t\n4 f\n"));
        assert_eq!(
            ran(&mut host, "-k t"),
            wrote("0 t/u/v\n2 t/u\n4 t\n"),
            "in kilobytes each file's own are rounded up"
        );
        assert_eq!(
            ran(&mut host, "f"),
            wrote("4 f\n"),
            "a file given is written"
        );
        assert_eq!(ran(&mut host, "t/u/v"), wrote("0 t/u/v\n"));
    }

    #[test]
    fn with_no_path_the_working_directory_is_walked() {
        let mut host = Host::new("dot", &[("x", 512)]);
        assert_eq!(ran(&mut host, "-s"), wrote("1 .\n"));
    }

    #[test]
    fn what_cannot_be_read_is_said_and_the_rest_counted() {
        let mut host = Host::new("missing", TREE);
        assert_eq!(
            ran(&mut host, "-s nope t"),
            (false, "6 t\n".into(), "du: nope: no such file\n".into())
        );
        assert_eq!(joined("a/", "b"), "a/b");
        assert_eq!(joined("a", "b"), "a/b");
    }

    #[test]
    fn a_directory_within_that_cannot_be_read_fails_the_walk() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut host = Host::new("shut", &[("t/a/x", 512), ("t/b", 512)]);
        let shut = host.root.join("t/a");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&shut).is_ok();
        let found = ran(&mut host, "-s t");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            // One who may read anything reads it all the same.
            assert_eq!(found, wrote("2 t\n"));
        } else {
            assert_eq!(
                found,
                (false, "1 t\n".into(), "du: t/a: could not be read\n".into())
            );
        }
    }

    #[test]
    fn blocks_are_the_bytes_each_file_fills_rounded_up() {
        let mut host = Host::new(
            "blocks",
            &[
                ("e", 0),
                ("one", 1),
                ("full", 512),
                ("more", 513),
                ("k", 1025),
            ],
        );
        assert_eq!(
            ran(&mut host, "e one full more k"),
            wrote("0 e\n1 one\n1 full\n2 more\n3 k\n")
        );
        assert_eq!(
            ran(&mut host, "-k e one full more k"),
            wrote("0 e\n1 one\n1 full\n1 more\n2 k\n")
        );
        let mut host = Host::new("sum", &[("d/a", 1), ("d/b", 1), ("d/c", 1)]);
        assert_eq!(
            ran(&mut host, "-s d"),
            wrote("3 d\n"),
            "each file fills a block of its own, so three are three"
        );
        assert_eq!(ran(&mut host, "-ks d"), wrote("3 d\n"));
        assert_eq!(
            ran(&mut host, "-as d"),
            wrote("3 d\n"),
            "-s, given last, counts"
        );
        assert_eq!(
            ran(&mut host, "-sa d"),
            wrote("1 d/a\n1 d/b\n1 d/c\n3 d\n"),
            "and -a so"
        );
    }
}
