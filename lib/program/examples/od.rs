//! od: writes what files hold as numbers and characters, as POSIX's od
//! does. Each line takes sixteen bytes and starts with where they are,
//! once for each type `-t` or the letters `-b`, `-c`, `-d`, `-o`, `-s`
//! and `-x` ask for, or as two-byte octal words when none is asked.
//! `-A` names the base of the addresses, or `n` for none, `-j` skips
//! bytes first and `-N` stops after so many. A line the same as the one
//! before is a `*`, unless `-v` asks for every line. The files are read
//! as one text, `-` is the standard input, and with none it is the
//! standard input alone. Long doubles are not read.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

use program::Handle;
use program::text::{self, Pieces, Source};

program::main!(od);

/// How many bytes a line takes.
const LINE: usize = 16;

/// The most bytes a character after the first of it takes.
const MOST_AFTER: usize = 3;

/// How a value is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A character by its name, and only its low seven bits.
    Named,
    /// A character as it is, by its escape or as an octal number.
    Character,
    Number(Number),
}

/// How a number is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Number {
    Signed,
    Unsigned,
    Octal,
    Hex,
    Float,
}

/// One type asked for: how its values are written and how many bytes
/// each takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Type {
    kind: Kind,
    size: usize,
}

impl Type {
    const fn new(kind: Kind, size: usize) -> Type {
        Type { kind, size }
    }

    /// How wide its widest value is.
    fn width(self) -> usize {
        let bits = 8 * self.size;
        match self.kind {
            Kind::Named | Kind::Character => 3,
            Kind::Number(Number::Signed) => (i64::MIN >> (64 - bits)).to_string().len(),
            Kind::Number(Number::Unsigned) => (u64::MAX >> (64 - bits)).to_string().len(),
            Kind::Number(Number::Octal) => bits.div_ceil(3),
            Kind::Number(Number::Hex) => bits / 4,
            // A sign, nine digits, a point and e-38.
            Kind::Number(Number::Float) if self.size == 4 => 15,
            // A sign, seventeen digits, a point and e-308.
            Kind::Number(Number::Float) => 24,
        }
    }
}

/// The base addresses are written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    Octal,
    Decimal,
    Hex,
}

/// Where a line starts, as it is written in `base`.
fn address(base: Base, at: u64) -> String {
    match base {
        Base::Octal => format!("{at:07o}"),
        Base::Decimal => format!("{at:07}"),
        Base::Hex => format!("{at:06x}"),
    }
}

/// What od's words ask for.
#[derive(Debug, PartialEq)]
struct Asked<'a> {
    types: Vec<Type>,
    /// None when no addresses are written.
    base: Option<Base>,
    skip: u64,
    count: Option<u64>,
    every: bool,
    files: &'a [String],
}

/// What od's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        types: Vec::new(),
        base: Some(Base::Octal),
        skip: 0,
        count: None,
        every: false,
        files: &[],
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
            let short = match letter {
                'b' => Type::new(Kind::Number(Number::Octal), 1),
                'c' => Type::new(Kind::Character, 1),
                'd' => Type::new(Kind::Number(Number::Unsigned), 2),
                'o' => Type::new(Kind::Number(Number::Octal), 2),
                's' => Type::new(Kind::Number(Number::Signed), 2),
                'x' => Type::new(Kind::Number(Number::Hex), 2),
                'v' => {
                    asked.every = true;
                    continue;
                }
                'A' | 'j' | 'N' | 't' => {
                    // These take the rest of the word, or else the next.
                    let after = &letters[at + 1..];
                    let value = if after.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("od: -{letter} wants its word\n"))?;
                        words = rest;
                        value.as_str()
                    } else {
                        after
                    };
                    let number = || {
                        bytes(value)
                            .ok_or_else(|| format!("od: -{letter} wants a number, not {value}\n"))
                    };
                    match letter {
                        'A' => asked.base = base(value)?,
                        'j' => asked.skip = number()?,
                        'N' => asked.count = Some(number()?),
                        _ => asked.types.extend(types(value)?),
                    }
                    break;
                }
                _ => return Err(format!("od: -{letter} is no option\n")),
            };
            asked.types.push(short);
        }
    }
    if asked.types.is_empty() {
        asked.types.push(Type::new(Kind::Number(Number::Octal), 2));
    }
    asked.files = words;
    Ok(asked)
}

/// The base `-A` names, None for `n`.
fn base(value: &str) -> Result<Option<Base>, String> {
    match value {
        "o" => Ok(Some(Base::Octal)),
        "d" => Ok(Some(Base::Decimal)),
        "x" => Ok(Some(Base::Hex)),
        "n" => Ok(None),
        _ => Err(format!("od: {value} is no base of addresses\n")),
    }
}

/// A number of bytes as `-j` and `-N` take it: decimal, octal after a 0
/// or hexadecimal after 0x, and unless hexadecimal times 512, 1024 or
/// 1048576 with a b, k or m after it.
fn bytes(text: &str) -> Option<u64> {
    let (digits, radix, unit) =
        if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (hex, 16, 1)
        } else {
            let (digits, unit) = match text.as_bytes().last() {
                Some(b'b') => (&text[..text.len() - 1], 512),
                Some(b'k') => (&text[..text.len() - 1], 1024),
                Some(b'm') => (&text[..text.len() - 1], 1 << 20),
                _ => (text, 1),
            };
            let radix = if digits.len() > 1 && digits.starts_with('0') {
                8
            } else {
                10
            };
            (digits, radix, unit)
        };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u64::from_str_radix(digits, radix).ok()?.checked_mul(unit)
}

/// The types a `-t` word names one after another, each a letter and for
/// numbers how many bytes they take: 1, 2, 4 or 8, or C, S, I and L for
/// C's char, short, int and long, and for floats 4 or 8, or F and D for
/// C's float and double.
fn types(text: &str) -> Result<Vec<Type>, String> {
    if text.is_empty() {
        return Err(String::from("od: -t wants a type\n"));
    }
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(letter) = rest.chars().next() {
        rest = &rest[letter.len_utf8()..];
        let number = match letter {
            'a' => {
                found.push(Type::new(Kind::Named, 1));
                continue;
            }
            'c' => {
                found.push(Type::new(Kind::Character, 1));
                continue;
            }
            'd' => Number::Signed,
            'u' => Number::Unsigned,
            'o' => Number::Octal,
            'x' => Number::Hex,
            'f' => Number::Float,
            _ => return Err(format!("od: {letter} is no type\n")),
        };
        let float = number == Number::Float;
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let size = if digits > 0 {
            let (size, after) = rest.split_at(digits);
            rest = after;
            let fits = if float {
                matches!(size, "4" | "8")
            } else {
                matches!(size, "1" | "2" | "4" | "8")
            };
            if !fits {
                return Err(if float {
                    format!("od: floats are 4 or 8 bytes, not {size}\n")
                } else {
                    format!("od: numbers are 1, 2, 4 or 8 bytes, not {size}\n")
                });
            }
            usize::from(size.as_bytes()[0] - b'0')
        } else {
            let named = match (float, rest.chars().next()) {
                (true, Some('L')) => return Err(String::from("od: long doubles are not read\n")),
                (false, Some('C')) => Some(1),
                (false, Some('S')) => Some(2),
                (true, Some('F')) | (false, Some('I')) => Some(4),
                (true, Some('D')) | (false, Some('L')) => Some(8),
                _ => None,
            };
            if named.is_some() {
                rest = &rest[1..];
            }
            named.unwrap_or(if float { 8 } else { 4 })
        };
        found.push(Type::new(Kind::Number(number), size));
    }
    Ok(found)
}

/// The names `-t a` writes for the characters that have no picture.
const NAMES: [&str; 33] = [
    "nul", "soh", "stx", "etx", "eot", "enq", "ack", "bel", "bs", "ht", "nl", "vt", "ff", "cr",
    "so", "si", "dle", "dc1", "dc2", "dc3", "dc4", "nak", "syn", "etb", "can", "em", "sub", "esc",
    "fs", "gs", "rs", "us", "sp",
];

/// What `-t a` writes for `byte`.
fn named(byte: u8) -> String {
    match byte & 0x7f {
        low if usize::from(low) < NAMES.len() => String::from(NAMES[usize::from(low)]),
        0x7f => String::from("del"),
        low => char::from(low).to_string(),
    }
}

/// What `-t c` writes for each byte of `line`, `after` being the bytes
/// that follow it and the first `carried` of it ending a character the
/// line before began; and how many bytes of the next line end one this
/// line begins. A character of more than one byte is written at its
/// first, and its others are `**`.
fn characters(line: &[u8], after: &[u8], carried: usize) -> (Vec<String>, usize) {
    let mut written: Vec<String> = Vec::new();
    let mut at = carried.min(line.len());
    written.resize(at, String::from("**"));
    let mut spilled = carried - at;
    while at < line.len() {
        let byte = line[at];
        let escape = match byte {
            0 => Some("\\0"),
            7 => Some("\\a"),
            8 => Some("\\b"),
            b'\t' => Some("\\t"),
            b'\n' => Some("\\n"),
            11 => Some("\\v"),
            12 => Some("\\f"),
            b'\r' => Some("\\r"),
            _ => None,
        };
        if let Some(escape) = escape {
            written.push(String::from(escape));
        } else if (b' '..=b'~').contains(&byte) {
            written.push(char::from(byte).to_string());
        } else if let Some(c) = character(&line[at..], after) {
            let len = c.len_utf8();
            written.push(c.to_string());
            let here = len.min(line.len() - at);
            written.extend((1..here).map(|_| String::from("**")));
            spilled = len - here;
            at += here;
            continue;
        } else {
            written.push(format!("{byte:03o}"));
        }
        at += 1;
    }
    (written, spilled)
}

/// The character of more than one byte `here` starts, reading on into
/// `after`, when it is a whole one with a picture.
fn character(here: &[u8], after: &[u8]) -> Option<char> {
    let len = match here.first()? {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let whole: Vec<u8> = here.iter().chain(after).take(len).copied().collect();
    let c = core::str::from_utf8(&whole).ok()?.chars().next()?;
    (!c.is_control()).then_some(c)
}

/// A float od writes.
trait Float: Copy + PartialEq + core::fmt::LowerExp + core::str::FromStr {
    /// The digits any value has right, and the most any needs.
    const DIGITS: usize;
    const MOST: usize;

    fn too_small_to_be_normal(self) -> bool;
    fn is_nan(self) -> bool;
    fn is_infinite(self) -> bool;
    fn is_sign_negative(self) -> bool;
}

impl Float for f32 {
    const DIGITS: usize = f32::DIGITS as usize;
    const MOST: usize = 9;

    fn too_small_to_be_normal(self) -> bool {
        self.abs() < f32::MIN_POSITIVE
    }
    fn is_nan(self) -> bool {
        self.is_nan()
    }
    fn is_infinite(self) -> bool {
        self.is_infinite()
    }
    fn is_sign_negative(self) -> bool {
        self.is_sign_negative()
    }
}

impl Float for f64 {
    const DIGITS: usize = f64::DIGITS as usize;
    const MOST: usize = 17;

    fn too_small_to_be_normal(self) -> bool {
        self.abs() < f64::MIN_POSITIVE
    }
    fn is_nan(self) -> bool {
        self.is_nan()
    }
    fn is_infinite(self) -> bool {
        self.is_infinite()
    }
    fn is_sign_negative(self) -> bool {
        self.is_sign_negative()
    }
}

/// `value` as C's `%g` writes it with the fewest digits that read back
/// as the same value, but never fewer than the digits any value of its
/// type has right, unless it is too small to be normal.
fn float<F: Float>(value: F) -> String {
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value.is_nan() {
        return format!("{sign}nan");
    }
    if value.is_infinite() {
        return format!("{sign}inf");
    }
    let least = if value.too_small_to_be_normal() {
        1
    } else {
        F::DIGITS
    };
    let digits = (least..F::MOST)
        .find(|&digits| {
            format!("{:.*e}", digits - 1, value)
                .parse::<F>()
                .is_ok_and(|back| back == value)
        })
        .unwrap_or(F::MOST);
    like_c(&format!("{:.*e}", digits - 1, value), digits)
}

/// What C's `%.{digits}g` writes for the value Rust's `{:e}` wrote as
/// `text` with as many digits: in the style of `%e` when its exponent is
/// below -4 or not below `digits`, else without one, and either way
/// without zeros at the end of its fraction.
fn like_c(text: &str, digits: usize) -> String {
    let (mantissa, exponent) = text.split_once('e').unwrap_or((text, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |m| ("-", m));
    let figures: String = mantissa.chars().filter(|&c| c != '.').collect();
    let with_point = |whole: &str, fraction: &str| {
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            format!("{sign}{whole}")
        } else {
            format!("{sign}{whole}.{fraction}")
        }
    };
    let below = usize::try_from(exponent.unsigned_abs()).unwrap_or(usize::MAX);
    if exponent < -4 || exponent >= i32::try_from(digits).unwrap_or(i32::MAX) {
        let (first, fraction) = figures.split_at(1);
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{exponent_sign}{below:02}", with_point(first, fraction))
    } else if exponent >= 0 {
        let (whole, fraction) = figures.split_at(below + 1);
        with_point(whole, fraction)
    } else {
        let zeros = "0".repeat(below - 1);
        with_point("0", &format!("{zeros}{figures}"))
    }
}

/// The number `bytes` hold, `size` of them or fewer at the end of the
/// text with the rest taken as zeros, as `how` writes it.
fn value(how: Number, size: usize, bytes: &[u8]) -> String {
    let mut whole = [0u8; 8];
    whole[..bytes.len()].copy_from_slice(bytes);
    let number = u64::from_le_bytes(whole);
    let unused = 64 - 8 * size;
    let width = Type::new(Kind::Number(how), size).width();
    match how {
        Number::Signed => (number.cast_signed() << unused >> unused).to_string(),
        Number::Unsigned => number.to_string(),
        Number::Octal => format!("{number:0width$o}"),
        Number::Hex => format!("{number:0width$x}"),
        Number::Float if size == 4 => {
            let [a, b, c, d, ..] = whole;
            float(f32::from_le_bytes([a, b, c, d]))
        }
        Number::Float => float(f64::from_bits(number)),
    }
}

/// How wide each field of each type is on a line of sixteen bytes, the
/// space before it counted. Each type's line is as wide as the widest,
/// what a type lacks given to its fields from the last on.
fn widths(types: &[Type]) -> Vec<Vec<usize>> {
    let natural = |t: &Type| LINE / t.size * (t.width() + 1);
    let widest = types.iter().map(natural).max().unwrap_or(0);
    types
        .iter()
        .map(|t| {
            let fields = LINE / t.size;
            let lacking = widest - natural(t);
            let share = |i: usize| lacking * (fields - i) / fields;
            (0..fields)
                .map(|i| t.width() + 1 + share(i) - share(i + 1))
                .collect()
        })
        .collect()
}

/// Writes the text a line of sixteen bytes at a time as they come, once
/// for each type.
struct Dump {
    types: Vec<Type>,
    widths: Vec<Vec<usize>>,
    base: Option<Base>,
    every: bool,
    /// Where the next line starts.
    at: u64,
    /// The bytes not written yet, and how many of the first of them end a
    /// character the line before began.
    held: Vec<u8>,
    carried: usize,
    /// The last line written, and whether the lines since were the same.
    last: Option<Vec<u8>>,
    starred: bool,
}

impl Dump {
    fn new(asked: &Asked) -> Dump {
        Dump {
            widths: widths(&asked.types),
            types: asked.types.clone(),
            base: asked.base,
            every: asked.every,
            at: asked.skip,
            held: Vec::new(),
            carried: 0,
            last: None,
            starred: false,
        }
    }

    /// Takes the next `bytes` of the text, and writes onto `out` each line
    /// once the bytes after it that a character at its end may need are
    /// there too.
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        self.held.extend_from_slice(bytes);
        while self.held.len() >= LINE + MOST_AFTER {
            self.line(LINE, out);
        }
    }

    /// Writes what is left of the text, and then where it ended.
    fn end(mut self, out: &mut Vec<u8>) {
        while !self.held.is_empty() {
            self.line(self.held.len().min(LINE), out);
        }
        if let Some(base) = self.base {
            out.extend_from_slice(address(base, self.at).as_bytes());
            out.push(b'\n');
        }
    }

    /// Writes the first `len` bytes held as a line, or a `*` for it when
    /// it is the same as the line before.
    fn line(&mut self, len: usize, out: &mut Vec<u8>) {
        let line: Vec<u8> = self.held.drain(..len).collect();
        let after = &self.held[..self.held.len().min(MOST_AFTER)];
        let (characters, spilled) = characters(&line, after, self.carried);
        self.carried = spilled;
        let at = self.at;
        self.at += len as u64;
        if !self.every && self.last.as_ref() == Some(&line) {
            if !self.starred {
                out.extend_from_slice(b"*\n");
            }
            self.starred = true;
            return;
        }
        self.starred = false;
        let place = self.base.map(|base| address(base, at)).unwrap_or_default();
        for (index, (t, widths)) in self.types.iter().zip(&self.widths).enumerate() {
            let mut row = if index == 0 {
                place.clone()
            } else {
                " ".repeat(place.len())
            };
            for (field, (bytes, &width)) in line.chunks(t.size).zip(widths).enumerate() {
                let written = match t.kind {
                    Kind::Named => named(bytes[0]),
                    Kind::Character => characters[field].clone(),
                    Kind::Number(how) => value(how, t.size, bytes),
                };
                let _ = write!(row, "{written:>width$}");
            }
            row.push('\n');
            out.extend_from_slice(row.as_bytes());
        }
        self.last = Some(line);
    }
}

/// What of the text `-j` and `-N` let through.
#[derive(Debug)]
struct Window {
    skip: u64,
    left: Option<u64>,
}

/// How many of `len` bytes are at most `most`.
fn at_most(most: u64, len: usize) -> usize {
    usize::try_from(most).map_or(len, |most| most.min(len))
}

impl Window {
    /// The part of `piece`, the next bytes of the text, let through.
    fn through<'p>(&mut self, piece: &'p [u8]) -> &'p [u8] {
        let skipped = at_most(self.skip, piece.len());
        self.skip -= skipped as u64;
        let piece = &piece[skipped..];
        let Some(left) = &mut self.left else {
            return piece;
        };
        let kept = at_most(*left, piece.len());
        *left -= kept as u64;
        &piece[..kept]
    }

    /// Whether the text needs no more reading: all it skips is skipped,
    /// and nothing more will be let through.
    fn shut(&self) -> bool {
        self.skip == 0 && self.left == Some(0)
    }
}

#[cfg_attr(test, allow(dead_code))]
fn od(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let input = [String::from("-")];
    let files = if asked.files.is_empty() {
        &input[..]
    } else {
        asked.files
    };
    let mut window = Window {
        skip: asked.skip,
        left: asked.count,
    };
    let mut dump = Dump::new(&asked);
    let mut failed = false;
    let mut out = text::standard_output();
    let mut lines = Vec::new();
    let mut piece = Vec::new();
    for path in files {
        if window.shut() {
            break;
        }
        let mut source = match Source::open(path) {
            Ok(source) => source,
            Err(e) => {
                let _ = program::write_error(&text::trouble("od", path, e));
                failed = true;
                continue;
            }
        };
        while !window.shut() {
            piece.clear();
            match source.more(&mut piece) {
                Ok(true) => dump.feed(window.through(&piece), &mut lines),
                Ok(false) => break,
                Err(e) => {
                    let _ = program::write_error(&text::trouble("od", path, e));
                    failed = true;
                    break;
                }
            }
            out.put(&lines);
            lines.clear();
        }
        source.close();
    }
    if window.skip > 0 {
        let _ = program::write_error("od: -j skips past the end of the input\n");
        return 1;
    }
    dump.end(&mut lines);
    out.put(&lines);
    u64::from(!out.finish() || failed)
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

    /// What od asked with `options` writes for `input`, read in pieces
    /// of `piece` bytes.
    fn dumped_in(options: &str, input: &[u8], piece: usize) -> String {
        let words = words(options);
        let asked = asked(&words).unwrap();
        let mut window = Window {
            skip: asked.skip,
            left: asked.count,
        };
        let mut dump = Dump::new(&asked);
        let mut out = Vec::new();
        for piece in input.chunks(piece) {
            if window.shut() {
                break;
            }
            dump.feed(window.through(piece), &mut out);
        }
        dump.end(&mut out);
        String::from_utf8(out).unwrap()
    }

    fn dumped(options: &str, input: &[u8]) -> String {
        dumped_in(options, input, program::abi::MESSAGE_BYTES)
    }

    fn wrong(options: &str) -> String {
        asked(&words(options)).unwrap_err()
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn two_byte_octal_words_by_default() {
        assert_eq!(dumped("", b"abc\n"), "0000000 061141 005143\n0000004\n");
        assert_eq!(dumped("", b""), "0000000\n");
    }

    #[test]
    fn characters_by_their_escapes_or_in_octal() {
        assert_eq!(
            dumped("-c", b"abc\n\0\x01\x7f\x80\x07\x0b\x1b\x1f ~\x08\x0c\r\t"),
            "0000000   a   b   c  \\n  \\0 001 177 200  \\a  \\v 033 037       ~  \\b  \\f\n\
             0000020  \\r  \\t\n\
             0000022\n"
        );
    }

    #[test]
    fn characters_by_their_names_from_the_low_seven_bits() {
        assert_eq!(
            dumped("-t a", b"abc\n\0\x01\x7f\x80 \xff\xa0"),
            "0000000   a   b   c  nl nul soh del nul  sp del  sp\n0000013\n"
        );
        let all: Vec<u8> = (0..=0x7f).collect();
        let names: Vec<String> = all.iter().map(|&b| named(b)).collect();
        assert_eq!(names[10], "nl");
        assert_eq!(names[31], "us");
        assert_eq!(names[32], "sp");
        assert_eq!(names[33], "!");
        assert_eq!(names[126], "~");
        assert_eq!(names[127], "del");
        assert!(names.iter().all(|n| (1..=3).contains(&n.len())));
    }

    #[test]
    fn every_character_without_a_picture_has_its_name() {
        let low: Vec<u8> = (0..32).collect();
        assert_eq!(
            dumped("-A n -t a", &low),
            " nul soh stx etx eot enq ack bel  bs  ht  nl  vt  ff  cr  so  si\n \
             dle dc1 dc2 dc3 dc4 nak syn etb can  em sub esc  fs  gs  rs  us\n"
        );
        let high: Vec<u8> = (0x80..=0xff).collect();
        let named_low: Vec<u8> = (0..0x80).collect();
        assert_eq!(
            dumped("-A n -t a", &high),
            dumped("-A n -t a", &named_low),
            "only the low seven bits name a byte"
        );
    }

    #[test]
    fn bytes_past_ascii_that_start_no_character_are_octal() {
        assert_eq!(
            dumped("-c", b"\x80\xff\xc2\xe0"),
            "0000000 200 377 302 340\n0000004\n"
        );
        for byte in 0x80..=0xffu8 {
            let (written, spilled) = characters(&[byte], &[], 0);
            assert_eq!(written, [alloc::format!("{byte:03o}")]);
            assert_eq!(spilled, 0);
        }
    }

    #[test]
    fn floats_of_each_size_take_zeros_past_the_end() {
        assert_eq!(
            dumped("-t fF -t fD", b"abcd"),
            "0000000   1.6777999e+22\n\
             \x20                      8.321225784e-315\n\
             0000004\n"
        );
        assert_eq!(dumped("-t f4", b"abcd"), dumped("-t fF", b"abcd"));
        assert_eq!(dumped("-t f8", b"abcd"), dumped("-t fD", b"abcd"));
        assert_eq!(dumped("-t f", b"abcd"), dumped("-t fD", b"abcd"));
    }

    #[test]
    fn several_types_line_up_with_the_widest() {
        assert_eq!(
            dumped("-t x1 -c", b"abc\n"),
            "0000000  61  62  63  0a\n          a   b   c  \\n\n0000004\n"
        );
        assert_eq!(
            dumped("-A x -t d1 -t u1 -t o1 -t x1", b"ab\xff\x80\x7fgh"),
            "000000   97   98   -1 -128  127  103  104\n\
             \x20        97   98  255  128  127  103  104\n\
             \x20       141  142  377  200  177  147  150\n\
             \x20        61   62   ff   80   7f   67   68\n\
             000007\n"
        );
    }

    #[test]
    fn what_a_type_lacks_goes_to_its_fields_from_the_last_on() {
        assert_eq!(
            dumped("-t x1 -t o2", b"abcdefghijklmnop"),
            "0000000  61 62  63 64  65 66  67 68  69 6a  6b 6c  6d 6e  6f 70\n\
             \x20       061141 062143 063145 064147 065151 066153 067155 070157\n\
             0000020\n"
        );
        assert_eq!(
            dumped("-t x1 -t x8", b"abcdefghijklmnop"),
            "0000000 61 62 63 64 65 66 67 68 69 6a 6b 6c 6d 6e 6f 70\n\
             \x20              6867666564636261        706f6e6d6c6b6a69\n\
             0000020\n"
        );
        let x1 = Type::new(Kind::Number(Number::Hex), 1);
        let o2 = Type::new(Kind::Number(Number::Octal), 2);
        let widths = widths(&[x1, o2]);
        assert_eq!(widths[1], [7; 8]);
        assert_eq!(widths[0], [4, 3, 4, 3, 4, 3, 4, 3, 4, 3, 4, 3, 4, 3, 4, 3]);
        assert_eq!(widths[0].iter().sum::<usize>(), 56);
    }

    #[test]
    fn the_letters_name_their_types_and_the_last_values_take_zeros() {
        assert_eq!(
            dumped("-s -d -o -x -b -c", b"abc"),
            "0000000   25185      99\n\
             \x20         25185      99\n\
             \x20        061141  000143\n\
             \x20          6261    0063\n\
             \x20       141 142 143\n\
             \x20         a   b   c\n\
             0000003\n"
        );
    }

    #[test]
    fn each_letter_is_the_type_posix_gives_it() {
        let input = b"\x01\x80\xff\x7f\xfe\xffabc";
        let letters = [
            ("-b", "-t o1"),
            ("-c", "-t c"),
            ("-d", "-t u2"),
            ("-o", "-t o2"),
            ("-s", "-t d2"),
            ("-x", "-t x2"),
        ];
        for (letter, named) in letters {
            assert_eq!(dumped(letter, input), dumped(named, input), "{letter}");
        }
        assert_ne!(dumped("-d", input), dumped("-s", input));
    }

    #[test]
    fn numbers_of_each_size() {
        assert_eq!(
            dumped(
                "-t d2 -t d4 -t d8",
                b"\xff\xff\xfe\xff\xff\xff\xff\x7f\0\0\0\0\0\0\0\x80"
            ),
            "0000000     -1     -2     -1  32767      0      0      0 -32768\n\
             \x20              -65537    2147483647             0   -2147483648\n\
             \x20               9223372036854710271        -9223372036854775808\n\
             0000020\n"
        );
        assert_eq!(
            dumped("-t u2 -t u4 -t u8 -t o8", &[0xff; 16]),
            "0000000 65535 65535 65535 65535 65535 65535 65535 65535\n\
             \x20        4294967295  4294967295  4294967295  4294967295\n\
             \x20          18446744073709551615    18446744073709551615\n\
             \x20        1777777777777777777777  1777777777777777777777\n\
             0000020\n"
        );
        assert_eq!(
            dumped("-t xC -t dS -t uI -t oL", b"abcdefgh"),
            "0000000  61 62  63 64  65 66  67 68\n\
             \x20        25185  25699  26213  26727\n\
             \x20          1684234849    1751606885\n\
             \x20            0641473146254430661141\n\
             0000010\n"
        );
    }

    #[test]
    fn each_width_is_that_of_the_widest_value() {
        for how in [Number::Signed, Number::Unsigned, Number::Octal, Number::Hex] {
            for size in [1, 2, 4, 8] {
                let width = Type::new(Kind::Number(how), size).width();
                let widest = [[0x00, 0x80], [0xff, 0xff], [0xff, 0x7f]]
                    .iter()
                    .map(|&[low, high]| {
                        let mut bytes = alloc::vec![low; size];
                        bytes[size - 1] = high;
                        value(how, size, &bytes).len()
                    })
                    .max();
                assert_eq!(widest, Some(width), "{how:?} {size}");
            }
        }
    }

    #[test]
    fn addresses_in_each_base_or_none() {
        let input: Vec<u8> = (b'a'..=b'z').chain(b'0'..=b'9').collect();
        assert_eq!(
            dumped("-A d -j 3 -N 20 -t c", &input),
            "0000003   d   e   f   g   h   i   j   k   l   m   n   o   p   q   r   s\n\
             0000019   t   u   v   w\n\
             0000023\n"
        );
        assert_eq!(dumped("-A n -t x2", b"abcde"), " 6261 6463 0065\n");
        assert_eq!(
            dumped("-An -tx1 -tc", b"abc"),
            "  61  62  63\n   a   b   c\n"
        );
        assert_eq!(
            dumped("-A x -j 16 -c", b"abcdefghijklmnopq"),
            "000010   q\n000011\n"
        );
        assert_eq!(dumped("-Ao -c", b"a"), "0000000   a\n0000001\n");
        assert_eq!(address(Base::Hex, 0x1234_5678), "12345678");
        assert_eq!(address(Base::Octal, 0o12_345_670), "12345670");
        assert_eq!(address(Base::Decimal, 12_345_678), "12345678");
    }

    #[test]
    fn lines_the_same_as_the_one_before_are_one_star() {
        let mut input = alloc::vec![b'a'; 64];
        input.extend_from_slice(b"xyz");
        let line = "   a".repeat(16);
        assert_eq!(
            dumped("-c", &input),
            alloc::format!("0000000{line}\n*\n0000100   x   y   z\n0000103\n")
        );
        assert_eq!(
            dumped("-c", &input[..64]),
            alloc::format!("0000000{line}\n*\n0000100\n")
        );
        assert_eq!(
            dumped("-v -c", &input[..48]),
            alloc::format!("0000000{line}\n0000020{line}\n0000040{line}\n0000060\n")
        );
        // A star stands for every line the same since, and a line that
        // differs starts it again.
        let mut twice = alloc::vec![b'b'; 32];
        twice.extend_from_slice(&[b'c'; 48]);
        let b = "   b".repeat(16);
        let c = "   c".repeat(16);
        assert_eq!(
            dumped("-c", &twice),
            alloc::format!("0000000{b}\n*\n0000040{c}\n*\n0000120\n")
        );
    }

    #[test]
    fn characters_of_more_than_one_byte_and_their_stars() {
        assert_eq!(
            dumped("-c", "a£é€😀".as_bytes()),
            "0000000   a   £  **   é  **   €  **  **   😀  **  **  **\n0000014\n"
        );
        // One a line ends with, the next line's first bytes end.
        let mut input = alloc::vec![b'.'; 14];
        input.extend_from_slice("€b".as_bytes());
        assert_eq!(
            dumped("-c", &input),
            alloc::format!(
                "0000000{}   €  **\n0000020  **   b\n0000022\n",
                "   .".repeat(14)
            )
        );
        // Not whole, not one, or one without a picture: each byte in octal.
        assert_eq!(
            dumped("-c", b"\xc3A\xe2\x82\xc2\x85\xc3"),
            "0000000 303   A 342 202 302 205 303\n0000007\n"
        );
        // Cut off by -N, a character is not whole.
        assert_eq!(
            dumped("-N 2 -c", "€".as_bytes()),
            "0000000 342 202\n0000002\n"
        );
    }

    #[test]
    fn characters_come_out_the_same_whatever_the_pieces() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let alphabet: Vec<&[u8]> = alloc::vec![
            b"a",
            b"\n",
            b"\0",
            b"\x80",
            b"\xc3",
            "é".as_bytes(),
            "€".as_bytes(),
            "😀".as_bytes(),
            b"aaaaaaaaaaaaaaaa"
        ];
        for _ in 0..200 {
            let mut input = Vec::new();
            for _ in 0..shuffled(&mut seed) % 40 {
                let at = usize::try_from(shuffled(&mut seed) % 9).unwrap();
                input.extend_from_slice(alphabet[at]);
            }
            let whole = dumped_in("-c -t x1", &input, input.len().max(1));
            for piece in [1, 2, 3, 7, 16, 19] {
                assert_eq!(dumped_in("-c -t x1", &input, piece), whole, "{input:?}");
            }
        }
    }

    #[test]
    fn floats_in_as_few_digits_as_read_back() {
        let floats: Vec<u8> = [100.0f32, 123_456_789.0, 0.0001, 1e-5, -0.0, f32::INFINITY]
            .iter()
            .chain(&[f32::NAN, 1.234_56])
            .flat_map(|f| f.to_le_bytes())
            .collect();
        assert_eq!(
            dumped("-t f4", &floats),
            "0000000             100   1.2345679e+08          0.0001           1e-05\n\
             0000020              -0             inf             nan         1.23456\n\
             0000040\n"
        );
        let doubles: Vec<u8> = [100.0f64, 1e16, 1e15, 0.1, f64::NEG_INFINITY, 1.5e300]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        assert_eq!(
            dumped("-t fD", &doubles),
            "0000000                      100                    1e+16\n\
             0000020                    1e+15                      0.1\n\
             0000040                     -inf                 1.5e+300\n\
             0000060\n"
        );
    }

    #[test]
    fn floats_too_small_to_be_normal_may_take_fewer_digits() {
        let tiny: Vec<u8> = [1u32, 0x007f_ffff, 0xffc0_0000, 0x0080_0000]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        assert_eq!(
            dumped("-t f4", &tiny),
            "0000000           1e-45   1.1754942e-38            -nan   1.1754944e-38\n0000020\n"
        );
        let doubles: Vec<u8> = [5e-324f64, 2.225_073_858_507_201_4e-308]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        assert_eq!(
            dumped("-t f", &doubles),
            "0000000                   5e-324  2.2250738585072014e-308\n0000020\n"
        );
        assert_eq!(float(0.0f64), "0");
        assert_eq!(float(-0.0f32), "-0");
        assert_eq!(float(f64::NAN), "nan");
        assert_eq!(float(-f64::NAN), "-nan");
        assert_eq!(float(f64::INFINITY), "inf");
        assert_eq!(float(f32::NEG_INFINITY), "-inf");
    }

    #[test]
    fn floats_and_doubles_together_line_up() {
        let mut input = Vec::new();
        for bits in [0x3f80_0000u32, 0, 0x3ff0_0000] {
            input.extend_from_slice(&bits.to_le_bytes());
        }
        assert_eq!(
            dumped("-t f4 -t f8", &input),
            "0000000               1               0           1.875\n\
             \x20                      5.263544247e-315                5.299808824e-315\n\
             0000014\n"
        );
    }

    #[test]
    fn floats_read_back_and_need_every_digit_they_take() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..4000 {
            let bits = shuffled(&mut seed);
            let double = f64::from_bits(bits);
            let single = f32::from_bits(u32::try_from(bits >> 32).unwrap());
            for (text, width) in [(float(double), 24), (float(single), 15)] {
                assert!(text.len() <= width, "{text}");
            }
            if double.is_finite() {
                let text = float(double);
                assert_eq!(text.parse::<f64>().unwrap().to_bits(), bits, "{text}");
                // Past the least, the last digit taken is never a zero, or
                // one fewer would have done.
                let figures: String = text
                    .split('e')
                    .next()
                    .unwrap()
                    .chars()
                    .filter(char::is_ascii_digit)
                    .collect();
                let digits = figures.trim_start_matches('0').trim_end_matches('0').len();
                let least = if double.abs() < f64::MIN_POSITIVE {
                    1
                } else {
                    15
                };
                if digits > least {
                    let fewer = alloc::format!("{:.*e}", digits - 2, double);
                    assert_ne!(fewer.parse::<f64>().unwrap(), double, "{text}");
                }
            }
            if single.is_finite() {
                let text = float(single);
                assert_eq!(text.parse::<f32>().unwrap(), single, "{text}");
            }
        }
    }

    #[test]
    fn c_writes_large_and_small_numbers_with_an_exponent() {
        assert_eq!(like_c("1.2345679e8", 8), "1.2345679e+08");
        assert_eq!(like_c("1.23456800e8", 9), "123456800");
        assert_eq!(like_c("1.00000e2", 6), "100");
        assert_eq!(like_c("1.00000e-4", 6), "0.0001");
        assert_eq!(like_c("1.25000e-4", 6), "0.000125");
        assert_eq!(like_c("1.00000e-5", 6), "1e-05");
        assert_eq!(like_c("-2.50000e-5", 6), "-2.5e-05");
        assert_eq!(like_c("1.00000e6", 6), "1e+06");
        assert_eq!(like_c("9.99999e5", 6), "999999");
        assert_eq!(like_c("-1.50000e0", 6), "-1.5");
        assert_eq!(like_c("1.5e300", 2), "1.5e+300");
    }

    #[test]
    fn type_strings_one_after_another() {
        let t = |kind, size| Type::new(kind, size);
        let n = Kind::Number;
        assert_eq!(
            types("x1c").unwrap(),
            [t(n(Number::Hex), 1), t(Kind::Character, 1)]
        );
        assert_eq!(
            types("adoux").unwrap(),
            [
                t(Kind::Named, 1),
                t(n(Number::Signed), 4),
                t(n(Number::Octal), 4),
                t(n(Number::Unsigned), 4),
                t(n(Number::Hex), 4)
            ]
        );
        assert_eq!(
            types("dCdSdIdLd8").unwrap(),
            [1, 2, 4, 8, 8].map(|size| t(n(Number::Signed), size))
        );
        assert_eq!(
            types("ffFfDf4f8").unwrap(),
            [8, 4, 8, 4, 8].map(|size| t(n(Number::Float), size))
        );
        assert_eq!(
            types("o2a").unwrap(),
            [t(n(Number::Octal), 2), t(Kind::Named, 1)]
        );
    }

    #[test]
    fn type_strings_that_do_not_read() {
        assert_eq!(wrong("-t"), "od: -t wants its word\n");
        assert_eq!(wrong("-t q"), "od: q is no type\n");
        assert_eq!(wrong("-t x1q"), "od: q is no type\n");
        assert_eq!(
            wrong("-t x3"),
            "od: numbers are 1, 2, 4 or 8 bytes, not 3\n"
        );
        assert_eq!(
            wrong("-t d16"),
            "od: numbers are 1, 2, 4 or 8 bytes, not 16\n"
        );
        assert_eq!(wrong("-t f2"), "od: floats are 4 or 8 bytes, not 2\n");
        assert_eq!(wrong("-t fL"), "od: long doubles are not read\n");
        assert_eq!(wrong("-t fC"), "od: C is no type\n");
        assert_eq!(wrong("-t xF"), "od: F is no type\n");
        assert_eq!(
            asked(&[String::from("-t"), String::new()]).unwrap_err(),
            "od: -t wants a type\n"
        );
    }

    #[test]
    fn options_and_their_words() {
        let given = words("-vA x -j0x10 -N 1k -tx1 -bc -- -a b");
        let asked = asked(&given).unwrap();
        assert_eq!(asked.base, Some(Base::Hex));
        assert_eq!(asked.skip, 16);
        assert_eq!(asked.count, Some(1024));
        assert!(asked.every);
        assert_eq!(
            asked.types,
            [
                Type::new(Kind::Number(Number::Hex), 1),
                Type::new(Kind::Number(Number::Octal), 1),
                Type::new(Kind::Character, 1)
            ]
        );
        assert_eq!(asked.files, ["-a", "b"]);
        let plain = words("- a");
        let plain = super::asked(&plain).unwrap();
        assert_eq!(plain.files, ["-", "a"]);
        assert_eq!(plain.base, Some(Base::Octal));
        assert_eq!(plain.types, [Type::new(Kind::Number(Number::Octal), 2)]);
        assert!(!plain.every && plain.skip == 0 && plain.count.is_none());
        assert_eq!(super::asked(&words("-An")).unwrap().base, None);
        assert_eq!(
            super::asked(&words("-A d")).unwrap().base,
            Some(Base::Decimal)
        );
        assert_eq!(super::asked(&words("-Ao")).unwrap().base, Some(Base::Octal));
    }

    #[test]
    fn options_that_do_not_read() {
        assert_eq!(wrong("-y"), "od: -y is no option\n");
        assert_eq!(wrong("-cy"), "od: -y is no option\n");
        assert_eq!(wrong("-A"), "od: -A wants its word\n");
        assert_eq!(wrong("-A q"), "od: q is no base of addresses\n");
        assert_eq!(wrong("-j"), "od: -j wants its word\n");
        assert_eq!(wrong("-j z"), "od: -j wants a number, not z\n");
        assert_eq!(wrong("-N 1q"), "od: -N wants a number, not 1q\n");
    }

    #[test]
    fn numbers_of_bytes_in_three_bases_and_three_units() {
        assert_eq!(bytes("0"), Some(0));
        assert_eq!(bytes("9"), Some(9), "one digit is decimal, 8 and 9 too");
        assert_eq!(bytes("17"), Some(17));
        assert_eq!(bytes("017"), Some(15));
        assert_eq!(bytes("0x1f"), Some(31));
        assert_eq!(bytes("0X1F"), Some(31));
        assert_eq!(bytes("0x1b"), Some(27));
        assert_eq!(bytes("2b"), Some(1024));
        assert_eq!(bytes("2k"), Some(2048));
        assert_eq!(bytes("2m"), Some(2 << 20));
        assert_eq!(bytes("010k"), Some(8 * 1024));
        assert_eq!(bytes("0b"), Some(0));
        for not in ["", "b", "0x", "08", "+1", "-1", "1kb", "x1", "1 ", "0xg"] {
            assert_eq!(bytes(not), None, "{not}");
        }
        assert_eq!(bytes("18446744073709551615"), Some(u64::MAX));
        assert_eq!(bytes("18446744073709551616"), None);
        assert_eq!(bytes("18014398509481984k"), None);
    }

    #[test]
    fn the_window_skips_and_then_lets_so_many_through() {
        let mut window = Window {
            skip: 3,
            left: Some(4),
        };
        assert!(!window.shut());
        assert_eq!(window.through(b"ab"), b"");
        assert_eq!(window.through(b"cde"), b"de");
        assert!(!window.shut());
        assert_eq!(window.through(b"fghij"), b"fg");
        assert!(window.shut());
        assert_eq!(window.through(b"k"), b"");
        let mut open = Window {
            skip: 0,
            left: None,
        };
        assert_eq!(open.through(b"abc"), b"abc");
        assert!(!open.shut());
        // Even when nothing is let through, what is skipped is read.
        let none = Window {
            skip: 5,
            left: Some(0),
        };
        assert!(!none.shut());
        assert_eq!(at_most(u64::MAX, 7), 7);
        assert_eq!(at_most(3, 7), 3);
    }

    #[test]
    fn skipping_and_stopping_in_any_pieces() {
        let input: Vec<u8> = (0..=255).collect();
        let whole = dumped_in("-j 0x21 -N 0100 -t x1", &input, 256);
        assert!(whole.starts_with("0000041 21 22 23"));
        assert!(whole.ends_with("0000141\n"));
        for piece in [1, 5, 16, 33, 64] {
            assert_eq!(dumped_in("-j 0x21 -N 0100 -t x1", &input, piece), whole);
        }
    }

    /// Random bytes, among them whole characters of more than one byte.
    fn random_text(seed: &mut u64, most: u64) -> Vec<u8> {
        let mut text = Vec::new();
        for _ in 0..shuffled(seed) % most {
            match shuffled(seed) % 6 {
                0 => text.extend_from_slice("é".as_bytes()),
                1 => text.extend_from_slice("€".as_bytes()),
                2 => text.extend_from_slice("😀".as_bytes()),
                3 => text.extend_from_slice(b"ab \t\n"),
                _ => text.push(shuffled(seed).to_le_bytes()[3]),
            }
        }
        text
    }

    /// What one field `-c` wrote stands for: the bytes of its character,
    /// none for the star of one begun before.
    fn character_back(field: &str) -> Vec<u8> {
        let escapes = [
            ("\\0", 0),
            ("\\a", 7),
            ("\\b", 8),
            ("\\t", b'\t'),
            ("\\n", b'\n'),
            ("\\v", 11),
            ("\\f", 12),
            ("\\r", b'\r'),
        ];
        if let Some(&(_, byte)) = escapes.iter().find(|(e, _)| *e == field) {
            return alloc::vec![byte];
        }
        match field {
            "**" => Vec::new(),
            "" => alloc::vec![b' '],
            octal if octal.len() == 3 && octal.bytes().all(|b| (b'0'..=b'7').contains(&b)) => {
                alloc::vec![u8::from_str_radix(octal, 8).unwrap()]
            }
            one => {
                assert_eq!(one.chars().count(), 1, "{one:?}");
                one.as_bytes().to_vec()
            }
        }
    }

    /// The bytes one field of a number type stands for.
    fn number_back(field: &str, how: Number, size: usize) -> Vec<u8> {
        let value = match how {
            Number::Signed => field.parse::<i64>().unwrap().cast_unsigned(),
            Number::Unsigned => field.parse::<u64>().unwrap(),
            Number::Octal => u64::from_str_radix(field, 8).unwrap(),
            Number::Hex => u64::from_str_radix(field, 16).unwrap(),
            Number::Float => unreachable!("floats are read back by their own test"),
        };
        if size < 8 {
            let unused = 64 - 8 * size;
            let fits = match how {
                Number::Signed => {
                    let signed = value.cast_signed();
                    signed << unused >> unused == signed
                }
                _ => value >> (8 * size) == 0,
            };
            assert!(fits, "{field} is wider than {size} bytes");
        }
        value.to_le_bytes()[..size].to_vec()
    }

    #[test]
    fn what_od_writes_reads_back_as_the_bytes_it_was() {
        let mut seed = 0x5851_f42d_4c95_7f2d_u64;
        let kinds = [
            Kind::Character,
            Kind::Number(Number::Signed),
            Kind::Number(Number::Unsigned),
            Kind::Number(Number::Octal),
            Kind::Number(Number::Hex),
        ];
        for _ in 0..300 {
            let input = random_text(&mut seed, 60);
            let mut types = Vec::new();
            for _ in 0..=shuffled(&mut seed) % 3 {
                let kind = kinds[usize::try_from(shuffled(&mut seed) % 5).unwrap()];
                let size = match kind {
                    Kind::Character => 1,
                    _ => 1 << (shuffled(&mut seed) % 4),
                };
                types.push(Type::new(kind, size));
            }
            let options: String = types
                .iter()
                .map(|t| match t.kind {
                    Kind::Character => String::from(" -c"),
                    Kind::Number(how) => {
                        let letter = match how {
                            Number::Signed => 'd',
                            Number::Unsigned => 'u',
                            Number::Octal => 'o',
                            _ => 'x',
                        };
                        alloc::format!(" -t {letter}{}", t.size)
                    }
                    Kind::Named => String::new(),
                })
                .collect();
            let written = dumped(&alloc::format!("-A d -v{options}"), &input);
            let mut lines: Vec<&str> = written.lines().collect();
            assert_eq!(
                lines.pop().map(str::to_string),
                Some(alloc::format!("{:07}", input.len())),
                "{options}"
            );
            assert_eq!(lines.len(), input.len().div_ceil(LINE) * types.len());
            let widths = widths(&types);
            let mut back = alloc::vec![Vec::new(); types.len()];
            for (index, rows) in lines.chunks(types.len()).enumerate() {
                let len = (input.len() - index * LINE).min(LINE);
                for (k, row) in rows.iter().enumerate() {
                    let row: Vec<char> = row.chars().collect();
                    let place: String = row[..7].iter().collect();
                    if k == 0 {
                        assert_eq!(place, alloc::format!("{:07}", index * LINE));
                    } else {
                        assert_eq!(place, "       ");
                    }
                    if len == LINE {
                        let wide = 7 + widths[k].iter().sum::<usize>();
                        assert_eq!(row.len(), wide, "a full line is as wide as any");
                    }
                    let mut at = 7;
                    for &width in widths[k].iter().take(len.div_ceil(types[k].size)) {
                        let field: String = row[at..at + width].iter().collect();
                        assert!(field.starts_with(' '), "{field:?} has a space before it");
                        at += width;
                        let field = field.trim_start();
                        back[k].extend(match types[k].kind {
                            Kind::Number(how) => number_back(field, how, types[k].size),
                            _ => character_back(field),
                        });
                    }
                    assert_eq!(at, row.len(), "nothing after the last field");
                }
            }
            for (t, mut bytes) in types.iter().zip(back) {
                bytes.truncate(input.len());
                assert_eq!(bytes, input, "{t:?}");
            }
        }
    }

    /// What od wrote with its stars, each written out as the lines it
    /// stands for, as `-v` writes them; addresses are decimal and each
    /// line has `rows` rows.
    fn stars_written_out(written: &str, rows: usize) -> String {
        let lines: Vec<&str> = written.lines().collect();
        let mut out = String::new();
        let mut before: Vec<&str> = Vec::new();
        let mut at = 0;
        while at < lines.len() {
            if lines[at] == "*" {
                let last: usize = before[0][..7].parse().unwrap();
                let next: usize = lines[at + 1][..7].parse().unwrap();
                for place in (last + LINE..next).step_by(LINE) {
                    let _ = writeln!(out, "{place:07}{}", &before[0][7..]);
                    for row in &before[1..] {
                        out.push_str(row);
                        out.push('\n');
                    }
                }
                at += 1;
                continue;
            }
            let block = &lines[at..(at + rows).min(lines.len())];
            for row in block {
                out.push_str(row);
                out.push('\n');
            }
            before = block.to_vec();
            at += block.len().max(1);
        }
        out
    }

    #[test]
    fn a_star_stands_for_the_lines_v_writes() {
        let mut seed = 0xd1b5_4a32_d192_ed03_u64;
        for _ in 0..200 {
            let mut input = Vec::new();
            for _ in 0..shuffled(&mut seed) % 12 {
                let line = shuffled(&mut seed).to_le_bytes()[0] % 3;
                let times = 1 + shuffled(&mut seed) % 4;
                for _ in 0..times {
                    input.extend_from_slice(&[b'a' + line; LINE]);
                }
            }
            input.extend(random_text(&mut seed, 20));
            let starred = dumped("-A d -c -t x2", &input);
            let every = dumped("-A d -v -c -t x2", &input);
            assert_eq!(stars_written_out(&starred, 2), every);
            let repeats = input
                .chunks(LINE)
                .zip(input.chunks(LINE).skip(1))
                .any(|(a, b)| a == b && b.len() == LINE);
            let lines: Vec<&str> = starred.lines().collect();
            assert_eq!(lines.contains(&"*"), repeats);
            assert!(
                lines.windows(2).all(|pair| pair != ["*", "*"]),
                "one star for a run of lines"
            );
        }
    }

    #[test]
    fn the_window_lets_through_what_j_and_n_name_whatever_the_pieces() {
        let mut seed = 0x94d0_49bb_1331_11eb_u64;
        let input: Vec<u8> = (0..=255).cycle().take(700).collect();
        for _ in 0..500 {
            let skip = shuffled(&mut seed) % 800;
            let count = (!shuffled(&mut seed).is_multiple_of(3)).then(|| shuffled(&mut seed) % 800);
            let mut window = Window { skip, left: count };
            let mut through = Vec::new();
            let mut rest = &input[..];
            while !rest.is_empty() && !window.shut() {
                let piece = usize::try_from(1 + shuffled(&mut seed) % 130).unwrap();
                let (now, later) = rest.split_at(piece.min(rest.len()));
                through.extend_from_slice(window.through(now));
                rest = later;
            }
            let from = usize::try_from(skip).unwrap().min(input.len());
            let to = count.map_or(input.len(), |count| {
                (from + usize::try_from(count).unwrap()).min(input.len())
            });
            assert_eq!(through, input[from..to], "-j {skip} -N {count:?}");
            let past = skip > 700;
            assert_eq!(window.skip > 0, past, "-j {skip}");
        }
    }
}
