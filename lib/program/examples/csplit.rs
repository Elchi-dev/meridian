//! csplit: cuts a file into files at the lines its operands name, as
//! POSIX's csplit does: before the line a basic regular expression
//! matches, moved by an offset, with `/re/`; leaving out what comes
//! before such a line with `%re%`; before the line of a number; and each
//! again as often as a `{n}` after it says. The last file holds what is
//! left. The files are named `xx00`, `xx01` and on, or after the prefix
//! `-f` gives with as many digits as `-n` says, and the size of each is
//! written unless `-s` is given. When an operand finds no line, what is
//! left goes into one more file as GNU's csplit has it, the files are
//! removed unless `-k` keeps them, and csplit ends with 1.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use program::abi::file::{CREATE_ALWAYS, WRITE};
use program::{Error, Handle, text};
use regex::{Regex, Syntax};

program::main!(csplit);

/// Where an operand cuts.
#[derive(Debug)]
enum Cut {
    /// Before the line `regex` matches, moved by `offset` lines; with
    /// `skip`, what comes before it goes into no file.
    Before {
        regex: Regex,
        offset: isize,
        skip: bool,
    },
    /// Before line `n`, counted from 1.
    Line(usize),
}

/// An operand: where it cuts, how many times more, and how it was
/// written, for what csplit says of it.
#[derive(Debug)]
struct Operand<'w> {
    cut: Cut,
    again: usize,
    written: &'w str,
}

/// What csplit's words ask for.
#[derive(Debug)]
struct Asked<'w> {
    prefix: &'w str,
    keep: bool,
    digits: usize,
    silent: bool,
    file: &'w str,
    operands: Vec<Operand<'w>>,
}

/// The expression between the two `delimiter`s that open `written` and
/// end it but for an offset, a backslash before the delimiter making it
/// stand for itself, and the offset after it.
fn delimited(written: &str, delimiter: char) -> Option<(String, &str)> {
    let mut expression = String::new();
    let mut chars = written[1..].char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some((_, d)) if d == delimiter => expression.push(d),
                Some((_, other)) => {
                    expression.push('\\');
                    expression.push(other);
                }
                None => expression.push('\\'),
            },
            _ if c == delimiter => return Some((expression, &written[1 + at + 1..])),
            _ => expression.push(c),
        }
    }
    None
}

/// An offset as an operand writes one after its expression: none, or a
/// number of lines with a sign or none.
fn offset(written: &str) -> Option<isize> {
    if written.is_empty() {
        return Some(0);
    }
    let digits = written.strip_prefix(['+', '-']).unwrap_or(written);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let lines: isize = digits.parse().ok()?;
    Some(if written.starts_with('-') {
        -lines
    } else {
        lines
    })
}

/// The operand `written` stands for, or why it stands for none.
fn operand(written: &str) -> Result<Operand<'_>, String> {
    let wrong = || format!("csplit: {written} is no operand\n");
    let cut = match written.chars().next() {
        Some(delimiter @ ('/' | '%')) => {
            let (expression, after) = delimited(written, delimiter).ok_or_else(wrong)?;
            let offset = offset(after).ok_or_else(wrong)?;
            let regex = Regex::new(&expression, Syntax::Basic, false)
                .map_err(|e| format!("csplit: {written}: {}\n", e.code.says()))?;
            Cut::Before {
                regex,
                offset,
                skip: delimiter == '%',
            }
        }
        Some('0'..='9') => {
            let line: usize = written.parse().map_err(|_| wrong())?;
            if line == 0 {
                return Err(format!("csplit: {written}: lines are counted from 1\n"));
            }
            Cut::Line(line)
        }
        _ => return Err(wrong()),
    };
    Ok(Operand {
        cut,
        again: 0,
        written,
    })
}

/// What an option that takes a value wants.
fn wants(letter: char) -> &'static str {
    if letter == 'f' {
        "a prefix"
    } else {
        "a number"
    }
}

/// What csplit's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let (mut prefix, mut keep, mut digits, mut silent) = ("xx", false, 2, false);
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
                'k' => keep = true,
                's' => silent = true,
                'f' | 'n' => {
                    let attached = &letters[at + 1..];
                    let value = if attached.is_empty() {
                        let (value, rest) = words.split_first().ok_or_else(|| {
                            format!("csplit: -{letter} wants {}\n", wants(letter))
                        })?;
                        words = rest;
                        value.as_str()
                    } else {
                        attached
                    };
                    if letter == 'f' {
                        prefix = value;
                    } else {
                        digits = value.parse().ok().filter(|&n| n > 0).ok_or_else(|| {
                            format!("csplit: -n wants a number from 1, not {value}\n")
                        })?;
                    }
                    break;
                }
                _ => return Err(format!("csplit: -{letter} is no option\n")),
            }
        }
    }
    let Some((file, rest)) = words.split_first() else {
        return Err(String::from(
            "csplit: cuts a file at the lines it is told\n",
        ));
    };
    if rest.is_empty() {
        return Err(String::from(
            "csplit: cuts a file at the lines it is told\n",
        ));
    }
    let mut operands: Vec<Operand> = Vec::new();
    for word in rest {
        if let Some(count) = word.strip_prefix('{').and_then(|w| w.strip_suffix('}')) {
            let again = count
                .parse()
                .map_err(|_| format!("csplit: {word} is no count\n"))?;
            let last = operands
                .last_mut()
                .ok_or_else(|| format!("csplit: {word} repeats no operand\n"))?;
            last.again = again;
            continue;
        }
        operands.push(operand(word)?);
    }
    Ok(Asked {
        prefix,
        keep,
        digits,
        silent,
        file,
        operands,
    })
}

/// The lines of `bytes`, each with its line feed.
fn lines_of(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|&b| b == b'\n').collect()
}

/// The runs of lines of `lines` that go into files, in order, cut where
/// `operands` say; and what went wrong, after which the lines left went
/// into one more file.
fn sections(lines: &[&[u8]], operands: &[Operand]) -> (Vec<Range<usize>>, Option<String>) {
    let mut found = Vec::new();
    let mut current = 0;
    let mut searched = false;
    for operand in operands {
        for time in 0..=operand.again {
            let end = match &operand.cut {
                Cut::Line(n) => {
                    // A number again cuts as many lines further each time.
                    let line = n.saturating_mul(time + 1);
                    let Some(end) = line
                        .checked_sub(1)
                        .filter(|&end| end >= current && end < lines.len())
                    else {
                        found.push(current..lines.len());
                        let why = format!("csplit: {}: no such line\n", operand.written);
                        return (found, Some(why));
                    };
                    end
                }
                Cut::Before {
                    regex,
                    offset,
                    skip,
                } => {
                    // The first search may match the first line itself.
                    let from = if searched || current > 0 {
                        current + 1
                    } else {
                        current
                    };
                    searched = true;
                    let matched = (from..lines.len()).find(|&at| {
                        let line = lines[at].strip_suffix(b"\n").unwrap_or(lines[at]);
                        regex
                            .find(&regex::Text::new(line), 0)
                            .is_ok_and(|m| m.is_some())
                    });
                    let end = matched.and_then(|at| at.checked_add_signed(*offset));
                    match end.filter(|&end| end >= current && end <= lines.len()) {
                        Some(end) if *skip => {
                            current = end;
                            continue;
                        }
                        Some(end) => end,
                        None => {
                            found.push(current..lines.len());
                            let why = if matched.is_some() {
                                "its offset goes out of the lines left"
                            } else {
                                "no line matches it"
                            };
                            return (found, Some(format!("csplit: {}: {why}\n", operand.written)));
                        }
                    }
                }
            };
            found.push(current..end);
            current = end;
        }
    }
    found.push(current..lines.len());
    (found, None)
}

/// The name of file `number`, counted from 0, after `prefix` with
/// `digits` digits; none when the number takes more.
fn named(prefix: &str, number: usize, digits: usize) -> Option<String> {
    let suffix = format!("{number:0digits$}");
    (suffix.len() <= digits).then(|| format!("{prefix}{suffix}"))
}

/// What csplit reads, writes and says.
trait Files {
    /// What the file at `path`, or the standard input for none, holds.
    fn read(&mut self, path: Option<&str>) -> Result<Vec<u8>, Error>;

    /// Makes the file at `path` hold `bytes`.
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error>;

    /// Removes the file at `path`.
    fn remove(&mut self, path: &str);

    /// Writes `bytes` on the standard output.
    fn out(&mut self, bytes: &[u8]);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// Cuts the file `asked` names, and answers the status csplit ends with.
fn split(files: &mut impl Files, asked: &Asked) -> u64 {
    let path = Some(asked.file).filter(|&f| f != "-");
    let bytes = match files.read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            files.say(&text::trouble("csplit", asked.file, e));
            return 1;
        }
    };
    let lines = lines_of(&bytes);
    let (found, mut wrong) = sections(&lines, &asked.operands);
    let mut made = Vec::new();
    for (number, section) in found.into_iter().enumerate() {
        let Some(name) = named(asked.prefix, number, asked.digits) else {
            // What went wrong first is what csplit says.
            wrong = wrong.or(Some(format!(
                "csplit: more files than {} digits can name\n",
                asked.digits
            )));
            break;
        };
        let bytes = lines[section].concat();
        if let Err(e) = files.write(&name, &bytes) {
            wrong = Some(text::trouble("csplit", &name, e));
            break;
        }
        made.push(name);
        if !asked.silent {
            files.out(format!("{}\n", bytes.len()).as_bytes());
        }
    }
    let Some(why) = wrong else {
        return 0;
    };
    files.say(&why);
    if !asked.keep {
        for name in &made {
            files.remove(name);
        }
    }
    1
}

/// Meridian's files, and the program's own output.
struct System {
    out: text::Out<text::Write>,
}

impl Files for System {
    fn read(&mut self, path: Option<&str>) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        text::pieces(path, |piece| {
            bytes.extend_from_slice(piece);
            true
        })?;
        Ok(bytes)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let file = program::open_with(path, WRITE, CREATE_ALWAYS)?;
        let written = program::write_file(file, 0, bytes);
        let _ = program::close(file);
        written
    }

    fn remove(&mut self, path: &str) {
        let _ = program::remove(path);
    }

    fn out(&mut self, bytes: &[u8]) {
        self.out.put(bytes);
    }

    fn say(&mut self, text: &str) {
        self.out.flush();
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn csplit(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut system = System {
        out: text::standard_output(),
    };
    let status = split(&mut system, &asked);
    if system.out.finish() { status } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write as _;

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

    const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

    /// The sizes of the files csplit with the operands `text` cuts `TEN`
    /// into, and what went wrong.
    fn sizes(text: &str) -> (Vec<usize>, Option<String>) {
        let given = words(&format!("f {text}"));
        let asked = asked(&given).unwrap();
        let lines = lines_of(TEN.as_bytes());
        let (found, wrong) = sections(&lines, &asked.operands);
        let sizes = found
            .into_iter()
            .map(|run| lines[run].concat().len())
            .collect();
        (sizes, wrong)
    }

    #[test]
    fn files_are_cut_where_gnu_s_csplit_cuts_them() {
        assert_eq!(
            sizes("3 {2}"),
            (Vec::from([4, 6, 6, 5]), None),
            "every third line"
        );
        assert_eq!(
            sizes("1"),
            (Vec::from([0, 21]), None),
            "nothing before the first line"
        );
        assert_eq!(sizes("10"), (Vec::from([18, 3]), None));
        assert_eq!(
            sizes("/5/ %8%"),
            (Vec::from([8, 7]), None),
            "what % passes goes nowhere"
        );
        assert_eq!(
            sizes("/1/"),
            (Vec::from([0, 21]), None),
            "the first search may match the first line"
        );
        assert_eq!(
            sizes("/1/ {1}"),
            (Vec::from([0, 18, 3]), None),
            "again it starts after it"
        );
        assert_eq!(sizes("/5/+2"), (Vec::from([12, 9]), None));
        assert_eq!(sizes("/5/-2"), (Vec::from([4, 17]), None));
        assert_eq!(sizes("3 /7/"), (Vec::from([4, 8, 9]), None));
        assert_eq!(sizes("%3% 6"), (Vec::from([6, 11]), None));
        assert_eq!(
            sizes("/10/+1"),
            (Vec::from([21, 0]), None),
            "an offset may reach the end"
        );
        assert_eq!(sizes("%10%+1"), (Vec::from([0]), None));
        let lines = lines_of(b"a\nB\nb\n");
        let given = words("f /b/");
        let taken = asked(&given).unwrap();
        assert_eq!(
            sections(&lines, &taken.operands),
            (Vec::from([0..2, 2..3]), None),
            "letters match as they are written"
        );
    }

    #[test]
    fn an_operand_that_finds_no_line_leaves_the_rest_in_one_file() {
        let none = Some(String::from("csplit: /x/: no line matches it\n"));
        assert_eq!(sizes("2 /x/"), (Vec::from([2, 19]), none));
        assert_eq!(
            sizes("/5/+2 /7/-1"),
            (
                Vec::from([12, 9]),
                Some("csplit: /7/-1: no line matches it\n".into())
            ),
            "a search starts after the line it is on"
        );
        assert_eq!(
            sizes("11"),
            (Vec::from([21]), Some("csplit: 11: no such line\n".into()))
        );
        assert_eq!(
            sizes("5 3"),
            (Vec::from([8, 13]), Some("csplit: 3: no such line\n".into())),
            "no going back"
        );
        assert_eq!(
            sizes("/9/+5"),
            (
                Vec::from([21]),
                Some("csplit: /9/+5: its offset goes out of the lines left\n".into())
            )
        );
        assert_eq!(
            sizes("4 /5/-3"),
            (
                Vec::from([6, 15]),
                Some("csplit: /5/-3: its offset goes out of the lines left\n".into())
            )
        );
        assert_eq!(
            sizes("4 {3}"),
            (
                Vec::from([6, 8, 7]),
                Some("csplit: 4: no such line\n".into())
            )
        );
    }

    #[test]
    fn its_words_are_options_a_file_and_operands() {
        let given = words("-ks -f out -n 3 - /a/ {2} %b%-1 7");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (
                taken.prefix,
                taken.keep,
                taken.digits,
                taken.silent,
                taken.file
            ),
            ("out", true, 3, true, "-")
        );
        let shape: Vec<(&str, usize)> = taken
            .operands
            .iter()
            .map(|o| (o.written, o.again))
            .collect();
        assert_eq!(shape, [("/a/", 2), ("%b%-1", 0), ("7", 0)]);
        assert!(matches!(
            taken.operands[1].cut,
            Cut::Before {
                offset: -1,
                skip: true,
                ..
            }
        ));
        assert!(matches!(
            taken.operands[0].cut,
            Cut::Before {
                offset: 0,
                skip: false,
                ..
            }
        ));
        assert!(matches!(taken.operands[2].cut, Cut::Line(7)));
        let given = words("f 1");
        let plain = asked(&given).unwrap();
        assert_eq!(
            (plain.prefix, plain.keep, plain.digits, plain.silent),
            ("xx", false, 2, false)
        );
        let given = words("-fp -n4 f 1");
        let attached = asked(&given).unwrap();
        assert_eq!((attached.prefix, attached.digits), ("p", 4));
    }

    #[test]
    fn words_it_cannot_take_are_refused() {
        let refused = |text: &str| asked(&words(text)).err();
        let cuts = Some(String::from(
            "csplit: cuts a file at the lines it is told\n",
        ));
        assert_eq!(refused(""), cuts);
        assert_eq!(refused("f"), cuts);
        assert_eq!(refused("-x f 1"), Some("csplit: -x is no option\n".into()));
        assert_eq!(refused("-f"), Some("csplit: -f wants a prefix\n".into()));
        assert_eq!(refused("-n"), Some("csplit: -n wants a number\n".into()));
        assert_eq!(
            refused("-n 0 f 1"),
            Some("csplit: -n wants a number from 1, not 0\n".into())
        );
        assert_eq!(
            refused("-n x f 1"),
            Some("csplit: -n wants a number from 1, not x\n".into())
        );
        assert_eq!(refused("f x"), Some("csplit: x is no operand\n".into()));
        assert_eq!(refused("f /a"), Some("csplit: /a is no operand\n".into()));
        assert_eq!(
            refused("f /a/x"),
            Some("csplit: /a/x is no operand\n".into())
        );
        assert_eq!(
            refused("f /a/+"),
            Some("csplit: /a/+ is no operand\n".into())
        );
        assert_eq!(
            refused("f 0"),
            Some("csplit: 0: lines are counted from 1\n".into())
        );
        assert_eq!(refused("f 1x"), Some("csplit: 1x is no operand\n".into()));
        assert_eq!(
            refused("f {2}"),
            Some("csplit: {2} repeats no operand\n".into())
        );
        assert_eq!(refused("f 1 {x}"), Some("csplit: {x} is no count\n".into()));
        let bad = refused("f /\\(/").unwrap();
        assert!(bad.starts_with("csplit: /\\(/: "), "{bad}");
    }

    #[test]
    fn expressions_and_offsets_are_read_as_written() {
        assert_eq!(
            delimited("/a\\/b/+3", '/'),
            Some((String::from("a/b"), "+3"))
        );
        assert_eq!(delimited("%a\\%b%", '%'), Some((String::from("a%b"), "")));
        assert_eq!(
            delimited("/a\\.b/", '/'),
            Some((String::from("a\\.b"), "")),
            "other escapes stay"
        );
        assert_eq!(delimited("/a\\", '/'), None);
        assert_eq!(delimited("/ab", '/'), None);
        assert_eq!(offset(""), Some(0));
        assert_eq!(offset("+3"), Some(3));
        assert_eq!(offset("3"), Some(3));
        assert_eq!(offset("-3"), Some(-3));
        assert_eq!(offset("-"), None);
        assert_eq!(offset("3x"), None);
        assert_eq!(named("xx", 0, 2).as_deref(), Some("xx00"));
        assert_eq!(named("xx", 7, 3).as_deref(), Some("xx007"));
        assert_eq!(named("p", 99, 2).as_deref(), Some("p99"));
        assert_eq!(named("p", 100, 2), None);
        assert_eq!(lines_of(b"a\nb"), [&b"a\n"[..], b"b"]);
        assert_eq!(lines_of(b""), Vec::<&[u8]>::new());
    }

    /// The files csplit makes among the host's temporary files, gone once
    /// dropped, its standard input, and what it wrote and said.
    struct Host {
        root: std::path::PathBuf,
        input: Vec<u8>,
        written: String,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-csplit-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                input: Vec::new(),
                written: String::new(),
                said: String::new(),
            }
        }

        /// The names of the files there, in order.
        fn files(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.root)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }

        fn holds(&self, name: &str) -> String {
            String::from_utf8(std::fs::read(confined(&self.root, name)).unwrap()).unwrap()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Files for Host {
        fn read(&mut self, path: Option<&str>) -> Result<Vec<u8>, Error> {
            match path {
                None => Ok(core::mem::take(&mut self.input)),
                Some(path) => {
                    std::fs::read(confined(&self.root, path)).map_err(|_| Error::NotFound)
                }
            }
        }

        fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
            std::fs::write(confined(&self.root, path), bytes).map_err(|_| Error::Unwritable)
        }

        fn remove(&mut self, path: &str) {
            let _ = std::fs::remove_file(confined(&self.root, path));
        }

        fn out(&mut self, bytes: &[u8]) {
            self.written.push_str(&String::from_utf8_lossy(bytes));
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How csplit ran on `host` with the words `text`: its status, what it
    /// wrote and what it said.
    fn ran(host: &mut Host, text: &str) -> (u64, String, String) {
        let given = words(text);
        let status = split(host, &asked(&given).unwrap());
        (
            status,
            core::mem::take(&mut host.written),
            core::mem::take(&mut host.said),
        )
    }

    #[test]
    fn each_piece_goes_into_a_file_of_its_own_and_its_size_is_said() {
        let mut host = Host::new("pieces");
        host.input = TEN.as_bytes().to_vec();
        assert_eq!(
            ran(&mut host, "- 4 /8/"),
            (0, "6\n8\n7\n".into(), String::new())
        );
        assert_eq!(host.files(), ["xx00", "xx01", "xx02"]);
        assert_eq!(
            (host.holds("xx00"), host.holds("xx01"), host.holds("xx02")),
            (
                "1\n2\n3\n".into(),
                "4\n5\n6\n7\n".into(),
                "8\n9\n10\n".into()
            )
        );
        std::fs::write(host.root.join("in"), TEN).unwrap();
        assert_eq!(
            ran(&mut host, "-s -f part -n 3 in 6"),
            (0, String::new(), String::new()),
            "-s says nothing"
        );
        assert_eq!(host.holds("part000"), "1\n2\n3\n4\n5\n");
        assert_eq!(host.holds("part001"), "6\n7\n8\n9\n10\n");
        assert_eq!(
            ran(&mut host, "nope 1"),
            (1, String::new(), "csplit: nope: no such file\n".into())
        );
    }

    #[test]
    fn the_files_made_are_removed_after_trouble_unless_kept() {
        let mut host = Host::new("trouble");
        std::fs::write(host.root.join("in"), TEN).unwrap();
        assert_eq!(
            ran(&mut host, "-f a in 2 /x/"),
            (
                1,
                "2\n19\n".into(),
                "csplit: /x/: no line matches it\n".into()
            )
        );
        assert_eq!(host.files(), ["in"], "removed");
        assert_eq!(ran(&mut host, "-k -f b in 2 /x/").0, 1);
        assert_eq!(host.files(), ["b00", "b01", "in"], "kept");
        assert_eq!(
            ran(&mut host, "-n 1 -f c in 1 {10}"),
            (
                1,
                "0\n2\n2\n2\n2\n2\n2\n2\n2\n2\n".into(),
                "csplit: 1: no such line\n".into()
            ),
            "the trouble was first"
        );
        let mut many = Host::new("many");
        std::fs::write(many.root.join("in"), TEN).unwrap();
        assert_eq!(
            ran(&mut many, "-n 1 -f d in 1 {9}"),
            (
                1,
                "0\n2\n2\n2\n2\n2\n2\n2\n2\n2\n".into(),
                "csplit: more files than 1 digits can name\n".into()
            )
        );
        assert_eq!(many.files(), ["in"]);
    }

    #[test]
    fn pieces_cut_at_random_lines_give_the_file_back() {
        let mut seed = 0x6a09_e667_f3bc_c908_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..500 {
            let count = 1 + next() % 30;
            let mut text = String::new();
            for n in 0..count {
                let _ = writeln!(text, "w{}", next() % 4 + n % 2);
            }
            let lines = lines_of(text.as_bytes());
            let mut at = 0;
            let mut written = String::from("f");
            while next() % 4 != 0 && at < lines.len() {
                at += 1 + usize::try_from(next()).unwrap() % (lines.len() - at);
                let _ = write!(written, " {at}");
            }
            if next() % 2 == 0 || written == "f" {
                let _ = write!(written, " /w{}/", next() % 5);
            }
            let given = words(&written);
            let taken = asked(&given).unwrap();
            let (found, _) = sections(&lines, &taken.operands);
            let back: Vec<u8> = found
                .iter()
                .flat_map(|run| lines[run.clone()].concat())
                .collect();
            assert_eq!(back, text.as_bytes(), "{written}");
            for pair in found.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "pieces follow each other");
            }
        }
    }
}
