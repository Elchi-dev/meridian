//! dirname: a path without its last name, as POSIX's dirname has it.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::string::String;

use program::Handle;

program::main!(dirname);

/// `path` without its last name and the slashes after and before it: `.`
/// for a name alone or no path, and `/` for one at the root or slashes
/// alone.
fn directory(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "." } else { "/" };
    }
    let Some(end) = trimmed.rfind('/') else {
        return ".";
    };
    let parent = trimmed[..end].trim_end_matches('/');
    if parent.is_empty() { "/" } else { parent }
}

/// The path dirname's words name, after a `--` that ends options it has
/// none of; none for no path or more than one.
fn asked(arguments: &[String]) -> Option<&str> {
    let words = match arguments {
        [dashes, rest @ ..] if dashes == "--" => rest,
        words => words,
    };
    match words {
        [path] => Some(path),
        _ => None,
    }
}

#[cfg_attr(test, allow(dead_code))]
fn dirname(_: Handle) -> u64 {
    let Some(path) = asked(program::arguments()) else {
        let _ = program::write_error("dirname: takes one path\n");
        return 2;
    };
    let mut line = String::from(directory(path));
    line.push('\n');
    u64::from(program::write(&line).is_err())
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

    #[test]
    fn its_word_is_one_path() {
        assert_eq!(asked(&words("/a/b")), Some("/a/b"));
        assert_eq!(
            asked(&words("-- -x")),
            Some("-x"),
            "after -- a dash is a path"
        );
        assert_eq!(asked(&words("--")), None, "a -- alone leaves no path");
        assert_eq!(asked(&words("-- --")), Some("--"));
        assert_eq!(asked(&words("-x")), Some("-x"), "it has no options");
        assert_eq!(asked(&[]), None);
        assert_eq!(asked(&words("a b")), None);
        assert_eq!(asked(&words("-- a b")), None);
    }

    #[test]
    fn names_past_ascii_stay_whole() {
        assert_eq!(directory("/Dokumente/Übersicht.txt"), "/Dokumente");
        assert_eq!(directory("日本/語"), "日本");
        assert_eq!(directory("/naïve/"), "/");
    }

    #[test]
    fn the_last_name_goes() {
        for (path, want) in [
            ("/a/b/c", "/a/b"),
            ("a/b", "a"),
            ("c", "."),
            ("/a/b/", "/a"),
            ("/a//b//", "/a"),
            ("a//b", "a"),
            ("/a", "/"),
            ("//a", "/"),
            ("/", "/"),
            ("//", "/"),
            ("", "."),
            (".", "."),
            ("../x", ".."),
            ("/a/.", "/a"),
        ] {
            assert_eq!(directory(path), want, "{path:?}");
        }
    }
}
