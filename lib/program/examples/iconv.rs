//! iconv: writes the text of files, or of its standard input, which `-`
//! names too, in another character set, as POSIX's iconv does: from the
//! set `-f` names into the one `-t` names, each UTF-8 when not named,
//! among those `iconv -l` lists (decision record 0032). Bytes that are no
//! character of the first set, or a character the second has no bytes
//! for, stop it; with `-c` they are left out, unsaid, and it goes on.
//! `-s` says nothing of either, nor of a text that ends inside a
//! character. The
//! files make one text, so UTF-16 and UTF-32 put their mark before the
//! first alone. It ends with 0 when everything went over whole, and 1
//! otherwise, with `-c` too.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use charset::{Charset, Decoder, Encoder, Step};
use program::text::{self, Pieces, Source};
use program::{Error, Handle};

program::main!(iconv);

/// What iconv's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    from: Charset,
    to: Charset,
    /// Whether what does not go over is left out, as `-c` asks.
    omit: bool,
    /// Whether nothing is said of it, as `-s` asks.
    quiet: bool,
    /// Whether the sets are listed instead, as `-l` asks.
    list: bool,
    files: &'w [String],
}

/// The set the word `name` names.
fn set(name: &str) -> Result<Charset, String> {
    Charset::named(name).ok_or_else(|| {
        format!("iconv: {name} is no character set iconv knows; iconv -l lists them\n")
    })
}

/// What iconv's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        from: Charset::Utf8,
        to: Charset::Utf8,
        omit: false,
        quiet: false,
        list: false,
        files: &[],
    };
    let mut at = 0;
    while let Some(word) = arguments.get(at) {
        if word == "--" {
            at += 1;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        at += 1;
        for (i, letter) in letters.char_indices() {
            match letter {
                'c' => asked.omit = true,
                's' => asked.quiet = true,
                'l' => asked.list = true,
                'f' | 't' => {
                    // The set's name goes on in the word, or is the next.
                    let name = match &letters[i + 1..] {
                        "" => {
                            let next = arguments.get(at).ok_or_else(|| {
                                format!("iconv: -{letter} wants a character set\n")
                            })?;
                            at += 1;
                            next.as_str()
                        }
                        glued => glued,
                    };
                    let named = set(name)?;
                    if letter == 'f' {
                        asked.from = named;
                    } else {
                        asked.to = named;
                    }
                    break;
                }
                _ => return Err(format!("iconv: -{letter} is no option\n")),
            }
        }
    }
    asked.files = &arguments[at..];
    Ok(asked)
}

/// What `iconv -l` writes: each set on a line, by all its names.
fn listed() -> String {
    let mut text = String::new();
    for set in Charset::ALL {
        text.push_str(&set.names().join(", "));
        text.push('\n');
    }
    text
}

/// What stopped a text going over, or was left out of it, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trouble {
    /// Bytes from this one on are no character.
    NoCharacter(u64),
    /// The character there has no bytes in the set written.
    NoBytes(char, u64),
    /// The text ends inside a character.
    Cut,
    /// The text could not be read.
    Unread(Error),
}

/// Writes what `input` reads in `asked.from` through `encoder` into
/// `out`, and answers the first trouble: where it stopped, or with `-c`
/// the first of what it left out.
fn converted(
    input: &mut impl Pieces,
    asked: &Asked,
    encoder: &mut Encoder,
    out: &mut impl FnMut(&[u8]),
) -> Option<Trouble> {
    let mut decoder = Decoder::new(asked.from);
    let (mut held, mut written) = (Vec::new(), Vec::new());
    // Where in the text the first byte held lies.
    let mut start = 0u64;
    let mut first = None;
    let mut open = true;
    loop {
        let mut used = 0;
        let stopped = loop {
            let trouble = match decoder.step(&held[used..]) {
                Step::Char(c, len) => {
                    let put = encoder.put(c, &mut written);
                    let at = start + used as u64;
                    used += len;
                    if put {
                        continue;
                    }
                    Trouble::NoBytes(c, at)
                }
                Step::Mark(len) => {
                    used += len;
                    continue;
                }
                Step::Bad(len) => {
                    let at = start + used as u64;
                    used += len;
                    Trouble::NoCharacter(at)
                }
                Step::More => break false,
            };
            first.get_or_insert(trouble);
            if !asked.omit {
                break true;
            }
        };
        out(&written);
        written.clear();
        if stopped {
            return first;
        }
        held.drain(..used);
        start += used as u64;
        if !open {
            return if held.is_empty() {
                first
            } else {
                first.or(Some(Trouble::Cut))
            };
        }
        match input.more(&mut held) {
            Ok(more) => open = more,
            Err(e) => return Some(Trouble::Unread(e)),
        }
    }
}

/// What iconv says of `trouble` in the text of `name`, none when `-s`
/// keeps it quiet or `-c` left it out.
fn said(trouble: Trouble, name: &str, asked: &Asked) -> Option<String> {
    let (from, to) = (asked.from.names()[0], asked.to.names()[0]);
    let text = match trouble {
        Trouble::Unread(e) => return Some(text::trouble("iconv", name, e)),
        _ if asked.quiet => return None,
        Trouble::NoCharacter(_) | Trouble::NoBytes(..) if asked.omit => return None,
        Trouble::NoCharacter(at) => format!("iconv: {name}: no {from} character at byte {at}\n"),
        Trouble::NoBytes(c, at) => format!(
            "iconv: {name}: U+{:04X} at byte {at} has no bytes in {to}\n",
            u32::from(c)
        ),
        Trouble::Cut => format!("iconv: {name}: ends inside a {from} character\n"),
    };
    Some(text)
}

/// What iconv does with files and its streams.
trait Files {
    /// What a file is read from.
    type Input: Pieces;

    /// The file at `path`, or the standard input for `-`.
    fn open(&mut self, path: &str) -> Result<Self::Input, Error>;

    /// Closes what `open` gave.
    fn close(&mut self, input: Self::Input);

    /// Writes `bytes` on the standard output.
    fn out(&mut self, bytes: &[u8]);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// Writes the files `asked` names in the set it asks for; whether all of
/// them went over whole. It stops at what does not go over, unless `-c`.
fn convert_all(files: &mut impl Files, asked: &Asked) -> bool {
    if asked.list {
        files.out(listed().as_bytes());
        return true;
    }
    let standard = [String::from("-")];
    let paths = if asked.files.is_empty() {
        &standard[..]
    } else {
        asked.files
    };
    let mut encoder = Encoder::new(asked.to);
    let mut whole = true;
    for path in paths {
        let name = if path == "-" {
            "the standard input"
        } else {
            path
        };
        let mut input = match files.open(path) {
            Ok(input) => input,
            Err(e) => {
                files.say(&text::trouble("iconv", name, e));
                whole = false;
                continue;
            }
        };
        let trouble = converted(&mut input, asked, &mut encoder, &mut |bytes| {
            files.out(bytes);
        });
        files.close(input);
        let Some(trouble) = trouble else {
            continue;
        };
        whole = false;
        if let Some(text) = said(trouble, name, asked) {
            files.say(&text);
        }
        // A file that could not be read is passed over, as one that does
        // not open; -c goes past the rest.
        if !asked.omit && !matches!(trouble, Trouble::Unread(_)) {
            return false;
        }
    }
    whole
}

/// Meridian's files and the program's streams.
struct System {
    out: text::Out<text::Write>,
}

impl Files for System {
    type Input = Source;

    fn open(&mut self, path: &str) -> Result<Source, Error> {
        Source::open(path)
    }

    fn close(&mut self, input: Source) {
        input.close();
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
fn iconv(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => {
            let mut system = System {
                out: text::standard_output(),
            };
            let whole = convert_all(&mut system, &asked);
            let written = system.out.finish();
            u64::from(!(whole && written))
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
    use std::io::{Read as _, Write as _};

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// A text read on the host: a file, `piece` bytes at a time, or bytes
    /// handed over in pieces of that many.
    enum Input {
        File(std::fs::File, usize),
        Bytes(Vec<u8>, usize),
    }

    impl Pieces for Input {
        fn more(&mut self, held: &mut Vec<u8>) -> Result<bool, Error> {
            match self {
                Input::File(file, piece) => {
                    let mut room = vec![0u8; *piece];
                    let len = file.read(&mut room).map_err(|e| match e.kind() {
                        std::io::ErrorKind::IsADirectory => Error::NotAFile,
                        _ => Error::Unreadable,
                    })?;
                    held.extend_from_slice(&room[..len]);
                    Ok(len > 0)
                }
                Input::Bytes(bytes, piece) => {
                    let take = (*piece).min(bytes.len());
                    held.extend(bytes.drain(..take));
                    Ok(take > 0)
                }
            }
        }
    }

    /// Files among the host's temporary ones, gone once dropped, the
    /// standard input, and what iconv wrote and said.
    struct Host {
        root: std::path::PathBuf,
        input: Vec<u8>,
        out: Vec<u8>,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-iconv-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                input: Vec::new(),
                out: Vec::new(),
                said: String::new(),
            }
        }

        fn put(&self, path: &str, bytes: &[u8]) {
            std::fs::write(self.root.join(path), bytes).unwrap();
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Files for Host {
        type Input = Input;

        fn open(&mut self, path: &str) -> Result<Input, Error> {
            if path == "-" {
                return Ok(Input::Bytes(core::mem::take(&mut self.input), 7));
            }
            assert!(!path.contains('/'), "{path} stays in the test's directory");
            let file = std::fs::File::open(self.root.join(path)).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::NotFound,
                _ => Error::Unreadable,
            })?;
            Ok(Input::File(file, 7))
        }

        fn close(&mut self, _: Input) {}

        fn out(&mut self, bytes: &[u8]) {
            self.out.extend_from_slice(bytes);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How iconv with the words `line` ran on `host`: whether all went
    /// over whole, what it wrote and what it said.
    fn ran(host: &mut Host, line: &str) -> (bool, Vec<u8>, String) {
        let given = words(line);
        let taken = asked(&given).unwrap();
        let whole = convert_all(host, &taken);
        (
            whole,
            core::mem::take(&mut host.out),
            core::mem::take(&mut host.said),
        )
    }

    /// What `text` becomes from `line`'s words, read in pieces of `piece`
    /// bytes, and the first trouble.
    fn turned(line: &str, text: &[u8], piece: usize) -> (Vec<u8>, Option<Trouble>) {
        let given = words(line);
        let taken = asked(&given).unwrap();
        let mut out = Vec::new();
        let mut input = Input::Bytes(text.to_vec(), piece);
        let trouble = converted(&mut input, &taken, &mut Encoder::new(taken.to), &mut |b| {
            out.extend_from_slice(b);
        });
        (out, trouble)
    }

    #[test]
    fn its_words_are_options_then_files() {
        let given = words("");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                from: Charset::Utf8,
                to: Charset::Utf8,
                omit: false,
                quiet: false,
                list: false,
                files: &given[..],
            })
        );
        let given = words("-cs -f latin1 -tutf-16 a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                from: Charset::Latin1,
                to: Charset::Utf16,
                omit: true,
                quiet: true,
                list: false,
                files: &given[4..],
            })
        );
        let given = words("-fcp437 -l -- -c");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (taken.from, taken.list, taken.files),
            (Charset::Ibm437, true, &given[3..])
        );
        for (line, why) in [
            ("-x", "iconv: -x is no option\n"),
            ("-f", "iconv: -f wants a character set\n"),
            ("-c -t", "iconv: -t wants a character set\n"),
            (
                "-f UTF-7",
                "iconv: UTF-7 is no character set iconv knows; iconv -l lists them\n",
            ),
            (
                "-tebcdic",
                "iconv: ebcdic is no character set iconv knows; iconv -l lists them\n",
            ),
        ] {
            assert_eq!(asked(&words(line)).err().as_deref(), Some(why), "{line}");
        }
    }

    #[test]
    fn l_lists_each_set_by_all_its_names() {
        assert_eq!(
            listed(),
            "UTF-8, UTF8\nUTF-16, UTF16\nUTF-16BE, UTF16BE\nUTF-16LE, UTF16LE\n\
             UTF-32, UTF32\nUTF-32BE, UTF32BE\nUTF-32LE, UTF32LE\n\
             US-ASCII, ASCII, ANSI_X3.4-1968\n\
             ISO-8859-1, ISO8859-1, ISO_8859-1, LATIN1, L1, CP819, IBM819\n\
             WINDOWS-1252, CP1252, MS-ANSI\nIBM437, CP437, 437\n"
        );
        let mut host = Host::new("list");
        assert_eq!(
            ran(&mut host, "-l"),
            (true, listed().into_bytes(), String::new())
        );
    }

    #[test]
    fn text_goes_over_or_stops_at_what_does_not() {
        for piece in [1, 2, 3, 100] {
            assert_eq!(
                turned("-t latin1", "caf\u{e9}\n".as_bytes(), piece),
                (b"caf\xe9\n".to_vec(), None)
            );
            assert_eq!(
                turned("-f latin1", b"caf\xe9", piece),
                ("caf\u{e9}".into(), None)
            );
            assert_eq!(
                turned("-t ascii", "caf\u{e9}!".as_bytes(), piece),
                (b"caf".to_vec(), Some(Trouble::NoBytes('\u{e9}', 3))),
                "pieces of {piece}"
            );
            assert_eq!(
                turned("-c -t ascii", "caf\u{e9}!\u{20ac}".as_bytes(), piece),
                (b"caf!".to_vec(), Some(Trouble::NoBytes('\u{e9}', 3))),
                "-c goes past, the first trouble kept"
            );
            assert_eq!(
                turned("", b"ab\xffcd", piece),
                (b"ab".to_vec(), Some(Trouble::NoCharacter(2)))
            );
            assert_eq!(
                turned("-c", b"ab\xffcd\xfe", piece),
                (b"abcd".to_vec(), Some(Trouble::NoCharacter(2)))
            );
            assert_eq!(
                turned("", b"a\xe2\x82", piece),
                (b"a".to_vec(), Some(Trouble::Cut))
            );
            assert_eq!(
                turned("-c", b"\xffa\xe2\x82", piece),
                (b"a".to_vec(), Some(Trouble::NoCharacter(0)))
            );
            assert_eq!(
                turned("-c", b"a\xe2\x82", piece),
                (b"a".to_vec(), Some(Trouble::Cut))
            );
            assert_eq!(
                turned("-f utf-16 -t cp437", b"\xfe\xff\x25\x00\x00a", piece),
                (b"\xc4a".to_vec(), None),
                "the mark is read and written as nothing"
            );
            assert_eq!(turned("", b"", piece), (Vec::new(), None));
        }
    }

    #[test]
    fn files_make_one_text_with_one_mark() {
        let mut host = Host::new("files");
        host.put("a", b"a");
        host.put("b", b"b\xff");
        host.input = b"c".to_vec();
        assert_eq!(
            ran(&mut host, "-t utf-16 a - a"),
            (true, b"\xff\xfea\x00c\x00a\x00".to_vec(), String::new())
        );
        assert_eq!(
            ran(&mut host, "-t utf-16le a nope a"),
            (
                false,
                b"a\x00a\x00".to_vec(),
                "iconv: nope: no such file\n".into()
            ),
            "a file that does not open is passed over"
        );
        assert_eq!(
            ran(&mut host, "a b a"),
            (
                false,
                b"ab".to_vec(),
                "iconv: b: no UTF-8 character at byte 1\n".into()
            ),
            "it stops at what does not go over"
        );
        assert_eq!(
            ran(&mut host, "-c a b a"),
            (false, b"aba".to_vec(), String::new()),
            "-c goes on, quietly"
        );
        assert_eq!(
            ran(&mut host, "-s a b a"),
            (false, b"ab".to_vec(), String::new())
        );
        std::fs::create_dir(host.root.join("d")).unwrap();
        assert_eq!(
            ran(&mut host, "-s a d a"),
            (
                false,
                b"aa".to_vec(),
                "iconv: d: a directory, not a file\n".into()
            ),
            "what cannot be read is said all the same, and passed over"
        );
    }

    #[test]
    fn trouble_is_said_by_where_it_is() {
        let given = words("-f utf-8 -t latin1");
        let taken = asked(&given).unwrap();
        for (trouble, text) in [
            (
                Trouble::NoCharacter(12),
                "iconv: f: no UTF-8 character at byte 12\n",
            ),
            (
                Trouble::NoBytes('\u{20ac}', 3),
                "iconv: f: U+20AC at byte 3 has no bytes in ISO-8859-1\n",
            ),
            (
                Trouble::NoBytes('\u{1f600}', 0),
                "iconv: f: U+1F600 at byte 0 has no bytes in ISO-8859-1\n",
            ),
            (Trouble::Cut, "iconv: f: ends inside a UTF-8 character\n"),
            (Trouble::Unread(Error::NotFound), "iconv: f: no such file\n"),
        ] {
            assert_eq!(said(trouble, "f", &taken).as_deref(), Some(text));
        }
        let given = words("-s");
        let quiet = asked(&given).unwrap();
        for trouble in [
            Trouble::NoCharacter(1),
            Trouble::NoBytes('x', 1),
            Trouble::Cut,
        ] {
            assert_eq!(said(trouble, "f", &quiet), None, "{trouble:?}");
        }
        assert_eq!(
            said(Trouble::Unread(Error::Unreadable), "f", &quiet).as_deref(),
            Some("iconv: f: could not be read\n")
        );
        let given = words("-c");
        let omitting = asked(&given).unwrap();
        assert_eq!(
            said(Trouble::NoCharacter(1), "f", &omitting),
            None,
            "-c leaves out unsaid"
        );
        assert_eq!(said(Trouble::NoBytes('x', 1), "f", &omitting), None);
        assert_eq!(
            said(Trouble::Cut, "f", &omitting).as_deref(),
            Some("iconv: f: ends inside a UTF-8 character\n"),
            "but a text cut short is said, as glibc says it"
        );
    }

    #[test]
    fn texts_go_over_as_glibc_s_iconv_takes_them() {
        let host = Host::new("glibc");
        let mut seed = 0xbb67_ae85_84ca_a73b_u64;
        let mut next = |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        let some = [
            "a",
            "Z",
            " ",
            "\n",
            "\u{e9}",
            "\u{20ac}",
            "\u{2500}",
            "\u{fc}",
            "\u{1f600}",
            "\u{a0}",
        ];
        for round in 0..40 {
            let mut text = String::new();
            for _ in 0..next(30) {
                text.push_str(some[usize::try_from(next(some.len() as u64)).unwrap()]);
            }
            let mut bytes = text.into_bytes();
            if round % 3 == 0 && !bytes.is_empty() {
                let at = usize::try_from(next(bytes.len() as u64)).unwrap();
                bytes.insert(at, 0xff);
            }
            host.put("t", &bytes);
            for to in [
                "ISO-8859-1",
                "WINDOWS-1252",
                "IBM437",
                "US-ASCII",
                "UTF-16",
                "UTF-32BE",
            ] {
                // Whether glibc went over whole without -c: with -c some
                // glibc ends with 1 and some with 0, where POSIX has -c
                // change no status, so -c's status is taken from this.
                let mut whole_without = true;
                for c in ["", "-c "] {
                    let line = format!("{c}-t {to} t");
                    let mut theirs = std::process::Command::new("iconv")
                        .args(words(&line))
                        .current_dir(&host.root)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .unwrap();
                    let mut written = Vec::new();
                    theirs
                        .stdout
                        .take()
                        .unwrap()
                        .read_to_end(&mut written)
                        .unwrap();
                    let status = theirs.wait().unwrap();
                    let mut mine = Host::new(&format!("glibc-{round}"));
                    mine.put("t", &bytes);
                    let (whole, ours, _) = ran(&mut mine, &line);
                    if c.is_empty() {
                        whole_without = status.success();
                    }
                    assert_eq!((whole, ours), (whole_without, written), "{line} {bytes:x?}");
                }
            }
        }
        let _ = std::io::stdout().flush();
    }
}
