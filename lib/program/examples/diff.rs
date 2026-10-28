//! diff: says how two files differ, as POSIX's diff does: by default as
//! the lines to change, with `-c` or `-C n` with lines of context around
//! each change, with `-u` or `-U n` in the unified form, with `-e` as a
//! script for ed, and with `-f` as one in the order of the file. With
//! `-b` a run of blanks counts as one, and none at the end of a line.
//! Given two directories it compares the files of one name in both, and
//! with `-r` the directories too; given a directory and a file, the file
//! of that name in the directory. A `-` is the standard input. It ends
//! with 0 when the files are alike, 1 when they differ, and 2 on trouble.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use diff::{Form, Text};
use program::{Error, Handle, abi, text};

program::main!(diff);

/// What diff ends with: the files were alike, they differed, or there
/// was trouble. Of several comparisons the worst counts.
const ALIKE: u64 = 0;
const DIFFERENT: u64 = 1;
const TROUBLE: u64 = 2;

/// What diff's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    form: Form,
    /// Whether a run of blanks counts as one, and none at a line's end.
    blanks: bool,
    /// Whether directories found in both are compared too.
    deep: bool,
    /// The words before the files, which diff says again before the
    /// differences of each two files it finds in directories.
    options: &'w [String],
    files: (&'w str, &'w str),
}

/// What diff's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut form = Form::Normal;
    let (mut blanks, mut deep) = (false, false);
    let mut words = arguments;
    let mut options = 0;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        words = rest;
        for (at, letter) in letters.char_indices() {
            match letter {
                'b' => blanks = true,
                'r' => deep = true,
                'c' => form = Form::Context(3),
                'u' => form = Form::Unified(3),
                'e' => form = Form::Ed,
                'f' => form = Form::Forward,
                'C' | 'U' => {
                    // The number may follow the letter or be the next word.
                    let attached = &letters[at + letter.len_utf8()..];
                    let number = if attached.is_empty() {
                        let (number, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("diff: -{letter} wants a number of lines\n"))?;
                        words = rest;
                        number.as_str()
                    } else {
                        attached
                    };
                    let lines = number.parse().map_err(|_| {
                        format!("diff: -{letter} wants a number of lines, not {number}\n")
                    })?;
                    form = if letter == 'C' {
                        Form::Context(lines)
                    } else {
                        Form::Unified(lines)
                    };
                    break;
                }
                _ => return Err(format!("diff: -{letter} is no option\n")),
            }
        }
        options = arguments.len() - words.len();
    }
    match words {
        [first, second] => Ok(Asked {
            form,
            blanks,
            deep,
            options: &arguments[..options],
            files: (first, second),
        }),
        _ => Err(String::from("diff: compares two files\n")),
    }
}

/// Where diff finds files, and where it writes.
trait Files {
    /// Whether the file at `path` is a directory.
    fn directory(&mut self, path: &str) -> Result<bool, Error>;

    /// The names in the directory at `path`.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// What the file at `path`, or the standard input for `-`, holds.
    fn bytes(&mut self, path: &str) -> Result<Vec<u8>, Error>;

    /// Writes `bytes` on the standard output.
    fn out(&mut self, bytes: &[u8]);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// Says what diff could not do with the file at `path`.
fn trouble(files: &mut impl Files, path: &str, e: Error) -> u64 {
    files.say(&text::trouble("diff", path, e));
    TROUBLE
}

/// Whether each of `paths` is a directory, `-` never; or, said already,
/// the trouble finding out.
fn kinds(files: &mut impl Files, (a, b): (&str, &str)) -> Result<(bool, bool), u64> {
    let mut kind = |path: &str| {
        if path == "-" {
            return Ok(false);
        }
        files.directory(path).map_err(|e| trouble(files, path, e))
    };
    Ok((kind(a)?, kind(b)?))
}

/// `name` in the directory at `directory`.
fn joined(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// The file in the directory at `directory` named as the last name of
/// `path`.
fn inside(directory: &str, path: &str) -> String {
    let name = path.rsplit('/').find(|n| !n.is_empty()).unwrap_or(path);
    joined(directory, name)
}

/// Compares the two files `asked` names, as diff does when given them.
fn compare(files: &mut impl Files, asked: &Asked) -> u64 {
    let (a, b) = asked.files;
    let kinds = match kinds(files, (a, b)) {
        Ok(kinds) => kinds,
        Err(outcome) => return outcome,
    };
    match kinds {
        (true, true) => directories(files, asked, (a, b)),
        (false, false) => pair(files, asked, (a, b), false),
        _ if a == "-" || b == "-" => {
            files.say("diff: cannot compare - to a directory\n");
            TROUBLE
        }
        (true, false) => pair(files, asked, (&inside(a, b), b), false),
        (false, true) => pair(files, asked, (a, &inside(b, a)), false),
    }
}

/// Compares the directories at `paths` name by name, in the order of
/// their bytes.
fn directories(files: &mut impl Files, asked: &Asked, (a, b): (&str, &str)) -> u64 {
    let mut left = match files.names(a) {
        Ok(names) => names,
        Err(e) => return trouble(files, a, e),
    };
    let mut right = match files.names(b) {
        Ok(names) => names,
        Err(e) => return trouble(files, b, e),
    };
    left.sort_unstable();
    right.sort_unstable();
    let mut all: Vec<&String> = left.iter().chain(&right).collect();
    all.sort_unstable();
    all.dedup();
    let mut outcome = ALIKE;
    for name in all {
        let found = match (left.binary_search(name), right.binary_search(name)) {
            (Ok(_), Ok(_)) => entry(files, asked, (&joined(a, name), &joined(b, name))),
            (Ok(_), Err(_)) => only(files, a, name),
            _ => only(files, b, name),
        };
        outcome = outcome.max(found);
    }
    outcome
}

/// Says that `name` is in the directory at `directory` only.
fn only(files: &mut impl Files, directory: &str, name: &str) -> u64 {
    files.out(format!("Only in {directory}: {name}\n").as_bytes());
    DIFFERENT
}

/// What diff says a file is.
fn kind(directory: bool) -> &'static str {
    if directory {
        "directory"
    } else {
        "regular file"
    }
}

/// Compares what has one name in two directories, at `paths`.
fn entry(files: &mut impl Files, asked: &Asked, (a, b): (&str, &str)) -> u64 {
    let kinds = match kinds(files, (a, b)) {
        Ok(kinds) => kinds,
        Err(outcome) => return outcome,
    };
    match kinds {
        (true, true) if asked.deep => directories(files, asked, (a, b)),
        (true, true) => {
            files.out(format!("Common subdirectories: {a} and {b}\n").as_bytes());
            ALIKE
        }
        (false, false) => pair(files, asked, (a, b), true),
        (x, y) => {
            let said = format!(
                "File {a} is a {} while file {b} is a {}\n",
                kind(x),
                kind(y)
            );
            files.out(said.as_bytes());
            DIFFERENT
        }
    }
}

/// Compares the files at `paths`, said first as diff says two files it
/// found in directories when `found`.
fn pair(files: &mut impl Files, asked: &Asked, (a, b): (&str, &str), found: bool) -> u64 {
    let first = match files.bytes(a) {
        Ok(bytes) => bytes,
        Err(e) => return trouble(files, a, e),
    };
    let second = match files.bytes(b) {
        Ok(bytes) => bytes,
        Err(e) => return trouble(files, b, e),
    };
    if first == second {
        return ALIKE;
    }
    if first.contains(&0) || second.contains(&0) {
        files.out(format!("Files {a} and {b} differ\n").as_bytes());
        return DIFFERENT;
    }
    let (written, unended) = differences(asked, (&first, &second), (a, b));
    let mut outcome = ALIKE;
    if !written.is_empty() {
        if found {
            let mut header = String::from("diff ");
            for word in asked.options {
                header = header + word + " ";
            }
            files.out(format!("{header}{a} {b}\n").as_bytes());
        }
        files.out(&written);
        outcome = DIFFERENT;
    }
    for (path, unended) in [(a, unended.0), (b, unended.1)] {
        if unended {
            files.say(&format!("diff: {path}: No newline at end of file\n"));
            outcome = TROUBLE;
        }
    }
    outcome
}

/// What diff writes for the texts `bytes`, named `names`, as `asked`; and
/// which of them end without a line feed where the form cannot say so.
fn differences(
    asked: &Asked,
    bytes: (&[u8], &[u8]),
    names: (&str, &str),
) -> (Vec<u8>, (bool, bool)) {
    let (mut a, mut b) = (Text::new(bytes.0), Text::new(bytes.1));
    let says = asked.form.says_unended();
    let unended = (!says && !a.ended, !says && !b.ended);
    if !says {
        (a.ended, b.ended) = (true, true);
    }
    let (left, right) = diff::keys(&a, &b, asked.blanks);
    let mut written = Vec::new();
    diff::write(
        asked.form,
        (&a, &b),
        &diff::hunks(&left, &right),
        names,
        &mut written,
    );
    (written, unended)
}

/// Meridian's files, and the program's own output.
struct System {
    out: text::Out<text::Write>,
}

impl Files for System {
    fn directory(&mut self, path: &str) -> Result<bool, Error> {
        let file = program::open(path, abi::file::READ)?;
        let info = program::info(file);
        let _ = program::close(file);
        Ok(info?.directory)
    }

    fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
        let directory = program::open(path, abi::file::READ)?;
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

    fn bytes(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        text::pieces(Some(path).filter(|&p| p != "-"), |piece| {
            bytes.extend_from_slice(piece);
            true
        })?;
        Ok(bytes)
    }

    fn out(&mut self, bytes: &[u8]) {
        self.out.put(bytes);
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn diff(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return TROUBLE;
        }
    };
    let mut system = System {
        out: text::standard_output(),
    };
    let outcome = compare(&mut system, &asked);
    if system.out.finish() {
        outcome
    } else {
        TROUBLE
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

    type Taken = (Form, bool, bool, String, (String, String));

    /// What `asked` makes of `text`: the form, -b and -r, the options
    /// as words again, and the files.
    fn asked_of(text: &str) -> Result<Taken, String> {
        let given = words(text);
        asked(&given).map(|asked| {
            let (a, b) = asked.files;
            let options = asked.options.join(" ");
            (
                asked.form,
                asked.blanks,
                asked.deep,
                options,
                (a.into(), b.into()),
            )
        })
    }

    fn plain(form: Form, options: &str) -> Taken {
        (
            form,
            false,
            false,
            options.into(),
            ("/a".into(), "/b".into()),
        )
    }

    #[test]
    fn its_words_are_options_and_two_files() {
        assert_eq!(asked_of("/a /b"), Ok(plain(Form::Normal, "")));
        assert_eq!(asked_of("-c /a /b"), Ok(plain(Form::Context(3), "-c")));
        assert_eq!(asked_of("-u /a /b"), Ok(plain(Form::Unified(3), "-u")));
        assert_eq!(asked_of("-e /a /b"), Ok(plain(Form::Ed, "-e")));
        assert_eq!(asked_of("-f /a /b"), Ok(plain(Form::Forward, "-f")));
        assert_eq!(asked_of("-C 5 /a /b"), Ok(plain(Form::Context(5), "-C 5")));
        assert_eq!(asked_of("-C5 /a /b"), Ok(plain(Form::Context(5), "-C5")));
        assert_eq!(asked_of("-U0 /a /b"), Ok(plain(Form::Unified(0), "-U0")));
        assert_eq!(
            asked_of("-U 12 /a /b"),
            Ok(plain(Form::Unified(12), "-U 12"))
        );
        assert_eq!(
            asked_of("-c -u /a /b"),
            Ok(plain(Form::Unified(3), "-c -u")),
            "the last form counts"
        );
        assert_eq!(
            asked_of("-bU2 /a /b"),
            Ok((
                Form::Unified(2),
                true,
                false,
                "-bU2".into(),
                ("/a".into(), "/b".into())
            ))
        );
        assert_eq!(
            asked_of("-rb /a /b"),
            Ok((
                Form::Normal,
                true,
                true,
                "-rb".into(),
                ("/a".into(), "/b".into())
            ))
        );
        assert_eq!(
            asked_of("-r -- -u /b"),
            Ok((
                Form::Normal,
                false,
                true,
                "-r".into(),
                ("-u".into(), "/b".into())
            )),
            "-- ends the options and is not said again"
        );
        assert_eq!(
            asked_of("- /b"),
            Ok((
                Form::Normal,
                false,
                false,
                String::new(),
                ("-".into(), "/b".into())
            )),
            "a - is a file"
        );
    }

    #[test]
    fn words_it_cannot_take_are_refused() {
        let two = Err(String::from("diff: compares two files\n"));
        assert_eq!(asked_of("/a"), two);
        assert_eq!(asked_of(""), two);
        assert_eq!(asked_of("/a /b /c"), two);
        assert_eq!(asked_of("/a /b -u"), two, "options come before the files");
        assert_eq!(asked_of("-x /a /b"), Err("diff: -x is no option\n".into()));
        assert_eq!(asked_of("-bx /a /b"), Err("diff: -x is no option\n".into()));
        assert_eq!(
            asked_of("-U"),
            Err("diff: -U wants a number of lines\n".into())
        );
        assert_eq!(
            asked_of("-C x /a /b"),
            Err("diff: -C wants a number of lines, not x\n".into())
        );
        assert_eq!(
            asked_of("-U -1 /a /b"),
            Err("diff: -U wants a number of lines, not -1\n".into())
        );
        assert_eq!(
            asked_of("-U3b /a /b"),
            Err("diff: -U wants a number of lines, not 3b\n".into()),
            "the number is the rest of the word"
        );
    }

    /// What `differences` writes for `a` and `b` in `form`, with -b when
    /// `blanks`, and which it found unended where the form cannot say so.
    fn written(form: Form, blanks: bool, a: &str, b: &str) -> (String, (bool, bool)) {
        let asked = Asked {
            form,
            blanks,
            deep: false,
            options: &[],
            files: ("a", "b"),
        };
        let (written, unended) = differences(&asked, (a.as_bytes(), b.as_bytes()), ("A", "B"));
        (String::from_utf8(written).unwrap(), unended)
    }

    #[test]
    fn differences_are_written_in_the_form_asked() {
        let neither = (false, false);
        assert_eq!(
            written(Form::Normal, false, "x\ny\n", "x\nz\n"),
            ("2c2\n< y\n---\n> z\n".into(), neither)
        );
        assert_eq!(
            written(Form::Unified(1), false, "x\ny\n", "x\nz\n"),
            (
                "--- A\n+++ B\n@@ -1,2 +1,2 @@\n x\n-y\n+z\n".into(),
                neither
            )
        );
        assert_eq!(
            written(Form::Normal, true, "a  b \n", "a b\n"),
            (String::new(), neither),
            "with -b"
        );
        assert_eq!(
            written(Form::Normal, false, "a  b \n", "a b\n").0,
            "1c1\n< a  b \n---\n> a b\n"
        );
        assert_eq!(
            written(Form::Normal, false, "x", "x\n"),
            (
                "1c1\n< x\n\\ No newline at end of file\n---\n> x\n".into(),
                neither
            )
        );
        assert_eq!(
            written(Form::Ed, false, "x", "x\n"),
            (String::new(), (true, false)),
            "an ed script cannot say a line feed is missing"
        );
        assert_eq!(
            written(Form::Forward, false, "x\n", "y"),
            ("c1\ny\n.\n".into(), (false, true))
        );
        assert_eq!(
            written(Form::Ed, false, "", "y\n"),
            ("0a\ny\n.\n".into(), neither),
            "an empty file ends as a text should"
        );
    }

    /// A tree of files among the host's temporary files, gone once
    /// dropped, the standard input diff reads, and what it wrote and
    /// said. It lists names the other way round from the order diff
    /// wants, so nothing leans on how a disk happens to keep them.
    struct Host {
        root: std::path::PathBuf,
        input: Vec<u8>,
        written: Vec<u8>,
        said: String,
    }

    impl Host {
        /// The tree that holds `files`, each a path and what it holds; a
        /// path that ends in `/` is a directory.
        fn new(name: &str, files: &[(&str, &str)]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-diff-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for (path, bytes) in files {
                let at = root.join(path.trim_end_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, bytes).unwrap();
                }
            }
            Host {
                root,
                input: Vec::new(),
                written: Vec::new(),
                said: String::new(),
            }
        }
    }

    /// The error a Meridian call answers with where the host's failed so.
    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            std::io::ErrorKind::IsADirectory => Error::NotAFile,
            _ => Error::Unreadable,
        }
    }

    impl Files for Host {
        fn directory(&mut self, path: &str) -> Result<bool, Error> {
            let found = std::fs::metadata(self.root.join(path));
            found.map(|m| m.is_dir()).map_err(|e| error(&e))
        }

        fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
            let listed = std::fs::read_dir(self.root.join(path)).map_err(|e| error(&e))?;
            let mut names: Vec<String> = listed
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort_unstable_by(|x, y| y.cmp(x));
            Ok(names)
        }

        fn bytes(&mut self, path: &str) -> Result<Vec<u8>, Error> {
            if path == "-" {
                return Ok(core::mem::take(&mut self.input));
            }
            std::fs::read(self.root.join(path)).map_err(|e| error(&e))
        }

        fn out(&mut self, bytes: &[u8]) {
            self.written.extend_from_slice(bytes);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// How diff ran on `host` with the words `text`: what it ended with,
    /// what it wrote and what it said.
    fn ran(host: &mut Host, text: &str) -> (u64, String, String) {
        let given = words(text);
        let outcome = compare(host, &asked(&given).unwrap());
        let written = String::from_utf8(core::mem::take(&mut host.written)).unwrap();
        (outcome, written, core::mem::take(&mut host.said))
    }

    fn wrote(outcome: u64, written: &str) -> (u64, String, String) {
        (outcome, written.into(), String::new())
    }

    fn said(said: &str) -> (u64, String, String) {
        (TROUBLE, String::new(), said.into())
    }

    #[test]
    fn two_files_alike_differing_or_missing() {
        let mut host = Host::new(
            "files",
            &[("a", "x\ny\n"), ("b", "x\nz\n"), ("c", "x\ny\n")],
        );
        assert_eq!(ran(&mut host, "a c"), wrote(ALIKE, ""));
        assert_eq!(
            ran(&mut host, "a b"),
            wrote(DIFFERENT, "2c2\n< y\n---\n> z\n")
        );
        assert_eq!(
            ran(&mut host, "-u a b"),
            wrote(DIFFERENT, "--- a\n+++ b\n@@ -1,2 +1,2 @@\n x\n-y\n+z\n")
        );
        assert_eq!(ran(&mut host, "-e b a"), wrote(DIFFERENT, "2c\ny\n.\n"));
        assert_eq!(ran(&mut host, "a nope"), said("diff: nope: no such file\n"));
        assert_eq!(ran(&mut host, "nope a"), said("diff: nope: no such file\n"));
    }

    #[test]
    fn files_that_are_no_text_are_only_said_to_differ() {
        let mut host = Host::new("binary", &[("a", "a\0b"), ("b", "a\0c"), ("c", "x\n")]);
        assert_eq!(
            ran(&mut host, "a b"),
            wrote(DIFFERENT, "Files a and b differ\n")
        );
        assert_eq!(
            ran(&mut host, "-u c a"),
            wrote(DIFFERENT, "Files c and a differ\n")
        );
        assert_eq!(ran(&mut host, "a a"), wrote(ALIKE, ""));
    }

    #[test]
    fn a_dash_is_the_standard_input() {
        let mut host = Host::new("input", &[("a", "x\ny\n"), ("d/", "")]);
        host.input = b"x\n".to_vec();
        assert_eq!(ran(&mut host, "- a"), wrote(DIFFERENT, "1a2\n> y\n"));
        host.input = b"x\ny\n".to_vec();
        assert_eq!(ran(&mut host, "a -"), wrote(ALIKE, ""));
        let refused = said("diff: cannot compare - to a directory\n");
        assert_eq!(ran(&mut host, "- d"), refused);
        assert_eq!(ran(&mut host, "d -"), refused);
    }

    #[test]
    fn a_file_and_a_directory_compare_the_file_of_its_name_in_it() {
        let mut host = Host::new(
            "inside",
            &[("f", "1\n"), ("d/f", "2\n"), ("e/", ""), ("x/f", "1\n")],
        );
        assert_eq!(
            ran(&mut host, "f d"),
            wrote(DIFFERENT, "1c1\n< 1\n---\n> 2\n")
        );
        assert_eq!(
            ran(&mut host, "d f"),
            wrote(DIFFERENT, "1c1\n< 2\n---\n> 1\n")
        );
        assert_eq!(
            ran(&mut host, "-u x/f d/"),
            wrote(DIFFERENT, "--- x/f\n+++ d/f\n@@ -1 +1 @@\n-1\n+2\n"),
            "the last name of a path, in a directory written with a slash"
        );
        assert_eq!(ran(&mut host, "f e"), said("diff: e/f: no such file\n"));
    }

    /// Two directories much like those GNU's diff was tried on.
    fn trees(name: &str) -> Host {
        Host::new(
            name,
            &[
                ("a/f", "x\n"),
                ("b/f", "y\n"),
                ("a/g", "same\n"),
                ("b/g", "same\n"),
                ("a/onlya", "q\n"),
                ("b/onlyb", "q\n"),
                ("a/only/", ""),
                ("a/s/t/deep", "1\n"),
                ("b/s/t/deep", "2\n"),
                ("a/both", "k\n"),
                ("b/both/", ""),
                ("a/sub/", ""),
                ("b/sub", "k\n"),
                ("a/bin", "a\0b"),
                ("b/bin", "a\0c"),
            ],
        )
    }

    #[test]
    fn directories_are_compared_name_by_name() {
        let mut host = trees("directories");
        assert_eq!(
            ran(&mut host, "a b"),
            wrote(
                DIFFERENT,
                "Files a/bin and b/bin differ\n\
                 File a/both is a regular file while file b/both is a directory\n\
                 diff a/f b/f\n1c1\n< x\n---\n> y\n\
                 Only in a: only\nOnly in a: onlya\nOnly in b: onlyb\n\
                 Common subdirectories: a/s and b/s\n\
                 File a/sub is a directory while file b/sub is a regular file\n"
            )
        );
    }

    #[test]
    fn with_r_directories_in_both_are_compared_too() {
        let mut host = trees("deep");
        assert_eq!(
            ran(&mut host, "-r -u a b"),
            wrote(
                DIFFERENT,
                "Files a/bin and b/bin differ\n\
                 File a/both is a regular file while file b/both is a directory\n\
                 diff -r -u a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+y\n\
                 Only in a: only\nOnly in a: onlya\nOnly in b: onlyb\n\
                 diff -r -u a/s/t/deep b/s/t/deep\n--- a/s/t/deep\n+++ b/s/t/deep\n\
                 @@ -1 +1 @@\n-1\n+2\n\
                 File a/sub is a directory while file b/sub is a regular file\n"
            )
        );
    }

    #[test]
    fn directories_alike_but_for_those_in_them() {
        let mut host = Host::new(
            "alike",
            &[
                ("p/s/x", "1\n"),
                ("q/s/x", "1\n"),
                ("p/y", "a\n"),
                ("q/y", "a\n"),
                ("e/", ""),
                ("f/", ""),
            ],
        );
        assert_eq!(
            ran(&mut host, "p q"),
            wrote(ALIKE, "Common subdirectories: p/s and q/s\n"),
            "only a difference, not a directory in both, makes diff end with 1"
        );
        assert_eq!(ran(&mut host, "-r p/ q/"), wrote(ALIKE, ""));
        assert_eq!(ran(&mut host, "e f"), wrote(ALIKE, ""));
        assert_eq!(
            ran(&mut host, "e p"),
            wrote(DIFFERENT, "Only in p: s\nOnly in p: y\n")
        );
    }

    #[test]
    fn the_worst_of_what_diff_found_counts() {
        let mut host = Host::new(
            "worst",
            &[
                ("p/x", "1"),
                ("q/x", "2\n"),
                ("p/y", "a\n"),
                ("q/y", "b\n"),
                ("p/z", "c\n"),
            ],
        );
        let (outcome, written, said) = ran(&mut host, "-e p q");
        assert_eq!(
            (outcome, written.as_str()),
            (
                TROUBLE,
                "diff -e p/x q/x\n1c\n2\n.\ndiff -e p/y q/y\n1c\nb\n.\nOnly in p: z\n"
            ),
            "and diff goes on after trouble"
        );
        assert_eq!(said, "diff: p/x: No newline at end of file\n");
        assert_eq!(ran(&mut host, "p/y q/y").0, DIFFERENT);
        assert_eq!(ran(&mut host, "p/y p/y").0, ALIKE);
    }

    #[test]
    fn an_ed_script_says_a_line_feed_it_cannot_write_is_missing() {
        let mut host = Host::new(
            "unended",
            &[("a", "x\ny"), ("b", "x\nz\n"), ("c", "x\ny\n")],
        );
        let missing = String::from("diff: a: No newline at end of file\n");
        assert_eq!(
            ran(&mut host, "-e a b"),
            (TROUBLE, "2c\nz\n.\n".into(), missing.clone())
        );
        assert_eq!(
            ran(&mut host, "-f b a"),
            (TROUBLE, "c2\ny\n.\n".into(), missing.clone())
        );
        assert_eq!(
            ran(&mut host, "-e a c"),
            (TROUBLE, String::new(), missing),
            "even when it is all that differs"
        );
        assert_eq!(
            ran(&mut host, "a c"),
            wrote(
                DIFFERENT,
                "2c2\n< y\n\\ No newline at end of file\n---\n> y\n"
            )
        );
        assert_eq!(ran(&mut host, "-e a a"), wrote(ALIKE, ""));
    }

    #[test]
    fn names_join_with_one_slash() {
        assert_eq!(joined("/a", "b"), "/a/b");
        assert_eq!(joined("/a/", "b"), "/a/b");
        assert_eq!(joined("/", "b"), "/b");
        assert_eq!(inside("/d", "/x/y/f"), "/d/f");
        assert_eq!(inside("/d", "f"), "/d/f");
        assert_eq!(inside("/d", "/x/f/"), "/d/f");
        assert_eq!(kind(true), "directory");
        assert_eq!(kind(false), "regular file");
    }
}
