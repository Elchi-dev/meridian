//! patch: puts differences diff wrote into the files they were made
//! from, as POSIX's patch does. It reads diff's own form, the context and
//! unified forms and ed scripts, from its standard input or the file `-i`
//! names, finds each hunk where it says or near there, leaving out a line
//! or two of context at its ends when it has to, and keeps a hunk it finds
//! no place for in a file of rejects. `-p` says how much of the names a
//! patch gives to leave out, `-R` puts a patch in the other way round,
//! `-N` leaves one alone that seems put in already, `-l` matches blanks
//! loosely, `-D` keeps both versions for the C preprocessor, `-b` keeps
//! the file as it was, and `-o` writes the result elsewhere. It ends with
//! 0 when every hunk went in, 1 when one did not, and 2 on trouble.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use diff::Text;
use diff::apply::{self, Fit, Placed};
use diff::read::{self, Edit, Hunks, Kind, Patch};
use program::abi::file::{CREATE_ALWAYS, OPEN_ALWAYS, READ, WRITE};
use program::{Error, Handle, text};

program::main!(patch);

/// What patch ends with: every hunk went in, one did not, or there was
/// trouble. Of several patches the worst counts.
const DONE: u64 = 0;
const REJECTED: u64 = 1;
const TROUBLE: u64 = 2;

/// What patch's words ask for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Asked<'w> {
    /// Whether the file is kept as it was, with `.orig` after its name.
    backup: bool,
    kind: Option<Kind>,
    directory: Option<&'w str>,
    define: Option<&'w str>,
    input: Option<&'w str>,
    /// How closely hunks have to match: blanks loosely with `-l`, and a
    /// line or two of context at each end left out when it must.
    fit: Fit,
    /// Whether a patch that seems put in already is left alone.
    applied: bool,
    output: Option<&'w str>,
    strip: Option<usize>,
    reverse: bool,
    rejects: Option<&'w str>,
    file: Option<&'w str>,
}

/// What an option that takes a value wants.
fn wants(letter: char) -> &'static str {
    match letter {
        'd' => "a directory",
        'D' => "a name",
        'p' => "a number",
        _ => "a file",
    }
}

/// What patch's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        fit: Fit {
            loose: false,
            fuzz: 2,
        },
        ..Asked::default()
    };
    let mut words = arguments;
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
                'b' => asked.backup = true,
                'l' => asked.fit.loose = true,
                'N' => asked.applied = true,
                'R' => asked.reverse = true,
                'c' => asked.kind = Some(Kind::Context),
                'e' => asked.kind = Some(Kind::Ed),
                'n' => asked.kind = Some(Kind::Normal),
                'u' => asked.kind = Some(Kind::Unified),
                'd' | 'D' | 'i' | 'o' | 'p' | 'r' => {
                    // The value may follow the letter or be the next word.
                    let attached = &letters[at + 1..];
                    let value = if attached.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("patch: -{letter} wants {}\n", wants(letter)))?;
                        words = rest;
                        value.as_str()
                    } else {
                        attached
                    };
                    match letter {
                        'd' => asked.directory = Some(value),
                        'D' => asked.define = Some(value),
                        'i' => asked.input = Some(value),
                        'o' => asked.output = Some(value),
                        'r' => asked.rejects = Some(value),
                        _ => {
                            let strip = value
                                .parse()
                                .map_err(|_| format!("patch: -p wants a number, not {value}\n"))?;
                            asked.strip = Some(strip);
                        }
                    }
                    break;
                }
                _ => return Err(format!("patch: -{letter} is no option\n")),
            }
        }
    }
    match words {
        [] => {}
        [file] => asked.file = Some(file),
        _ => return Err(String::from("patch: patches one file\n")),
    }
    Ok(asked)
}

/// Where patch reads and writes, and whom it asks.
trait Files {
    /// What the file at `path` holds, or the standard input for none.
    fn read(&mut self, path: Option<&str>) -> Result<Vec<u8>, Error>;

    /// Whether something is at `path`.
    fn exists(&mut self, path: &str) -> bool;

    /// Makes the file at `path` hold `bytes`, or with `adding` puts them
    /// after what it holds.
    fn write(&mut self, path: &str, bytes: &[u8], adding: bool) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);

    /// The answer typed to `question`, none when no one can answer.
    fn ask(&mut self, question: &str) -> Option<String>;
}

/// Says what patch could not do with the file at `path`.
fn trouble(files: &mut impl Files, path: &str, e: Error) -> u64 {
    files.say(&text::trouble("patch", path, e));
    TROUBLE
}

/// `name` without as many of its leading parts as `strip` says, a
/// leading slash counting as one, or without all but the last when it
/// says none; none when it has fewer.
fn stripped(name: &str, strip: Option<usize>) -> Option<&str> {
    let Some(count) = strip else {
        return name.rsplit('/').next();
    };
    let mut rest = name;
    for _ in 0..count {
        rest = &rest[rest.find('/')? + 1..];
    }
    Some(rest)
}

/// Whether `patch` only puts lines into an empty file, as one that makes
/// a file does.
fn only_adds(patch: &Patch) -> bool {
    match &patch.hunks {
        Hunks::Edits(edits) => edits
            .iter()
            .all(|edit| edit.old == 0 && edit.side(false).next().is_none()),
        Hunks::Ed(_) => false,
    }
}

/// The file `patch` is for, and whether patch makes it: the file named,
/// else the first of the names the patch gives that is there, else the
/// new one for a patch that makes a file, else the one typed when asked.
fn target(files: &mut impl Files, asked: &Asked, patch: &Patch) -> Option<(String, bool)> {
    if let Some(file) = asked.file {
        let made = !files.exists(file) && only_adds(patch);
        return Some((file.into(), made));
    }
    let names: Vec<String> = [patch.old_name, patch.new_name, patch.index]
        .into_iter()
        .flatten()
        .filter(|&name| name != b"/dev/null")
        .filter_map(|name| {
            let name = String::from_utf8_lossy(name);
            stripped(&name, asked.strip).map(String::from)
        })
        .filter(|name| !name.is_empty())
        .collect();
    if let Some(found) = names.iter().find(|name| files.exists(name)) {
        return Some((found.clone(), false));
    }
    if only_adds(patch) {
        let new = patch
            .new_name
            .filter(|&name| name != b"/dev/null")
            .and_then(|name| {
                stripped(&String::from_utf8_lossy(name), asked.strip).map(String::from)
            })
            .filter(|name| !name.is_empty());
        if let Some(new) = new {
            return Some((new, true));
        }
    }
    let typed = files.ask("patch: which file is the patch for? ")?;
    let typed = typed.trim();
    (!typed.is_empty()).then(|| (typed.into(), !files.exists(typed) && only_adds(patch)))
}

/// `count` lines, in words.
fn lines(count: usize) -> String {
    if count == 1 {
        String::from("1 line")
    } else {
        format!("{count} lines")
    }
}

/// What patch says about hunk `number` of the patch for `name`, counted
/// from 1: nothing when it went in where it said, as it was.
fn said(name: &str, number: usize, placed: Placed) -> Option<String> {
    let Placed::At { line, offset, fuzz } = placed else {
        return Some(format!("patch: {name}: hunk {number} found no place\n"));
    };
    if offset == 0 && fuzz == 0 {
        return None;
    }
    let mut said = format!("patch: {name}: hunk {number} went in at line {}", line + 1);
    let way = match offset.cmp(&0) {
        core::cmp::Ordering::Greater => Some("later"),
        core::cmp::Ordering::Less => Some("earlier"),
        core::cmp::Ordering::Equal => None,
    };
    if let Some(way) = way {
        let _ = write!(
            said,
            ", {} {way} than it said",
            lines(offset.unsigned_abs())
        );
    }
    if fuzz > 0 {
        let _ = write!(
            said,
            ", leaving out up to {} of its context at each end",
            lines(fuzz)
        );
    }
    Some(said + "\n")
}

/// What patch has written so far into the files `-o` and `-r` name, which
/// take every patch's output one after another.
#[derive(Default)]
struct Going {
    output: bool,
    rejects: bool,
}

/// Puts in the patches the input `asked` names holds, and says how that
/// went.
fn run(files: &mut impl Files, asked: &Asked) -> u64 {
    let input = asked.input.filter(|&path| path != "-");
    let bytes = match files.read(input) {
        Ok(bytes) => bytes,
        Err(e) => return trouble(files, input.unwrap_or("-"), e),
    };
    let found = match read::patches(&bytes, asked.kind) {
        Ok(found) => found,
        Err(wrong) => {
            let name = input.unwrap_or("-");
            files.say(&format!(
                "patch: {name}: line {}: {}\n",
                wrong.line, wrong.why
            ));
            return TROUBLE;
        }
    };
    if found.is_empty() {
        files.say("patch: no patch in the input\n");
        return TROUBLE;
    }
    let mut going = Going::default();
    found
        .iter()
        .map(|patch| one(files, asked, patch, &mut going))
        .fold(DONE, u64::max)
}

/// Puts in one patch, and says how that went.
fn one(files: &mut impl Files, asked: &Asked, patch: &Patch, going: &mut Going) -> u64 {
    let Some((name, made)) = target(files, asked, patch) else {
        files.say("patch: no file for a patch, so it is left out\n");
        return REJECTED;
    };
    let old = if made {
        Vec::new()
    } else {
        match files.read(Some(&name)) {
            Ok(bytes) => bytes,
            Err(e) => return trouble(files, &name, e),
        }
    };
    let text = apply::lines(&Text::new(&old));
    let (new, rejected) = match &patch.hunks {
        Hunks::Ed(commands) => {
            if asked.reverse {
                files.say("patch: an ed script cannot be put in reversed\n");
                return TROUBLE;
            }
            match apply::run_ed(&text, commands) {
                Ok(new) => (new, Vec::new()),
                Err(at) => {
                    let said = format!(
                        "patch: {name}: the script's command {} names lines the file does not have\n",
                        at + 1
                    );
                    files.say(&said);
                    return TROUBLE;
                }
            }
        }
        Hunks::Edits(edits) => {
            let Some(placed) = placed(files, asked, &name, &text, edits) else {
                return DONE;
            };
            placed
        }
    };
    let out = asked.output.unwrap_or(&name);
    if asked.backup {
        let kept = match asked.output {
            None if !made => Some(Ok(old)),
            Some(out) if !going.output && files.exists(out) => Some(files.read(Some(out))),
            _ => None,
        };
        if let Some(kept) = kept {
            let orig = format!("{out}.orig");
            if let Err(e) = kept.and_then(|bytes| files.write(&orig, &bytes, false)) {
                return trouble(files, &orig, e);
            }
        }
    }
    let adding = asked.output.is_some() && going.output;
    if let Err(e) = files.write(out, &new, adding) {
        return trouble(files, out, e);
    }
    going.output |= asked.output.is_some();
    if rejected.is_empty() {
        return DONE;
    }
    let mut bytes = Vec::new();
    let names = [patch.old_name, patch.new_name]
        .map(|n| n.map_or_else(|| name.clone(), |n| String::from_utf8_lossy(n).into_owned()));
    diff::write::rejects(
        patch.kind == Kind::Unified,
        &rejected,
        (&names[0], &names[1]),
        &mut bytes,
    );
    let kept_in = asked
        .rejects
        .map_or_else(|| format!("{name}.rej"), String::from);
    let adding = asked.rejects.is_some() && going.rejects;
    if let Err(e) = files.write(&kept_in, &bytes, adding) {
        return trouble(files, &kept_in, e);
    }
    going.rejects |= asked.rejects.is_some();
    let total = match &patch.hunks {
        Hunks::Edits(edits) => edits.len(),
        Hunks::Ed(commands) => commands.len(),
    };
    files.say(&format!(
        "patch: {name}: {} of {total} hunks found no place, kept in {kept_in}\n",
        rejected.len()
    ));
    REJECTED
}

/// The hunks `edits` put into `text`, the file `name` holds: what it
/// becomes and the hunks that found no place; none when the patch seems
/// put in already and is left alone. When it seems so without `-N`, the
/// one at the console says whether to put it in the other way round.
fn placed<'a>(
    files: &mut impl Files,
    asked: &Asked,
    name: &str,
    text: &[read::Line],
    edits: &[Edit<'a>],
) -> Option<(Vec<u8>, Vec<Edit<'a>>)> {
    let fit = asked.fit;
    let mut edits: Vec<Edit> = if asked.reverse {
        edits.iter().map(Edit::reversed).collect()
    } else {
        edits.to_vec()
    };
    let exact = Fit { fuzz: 0, ..fit };
    let undone = edits.first().is_some_and(|first| {
        !apply::fits(text, first, fit) && apply::fits(text, &first.reversed(), exact)
    });
    if undone {
        if asked.applied {
            files.say(&format!(
                "patch: {name}: the patch seems put in already, so it is left alone\n"
            ));
            return None;
        }
        let question = format!(
            "patch: {name}: the patch seems made the other way round, or put in already; put it in reversed? [n] "
        );
        if files
            .ask(&question)
            .is_some_and(|answer| answer.starts_with(['y', 'Y']))
        {
            edits = edits.iter().map(Edit::reversed).collect();
        }
    }
    let (new, placed) = apply::apply(text, &edits, fit, asked.define.map(str::as_bytes));
    for (number, &place) in (1..).zip(&placed) {
        if let Some(said) = said(name, number, place) {
            files.say(&said);
        }
    }
    let rejected = edits
        .into_iter()
        .zip(placed)
        .filter(|(_, place)| *place == Placed::Rejected)
        .map(|(edit, _)| edit)
        .collect();
    Some((new, rejected))
}

/// Meridian's files, and the one at its console.
struct System;

impl Files for System {
    fn read(&mut self, path: Option<&str>) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        text::pieces(path, |piece| {
            bytes.extend_from_slice(piece);
            true
        })?;
        Ok(bytes)
    }

    fn exists(&mut self, path: &str) -> bool {
        program::open(path, READ).is_ok_and(|file| {
            let _ = program::close(file);
            true
        })
    }

    fn write(&mut self, path: &str, bytes: &[u8], adding: bool) -> Result<(), Error> {
        let how = if adding { OPEN_ALWAYS } else { CREATE_ALWAYS };
        let file = program::open_with(path, WRITE, how)?;
        let at = if adding {
            program::info(file).map(|info| info.size)
        } else {
            Ok(0)
        };
        let written = at.and_then(|at| program::write_file(file, at, bytes));
        let _ = program::close(file);
        written
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }

    fn ask(&mut self, question: &str) -> Option<String> {
        let _ = program::write_error(question);
        program::typed_line()
    }
}

#[cfg_attr(test, allow(dead_code))]
fn patch(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return TROUBLE;
        }
    };
    if let Some(directory) = asked.directory
        && let Err(e) = program::change_directory(directory)
    {
        let _ = program::write_error(&text::trouble("patch", directory, e));
        return TROUBLE;
    }
    run(&mut System, &asked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use diff::{Form, keys, write};

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    #[test]
    fn its_words_are_options_and_a_file() {
        let given = words("-blNR -u -d /w -D FOO -i p.diff -o out -p1 -r rej f");
        let all = asked(&given).unwrap();
        assert_eq!(
            all,
            Asked {
                backup: true,
                kind: Some(Kind::Unified),
                directory: Some("/w"),
                define: Some("FOO"),
                input: Some("p.diff"),
                fit: Fit {
                    loose: true,
                    fuzz: 2
                },
                applied: true,
                output: Some("out"),
                strip: Some(1),
                reverse: true,
                rejects: Some("rej"),
                file: Some("f"),
            }
        );
        let plain = words("");
        assert_eq!(
            asked(&plain).unwrap(),
            Asked {
                fit: Fit {
                    loose: false,
                    fuzz: 2
                },
                ..Asked::default()
            },
            "fuzz 2 unless told otherwise"
        );
        for (text, kind) in [
            ("-c", Kind::Context),
            ("-e", Kind::Ed),
            ("-n", Kind::Normal),
            ("-cu", Kind::Unified),
        ] {
            let given = words(text);
            assert_eq!(asked(&given).unwrap().kind, Some(kind), "{text}");
        }
        let given = words("-p 0 -ifile -- -f");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (taken.strip, taken.input, taken.file),
            (Some(0), Some("file"), Some("-f"))
        );
    }

    #[test]
    fn words_it_cannot_take_are_refused() {
        let refused = |text: &str| asked(&words(text)).err();
        assert_eq!(refused("-x"), Some("patch: -x is no option\n".into()));
        assert_eq!(refused("a b"), Some("patch: patches one file\n".into()));
        assert_eq!(refused("-p"), Some("patch: -p wants a number\n".into()));
        assert_eq!(
            refused("-p x"),
            Some("patch: -p wants a number, not x\n".into())
        );
        assert_eq!(refused("-d"), Some("patch: -d wants a directory\n".into()));
        assert_eq!(refused("-D"), Some("patch: -D wants a name\n".into()));
        for letter in ["i", "o", "r"] {
            assert_eq!(
                refused(&format!("-{letter}")),
                Some(format!("patch: -{letter} wants a file\n"))
            );
        }
    }

    #[test]
    fn names_lose_as_many_parts_as_asked() {
        assert_eq!(stripped("a/b/c", None), Some("c"));
        assert_eq!(stripped("c", None), Some("c"));
        assert_eq!(stripped("/a/b", Some(0)), Some("/a/b"));
        assert_eq!(
            stripped("/a/b", Some(1)),
            Some("a/b"),
            "a leading slash is a part"
        );
        assert_eq!(stripped("a/b/c", Some(2)), Some("c"));
        assert_eq!(stripped("a/b/c", Some(3)), None);
        assert_eq!(stripped("a/", None), Some(""));
    }

    #[test]
    fn a_hunk_is_spoken_of_only_when_it_moved_or_found_no_place() {
        assert_eq!(
            said(
                "f",
                1,
                Placed::At {
                    line: 4,
                    offset: 0,
                    fuzz: 0
                }
            ),
            None
        );
        assert_eq!(
            said(
                "f",
                2,
                Placed::At {
                    line: 6,
                    offset: 3,
                    fuzz: 0
                }
            )
            .as_deref(),
            Some("patch: f: hunk 2 went in at line 7, 3 lines later than it said\n")
        );
        assert_eq!(
            said(
                "f",
                1,
                Placed::At {
                    line: 0,
                    offset: -1,
                    fuzz: 1
                }
            )
            .as_deref(),
            Some(
                "patch: f: hunk 1 went in at line 1, 1 line earlier than it said, leaving out up to 1 line of its context at each end\n"
            )
        );
        assert_eq!(
            said(
                "f",
                3,
                Placed::At {
                    line: 9,
                    offset: 0,
                    fuzz: 2
                }
            )
            .as_deref(),
            Some(
                "patch: f: hunk 3 went in at line 10, leaving out up to 2 lines of its context at each end\n"
            )
        );
        assert_eq!(
            said("g", 4, Placed::Rejected).as_deref(),
            Some("patch: g: hunk 4 found no place\n")
        );
    }

    /// A tree of files among the host's temporary files, gone once
    /// dropped, the standard input patch reads, the answers typed to it,
    /// and what it asked and said.
    struct Host {
        root: std::path::PathBuf,
        input: Vec<u8>,
        answers: Vec<String>,
        asked: Vec<String>,
        said: String,
    }

    impl Host {
        fn new(name: &str, files: &[(&str, &str)]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-patch-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for (path, bytes) in files {
                let at = root.join(path);
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::fs::write(at, bytes).unwrap();
            }
            Host {
                root,
                input: Vec::new(),
                answers: Vec::new(),
                asked: Vec::new(),
                said: String::new(),
            }
        }

        /// What the file at `path` holds, none when nothing is there.
        fn holds(&self, path: &str) -> Option<String> {
            std::fs::read(self.root.join(path))
                .ok()
                .map(|b| String::from_utf8(b).unwrap())
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
            std::io::ErrorKind::IsADirectory => Error::NotAFile,
            _ => Error::Unreadable,
        }
    }

    impl Files for Host {
        fn read(&mut self, path: Option<&str>) -> Result<Vec<u8>, Error> {
            match path {
                None => Ok(core::mem::take(&mut self.input)),
                Some(path) => std::fs::read(self.root.join(path)).map_err(|e| error(&e)),
            }
        }

        fn exists(&mut self, path: &str) -> bool {
            self.root.join(path).exists()
        }

        fn write(&mut self, path: &str, bytes: &[u8], adding: bool) -> Result<(), Error> {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(adding)
                .truncate(!adding)
                .open(self.root.join(path))
                .map_err(|e| error(&e))?;
            file.write_all(bytes).map_err(|e| error(&e))
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }

        fn ask(&mut self, question: &str) -> Option<String> {
            self.asked.push(question.into());
            (!self.answers.is_empty()).then(|| self.answers.remove(0))
        }
    }

    /// How patch ran on `host` with the words `text` and `patch` as its
    /// input: what it ended with, and what it said.
    fn ran(host: &mut Host, text: &str, patch: &str) -> (u64, String) {
        host.input = patch.as_bytes().to_vec();
        let given = words(text);
        let outcome = run(host, &asked(&given).unwrap());
        (outcome, core::mem::take(&mut host.said))
    }

    /// What diff writes for `a` and `b` in `form`, named `names`.
    fn diffed(form: Form, a: &str, b: &str, names: (&str, &str)) -> String {
        let (mut a, mut b) = (Text::new(a.as_bytes()), Text::new(b.as_bytes()));
        if !form.says_unended() {
            (a.ended, b.ended) = (true, true);
        }
        let (left, right) = keys(&a, &b, false);
        let mut out = Vec::new();
        write(form, (&a, &b), &diff::hunks(&left, &right), names, &mut out);
        String::from_utf8(out).unwrap()
    }

    const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    const NEW: &str = "1\n2\n3x\n4\n5\n6\n8\n9\n10\nn\n";

    #[test]
    fn a_patch_goes_into_the_file_its_names_give() {
        let mut host = Host::new("names", &[("f", TEN), ("d/g", TEN)]);
        let patch = diffed(Form::Unified(3), TEN, NEW, ("a/f", "b/f"));
        assert_eq!(ran(&mut host, "-p1", &patch), (DONE, String::new()));
        assert_eq!(host.holds("f").as_deref(), Some(NEW));
        let patch = diffed(Form::Context(3), TEN, NEW, ("x/y/g", "x/y/g.new"));
        let mut deep = Host::new("names-deep", &[("g", TEN)]);
        assert_eq!(
            ran(&mut deep, "", &patch),
            (DONE, String::new()),
            "the last part of a name by default"
        );
        assert_eq!(deep.holds("g").as_deref(), Some(NEW));
        let mut named = Host::new("names-given", &[("h", TEN)]);
        assert_eq!(
            ran(&mut named, "h", &patch).0,
            DONE,
            "the file named over those the patch gives"
        );
        assert_eq!(named.holds("h").as_deref(), Some(NEW));
        let mut indexed = Host::new("names-index", &[("i", TEN)]);
        let normal = String::from("Index: i\n") + &diffed(Form::Normal, TEN, NEW, ("", ""));
        assert_eq!(ran(&mut indexed, "", &normal).0, DONE);
        assert_eq!(indexed.holds("i").as_deref(), Some(NEW));
    }

    #[test]
    fn a_hunk_that_moved_or_lost_context_is_said() {
        let moved = String::from("p\nq\n") + TEN;
        let mut host = Host::new("moved", &[("f", &moved)]);
        let patch = diffed(Form::Unified(3), TEN, NEW, ("f", "f"));
        assert_eq!(
            ran(&mut host, "", &patch),
            (
                DONE,
                "patch: f: hunk 1 went in at line 3, 2 lines later than it said\n".into()
            )
        );
        assert_eq!(host.holds("f"), Some(String::from("p\nq\n") + NEW));
        let mut fuzzy = Host::new("fuzzy", &[("f", &TEN.replacen("2\n", "two\n", 1))]);
        assert_eq!(
            ran(&mut fuzzy, "", &patch),
            (DONE, "patch: f: hunk 1 went in at line 1, leaving out up to 2 lines of its context at each end\n".into())
        );
        assert_eq!(fuzzy.holds("f"), Some(NEW.replacen("2\n", "two\n", 1)));
    }

    #[test]
    fn a_hunk_that_finds_no_place_is_kept_in_rejects() {
        let other = "20\n21\n22\n";
        let mut host = Host::new("rejects", &[("f", other), ("g", other)]);
        let patch = diffed(Form::Unified(1), TEN, NEW, ("f", "f"));
        let (outcome, said) = ran(&mut host, "", &patch);
        assert_eq!(outcome, REJECTED);
        assert_eq!(
            said,
            "patch: f: hunk 1 found no place\npatch: f: hunk 2 found no place\n\
             patch: f: hunk 3 found no place\npatch: f: 3 of 3 hunks found no place, kept in f.rej\n"
        );
        assert_eq!(
            host.holds("f").as_deref(),
            Some(other),
            "the file as it was"
        );
        let rejects = host.holds("f.rej").unwrap();
        assert_eq!(rejects, patch, "a unified patch's rejects are unified");
        let normal = diffed(Form::Normal, TEN, NEW, ("", ""));
        let twice = format!("Index: g\n{normal}Index: g\n{normal}");
        std::fs::write(host.root.join("all.rej"), "stale\n").unwrap();
        assert_eq!(ran(&mut host, "-r all.rej g", &twice).0, REJECTED);
        let all = host.holds("all.rej").unwrap();
        assert!(
            all.starts_with("*** g\n--- g\n***************\n*** 3 ****\n! 3\n--- 3 ----\n! 3x\n"),
            "{all}"
        );
        assert_eq!(
            all.matches("*** g\n").count(),
            2,
            "-r gathers every patch's rejects"
        );
        assert!(!host.exists("g.rej"));
    }

    #[test]
    fn a_patch_goes_in_the_other_way_round_when_asked() {
        let mut host = Host::new("reverse", &[("f", NEW)]);
        let patch = diffed(Form::Context(2), TEN, NEW, ("f", "f"));
        assert_eq!(ran(&mut host, "-R", &patch), (DONE, String::new()));
        assert_eq!(host.holds("f").as_deref(), Some(TEN));
        let ed = diffed(Form::Ed, TEN, NEW, ("", ""));
        assert_eq!(
            ran(&mut host, "-R f", &ed),
            (
                TROUBLE,
                "patch: an ed script cannot be put in reversed\n".into()
            )
        );
    }

    #[test]
    fn a_patch_put_in_already_is_asked_about_or_left_alone() {
        let patch = diffed(Form::Unified(3), TEN, NEW, ("f", "f"));
        let mut host = Host::new("applied", &[("f", NEW)]);
        assert_eq!(
            ran(&mut host, "-N", &patch),
            (
                DONE,
                "patch: f: the patch seems put in already, so it is left alone\n".into()
            )
        );
        assert_eq!(host.holds("f").as_deref(), Some(NEW));
        assert_eq!(host.asked, Vec::<String>::new());
        host.answers.push("yes".into());
        assert_eq!(ran(&mut host, "", &patch), (DONE, String::new()));
        assert_eq!(
            host.holds("f").as_deref(),
            Some(TEN),
            "taken the other way round"
        );
        assert_eq!(
            host.asked,
            [
                "patch: f: the patch seems made the other way round, or put in already; put it in reversed? [n] "
            ]
        );
        std::fs::write(host.root.join("f"), NEW).unwrap();
        host.answers.push("n".into());
        assert_eq!(ran(&mut host, "", &patch).0, REJECTED, "not taken");
        assert_eq!(host.holds("f").as_deref(), Some(NEW));
        assert_eq!(
            ran(&mut host, "", &patch).0,
            REJECTED,
            "no one to answer is a no"
        );
        let mut fresh = Host::new("not-applied", &[("f", TEN)]);
        assert_eq!(ran(&mut fresh, "-N", &patch), (DONE, String::new()));
        assert_eq!(
            fresh.asked,
            Vec::<String>::new(),
            "a patch that fits asks nothing"
        );
    }

    #[test]
    fn the_file_as_it_was_is_kept_with_b() {
        let patch = diffed(Form::Unified(3), TEN, NEW, ("f", "f"));
        let mut host = Host::new("backup", &[("f", TEN)]);
        assert_eq!(ran(&mut host, "-b", &patch).0, DONE);
        assert_eq!(
            (host.holds("f.orig").as_deref(), host.holds("f").as_deref()),
            (Some(TEN), Some(NEW))
        );
        let mut out = Host::new("backup-out", &[("f", TEN), ("o", "old\n")]);
        assert_eq!(ran(&mut out, "-b -o o", &patch).0, DONE);
        assert_eq!(
            out.holds("o.orig").as_deref(),
            Some("old\n"),
            "with -o the output file as it was"
        );
        assert_eq!(
            (out.holds("f").as_deref(), out.holds("o").as_deref()),
            (Some(TEN), Some(NEW))
        );
        assert!(!out.exists("f.orig"));
        let mut fresh = Host::new("backup-new", &[("f", TEN)]);
        assert_eq!(ran(&mut fresh, "-b -o o", &patch).0, DONE);
        assert!(!fresh.exists("o.orig"), "no output file was there to keep");
    }

    #[test]
    fn with_o_every_patch_s_result_follows_the_last() {
        let one = diffed(Form::Unified(1), "a\nb\n", "a\nc\n", ("f", "f"));
        let two = diffed(Form::Unified(1), "a\nb\n", "z\nb\n", ("f", "f"));
        let mut host = Host::new("output", &[("f", "a\nb\n"), ("o", "was\n")]);
        assert_eq!(ran(&mut host, "-o o", &(one + &two)).0, DONE);
        assert_eq!(host.holds("o").as_deref(), Some("a\nc\nz\nb\n"));
        assert_eq!(
            host.holds("f").as_deref(),
            Some("a\nb\n"),
            "the file stays as it was"
        );
    }

    #[test]
    fn with_d_both_versions_stay_for_the_preprocessor() {
        let patch = diffed(Form::Unified(3), TEN, NEW, ("f", "f"));
        let mut host = Host::new("define", &[("f", TEN)]);
        assert_eq!(ran(&mut host, "-D FOO", &patch).0, DONE);
        // As GNU's patch -D FOO writes it.
        assert_eq!(
            host.holds("f").as_deref(),
            Some(
                "1\n2\n#ifndef FOO\n3\n#else\n3x\n#endif\n4\n5\n6\n#ifndef FOO\n7\n#endif\n8\n9\n10\n#ifdef FOO\nn\n#endif\n"
            )
        );
    }

    #[test]
    fn an_ed_script_goes_into_the_file_named() {
        let ed = diffed(Form::Ed, TEN, NEW, ("", ""));
        let mut host = Host::new("ed", &[("f", TEN), ("short", "1\n")]);
        assert_eq!(ran(&mut host, "f", &ed), (DONE, String::new()));
        assert_eq!(host.holds("f").as_deref(), Some(NEW));
        assert_eq!(
            ran(&mut host, "short", &ed),
            (
                TROUBLE,
                "patch: short: the script's command 1 names lines the file does not have\n".into()
            )
        );
        assert_eq!(host.holds("short").as_deref(), Some("1\n"));
        assert_eq!(
            ran(&mut host, "-n f", &ed),
            (TROUBLE, "patch: no patch in the input\n".into()),
            "not as -n"
        );
    }

    #[test]
    fn a_patch_that_makes_a_file_makes_it() {
        let patch = diffed(Form::Unified(3), "", "x\ny\n", ("old/m", "new/m"));
        let mut host = Host::new("make", &[]);
        assert_eq!(ran(&mut host, "", &patch), (DONE, String::new()));
        assert_eq!(host.holds("m").as_deref(), Some("x\ny\n"));
        let mut named = Host::new("make-named", &[]);
        assert_eq!(ran(&mut named, "made", &patch).0, DONE);
        assert_eq!(named.holds("made").as_deref(), Some("x\ny\n"));
        let gnu = "--- /dev/null\n+++ b/n\n@@ -0,0 +1 @@\n+z\n";
        let mut null = Host::new("make-null", &[]);
        assert_eq!(ran(&mut null, "-p1", gnu).0, DONE);
        assert_eq!(null.holds("n").as_deref(), Some("z\n"));
    }

    #[test]
    fn without_a_file_the_one_at_the_console_is_asked() {
        let normal = diffed(Form::Normal, TEN, NEW, ("", ""));
        let mut host = Host::new("ask", &[("t", TEN)]);
        host.answers.push(" t ".into());
        assert_eq!(ran(&mut host, "", &normal), (DONE, String::new()));
        assert_eq!(host.asked, ["patch: which file is the patch for? "]);
        assert_eq!(host.holds("t").as_deref(), Some(NEW));
        assert_eq!(
            ran(&mut host, "", &normal),
            (
                REJECTED,
                "patch: no file for a patch, so it is left out\n".into()
            )
        );
        host.answers.push(String::new());
        assert_eq!(
            ran(&mut host, "", &normal).0,
            REJECTED,
            "an empty answer names none"
        );
    }

    #[test]
    fn input_that_is_no_patch_or_a_file_not_there_is_trouble() {
        let mut host = Host::new("trouble", &[("p.diff", "@@ -1 +1 @@\n-x\n")]);
        assert_eq!(
            ran(&mut host, "", "just words\n"),
            (TROUBLE, "patch: no patch in the input\n".into())
        );
        assert_eq!(
            ran(&mut host, "-i p.diff", ""),
            (
                TROUBLE,
                "patch: p.diff: line 3: the patch ends inside a hunk\n".into()
            )
        );
        assert_eq!(
            ran(&mut host, "-i nope", ""),
            (TROUBLE, "patch: nope: no such file\n".into())
        );
        let patch = diffed(Form::Unified(3), TEN, NEW, ("f", "f"));
        assert_eq!(
            ran(&mut host, "gone", &patch),
            (TROUBLE, "patch: gone: no such file\n".into())
        );
        assert_eq!(
            ran(&mut host, "-i - -p0 f", "x\n").0,
            TROUBLE,
            "a - is the standard input"
        );
    }

    /// A patch of `kind` with the names `old`, `new` and `index`, and one
    /// hunk that puts in `put` lines after `old_lines` old ones.
    fn shaped(names: [Option<&[u8]>; 3], old_lines: usize, put: usize) -> Patch<'_> {
        let line = read::Line {
            text: b"x",
            unended: false,
        };
        let lines = core::iter::repeat_n((read::Mark::Gone, line), old_lines)
            .chain(core::iter::repeat_n((read::Mark::Put, line), put))
            .collect();
        Patch {
            kind: Kind::Unified,
            old_name: names[0],
            new_name: names[1],
            index: names[2],
            hunks: Hunks::Edits(vec![Edit {
                old: 0,
                new: 0,
                lines,
            }]),
        }
    }

    #[test]
    fn the_file_is_the_first_name_that_is_there() {
        let mut host = Host::new("target", &[("a", ""), ("b", ""), ("c", "")]);
        let plain = Asked::default();
        let names =
            |a: &'static [u8], b: &'static [u8], c: &'static [u8]| [Some(a), Some(b), Some(c)];
        assert_eq!(
            target(&mut host, &plain, &shaped(names(b"a", b"b", b"c"), 1, 1)),
            Some(("a".into(), false))
        );
        assert_eq!(
            target(&mut host, &plain, &shaped(names(b"x", b"b", b"c"), 1, 1)),
            Some(("b".into(), false))
        );
        assert_eq!(
            target(&mut host, &plain, &shaped(names(b"x", b"y", b"c"), 1, 1)),
            Some(("c".into(), false))
        );
        assert_eq!(
            target(
                &mut host,
                &plain,
                &shaped(names(b"/dev/null", b"q/b", b"c"), 0, 1)
            ),
            Some(("b".into(), false)),
            "/dev/null stands for no file"
        );
        assert_eq!(
            target(&mut host, &plain, &shaped(names(b"x", b"new", b"y"), 0, 2)),
            Some(("new".into(), true)),
            "a patch that only puts lines in makes its new file"
        );
        let strip = Asked {
            strip: Some(3),
            ..Asked::default()
        };
        assert_eq!(
            target(
                &mut host,
                &strip,
                &shaped(names(b"p/q/a", b"b", b"c"), 1, 1)
            ),
            None,
            "names with too few parts name nothing, and no one answers"
        );
        assert_eq!(host.asked, ["patch: which file is the patch for? "]);
        assert_eq!(
            target(&mut host, &plain, &shaped([None, None, None], 1, 1)),
            None
        );
        let given = Asked {
            file: Some("z"),
            ..Asked::default()
        };
        assert_eq!(
            target(&mut host, &given, &shaped([None, None, None], 0, 1)),
            Some(("z".into(), true))
        );
        assert_eq!(
            target(&mut host, &given, &shaped([None, None, None], 1, 1)),
            Some(("z".into(), false))
        );
    }

    #[test]
    fn a_patch_makes_a_file_when_it_only_puts_lines_into_none() {
        assert!(only_adds(&shaped([None, None, None], 0, 2)));
        assert!(!only_adds(&shaped([None, None, None], 1, 2)));
        let mut later = shaped([None, None, None], 0, 1);
        if let Hunks::Edits(edits) = &mut later.hunks {
            edits[0].old = 3;
        }
        assert!(
            !only_adds(&later),
            "lines put in after some are none put into an empty file"
        );
        let ed = Patch {
            hunks: Hunks::Ed(Vec::new()),
            ..shaped([None, None, None], 0, 1)
        };
        assert!(!only_adds(&ed));
        assert_eq!(lines(1), "1 line");
        assert_eq!(lines(0), "0 lines");
        assert_eq!(lines(7), "7 lines");
    }

    #[test]
    fn hunks_that_fit_go_in_and_the_rest_are_kept() {
        let patch = diffed(Form::Unified(1), TEN, NEW, ("f", "f"));
        let changed = TEN.replacen("7\n", "seven\n", 1);
        let mut host = Host::new("some", &[("f", &changed)]);
        assert_eq!(
            ran(&mut host, "", &patch),
            (REJECTED, "patch: f: hunk 2 found no place\npatch: f: 1 of 3 hunks found no place, kept in f.rej\n".into())
        );
        assert_eq!(host.holds("f"), Some(NEW.replacen("6\n", "6\nseven\n", 1)));
        assert_eq!(
            host.holds("f.rej").unwrap(),
            "--- f\n+++ f\n@@ -6,3 +6,2 @@\n 6\n-7\n 8\n"
        );
    }

    #[test]
    fn with_l_blanks_match_loosely() {
        let patch = diffed(
            Form::Unified(3),
            "a  b\nc d\ne\n",
            "a  b\nc d\nE\n",
            ("f", "f"),
        );
        let mut host = Host::new("loose", &[("f", "a b\nc  d\ne\n")]);
        assert_eq!(ran(&mut host, "-l", &patch), (DONE, String::new()));
        assert_eq!(host.holds("f").as_deref(), Some("a b\nc  d\nE\n"));
    }

    #[test]
    fn a_file_typed_is_made_only_when_it_is_not_there() {
        let adding = "0a1\n> new\n";
        let mut host = Host::new("typed-there", &[("t", "old\n")]);
        host.answers.push("t".into());
        assert_eq!(ran(&mut host, "", adding).0, DONE);
        assert_eq!(
            host.holds("t").as_deref(),
            Some("new\nold\n"),
            "what it held stays"
        );
        host.answers.push("u".into());
        assert_eq!(ran(&mut host, "", adding).0, DONE);
        assert_eq!(host.holds("u").as_deref(), Some("new\n"), "made");
        host.answers.push("v".into());
        assert_eq!(
            ran(&mut host, "", "1c1\n< a\n---\n> b\n"),
            (TROUBLE, "patch: v: no such file\n".into()),
            "a patch with lines to find makes no file"
        );
    }

    #[test]
    fn files_kept_and_rejects_hold_only_this_patch_s() {
        let patch = diffed(Form::Unified(1), TEN, NEW, ("f", "f"));
        let mut host = Host::new(
            "fresh",
            &[
                ("f", TEN),
                ("f.orig", "older\n"),
                ("g", "x\n"),
                ("g.rej", "stale\n"),
            ],
        );
        assert_eq!(ran(&mut host, "-b", &patch).0, DONE);
        assert_eq!(
            host.holds("f.orig").as_deref(),
            Some(TEN),
            "the copy made anew"
        );
        assert_eq!(ran(&mut host, "g", &patch).0, REJECTED);
        assert_eq!(
            host.holds("g.rej").as_deref(),
            Some(patch.as_str()),
            "rejects made anew"
        );
    }

    #[test]
    fn every_form_diff_writes_patch_puts_back() {
        let mut seed = 0x1d8e_4e27_c47d_124f_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let words = ["a", "b", "", ".", "  x"];
        let mut host = Host::new("round", &[]);
        for round in 0..200 {
            let text = |next: &mut dyn FnMut() -> u64, most: u64| -> String {
                (0..next() % most)
                    .map(|_| String::from(words[usize::try_from(next() % 5).unwrap()]) + "\n")
                    .collect()
            };
            let a = text(&mut next, 15);
            let b = text(&mut next, 15);
            for form in [Form::Normal, Form::Context(2), Form::Unified(1), Form::Ed] {
                std::fs::write(host.root.join("f"), &a).unwrap();
                let patch = diffed(form, &a, &b, ("f", "f"));
                if patch.is_empty() {
                    continue;
                }
                let (outcome, said) = ran(&mut host, "f", &patch);
                assert_eq!((outcome, said.as_str()), (DONE, ""), "{form:?} {round}");
                assert_eq!(host.holds("f").unwrap(), b, "{form:?} of {a:?}:\n{patch}");
            }
        }
    }
}
