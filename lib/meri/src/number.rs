use core::fmt::Write;

use crate::Code;
use crate::reader::Value;

/// A float, with the text it was written as, which the canonical dump
/// shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Float<'a> {
    pub value: f64,
    pub literal: &'a str,
}

/// The number of a quantity, written as an integer or as a float.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number<'a> {
    Int(i64),
    Float(Float<'a>),
}

/// A number with a unit, such as `512MiB`, `1.5h` or `125%`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity<'a> {
    pub number: Number<'a>,
    pub unit: &'a str,
}

/// A decimal literal the reader has checked, taken apart.
struct Parts<'a> {
    negative: bool,
    int: &'a str,
    frac: &'a str,
    exp: i64,
}

impl<'a> Parts<'a> {
    fn of(literal: &'a str) -> Parts<'a> {
        let (mantissa, exp) = literal.split_once(['e', 'E']).unwrap_or((literal, ""));
        let unsigned = mantissa.trim_start_matches('-');
        let (int, frac) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        let mut value: i64 = 0;
        for d in exp.bytes().filter(u8::is_ascii_digit) {
            value = value.saturating_mul(10).saturating_add(i64::from(d - b'0'));
        }
        Parts {
            negative: mantissa.starts_with('-'),
            int,
            frac,
            exp: if exp.starts_with('-') { -value } else { value },
        }
    }

    /// The digits of the mantissa as values, without the point and the `_`.
    fn digits(&self) -> impl DoubleEndedIterator<Item = u8> + use<'a> {
        let int = self.int.bytes();
        int.chain(self.frac.bytes())
            .filter(u8::is_ascii_digit)
            .map(|d| d - b'0')
    }
}

/// How many significant digits a float keeps before the rest only counts
/// as zero or not zero. Rounding to the nearest double never needs more
/// than 768, and Rust's own parser makes the same cut.
const KEEP: usize = 800;

/// The double closest to a decimal literal the reader has checked. Rust's
/// parser does not take `_`, and a literal with them can be any length,
/// so the digits are first rewritten into a buffer of fixed size as
/// `[-]digits e exponent`, which has the same closest double.
fn float_value(literal: &str) -> Result<f64, Code> {
    let parts = Parts::of(literal);
    let mut buf = Buf {
        bytes: [0; KEEP + 16],
        len: 0,
    };
    if parts.negative {
        buf.push(b'-');
    }
    let start = buf.len;
    let mut digits = parts.digits().skip_while(|&d| d == 0);
    for d in digits.by_ref().take(KEEP) {
        buf.push(b'0' + d);
    }
    if buf.len == start {
        buf.push(b'0');
    }
    let mut dropped: i64 = 0;
    let mut nonzero_dropped = false;
    for d in digits {
        dropped += 1;
        nonzero_dropped |= d != 0;
    }
    if nonzero_dropped {
        buf.push(b'1');
        dropped -= 1;
    }
    let frac =
        i64::try_from(parts.frac.bytes().filter(u8::is_ascii_digit).count()).unwrap_or(i64::MAX);
    // Beyond this the result is infinite or zero for any number of digits
    // the buffer holds, and the exponent stays short.
    let exp = parts
        .exp
        .saturating_sub(frac)
        .saturating_add(dropped)
        .clamp(-99_999, 99_999);
    write!(buf, "e{exp}").map_err(|_| Code::NumberOverflow)?;
    core::str::from_utf8(&buf.bytes[..buf.len])
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .ok_or(Code::NumberOverflow)
}

/// Room for a float as [`float_value`] rewrites it.
struct Buf {
    bytes: [u8; KEEP + 16],
    len: usize,
}

impl Buf {
    fn push(&mut self, b: u8) {
        self.bytes[self.len] = b;
        self.len += 1;
    }
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            self.push(b);
        }
        Ok(())
    }
}

/// Reads the number that starts at `start`, by the rules of section 8, and
/// returns its value and where it ends.
pub(crate) fn read<'a>(
    src: &[u8],
    text: &'a str,
    start: usize,
) -> Result<(Value<'a>, usize), Code> {
    // Past the end comes a 0, which ends a number like any other byte that
    // cannot belong to one.
    let byte = |i: usize| src.get(i).copied().unwrap_or(0);
    let negative = byte(start) == b'-';
    let first = start + usize::from(negative);
    if !byte(first).is_ascii_digit() {
        return Err(Code::InvalidNumber);
    }
    if byte(first) == b'0' {
        let base = match byte(first + 1) {
            b'x' => 16,
            b'o' => 8,
            b'b' => 2,
            _ => 10,
        };
        if base != 10 && is_digit(byte(first + 2), base) {
            return radix(src, first + 2, base, negative);
        }
    }

    let mut end = digits_end(src, first, 10);
    if byte(first) == b'0' && end > first + 1 {
        return Err(Code::InvalidNumber);
    }
    let mut float = false;
    if byte(end) == b'.' {
        if !byte(end + 1).is_ascii_digit() {
            return Err(Code::InvalidNumber);
        }
        end = digits_end(src, end + 1, 10);
        float = true;
    }
    if let b'e' | b'E' = byte(end) {
        let sign = usize::from(matches!(byte(end + 1), b'+' | b'-'));
        if byte(end + 1 + sign).is_ascii_digit() {
            end = digits_end(src, end + 1 + sign, 10);
            float = true;
        }
    }
    let number_end = end;
    if byte(end) == b'%' {
        end += 1;
    } else {
        while byte(end).is_ascii_alphabetic() {
            end += 1;
        }
    }
    if is_number_tail(byte(end)) {
        return Err(Code::InvalidNumber);
    }

    let literal = &text[start..number_end];
    let number = if float {
        Number::Float(Float {
            value: float_value(literal)?,
            literal,
        })
    } else {
        Number::Int(int_value(&src[first..number_end], 10, negative)?)
    };
    let value = match (number, &text[number_end..end]) {
        (Number::Int(n), "") => Value::Int(n),
        (Number::Float(f), "") => Value::Float(f),
        (number, unit) => Value::Quantity(Quantity { number, unit }),
    };
    Ok((value, end))
}

/// Reads the digits of a hexadecimal, octal or binary number.
fn radix<'a>(
    src: &[u8],
    digits: usize,
    base: u32,
    negative: bool,
) -> Result<(Value<'a>, usize), Code> {
    let end = digits_end(src, digits, base);
    match src.get(end) {
        Some(b'a'..=b'z' | b'A'..=b'Z' | b'%') => Err(Code::UnitNotAllowed),
        Some(b'0'..=b'9' | b'_' | b'.') => Err(Code::InvalidNumber),
        _ => Ok((
            Value::Int(int_value(&src[digits..end], base, negative)?),
            end,
        )),
    }
}

/// The value of digits in `base`, with any `_` between them, as an `i64`.
fn int_value(digits: &[u8], base: u32, negative: bool) -> Result<i64, Code> {
    let mut n: i64 = 0;
    for d in digits.iter().filter_map(|&c| char::from(c).to_digit(base)) {
        let d = i64::from(d);
        n = n
            .checked_mul(i64::from(base))
            .and_then(|n| {
                if negative {
                    n.checked_sub(d)
                } else {
                    n.checked_add(d)
                }
            })
            .ok_or(Code::NumberOverflow)?;
    }
    Ok(n)
}

/// Where a run of digits that starts at `i` ends. A `_` belongs to it only
/// with a digit after it; anything else is the caller's business.
fn digits_end(src: &[u8], mut i: usize, base: u32) -> usize {
    loop {
        match src.get(i) {
            Some(&c) if is_digit(c, base) => i += 1,
            Some(b'_') if src.get(i + 1).is_some_and(|&c| is_digit(c, base)) => i += 2,
            _ => return i,
        }
    }
}

fn is_digit(c: u8, base: u32) -> bool {
    char::from(c).is_digit(base)
}

fn is_number_tail(c: u8) -> bool {
    matches!(c, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'%')
}
