//! Character sets (decision record 0032): bytes read into Unicode scalar
//! values and written back, a character at a time, for UTF-8, UTF-16 and
//! UTF-32 in either order, US-ASCII, ISO 8859-1, Windows-1252 and the PC's
//! code page 437. The tables of the last two are Microsoft's, as Unicode
//! publishes them. UTF-16 and UTF-32 named with no order are read in the
//! order a byte order mark gives, and else in the machine's, as glibc
//! reads them, and written after a mark, little-endian.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

/// A character set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    /// In the order a mark gives, else little-endian.
    Utf16,
    Utf16Be,
    Utf16Le,
    /// In the order a mark gives, else little-endian.
    Utf32,
    Utf32Be,
    Utf32Le,
    Ascii,
    Latin1,
    Windows1252,
    Ibm437,
}

/// The characters of Windows-1252's bytes 0x80 to 0x9F, 0 for the five it
/// leaves undefined; the rest are ISO 8859-1's.
const WINDOWS_1252: [u16; 32] = [
    0x20ac, 0, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017d, 0, 0, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x02dc,
    0x2122, 0x0161, 0x203a, 0x0153, 0, 0x017e, 0x0178,
];

/// The characters of code page 437's bytes 0x80 to 0xFF; the rest are
/// US-ASCII's.
const IBM_437: [u16; 128] = [
    0x00c7, 0x00fc, 0x00e9, 0x00e2, 0x00e4, 0x00e0, 0x00e5, 0x00e7, 0x00ea, 0x00eb, 0x00e8, 0x00ef,
    0x00ee, 0x00ec, 0x00c4, 0x00c5, 0x00c9, 0x00e6, 0x00c6, 0x00f4, 0x00f6, 0x00f2, 0x00fb, 0x00f9,
    0x00ff, 0x00d6, 0x00dc, 0x00a2, 0x00a3, 0x00a5, 0x20a7, 0x0192, 0x00e1, 0x00ed, 0x00f3, 0x00fa,
    0x00f1, 0x00d1, 0x00aa, 0x00ba, 0x00bf, 0x2310, 0x00ac, 0x00bd, 0x00bc, 0x00a1, 0x00ab, 0x00bb,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255d, 0x255c, 0x255b, 0x2510, 0x2514, 0x2534, 0x252c, 0x251c, 0x2500, 0x253c, 0x255e, 0x255f,
    0x255a, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256c, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256b, 0x256a, 0x2518, 0x250c, 0x2588, 0x2584, 0x258c, 0x2590, 0x2580,
    0x03b1, 0x00df, 0x0393, 0x03c0, 0x03a3, 0x03c3, 0x00b5, 0x03c4, 0x03a6, 0x0398, 0x03a9, 0x03b4,
    0x221e, 0x03c6, 0x03b5, 0x2229, 0x2261, 0x00b1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00f7, 0x2248,
    0x00b0, 0x2219, 0x00b7, 0x221a, 0x207f, 0x00b2, 0x25a0, 0x00a0,
];

impl Charset {
    /// Every set, in the order `iconv -l` lists them.
    pub const ALL: [Charset; 11] = [
        Charset::Utf8,
        Charset::Utf16,
        Charset::Utf16Be,
        Charset::Utf16Le,
        Charset::Utf32,
        Charset::Utf32Be,
        Charset::Utf32Le,
        Charset::Ascii,
        Charset::Latin1,
        Charset::Windows1252,
        Charset::Ibm437,
    ];

    /// Its names, the first the one it goes by, as glibc's iconv takes
    /// them.
    #[must_use]
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Charset::Utf8 => &["UTF-8", "UTF8"],
            Charset::Utf16 => &["UTF-16", "UTF16"],
            Charset::Utf16Be => &["UTF-16BE", "UTF16BE"],
            Charset::Utf16Le => &["UTF-16LE", "UTF16LE"],
            Charset::Utf32 => &["UTF-32", "UTF32"],
            Charset::Utf32Be => &["UTF-32BE", "UTF32BE"],
            Charset::Utf32Le => &["UTF-32LE", "UTF32LE"],
            Charset::Ascii => &["US-ASCII", "ASCII", "ANSI_X3.4-1968"],
            Charset::Latin1 => &[
                "ISO-8859-1",
                "ISO8859-1",
                "ISO_8859-1",
                "LATIN1",
                "L1",
                "CP819",
                "IBM819",
            ],
            Charset::Windows1252 => &["WINDOWS-1252", "CP1252", "MS-ANSI"],
            Charset::Ibm437 => &["IBM437", "CP437", "437"],
        }
    }

    /// The set `name` names, its letters capitals or small.
    #[must_use]
    pub fn named(name: &str) -> Option<Charset> {
        Charset::ALL.into_iter().find(|set| {
            set.names()
                .iter()
                .any(|known| known.eq_ignore_ascii_case(name))
        })
    }
}

/// The order of the bytes of a unit of UTF-16 or UTF-32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    Big,
    Little,
}

impl Order {
    /// The number the bytes `bytes` make in this order.
    fn read(self, bytes: &[u8]) -> u32 {
        let mut value = 0;
        for at in 0..bytes.len() {
            let byte = match self {
                Order::Big => bytes[at],
                Order::Little => bytes[bytes.len() - 1 - at],
            };
            value = value << 8 | u32::from(byte);
        }
        value
    }

    /// Writes the low `width` bytes of `value` onto `out` in this order.
    fn write(self, value: u32, width: usize, out: &mut Vec<u8>) {
        let bytes = value.to_be_bytes();
        let low = &bytes[4 - width..];
        match self {
            Order::Big => out.extend_from_slice(low),
            Order::Little => out.extend(low.iter().rev()),
        }
    }
}

/// What the first bytes of what is read hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// A character, and how many bytes it took.
    Char(char, usize),
    /// A byte order mark that says the order of what follows, and how many
    /// bytes it took.
    Mark(usize),
    /// Bytes that stand for no character, as many as go together: the
    /// most that start a character and could not end it, or one.
    Bad(usize),
    /// Bytes that start a character and end before it does.
    More,
}

/// What reads bytes of a set a character at a time.
#[derive(Clone, Copy, Debug)]
pub struct Decoder {
    set: Charset,
    order: Order,
    /// Whether the first character was read, after which no mark comes.
    begun: bool,
}

impl Decoder {
    #[must_use]
    pub fn new(set: Charset) -> Decoder {
        let order = match set {
            Charset::Utf16Be | Charset::Utf32Be => Order::Big,
            _ => Order::Little,
        };
        Decoder {
            set,
            order,
            begun: false,
        }
    }

    /// What the first bytes of `bytes` hold.
    pub fn step(&mut self, bytes: &[u8]) -> Step {
        if let (Charset::Utf16 | Charset::Utf32, false) = (self.set, self.begun) {
            let width = if self.set == Charset::Utf16 { 2 } else { 4 };
            let Some(first) = bytes.get(..width) else {
                return Step::More;
            };
            self.begun = true;
            for order in [Order::Big, Order::Little] {
                if order.read(first) == 0xfeff {
                    self.order = order;
                    return Step::Mark(width);
                }
            }
        }
        self.begun = true;
        match self.set {
            Charset::Utf8 => utf8(bytes),
            Charset::Utf16 | Charset::Utf16Be | Charset::Utf16Le => utf16(bytes, self.order),
            Charset::Utf32 | Charset::Utf32Be | Charset::Utf32Le => utf32(bytes, self.order),
            Charset::Ascii | Charset::Latin1 | Charset::Windows1252 | Charset::Ibm437 => {
                let Some(&byte) = bytes.first() else {
                    return Step::More;
                };
                byte_of(self.set, byte).map_or(Step::Bad(1), |c| Step::Char(c, 1))
            }
        }
    }
}

/// The character the byte `byte` of a set of single bytes stands for.
fn byte_of(set: Charset, byte: u8) -> Option<char> {
    let code = match (set, byte) {
        (Charset::Windows1252, 0x80..=0x9f) => WINDOWS_1252[usize::from(byte - 0x80)],
        (_, 0..=0x7f) | (Charset::Latin1 | Charset::Windows1252, _) => u16::from(byte),
        (Charset::Ibm437, _) => IBM_437[usize::from(byte - 0x80)],
        _ => 0,
    };
    (code != 0 || byte == 0)
        .then(|| char::from_u32(u32::from(code)))
        .flatten()
}

/// The first character of `bytes` in UTF-8, none of its overlong forms,
/// surrogates or numbers past U+10FFFF.
fn utf8(bytes: &[u8]) -> Step {
    let Some(&first) = bytes.first() else {
        return Step::More;
    };
    // How many bytes, and the bytes the second may be.
    let (len, second) = match first {
        0x00..=0x7f => return Step::Char(char::from(first), 1),
        0xc2..=0xdf => (2, 0x80..=0xbf),
        0xe0 => (3, 0xa0..=0xbf),
        0xed => (3, 0x80..=0x9f),
        0xe1..=0xef => (3, 0x80..=0xbf),
        0xf0 => (4, 0x90..=0xbf),
        0xf1..=0xf3 => (4, 0x80..=0xbf),
        0xf4 => (4, 0x80..=0x8f),
        _ => return Step::Bad(1),
    };
    for at in 1..len {
        let Some(&byte) = bytes.get(at) else {
            return Step::More;
        };
        let fits = if at == 1 {
            second.contains(&byte)
        } else {
            (0x80..=0xbf).contains(&byte)
        };
        if !fits {
            return Step::Bad(at);
        }
    }
    let code = bytes[1..len]
        .iter()
        .fold(u32::from(first) & (0x7f >> len), |code, &byte| {
            code << 6 | u32::from(byte & 0x3f)
        });
    char::from_u32(code).map_or(Step::Bad(len), |c| Step::Char(c, len))
}

/// The first character of `bytes` in UTF-16 of `order`, a surrogate only
/// in a pair.
fn utf16(bytes: &[u8], order: Order) -> Step {
    let Some(first) = bytes.get(..2) else {
        return Step::More;
    };
    let high = order.read(first);
    match high {
        0xd800..=0xdbff => {
            let Some(second) = bytes.get(2..4) else {
                return Step::More;
            };
            let low = order.read(second);
            if !(0xdc00..=0xdfff).contains(&low) {
                return Step::Bad(2);
            }
            let code = 0x10000 + ((high - 0xd800) << 10 | (low - 0xdc00));
            char::from_u32(code).map_or(Step::Bad(4), |c| Step::Char(c, 4))
        }
        0xdc00..=0xdfff => Step::Bad(2),
        _ => char::from_u32(high).map_or(Step::Bad(2), |c| Step::Char(c, 2)),
    }
}

/// The first character of `bytes` in UTF-32 of `order`.
fn utf32(bytes: &[u8], order: Order) -> Step {
    let Some(unit) = bytes.get(..4) else {
        return Step::More;
    };
    char::from_u32(order.read(unit)).map_or(Step::Bad(4), |c| Step::Char(c, 4))
}

/// What writes characters in a set.
#[derive(Clone, Copy, Debug)]
pub struct Encoder {
    set: Charset,
    /// Whether a character was written, before which UTF-16 and UTF-32
    /// write their mark.
    begun: bool,
}

impl Encoder {
    #[must_use]
    pub fn new(set: Charset) -> Encoder {
        Encoder { set, begun: false }
    }

    /// Writes `c` onto `out`, after the byte order mark that UTF-16 and
    /// UTF-32 with no order start with; false, with nothing written, when
    /// the set has no bytes for it.
    pub fn put(&mut self, c: char, out: &mut Vec<u8>) -> bool {
        let code = u32::from(c);
        let (width, order) = match self.set {
            Charset::Utf16 | Charset::Utf16Le => (2, Order::Little),
            Charset::Utf16Be => (2, Order::Big),
            Charset::Utf32 | Charset::Utf32Le => (4, Order::Little),
            Charset::Utf32Be => (4, Order::Big),
            _ => (0, Order::Little),
        };
        let mut room = [0u8; 4];
        match self.set {
            Charset::Utf8 => out.extend_from_slice(c.encode_utf8(&mut room).as_bytes()),
            _ if width > 0 => {
                if !self.begun && matches!(self.set, Charset::Utf16 | Charset::Utf32) {
                    order.write(0xfeff, width, out);
                }
                if width == 4 {
                    order.write(code, 4, out);
                } else {
                    let mut units = [0u16; 2];
                    for &unit in c.encode_utf16(&mut units).iter() {
                        order.write(u32::from(unit), 2, out);
                    }
                }
            }
            _ => match byte_for(self.set, code) {
                Some(byte) => out.push(byte),
                None => return false,
            },
        }
        self.begun = true;
        true
    }
}

/// The byte a set of single bytes writes the character `code` as.
fn byte_for(set: Charset, code: u32) -> Option<u8> {
    let byte = u8::try_from(code).ok();
    match set {
        Charset::Ascii => byte.filter(u8::is_ascii),
        Charset::Latin1 => byte,
        Charset::Windows1252 if byte.is_some_and(|b| !(0x80..=0x9f).contains(&b)) => byte,
        Charset::Ibm437 if code < 0x80 => byte,
        _ => {
            let table: &[u16] = if set == Charset::Windows1252 {
                &WINDOWS_1252
            } else {
                &IBM_437
            };
            let at = table.iter().position(|&known| u32::from(known) == code)?;
            u8::try_from(0x80 + at).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    /// What glibc's iconv writes for `input` from `from` to `to`, leaving
    /// out what does not convert when `skip`, and what it says.
    fn iconv(from: &str, to: &str, input: &[u8], skip: bool) -> (Vec<u8>, String) {
        let mut words = vec!["-f", from, "-t", to];
        if skip {
            words.push("-c");
        }
        let mut child = Command::new("iconv")
            .args(&words)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let done = child.wait_with_output().unwrap();
        (done.stdout, String::from_utf8(done.stderr).unwrap())
    }

    /// Reads `bytes` whole in `set`: the characters, then where the first
    /// bytes that are none start, or that it ends inside a character.
    fn read_all(set: Charset, bytes: &[u8]) -> (Vec<char>, Option<Result<usize, ()>>) {
        let mut decoder = Decoder::new(set);
        let (mut chars, mut at) = (Vec::new(), 0);
        while at < bytes.len() {
            match decoder.step(&bytes[at..]) {
                Step::Char(c, len) => {
                    chars.push(c);
                    at += len;
                }
                Step::Mark(len) => at += len,
                Step::Bad(_) => return (chars, Some(Ok(at))),
                Step::More => return (chars, Some(Err(()))),
            }
        }
        (chars, None)
    }

    fn chars_of(utf32be: &[u8]) -> Vec<char> {
        utf32be
            .chunks(4)
            .map(|unit| char::from_u32(u32::from_be_bytes(unit.try_into().unwrap())).unwrap())
            .collect()
    }

    /// Where glibc stopped, as [`read_all`] says it.
    fn fault(said: &str) -> Option<Result<usize, ()>> {
        if said.is_empty() {
            return None;
        }
        if said.contains("incomplete character") {
            return Some(Err(()));
        }
        let at = said.trim_end().rsplit(' ').next().unwrap();
        Some(Ok(at.parse().unwrap()))
    }

    #[test]
    fn sets_go_by_their_names_in_capitals_or_small() {
        assert_eq!(Charset::named("utf-8"), Some(Charset::Utf8));
        assert_eq!(Charset::named("Latin1"), Some(Charset::Latin1));
        assert_eq!(Charset::named("cp437"), Some(Charset::Ibm437));
        assert_eq!(Charset::named("windows-1252"), Some(Charset::Windows1252));
        assert_eq!(Charset::named("UTF-7"), None);
        assert_eq!(Charset::named(""), None);
        let mut seen = Vec::new();
        for set in Charset::ALL {
            for name in set.names() {
                assert_eq!(Charset::named(name), Some(set), "{name}");
                assert!(!seen.contains(name), "{name} names one set");
                seen.push(name);
                let (_, said) = iconv(name, "UTF-8", b"", false);
                assert_eq!(said, "", "glibc's iconv knows {name}");
            }
        }
    }

    #[test]
    fn every_byte_of_the_sets_of_bytes_is_what_glibc_reads() {
        for set in [
            Charset::Ascii,
            Charset::Latin1,
            Charset::Windows1252,
            Charset::Ibm437,
        ] {
            // Each byte on a line of its own; with -c what is none leaves
            // its line empty. The line feed is a line feed in every set.
            let input: Vec<u8> = (0..=255u8)
                .filter(|&b| b != b'\n')
                .flat_map(|b| [b, b'\n'])
                .collect();
            let (theirs, _) = iconv(set.names()[0], "UTF-32BE", &input, true);
            let theirs = chars_of(&theirs);
            let mut lines = theirs.split(|&c| c == '\n');
            for byte in (0..=255u8).filter(|&b| b != b'\n') {
                let line = lines.next().unwrap();
                let ours = match Decoder::new(set).step(&[byte]) {
                    Step::Char(c, 1) => Some(c),
                    Step::Bad(1) => None,
                    other => panic!("{other:?}"),
                };
                assert_eq!(ours, line.first().copied(), "{set:?} {byte:#04x}");
                assert!(line.len() <= 1);
            }
            assert_eq!(Decoder::new(set).step(b"\n"), Step::Char('\n', 1));
        }
    }

    #[test]
    fn what_the_sets_of_bytes_write_is_what_glibc_writes() {
        for set in [
            Charset::Ascii,
            Charset::Latin1,
            Charset::Windows1252,
            Charset::Ibm437,
        ] {
            let chars: Vec<char> = (0..0x3000)
                .filter_map(char::from_u32)
                .filter(|&c| c != '\n')
                .collect();
            let input: Vec<u8> = chars
                .iter()
                .flat_map(|&c| [u32::from(c), 10])
                .flat_map(u32::to_be_bytes)
                .collect();
            let (theirs, _) = iconv("UTF-32BE", set.names()[0], &input, true);
            let mut lines = theirs.split(|&b| b == b'\n');
            for &c in &chars {
                let line = lines.next().unwrap();
                let mut ours = Vec::new();
                let put = Encoder::new(set).put(c, &mut ours);
                assert_eq!(
                    (put, ours.as_slice()),
                    (!line.is_empty(), line),
                    "{set:?} {c:?}"
                );
            }
        }
    }

    #[test]
    fn bytes_that_are_none_go_as_far_as_they_could_start_a_character() {
        let utf8 = |bytes: &[u8]| Decoder::new(Charset::Utf8).step(bytes);
        for (bytes, step) in [
            (&b""[..], Step::More),
            (b"a", Step::Char('a', 1)),
            (b"\xc3\xa9", Step::Char('\u{e9}', 2)),
            (b"\xe2\x82\xac", Step::Char('\u{20ac}', 3)),
            (b"\xf0\x9f\x98\x80", Step::Char('\u{1f600}', 4)),
            (b"\xf4\x8f\xbf\xbf", Step::Char('\u{10ffff}', 4)),
            (b"\xe2\x82", Step::More),
            (b"\xe2\x82A", Step::Bad(2)),
            (b"\xf0\x80", Step::Bad(1)),
            (b"\xed\xa0\x80", Step::Bad(1)),
            (b"\xc0\xaf", Step::Bad(1)),
            (b"\xc1\xbf", Step::Bad(1)),
            (b"\xf4\x90\x80\x80", Step::Bad(1)),
            (b"\xf5", Step::Bad(1)),
            (b"\x80", Step::Bad(1)),
            (b"\xff", Step::Bad(1)),
            (b"\xf0\x9f\x98A", Step::Bad(3)),
        ] {
            assert_eq!(utf8(bytes), step, "{bytes:x?}");
        }
        let utf16 = |bytes: &[u8]| Decoder::new(Charset::Utf16Be).step(bytes);
        assert_eq!(utf16(b"\xd8\x3d\xde\x00"), Step::Char('\u{1f600}', 4));
        assert_eq!(
            utf16(b"\xd8\x3d\x00\x61"),
            Step::Bad(2),
            "a high surrogate alone"
        );
        assert_eq!(utf16(b"\xde\x00"), Step::Bad(2), "a low surrogate alone");
        assert_eq!(utf16(b"\xd8\x3d"), Step::More);
        assert_eq!(utf16(b"\x00"), Step::More);
        let utf32 = |bytes: &[u8]| Decoder::new(Charset::Utf32Le).step(bytes);
        assert_eq!(utf32(b"\x00\xd8\x00\x00"), Step::Bad(4), "a surrogate");
        assert_eq!(utf32(b"\x00\x00\x11\x00"), Step::Bad(4), "past U+10FFFF");
        assert_eq!(utf32(b"a\x00\x00"), Step::More);
    }

    #[test]
    fn a_mark_says_the_order_of_utf_16_and_utf_32_named_with_none() {
        let read = |set, bytes: &[u8]| read_all(set, bytes);
        assert_eq!(read(Charset::Utf16, b"\xfe\xff\x00a"), (vec!['a'], None));
        assert_eq!(read(Charset::Utf16, b"\xff\xfea\x00"), (vec!['a'], None));
        assert_eq!(
            read(Charset::Utf16, b"a\x00"),
            (vec!['a'], None),
            "the machine's order"
        );
        assert_eq!(
            read(Charset::Utf16, b"\xfe\xff\x00a\xfe\xff"),
            (vec!['a', '\u{feff}'], None),
            "a mark later is a character"
        );
        assert_eq!(
            read(Charset::Utf16Le, b"\xff\xfea\x00"),
            (vec!['\u{feff}', 'a'], None),
            "a set with an order takes the mark for a character"
        );
        assert_eq!(
            read(Charset::Utf32, b"\x00\x00\xfe\xff\x00\x00\x00a"),
            (vec!['a'], None)
        );
        assert_eq!(
            read(Charset::Utf32, b"\xff\xfe\x00\x00a\x00\x00\x00"),
            (vec!['a'], None)
        );
        assert_eq!(
            read(Charset::Utf16, b"\xfe\xff\xfe\xff\x00a"),
            (vec!['\u{feff}', 'a'], None),
            "a mark right after the mark is a character"
        );
        let mut decoder = Decoder::new(Charset::Utf16);
        assert_eq!(decoder.step(b"\xfe"), Step::More, "half a mark");
        assert_eq!(
            decoder.step(b"\xfe\xff\x00a"),
            Step::Mark(2),
            "and then whole"
        );
        let mut out = Vec::new();
        let mut utf16 = Encoder::new(Charset::Utf16);
        assert!(utf16.put('a', &mut out) && utf16.put('\u{1f600}', &mut out));
        assert_eq!(
            out, b"\xff\xfea\x00\x3d\xd8\x00\xde",
            "a mark once, little-endian"
        );
        let mut out = Vec::new();
        assert!(Encoder::new(Charset::Utf16Be).put('\u{1f600}', &mut out));
        assert_eq!(out, b"\xd8\x3d\xde\x00");
        let mut out = Vec::new();
        let mut utf32 = Encoder::new(Charset::Utf32);
        assert!(utf32.put('a', &mut out) && utf32.put('b', &mut out));
        assert_eq!(out, b"\xff\xfe\x00\x00a\x00\x00\x00b\x00\x00\x00");
    }

    /// Random bytes, most of them characters in `set` and some none.
    fn random_bytes(set: Charset, seed: &mut u64) -> Vec<u8> {
        let mut next = |below: u64| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            *seed % below
        };
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(set);
        for _ in 0..next(40) {
            match next(10) {
                0 => bytes.push(u8::try_from(next(256)).unwrap()),
                1 => {
                    let surrogate = u32::try_from(0xd800 + next(0x800)).unwrap();
                    Order::Big.write(surrogate, 2, &mut bytes);
                }
                _ => {
                    let code = match next(4) {
                        0 => next(0x80),
                        1 => next(0x800),
                        2 => next(0x10000),
                        _ => next(0x11_0000),
                    };
                    if let Some(c) = char::from_u32(u32::try_from(code).unwrap()) {
                        encoder.put(c, &mut bytes);
                    }
                }
            }
        }
        bytes
    }

    #[test]
    fn utf_text_reads_and_stops_where_glibc_s_iconv_does() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for set in [
            Charset::Utf8,
            Charset::Utf16,
            Charset::Utf16Be,
            Charset::Utf16Le,
            Charset::Utf32,
            Charset::Utf32Be,
            Charset::Utf32Le,
        ] {
            for _ in 0..150 {
                let bytes = random_bytes(set, &mut seed);
                let (theirs, said) = iconv(set.names()[0], "UTF-32BE", &bytes, false);
                let (chars, stopped) = read_all(set, &bytes);
                assert_eq!(chars, chars_of(&theirs), "{set:?} {bytes:x?}");
                assert_eq!(stopped, fault(&said), "{set:?} {bytes:x?}: {said}");
            }
        }
    }

    #[test]
    fn utf_text_is_written_as_glibc_s_iconv_writes_it() {
        let mut seed = 0x6a09_e667_f3bc_c908_u64;
        for set in [
            Charset::Utf8,
            Charset::Utf16,
            Charset::Utf16Be,
            Charset::Utf16Le,
            Charset::Utf32,
            Charset::Utf32Be,
            Charset::Utf32Le,
        ] {
            for _ in 0..60 {
                let (chars, _) = read_all(Charset::Utf8, &random_bytes(Charset::Utf8, &mut seed));
                let mut ours = Vec::new();
                let mut encoder = Encoder::new(set);
                for &c in &chars {
                    assert!(encoder.put(c, &mut ours));
                }
                let input: Vec<u8> = chars
                    .iter()
                    .flat_map(|&c| u32::from(c).to_be_bytes())
                    .collect();
                let (theirs, said) = iconv("UTF-32BE", set.names()[0], &input, false);
                assert_eq!((ours, said.as_str()), (theirs, ""), "{set:?} {chars:?}");
            }
        }
    }

    #[test]
    fn units_are_read_and_written_in_either_order() {
        assert_eq!(Order::Big.read(&[0x12, 0x34]), 0x1234);
        assert_eq!(Order::Little.read(&[0x12, 0x34]), 0x3412);
        assert_eq!(Order::Big.read(&[1, 2, 3, 4]), 0x0102_0304);
        assert_eq!(Order::Little.read(&[1, 2, 3, 4]), 0x0403_0201);
        let mut out = Vec::new();
        Order::Big.write(0x0102_0304, 4, &mut out);
        Order::Little.write(0x0102_0304, 4, &mut out);
        Order::Big.write(0xabcd, 2, &mut out);
        Order::Little.write(0xabcd, 2, &mut out);
        assert_eq!(out, [1, 2, 3, 4, 4, 3, 2, 1, 0xab, 0xcd, 0xcd, 0xab]);
    }

    #[test]
    fn every_pair_of_bytes_reads_as_rust_reads_utf_8() {
        for first in 0..=255u8 {
            for second in 0..=255u8 {
                for third in [0x41, 0x80, 0xbf, 0xc0] {
                    for fourth in [0x80, 0x41] {
                        let bytes = [first, second, third, fourth];
                        let step = Decoder::new(Charset::Utf8).step(&bytes);
                        // What Rust's own UTF-8 makes of the first character.
                        let theirs = (1..=4).find_map(|len| {
                            core::str::from_utf8(&bytes[..len])
                                .ok()
                                .map(|text| (text.chars().next().unwrap(), len))
                        });
                        match step {
                            Step::Char(c, len) => assert_eq!(Some((c, len)), theirs, "{bytes:x?}"),
                            Step::Bad(len) => {
                                assert!(theirs.is_none(), "{bytes:x?}");
                                let valid = core::str::from_utf8(&bytes).unwrap_err().error_len();
                                assert_eq!(Some(len), valid, "{bytes:x?}");
                            }
                            other => panic!("{bytes:x?}: {other:?}"),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_character_goes_through_utf_16_and_utf_32_and_back() {
        for code in (0..=0x10_ffffu32)
            .step_by(7)
            .chain([0xd7ff, 0xe000, 0xffff, 0x1_0000, 0x10_ffff])
        {
            let Some(c) = char::from_u32(code) else {
                continue;
            };
            for set in [
                Charset::Utf8,
                Charset::Utf16Be,
                Charset::Utf16Le,
                Charset::Utf32Be,
                Charset::Utf32Le,
            ] {
                let mut bytes = Vec::new();
                assert!(Encoder::new(set).put(c, &mut bytes), "{set:?} {c:?}");
                assert_eq!(read_all(set, &bytes), (vec![c], None), "{set:?} {c:?}");
            }
            let mut be = Vec::new();
            Encoder::new(Charset::Utf16Be).put(c, &mut be);
            let mut units = [0u16; 2];
            let theirs: Vec<u8> = c
                .encode_utf16(&mut units)
                .iter()
                .flat_map(|u| u.to_be_bytes())
                .collect();
            assert_eq!(be, theirs, "Rust's own UTF-16 for {c:?}");
        }
        for unit in 0..=0xffffu32 {
            let step =
                Decoder::new(Charset::Utf16Be).step(&u16::try_from(unit).unwrap().to_be_bytes());
            let want = match unit {
                0xd800..=0xdbff => Step::More,
                0xdc00..=0xdfff => Step::Bad(2),
                _ => Step::Char(char::from_u32(unit).unwrap(), 2),
            };
            assert_eq!(step, want, "{unit:#06x}");
        }
    }

    #[test]
    fn the_sets_of_bytes_read_back_what_they_write() {
        for set in [
            Charset::Ascii,
            Charset::Latin1,
            Charset::Windows1252,
            Charset::Ibm437,
        ] {
            let mut read = 0;
            for byte in 0..=255u8 {
                if let Step::Char(c, 1) = Decoder::new(set).step(&[byte]) {
                    read += 1;
                    let mut out = Vec::new();
                    assert!(Encoder::new(set).put(c, &mut out), "{set:?} {c:?}");
                    assert_eq!(out, [byte], "{set:?} {c:?}");
                }
            }
            let expected = match set {
                Charset::Ascii => 128,
                Charset::Windows1252 => 251,
                _ => 256,
            };
            assert_eq!(read, expected, "{set:?} reads that many bytes");
            for code in 0..0x3000 {
                let Some(c) = char::from_u32(code) else {
                    continue;
                };
                let mut out = Vec::new();
                if Encoder::new(set).put(c, &mut out) {
                    assert_eq!(
                        Decoder::new(set).step(&out),
                        Step::Char(c, 1),
                        "{set:?} {c:?}"
                    );
                } else {
                    assert!(out.is_empty(), "{set:?} {c:?}: nothing written");
                }
            }
        }
    }

    #[test]
    fn the_code_pages_hold_the_characters_microsoft_gave_them() {
        let read = |set, byte| Decoder::new(set).step(&[byte]);
        let written = |set, c| {
            let mut out = Vec::new();
            Encoder::new(set).put(c, &mut out).then_some(out)
        };
        assert_eq!(read(Charset::Windows1252, 0x80), Step::Char('\u{20ac}', 1));
        assert_eq!(written(Charset::Windows1252, '\u{20ac}'), Some(vec![0x80]));
        for undefined in [0x81, 0x8d, 0x8f, 0x90, 0x9d] {
            assert_eq!(
                read(Charset::Windows1252, undefined),
                Step::Bad(1),
                "{undefined:#x}"
            );
        }
        assert_eq!(
            written(Charset::Windows1252, '\u{81}'),
            None,
            "C1 controls have no bytes"
        );
        assert_eq!(read(Charset::Windows1252, 0xe9), Step::Char('\u{e9}', 1));
        assert_eq!(
            read(Charset::Ibm437, 0xc4),
            Step::Char('\u{2500}', 1),
            "a line to draw boxes"
        );
        assert_eq!(written(Charset::Ibm437, '\u{2500}'), Some(vec![0xc4]));
        assert_eq!(read(Charset::Ibm437, 0x81), Step::Char('\u{fc}', 1));
        assert_eq!(read(Charset::Ibm437, 0xff), Step::Char('\u{a0}', 1));
        assert_eq!(
            read(Charset::Ibm437, 0x01),
            Step::Char('\u{1}', 1),
            "a control, as Unicode's table has it"
        );
        assert_eq!(written(Charset::Ibm437, '\u{e9}'), Some(vec![0x82]));
        assert_eq!(written(Charset::Ibm437, '\u{20ac}'), None);
        assert_eq!(written(Charset::Ibm437, '\u{80}'), None, "nor a C1 control");
        assert_eq!(written(Charset::Ibm437, '\u{7f}'), Some(vec![0x7f]));
        assert_eq!(read(Charset::Ascii, 0x80), Step::Bad(1));
        assert_eq!(written(Charset::Ascii, '\u{e9}'), None);
        assert_eq!(written(Charset::Latin1, '\u{ff}'), Some(vec![0xff]));
        assert_eq!(written(Charset::Latin1, '\u{100}'), None);
        assert_eq!(read(Charset::Latin1, 0x00), Step::Char('\0', 1));
    }
}
