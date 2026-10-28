//! uuencode: writes a file, or its standard input, as lines of text that
//! mail carries unchanged, for uudecode to make the file again: in the
//! historical encoding, or in base64 with `-m`, 45 bytes a line in both.
//! The first line names the file uudecode makes and its mode, which is
//! 644 as Meridian's files have no modes yet. It ends with 0 when all was
//! read and written, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Pieces, Source};

program::main!(uuencode);

/// The bytes a line holds.
const LINE: usize = 45;

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// What uuencode's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    base64: bool,
    /// The file to encode, `-` for the standard input.
    file: &'a str,
    /// The name uudecode gives the file it makes.
    name: &'a str,
}

/// What uuencode's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut base64 = false;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        match word.as_str() {
            "--" => {
                words = rest;
                break;
            }
            "-m" => base64 = true,
            _ if word.starts_with('-') && word != "-" => {
                return Err(format!("uuencode: {word} is no option\n"));
            }
            _ => break,
        }
        words = rest;
    }
    let (file, name) = match words {
        [name] => ("-", name),
        [file, name] => (file.as_str(), name),
        _ => {
            return Err(String::from(
                "uuencode: wants the name to decode to, after the file if there is one\n",
            ));
        }
    };
    if name.contains('\n') {
        return Err(String::from("uuencode: a name to decode to is one line\n"));
    }
    Ok(Asked { base64, file, name })
}

/// The historical encoding's character for six bits: a space and on,
/// with a backquote for nothing, which mail does not take off a line's
/// end.
fn historical(bits: u8) -> u8 {
    match bits {
        0 => b'`',
        bits => bits + b' ',
    }
}

/// The line that holds `bytes`, at most [`LINE`] of them.
fn line(bytes: &[u8], base64: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if !base64 {
        // At most LINE, the count fits its low byte.
        out.push(historical(bytes.len().to_le_bytes()[0]));
    }
    for group in bytes.chunks(3) {
        let at = |i: usize| group.get(i).copied().unwrap_or(0);
        let six = [
            at(0) >> 2,
            (at(0) << 4 | at(1) >> 4) & 0x3f,
            (at(1) << 2 | at(2) >> 6) & 0x3f,
            at(2) & 0x3f,
        ];
        for (i, bits) in six.into_iter().enumerate() {
            out.push(if !base64 {
                historical(bits)
            } else if i > group.len() {
                // Base64 fills out the last group with =.
                b'='
            } else {
                BASE64[usize::from(bits)]
            });
        }
    }
    out.push(b'\n');
    out
}

/// The line that begins what uuencode writes.
fn begin(base64: bool, name: &str) -> String {
    let kind = if base64 { "begin-base64" } else { "begin" };
    format!("{kind} 644 {name}\n")
}

/// The lines that end what uuencode writes: the historical encoding's
/// empty line and `end`, or base64's `====`.
fn end(base64: bool) -> &'static [u8] {
    if base64 { b"====\n" } else { b"`\nend\n" }
}

/// Encodes what comes, a piece at a time, into the lines it writes.
struct Encoder {
    base64: bool,
    held: Vec<u8>,
}

impl Encoder {
    /// The lines `bytes` fills, with what was held before them; the rest
    /// is held for the next.
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.held.extend_from_slice(bytes);
        let whole = self.held.len() / LINE * LINE;
        let mut out = Vec::new();
        for chunk in self.held[..whole].chunks(LINE) {
            out.extend_from_slice(&line(chunk, self.base64));
        }
        self.held.drain(..whole);
        out
    }

    /// The last line, if anything is held, and the end.
    fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.held.is_empty() {
            out.extend_from_slice(&line(&self.held, self.base64));
        }
        out.extend_from_slice(end(self.base64));
        out
    }
}

#[cfg_attr(test, allow(dead_code))]
fn uuencode(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let say = |e| {
        let _ = program::write_error(&text::trouble("uuencode", asked.file, e));
        1
    };
    // Opened first, a file not there has nothing written for it.
    let mut source = match Source::open(asked.file) {
        Ok(source) => source,
        Err(e) => return say(e),
    };
    let mut out = text::standard_output();
    out.put(begin(asked.base64, asked.name).as_bytes());
    let mut encoder = Encoder {
        base64: asked.base64,
        held: Vec::new(),
    };
    let mut piece = Vec::new();
    let read = loop {
        match source.more(&mut piece) {
            Ok(true) => out.put(&encoder.feed(&core::mem::take(&mut piece))),
            Ok(false) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    source.close();
    // Cut short, it has no end, so that uudecode says it is cut short.
    if read.is_ok() {
        out.put(&encoder.finish());
    }
    let written = out.finish();
    match read {
        Ok(()) => u64::from(!written),
        Err(e) => say(e),
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

    fn encoded(pieces: &[&[u8]], base64: bool) -> String {
        let mut encoder = Encoder {
            base64,
            held: Vec::new(),
        };
        let mut out = Vec::new();
        for piece in pieces {
            out.extend_from_slice(&encoder.feed(piece));
        }
        out.extend_from_slice(&encoder.finish());
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn its_words_are_a_file_and_a_name() {
        let given = words("-m f out");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                base64: true,
                file: "f",
                name: "out"
            })
        );
        let given = words("out");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                base64: false,
                file: "-",
                name: "out"
            }),
            "without a file, the standard input, which - is"
        );
        let given = words("-- -m out");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                base64: false,
                file: "-m",
                name: "out"
            })
        );
        let wants = "uuencode: wants the name to decode to, after the file if there is one\n";
        for text in ["", "-m", "a b c"] {
            assert_eq!(asked(&words(text)), Err(wants.into()), "{text:?}");
        }
        assert_eq!(
            asked(&words("-x out")),
            Err("uuencode: -x is no option\n".into())
        );
        assert_eq!(
            asked(&[String::from("a\nb")]),
            Err("uuencode: a name to decode to is one line\n".into())
        );
    }

    #[test]
    fn lines_hold_the_bytes_as_python_s_binascii_writes_them() {
        assert_eq!(line(b"a", false), b"!80``\n");
        assert_eq!(line(b"ab", false), b"\"86(`\n");
        assert_eq!(line(b"abc", false), b"#86)C\n");
        assert_eq!(line(b"hello\n", false), b"&:&5L;&\\*\n");
        assert_eq!(line(b"\0\0\0", false), b"#````\n", "nothing is a backquote");
        let counting: Vec<u8> = (0..45).collect();
        assert_eq!(
            line(&counting, false),
            b"M``$\"`P0%!@<(\"0H+#`T.#Q`1$A,4%187&!D:&QP='A\\@(2(C)\"4F)R@I*BLL\n"
        );
    }

    #[test]
    fn base64_lines_are_as_python_writes_them() {
        assert_eq!(line(b"a", true), b"YQ==\n");
        assert_eq!(line(b"ab", true), b"YWI=\n");
        assert_eq!(line(b"abc", true), b"YWJj\n");
        assert_eq!(line(b"hello\n", true), b"aGVsbG8K\n");
        let counting: Vec<u8> = (0..45).collect();
        assert_eq!(
            line(&counting, true),
            b"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKiss\n"
        );
        assert_eq!(
            line(b"\xff\xfe", true),
            b"//4=\n",
            "the last two characters"
        );
    }

    #[test]
    fn what_is_written_begins_and_ends_as_uudecode_looks_for() {
        assert_eq!(begin(false, "a b"), "begin 644 a b\n");
        assert_eq!(begin(true, "x"), "begin-base64 644 x\n");
        assert_eq!(encoded(&[], false), "`\nend\n", "nothing is an end alone");
        assert_eq!(encoded(&[], true), "====\n");
        assert_eq!(encoded(&[b"hello\n"], false), "&:&5L;&\\*\n`\nend\n");
        assert_eq!(encoded(&[b"hello\n"], true), "aGVsbG8K\n====\n");
    }

    #[test]
    fn lines_are_full_whatever_the_pieces() {
        let counting: Vec<u8> = (0..46).collect();
        let whole = encoded(&[&counting], false);
        assert_eq!(
            whole,
            "M``$\"`P0%!@<(\"0H+#`T.#Q`1$A,4%187&!D:&QP='A\\@(2(C)\"4F)R@I*BLL\n!+0``\n`\nend\n"
        );
        for size in [1, 2, 44, 45] {
            let pieces: Vec<&[u8]> = counting.chunks(size).collect();
            assert_eq!(encoded(&pieces, false), whole, "pieces of {size}");
        }
        assert_eq!(
            encoded(&[&counting[..20], &counting[20..]], true),
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKiss\nLQ==\n====\n"
        );
        let ninety: Vec<u8> = (0..90).collect();
        assert_eq!(
            encoded(&[&ninety], false).lines().count(),
            4,
            "two full lines, then the end's two"
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

    #[test]
    fn random_bytes_encode_as_their_six_bit_values() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = usize::try_from(seed % 46).unwrap();
            let bytes: Vec<u8> = (0..len)
                .map(|n| (seed >> (n % 57)).to_le_bytes()[0])
                .collect();
            let values = sixes(&bytes);
            let old = line(&bytes, false);
            let count = u8::try_from(len).unwrap();
            assert_eq!(old[0], if len == 0 { b'`' } else { b' ' + count });
            let back: Vec<u8> = old[1..old.len() - 1]
                .iter()
                .map(|&c| (c - b' ') & 0x3f)
                .collect();
            assert_eq!(back, values, "{bytes:?}");
            assert!(!old.contains(&b' '), "no blank for mail to take off");
            let new = line(&bytes, true);
            assert_eq!(new.len(), len.div_ceil(3) * 4 + 1);
            // The characters that carry bits, then = for the rest.
            let carried = (len * 4).div_ceil(3);
            let back: Vec<u8> = new[..carried]
                .iter()
                .map(|c| u8::try_from(BASE64.iter().position(|b| b == c).unwrap()).unwrap())
                .collect();
            assert_eq!(back, values[..carried], "{bytes:?}");
            assert!(new[carried..new.len() - 1].iter().all(|&c| c == b'='));
        }
    }

    #[test]
    fn less_than_a_line_waits_for_more() {
        let mut encoder = Encoder {
            base64: false,
            held: Vec::new(),
        };
        assert_eq!(encoder.feed(&[1; 44]), b"");
        assert_eq!(encoder.feed(&[2; 6]).len(), 62, "a full line out");
        assert_eq!(encoder.held, [2; 5], "the rest held");
        assert_eq!(encoder.feed(&[3; 85]).len(), 2 * 62);
        assert_eq!(encoder.held, b"", "two lines took all");
        assert_eq!(encoder.finish(), b"`\nend\n");
    }
}
