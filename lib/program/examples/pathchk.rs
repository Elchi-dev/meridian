//! pathchk: says which pathnames could not be used, as POSIX's pathchk
//! does: by default on Meridian, where a path takes 512 bytes at most and
//! its names are a FAT volume's, every directory on the way being one
//! that may be looked into; with `-p` on any POSIX system, 255 bytes at
//! most, names of 14 bytes at most of letters, digits, `.`, `_` and `-`;
//! and with `-P` no name begins with `-`. An empty pathname is never one.
//! Whether a file is there does not matter. It ends with 0 when every
//! pathname could be used, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Error, Handle};

program::main!(pathchk);

/// The longest path Meridian's calls take, in bytes.
const PATH_MAX: usize = program::abi::PATH_BYTES;

/// The longest path and name POSIX lets a portable program count on, in
/// bytes, without the zero byte C ends them with.
const PORTABLE_PATH: usize = 255;
const PORTABLE_NAME: usize = 14;

/// What pathchk's words ask for: how portable the pathnames must be,
/// and the pathnames.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    portable: bool,
    no_hyphens: bool,
    paths: &'w [String],
}

/// What pathchk's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let (mut portable, mut no_hyphens) = (false, false);
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
                'p' => portable = true,
                'P' => no_hyphens = true,
                _ => return Err(format!("pathchk: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    if words.is_empty() {
        return Err(String::from("pathchk: checks a pathname at least\n"));
    }
    Ok(Asked {
        portable,
        no_hyphens,
        paths: words,
    })
}

/// Whether `c` is in POSIX's portable filename character set.
fn portable_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// What makes `path` no pathname to use as `asked` says, if anything
/// does, without asking the file system.
fn written_wrong(path: &str, asked: &Asked) -> Option<String> {
    if path.is_empty() {
        return Some(String::from("an empty pathname names nothing"));
    }
    let longest = if asked.portable {
        PORTABLE_PATH
    } else {
        PATH_MAX
    };
    if path.len() > longest {
        return Some(format!("longer than {longest} bytes"));
    }
    for name in path.split('/').filter(|n| !n.is_empty()) {
        if asked.no_hyphens && name.starts_with('-') {
            return Some(format!("the name {name} begins with -"));
        }
        if asked.portable {
            if name.len() > PORTABLE_NAME {
                return Some(format!(
                    "the name {name} is longer than {PORTABLE_NAME} bytes"
                ));
            }
            if let Some(c) = name.chars().find(|&c| !portable_character(c)) {
                return Some(format!(
                    "the name {name} holds {c:?}, which is no portable character"
                ));
            }
        } else if !matches!(name, "." | "..") && !fat::name::valid(name) {
            return Some(format!("the name {name} is none a FAT volume takes"));
        }
    }
    None
}

/// What pathchk asks of the file system.
trait Tree {
    /// Whether the directory at `path` may be looked into; none when
    /// nothing is there.
    fn searchable(&mut self, path: &str) -> Option<bool>;
}

/// The first directory on the way to `path` that is there and may not
/// be looked into, if any.
fn shut(tree: &mut impl Tree, path: &str) -> Option<String> {
    let mut at = String::from(if path.starts_with('/') { "/" } else { "" });
    let names: Vec<&str> = path.split('/').filter(|n| !n.is_empty()).collect();
    // The last name is the file's own, which need not be a directory.
    for name in names.iter().take(names.len().saturating_sub(1)) {
        if !at.is_empty() && !at.ends_with('/') {
            at.push('/');
        }
        at.push_str(name);
        match tree.searchable(&at) {
            Some(true) => {}
            Some(false) => return Some(at),
            None => return None,
        }
    }
    None
}

/// Checks every pathname `asked` names, saying on `say` what is wrong,
/// and answers the status pathchk ends with.
fn check(tree: &mut impl Tree, asked: &Asked, say: &mut impl FnMut(&str)) -> u64 {
    let mut status = 0;
    for path in asked.paths {
        let wrong = written_wrong(path, asked).or_else(|| {
            let shut = (!asked.portable).then(|| shut(tree, path)).flatten()?;
            Some(format!("{shut} may not be looked into"))
        });
        if let Some(why) = wrong {
            say(&format!("pathchk: {path}: {why}\n"));
            status = 1;
        }
    }
    status
}

/// Meridian's files.
struct System;

impl Tree for System {
    fn searchable(&mut self, path: &str) -> Option<bool> {
        match program::open(path, program::abi::file::READ) {
            Ok(file) => {
                let _ = program::close(file);
                Some(true)
            }
            Err(Error::Denied) => Some(false),
            Err(_) => None,
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn pathchk(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => check(&mut System, &asked, &mut |text| {
            let _ = program::write_error(text);
        }),
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

    /// What is wrong with `path` with the options `options`.
    fn wrong(options: &str, path: &str) -> Option<String> {
        let given = words(&format!("{options} x"));
        let mut taken = asked(&given).unwrap();
        let path = [String::from(path)];
        taken.paths = &path;
        written_wrong(&path[0], &taken)
    }

    #[test]
    fn its_words_are_options_and_pathnames() {
        let given = words("-pP a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                portable: true,
                no_hyphens: true,
                paths: &given[1..]
            })
        );
        let given = words("-p -- -P");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                portable: true,
                no_hyphens: false,
                paths: &given[2..]
            })
        );
        let given = words("a");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                portable: false,
                no_hyphens: false,
                paths: &given[..]
            })
        );
        assert_eq!(
            asked(&words("-x a")),
            Err("pathchk: -x is no option\n".into())
        );
        assert_eq!(
            asked(&words("-p")),
            Err("pathchk: checks a pathname at least\n".into())
        );
        assert_eq!(
            asked(&words("")),
            Err("pathchk: checks a pathname at least\n".into())
        );
    }

    #[test]
    fn portable_pathnames_keep_posix_s_least_limits() {
        // Where GNU's pathchk -p and -P draw the lines.
        assert_eq!(
            wrong("-p", "a/b c"),
            Some("the name b c holds ' ', which is no portable character".into())
        );
        assert_eq!(wrong("-p", "abcdefghijklmn"), None, "fourteen bytes");
        assert_eq!(
            wrong("-p", "abcdefghijklmno"),
            Some("the name abcdefghijklmno is longer than 14 bytes".into())
        );
        assert_eq!(wrong("-p", &"x/".repeat(127)), None, "254 bytes");
        assert_eq!(
            wrong("-p", &("yy".to_owned() + &"/x".repeat(127))),
            Some("longer than 255 bytes".into())
        );
        assert_eq!(wrong("-p", "A-Z_a.z/0-9"), None);
        assert_eq!(
            wrong("-p", "\u{e9}t\u{e9}"),
            Some("the name \u{e9}t\u{e9} holds '\u{e9}', which is no portable character".into())
        );
        assert_eq!(
            wrong("-P", "a/-b"),
            Some("the name -b begins with -".into())
        );
        assert_eq!(
            wrong("", "a/-b"),
            None,
            "a leading hyphen only matters with -P"
        );
        assert_eq!(wrong("-P", "a//b/"), None, "slashes in a row part nothing");
        for options in ["", "-p", "-P"] {
            assert_eq!(
                wrong(options, ""),
                Some("an empty pathname names nothing".into()),
                "{options}"
            );
        }
    }

    #[test]
    fn meridian_s_pathnames_keep_its_calls_and_fat_s_names() {
        assert_eq!(wrong("", &"x/".repeat(256)), None, "512 bytes");
        assert_eq!(
            wrong("", &("y".to_owned() + &"/x".repeat(256))),
            Some("longer than 512 bytes".into())
        );
        assert_eq!(wrong("", &"x".repeat(255)), None);
        assert_eq!(
            wrong("", &"\u{e9}".repeat(255)),
            None,
            "255 characters, not bytes"
        );
        assert_eq!(
            wrong("", &"x".repeat(256)),
            Some(format!(
                "the name {} is none a FAT volume takes",
                "x".repeat(256)
            ))
        );
        for name in [
            "a:b", "a*", "a?", "a\"b", "a<b", "a|b", "a\\b", "a.", "a ", "...", "\u{1}",
        ] {
            assert_eq!(
                wrong("", name),
                Some(format!("the name {name} is none a FAT volume takes")),
                "{name:?}"
            );
        }
        assert_eq!(
            wrong("", "./../a/./b"),
            None,
            ". and .. are steps, no names"
        );
        assert_eq!(wrong("", "/System/x y.txt"), None);
        assert_eq!(wrong("", "-a"), None);
    }

    /// Directories among the host's temporary files, gone once dropped,
    /// one of which may not be looked into.
    struct Host {
        root: std::path::PathBuf,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root = std::env::temp_dir()
                .join(format!("meridian-pathchk-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("open/shut/deeper")).unwrap();
            std::fs::write(root.join("open/file"), "").unwrap();
            Host { root }
        }

        fn shut(&self, mode: u32) {
            use std::os::unix::fs::PermissionsExt as _;
            let at = self.root.join("open/shut");
            std::fs::set_permissions(at, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            self.shut(0o755);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Tree for Host {
        fn searchable(&mut self, path: &str) -> Option<bool> {
            match std::fs::read_dir(self.root.join(path.trim_start_matches('/'))) {
                Ok(_) => Some(true),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Some(false),
                Err(_) => None,
            }
        }
    }

    /// What pathchk with the words `text` says, and its status.
    fn checked(host: &mut Host, text: &str) -> (u64, String) {
        let given = words(text);
        let mut said = String::new();
        let status = check(host, &asked(&given).unwrap(), &mut |t| said.push_str(t));
        (status, said)
    }

    #[test]
    fn a_directory_on_the_way_must_be_one_to_look_into() {
        let mut host = Host::new("search");
        host.shut(0o000);
        // One who may read anything looks in all the same.
        let shut = std::fs::read_dir(host.root.join("open/shut")).is_err();
        assert_eq!(
            checked(&mut host, "open/file open/new/x /open/shut"),
            (0, String::new()),
            "the last name need not be a directory"
        );
        if shut {
            assert_eq!(
                checked(&mut host, "open/shut/deeper/x /open/shut/y"),
                (
                    1,
                    "pathchk: open/shut/deeper/x: open/shut may not be looked into\n\
                     pathchk: /open/shut/y: /open/shut may not be looked into\n"
                        .into()
                )
            );
        }
        assert_eq!(
            checked(&mut host, "-p open/shut/deeper/x"),
            (0, String::new()),
            "-p asks no file system"
        );
        assert_eq!(
            checked(&mut host, "missing/a/b"),
            (0, String::new()),
            "what is not there could be made"
        );
    }

    #[test]
    fn every_pathname_is_checked_and_each_wrong_one_said() {
        let mut host = Host::new("all");
        assert_eq!(
            checked(&mut host, "-p good b*d also"),
            (
                1,
                "pathchk: b*d: the name b*d holds '*', which is no portable character\n".into()
            )
        );
        assert_eq!(checked(&mut host, "x: y:").0, 1);
        assert_eq!(
            checked(&mut host, "x: y:").1,
            "pathchk: x:: the name x: is none a FAT volume takes\npathchk: y:: the name y: is none a FAT volume takes\n"
        );
    }
}
