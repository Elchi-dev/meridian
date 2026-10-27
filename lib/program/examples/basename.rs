//! basename: the last name of a path, without the suffix it is given, as
//! POSIX's basename has it.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::string::String;

use program::Handle;

program::main!(basename);

/// The last name of `path`, its slashes at the end gone, and `suffix`
/// taken off its end unless that would leave nothing: `.` for no path,
/// and `/` for one of slashes alone.
fn base<'p>(path: &'p str, suffix: Option<&str>) -> &'p str {
    if path.is_empty() {
        return ".";
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    match suffix.and_then(|suffix| last.strip_suffix(suffix)) {
        Some(rest) if !rest.is_empty() => rest,
        _ => last,
    }
}

/// The path and the suffix basename's words name, after a `--` that ends
/// options it has none of; none for too few or too many.
fn asked(arguments: &[String]) -> Option<(&str, Option<&str>)> {
    let words = match arguments {
        [dashes, rest @ ..] if dashes == "--" => rest,
        words => words,
    };
    match words {
        [path] => Some((path, None)),
        [path, suffix] => Some((path, Some(suffix))),
        _ => None,
    }
}

#[cfg_attr(test, allow(dead_code))]
fn basename(_: Handle) -> u64 {
    let Some((path, suffix)) = asked(program::arguments()) else {
        let _ = program::write_error("basename: takes a path, and a suffix to take off\n");
        return 2;
    };
    let mut line = String::from(base(path, suffix));
    line.push('\n');
    u64::from(program::write(&line).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_name_stays() {
        for (path, want) in [
            ("/a/b/c", "c"),
            ("a/b", "b"),
            ("c", "c"),
            ("/a/b/", "b"),
            ("/a/b///", "b"),
            ("a//b", "b"),
            ("/", "/"),
            ("//", "/"),
            ("///", "/"),
            ("", "."),
            (".", "."),
            ("..", ".."),
            ("/a/.", "."),
        ] {
            assert_eq!(base(path, None), want, "{path:?}");
        }
    }

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    #[test]
    fn its_words_are_a_path_and_a_suffix() {
        assert_eq!(asked(&words("/a/b")), Some(("/a/b", None)));
        assert_eq!(asked(&words("/a/b.c .c")), Some(("/a/b.c", Some(".c"))));
        assert_eq!(
            asked(&words("-- -x")),
            Some(("-x", None)),
            "after -- a dash is a path"
        );
        assert_eq!(asked(&words("-- -x -y")), Some(("-x", Some("-y"))));
        assert_eq!(asked(&words("-x")), Some(("-x", None)), "it has no options");
        assert_eq!(asked(&words("--")), None, "a -- alone leaves no path");
        assert_eq!(asked(&words("-- --")), Some(("--", None)));
        assert_eq!(asked(&[]), None);
        assert_eq!(asked(&words("a b c")), None);
        assert_eq!(asked(&words("-- a b c")), None);
    }

    #[test]
    fn names_past_ascii_stay_whole() {
        assert_eq!(base("/Dokumente/Übersicht.txt", Some(".txt")), "Übersicht");
        assert_eq!(base("/日本/語", None), "語");
        assert_eq!(base("/a/naïve/", None), "naïve");
    }

    #[test]
    fn a_suffix_comes_off_unless_it_is_all_there_is() {
        assert_eq!(base("/a/notes.txt", Some(".txt")), "notes");
        assert_eq!(base("/a/notes.txt/", Some(".txt")), "notes");
        assert_eq!(base("/a/.txt", Some(".txt")), ".txt", "all of the name");
        assert_eq!(base("/a/notes.txt", Some(".md")), "notes.txt");
        assert_eq!(base("/a/notes.txt", Some("")), "notes.txt");
        assert_eq!(base("/a/b.txt.txt", Some(".txt")), "b.txt", "once");
        assert_eq!(base("/", Some("/")), "/");
        assert_eq!(base("x", Some("x")), "x");
    }
}
