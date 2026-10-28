//! uudecode: makes again the file uuencode wrote as text, from the file
//! it is given or its standard input, in either encoding: at the path its
//! begin line names, or the one `-o` names, `/dev/stdout` standing for
//! the standard output. Lines before the begin line are passed over, as
//! mail puts its own there, and the file is written only once its end
//! line came. It ends with 0 when the file was made, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use abi::file::{CREATE_ALWAYS, WRITE};
use program::text::{self, LineReader, Source};
use program::{Error, Handle};

program::main!(uudecode);

/// The path that stands for the standard output.
const STDOUT: &str = "/dev/stdout";

/// What uudecode's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    /// The path to write to instead of the one the text names.
    out: Option<&'a str>,
    /// The file to read, `-` for the standard input.
    file: &'a str,
}

/// What uudecode's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut out = None;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        words = rest;
        match word.strip_prefix("-o") {
            Some("") => {
                let (path, rest) = words
                    .split_first()
                    .ok_or_else(|| String::from("uudecode: -o wants a path\n"))?;
                out = Some(path.as_str());
                words = rest;
            }
            Some(path) => out = Some(path),
            None => return Err(format!("uudecode: {word} is no option\n")),
        }
    }
    let file = match words {
        [] => "-",
        [file] => file.as_str(),
        _ => return Err(String::from("uudecode: decodes one file\n")),
    };
    Ok(Asked { out, file })
}

/// Where in the text uudecode is.
#[derive(Debug, PartialEq, Eq)]
enum State {
    /// Looking for the begin line.
    Before,
    Historical,
    /// The historical encoding's empty line came; `end` is next.
    Ending,
    /// Base64, and its characters not yet four.
    Base64(Vec<u8>),
    Ended,
}

/// Makes the file again from the lines of the text, one at a time.
struct Decoder {
    state: State,
    /// The path the begin line names.
    name: String,
    bytes: Vec<u8>,
    /// The number of the line in hand.
    line: usize,
}

/// The six bits the historical encoding's `c` stands for: a space and on,
/// a backquote for nothing as for a space.
fn historical(c: u8) -> Option<u8> {
    match c {
        b'`' => Some(0),
        b' '..=b'_' => Some(c - b' '),
        _ => None,
    }
}

/// The six bits the base64 character `c` stands for.
fn base64(c: u8) -> Option<u8> {
    let bits = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(bits)
}

/// The three bytes four six-bit values make.
fn bytes_of(six: [u8; 4]) -> [u8; 3] {
    [
        six[0] << 2 | six[1] >> 4,
        six[1] << 4 | six[2] >> 2,
        six[2] << 6 | six[3],
    ]
}

impl Decoder {
    fn new() -> Decoder {
        Decoder {
            state: State::Before,
            name: String::new(),
            bytes: Vec::new(),
            line: 0,
        }
    }

    /// What is wrong with the line in hand.
    fn wrong(&self, why: &str) -> String {
        format!("line {}: {why}", self.line)
    }

    /// Takes the next line of the text, without its line feed.
    fn take(&mut self, line: &[u8]) -> Result<(), String> {
        self.line += 1;
        // Mail may end lines with a carriage return too.
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        match &mut self.state {
            State::Before => self.begin(line),
            State::Historical => self.historical_line(line),
            State::Ending if line == b"end" => {
                self.state = State::Ended;
                Ok(())
            }
            State::Ending => Err(self.wrong("no end line after the last")),
            State::Base64(_) => self.base64_line(line),
            State::Ended => Ok(()),
        }
    }

    /// Looks at `line` for the begin line, and takes the name it gives.
    fn begin(&mut self, line: &[u8]) -> Result<(), String> {
        let (state, rest) = if let Some(rest) = line.strip_prefix(b"begin-base64 ") {
            (State::Base64(Vec::new()), rest)
        } else if let Some(rest) = line.strip_prefix(b"begin ") {
            (State::Historical, rest)
        } else {
            return Ok(());
        };
        let Some(space) = rest.iter().position(|&b| b == b' ') else {
            return Ok(());
        };
        let (mode, name) = (&rest[..space], &rest[space + 1..]);
        // Only a mode in octal makes it the begin line.
        if mode.is_empty() || !mode.iter().all(|b| (b'0'..=b'7').contains(b)) {
            return Ok(());
        }
        if name.is_empty() {
            return Err(self.wrong("a begin line without a name"));
        }
        self.name = String::from_utf8_lossy(name).into_owned();
        self.state = state;
        Ok(())
    }

    fn historical_line(&mut self, line: &[u8]) -> Result<(), String> {
        let Some((&first, rest)) = line.split_first() else {
            return Err(self.wrong("an empty line where one of the file's was wanted"));
        };
        let count = historical(first).ok_or_else(|| self.wrong("not a count of bytes"))?;
        if count == 0 {
            self.state = State::Ending;
            return Ok(());
        }
        let count = usize::from(count);
        let mut sixes = Vec::new();
        for &c in rest {
            sixes.push(
                historical(c).ok_or_else(|| {
                    self.wrong("a character the historical encoding does not have")
                })?,
            );
        }
        // A line some mail took the blanks off the end of has them back.
        sixes.resize(sixes.len().max(count.div_ceil(3) * 4), 0);
        let mut made = Vec::new();
        for group in sixes.as_chunks::<4>().0 {
            made.extend_from_slice(&bytes_of(*group));
        }
        made.truncate(count);
        self.bytes.extend_from_slice(&made);
        Ok(())
    }

    fn base64_line(&mut self, line: &[u8]) -> Result<(), String> {
        let State::Base64(held) = &mut self.state else {
            return Ok(());
        };
        if line == b"====" {
            if !held.is_empty() {
                return Err(self.wrong("the last characters are fewer than four"));
            }
            self.state = State::Ended;
            return Ok(());
        }
        let mut chars = core::mem::take(held);
        chars.extend_from_slice(line);
        let whole = chars.len() / 4 * 4;
        for group in chars[..whole].as_chunks::<4>().0 {
            let padding = group.iter().rev().take_while(|&&c| c == b'=').count();
            let mut six = [0u8; 4];
            for (at, &c) in group[..4 - padding].iter().enumerate() {
                six[at] =
                    base64(c).ok_or_else(|| self.wrong("a character base64 does not have"))?;
            }
            if padding > 2 {
                return Err(self.wrong("a character base64 does not have"));
            }
            self.bytes.extend_from_slice(&bytes_of(six)[..3 - padding]);
        }
        self.state = State::Base64(chars[whole..].to_vec());
        Ok(())
    }

    /// The path the text names and the file's bytes, or what is wrong.
    fn finish(self) -> Result<(String, Vec<u8>), String> {
        match self.state {
            State::Ended => Ok((self.name, self.bytes)),
            State::Before => Err(String::from("no line begins what uuencode writes")),
            _ => Err(String::from("it ends before its end line")),
        }
    }
}

/// Where uudecode writes the file it made.
trait Out {
    /// Makes the file at `path` hold `bytes`.
    fn file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error>;

    /// Writes `bytes` on the standard output.
    fn standard(&mut self, bytes: &[u8]) -> Result<(), Error>;
}

/// Writes `bytes` where `asked` says, or else at the path the text named.
fn put(out: &mut impl Out, asked: &Asked, named: String, bytes: &[u8]) -> Result<(), String> {
    let path = asked.out.map_or(named, String::from);
    let written = if path == STDOUT {
        out.standard(bytes)
    } else {
        out.file(&path, bytes)
    };
    written.map_err(|e| text::trouble("uudecode", &path, e))
}

/// Meridian's files and uudecode's standard output.
struct System;

impl Out for System {
    fn file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let file = program::open_with(path, WRITE, CREATE_ALWAYS)?;
        let written = program::write_file(file, 0, bytes);
        let closed = program::close(file);
        written.and(closed)
    }

    fn standard(&mut self, bytes: &[u8]) -> Result<(), Error> {
        program::write_bytes(bytes)
    }
}

/// Decodes the text of `asked`'s file, or why it could not.
fn decoded(asked: &Asked) -> Result<(String, Vec<u8>), String> {
    let source = Source::open(asked.file).map_err(|e| text::trouble("uudecode", asked.file, e))?;
    let mut lines = LineReader::new(source);
    let mut decoder = Decoder::new();
    let read = loop {
        match lines.line() {
            Ok(Some(line)) => {
                if let Err(why) = decoder.take(&line) {
                    break Err(format!("uudecode: {}: {why}\n", asked.file));
                }
            }
            Ok(None) => break Ok(()),
            Err(e) => break Err(text::trouble("uudecode", asked.file, e)),
        }
    };
    lines.into_pieces().close();
    read?;
    decoder
        .finish()
        .map_err(|why| format!("uudecode: {}: {why}\n", asked.file))
}

#[cfg_attr(test, allow(dead_code))]
fn uudecode(_: Handle) -> u64 {
    let made = asked(program::arguments()).and_then(|asked| {
        let (named, bytes) = decoded(&asked)?;
        put(&mut System, &asked, named, &bytes)
    });
    match made {
        Ok(()) => 0,
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

    /// The path and bytes the text `lines` makes, or what is wrong.
    fn decode(lines: &str) -> Result<(String, Vec<u8>), String> {
        let mut decoder = Decoder::new();
        for line in lines.split('\n') {
            decoder.take(line.as_bytes())?;
        }
        decoder.finish()
    }

    fn made(name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
        (name.into(), bytes.to_vec())
    }

    #[test]
    fn its_words_are_an_output_and_a_file() {
        assert_eq!(
            asked(&words("")),
            Ok(Asked {
                out: None,
                file: "-"
            })
        );
        assert_eq!(
            asked(&words("f")),
            Ok(Asked {
                out: None,
                file: "f"
            })
        );
        assert_eq!(
            asked(&words("-o x f")),
            Ok(Asked {
                out: Some("x"),
                file: "f"
            })
        );
        assert_eq!(
            asked(&words("-ox")),
            Ok(Asked {
                out: Some("x"),
                file: "-"
            })
        );
        assert_eq!(
            asked(&words("-- -o")),
            Ok(Asked {
                out: None,
                file: "-o"
            })
        );
        assert_eq!(
            asked(&words("-")),
            Ok(Asked {
                out: None,
                file: "-"
            })
        );
        assert_eq!(
            asked(&words("-o")),
            Err("uudecode: -o wants a path\n".into())
        );
        assert_eq!(
            asked(&words("-x")),
            Err("uudecode: -x is no option\n".into())
        );
        assert_eq!(
            asked(&words("a b")),
            Err("uudecode: decodes one file\n".into())
        );
        assert_eq!(
            asked(&words("f -o x")),
            Err("uudecode: decodes one file\n".into()),
            "no option after the file"
        );
    }

    #[test]
    fn what_uuencode_wrote_decodes_to_its_bytes() {
        assert_eq!(
            decode("begin 644 x\n&:&5L;&\\*\n`\nend"),
            Ok(made("x", b"hello\n"))
        );
        assert_eq!(
            decode("begin-base64 644 x\naGVsbG8K\n====\n"),
            Ok(made("x", b"hello\n"))
        );
        assert_eq!(decode("begin 644 a b\n`\nend"), Ok(made("a b", b"")));
        assert_eq!(decode("begin-base64 600 e\n====\n"), Ok(made("e", b"")));
        assert_eq!(
            decode("begin 644 x\n!80``\n\"86(`\n#86)C\n`\nend"),
            Ok(made("x", b"aababc")),
            "lines of one, two and three"
        );
        assert_eq!(
            decode("begin-base64 644 x\nYQ==\nYWI=\nYWJj\n====\n"),
            Ok(made("x", b"aababc"))
        );
    }

    #[test]
    fn mail_around_the_text_is_passed_over() {
        let mail = "From: a\nSubject: begin here\n\nbegin 644\nbegin x y\nbegin 644 x\n&:&5L;&\\*\n`\nend\n-- \nsigned";
        assert_eq!(decode(mail), Ok(made("x", b"hello\n")));
        assert_eq!(
            decode("begin 644 x\r\n&:&5L;&\\*\r\n`\r\nend\r\n"),
            Ok(made("x", b"hello\n")),
            "lines ended as mail may end them"
        );
        assert_eq!(
            decode("begin 644 x\n#80\n`\nend"),
            Ok(made("x", b"a\0\0")),
            "blanks taken off a line's end come back"
        );
        assert_eq!(
            decode("begin 644 x\n#    \n`\nend"),
            Ok(made("x", b"\0\0\0")),
            "a blank is nothing, as a backquote is"
        );
        assert_eq!(
            decode("begin-base64 644 x\naGVs\nbG8K\n====\n"),
            Ok(made("x", b"hello\n"))
        );
        assert_eq!(
            decode("begin-base64 644 x\naGV\nsbG8K\n====\n"),
            Ok(made("x", b"hello\n")),
            "a group may go on over two lines"
        );
    }

    #[test]
    fn a_text_not_whole_is_said() {
        assert_eq!(
            decode("hello"),
            Err("no line begins what uuencode writes".into())
        );
        assert_eq!(
            decode("begin 644 x\n&:&5L;&\\*"),
            Err("it ends before its end line".into())
        );
        assert_eq!(
            decode("begin-base64 644 x\naGVsbG8K"),
            Err("it ends before its end line".into())
        );
        assert_eq!(
            decode("begin 644 x\n`\nfin"),
            Err("line 3: no end line after the last".into())
        );
        assert_eq!(
            decode("begin 644 x\n&:&5L;~\\*\n`\nend"),
            Err("line 2: a character the historical encoding does not have".into())
        );
        assert_eq!(
            decode("begin 644 x\n~\n`\nend"),
            Err("line 2: not a count of bytes".into())
        );
        assert_eq!(
            decode("begin 644 x\n\n`\nend"),
            Err("line 2: an empty line where one of the file's was wanted".into())
        );
        assert_eq!(
            decode("begin-base64 644 x\naGV*bG8K\n====\n"),
            Err("line 2: a character base64 does not have".into())
        );
        assert_eq!(
            decode("begin-base64 644 x\naGVsb\n====\n"),
            Err("line 3: the last characters are fewer than four".into())
        );
        assert_eq!(
            decode("begin-base64 644 x\nY===\n====\n"),
            Err("line 2: a character base64 does not have".into())
        );
        assert_eq!(
            decode("begin 644 \n`\nend"),
            Err("line 1: a begin line without a name".into())
        );
    }

    #[test]
    fn the_alphabets_are_whole() {
        let values: Vec<u8> = (b' '..=b'`').filter_map(historical).collect();
        assert_eq!(values.len(), 65, "the blank and the backquote both 0");
        assert_eq!(values[..64], (0..64).collect::<Vec<u8>>());
        assert_eq!(historical(b'`'), Some(0));
        assert_eq!(historical(b'\x1f'), None);
        assert_eq!(historical(b'a'), None);
        let base: Vec<u8> = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            .iter()
            .filter_map(|&c| base64(c))
            .collect();
        assert_eq!(base, (0..64).collect::<Vec<u8>>());
        for c in *b"=-_ @[`{" {
            assert_eq!(base64(c), None, "{}", c as char);
        }
        assert_eq!(bytes_of([0x3f; 4]), [0xff; 3]);
        assert_eq!(bytes_of([0x18, 0x16, 0x09, 0x23]), *b"abc");
    }

    /// Every byte value, as Python's binascii writes them, 45 a line.
    const EVERY_BYTE: &str = r#"M``$"`P0%!@<("0H+#`T.#Q`1$A,4%187&!D:&QP='A\@(2(C)"4F)R@I*BLL
M+2XO,#$R,S0U-C<X.3H[/#T^/T!!0D-$149'2$E*2TQ-3D]045)35%565UA9
M6EM<75Y?8&%B8V1E9F=H:6IK;&UN;W!Q<G-T=79W>'EZ>WQ]?G^`@8*#A(6&
MAXB)BHN,C8Z/D)&2DY25EI>8F9J;G)V>GZ"AHJ.DI::GJ*FJJZRMKJ^PL;*S
MM+6VM[BYNKN\O;Z_P,'"P\3%QL?(R<K+S,W.S]#1TM/4U=;7V-G:V]S=WM_@
?X>+CY.7FY^CIZNOL[>[O\/'R\_3U]O?X^?K[_/W^_P``
"#;

    /// The bytes 0 to 99 as Python's base64 writes them, 76 characters a
    /// line as GNU's base64 does.
    const HUNDRED: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4\nOTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiYw==\n";

    #[test]
    fn lines_others_wrote_decode_too() {
        let text = format!("begin 644 all\n{EVERY_BYTE}`\nend\n");
        let every: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&text), Ok(made("all", &every)));
        let text = format!("begin-base64 644 h\n{HUNDRED}====\n");
        assert_eq!(decode(&text), Ok(made("h", &every[..100])));
        assert_eq!(
            decode("begin 644 x\n!80``X\n`\nend"),
            Ok(made("x", b"a")),
            "what a line holds past its count is let be"
        );
        assert_eq!(
            decode("begin 644 x\n#\n`\nend"),
            Ok(made("x", b"\0\0\0")),
            "a count alone is that many zeros, its blanks taken off"
        );
        assert_eq!(
            decode("From: a\nto: b\nbegin 644 x\n~\n"),
            Err("line 4: not a count of bytes".into()),
            "lines are counted from the first, mail and all"
        );
    }

    /// The six-bit values of `bytes` the long way: each group of three,
    /// filled out with zeros, as one number of 24 bits.
    fn sixes(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for group in bytes.chunks(3) {
            let mut n = 0u32;
            for i in 0..3 {
                n = n << 8 | u32::from(group.get(i).copied().unwrap_or(0));
            }
            for shift in [18, 12, 6, 0] {
                out.push(u8::try_from(n >> shift & 0x3f).unwrap());
            }
        }
        out
    }

    /// `bytes` in base64, lines of `width` characters.
    fn base64_of(bytes: &[u8], width: usize) -> String {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let carried = (bytes.len() * 4).div_ceil(3);
        let mut chars: Vec<u8> = sixes(bytes)
            .iter()
            .map(|&v| alphabet[usize::from(v)])
            .collect();
        for c in &mut chars[carried..] {
            *c = b'=';
        }
        let mut text = String::new();
        for line in chars.chunks(width) {
            text.push_str(&String::from_utf8_lossy(line));
            text.push('\n');
        }
        text
    }

    /// `bytes` in the historical encoding with nothing written as a
    /// blank, and the blanks at the ends of lines taken off, as mail
    /// did to what the first uuencode wrote.
    fn historical_of(bytes: &[u8]) -> String {
        let mut text = String::new();
        for line in bytes.chunks(45) {
            let mut chars = alloc::vec![b' ' + u8::try_from(line.len()).unwrap()];
            chars.extend(sixes(line).iter().map(|&v| b' ' + v));
            let kept = String::from_utf8_lossy(&chars);
            text.push_str(kept.trim_end_matches(' '));
            text.push('\n');
        }
        text
    }

    #[test]
    fn random_bytes_come_back_from_either_encoding() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = usize::try_from(seed % 300).unwrap();
            // Zeros among them, for the blanks mail takes off.
            let bytes: Vec<u8> = (0..len)
                .map(|n| (seed >> (n % 61)).to_le_bytes()[0] & 0xf3)
                .collect();
            let text = format!("begin 644 r\n{}`\nend\n", historical_of(&bytes));
            assert_eq!(decode(&text), Ok(made("r", &bytes)), "{text}");
            for width in [4, 7, 60, 76] {
                let text = format!("begin-base64 644 r\n{}====\n", base64_of(&bytes, width));
                assert_eq!(decode(&text), Ok(made("r", &bytes)), "{text}");
            }
        }
    }

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in uudecode reaches the host's own files.
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
    /// went to the standard output.
    struct Host {
        root: std::path::PathBuf,
        standard: Vec<u8>,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root = std::env::temp_dir()
                .join(format!("meridian-uudecode-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                standard: Vec::new(),
            }
        }

        fn holds(&self, path: &str) -> Option<Vec<u8>> {
            std::fs::read(self.root.join(path)).ok()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Out for Host {
        fn file(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
            std::fs::write(confined(&self.root, path), bytes).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::NotFound,
                _ => Error::Unwritable,
            })
        }

        fn standard(&mut self, bytes: &[u8]) -> Result<(), Error> {
            self.standard.extend_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn the_file_goes_where_o_says_or_the_text_does() {
        let mut host = Host::new("put");
        let plain = Asked {
            out: None,
            file: "-",
        };
        assert_eq!(put(&mut host, &plain, "x".into(), b"hi"), Ok(()));
        assert_eq!(host.holds("x").as_deref(), Some(&b"hi"[..]));
        let other = Asked {
            out: Some("y"),
            file: "-",
        };
        assert_eq!(put(&mut host, &other, "z".into(), b"yo"), Ok(()));
        assert_eq!(host.holds("y").as_deref(), Some(&b"yo"[..]));
        assert_eq!(host.holds("z"), None, "-o wins over the name");
        assert_eq!(put(&mut host, &plain, "x".into(), b""), Ok(()));
        assert_eq!(
            host.holds("x").as_deref(),
            Some(&b""[..]),
            "a file there is made anew"
        );
        assert_eq!(host.standard, b"", "none of it went out");
    }

    #[test]
    fn dev_stdout_is_the_standard_output() {
        let mut host = Host::new("stdout");
        let plain = Asked {
            out: None,
            file: "-",
        };
        assert_eq!(put(&mut host, &plain, STDOUT.into(), b"a"), Ok(()));
        let asked = Asked {
            out: Some(STDOUT),
            file: "-",
        };
        assert_eq!(put(&mut host, &asked, "x".into(), b"b"), Ok(()));
        assert_eq!(host.standard, b"ab");
        assert_eq!(host.holds("x"), None);
        assert_eq!(
            put(&mut host, &plain, "nowhere/x".into(), b"c"),
            Err("uudecode: nowhere/x: no such file\n".into()),
            "a path that cannot be written is said"
        );
    }

    #[test]
    fn what_comes_after_the_end_is_let_be() {
        assert_eq!(
            decode("begin 644 a\n`\nend\nbegin 644 b\n!80``\n`\nend"),
            Ok(made("a", b"")),
            "a second text is not the first's"
        );
        assert_eq!(
            decode("begin-base64 644 a\nYQ==YQ==\n====\n"),
            Ok(made("a", b"aa")),
            "groups padded alone, as one text after another"
        );
    }
}
