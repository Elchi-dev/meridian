//! awk's values: numbers, strings, and strings from the input that look
//! like numbers, which POSIX calls numeric strings and which are both at
//! once; and how one becomes another.

use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::format;

/// What an expression gives.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// What a variable holds before it is given anything: "" and 0 at
    /// once.
    Unset,
    Number(f64),
    Text(Vec<u8>),
    /// A string from the input that looks like a number, such as a field
    /// or a line `getline` read: compared as the number it writes.
    Numeric(Vec<u8>, f64),
}

/// Whether `byte` is a blank as awk's numbers take them around them.
fn blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// How many bytes at the start of `bytes` write a decimal number: a
/// sign, digits with a point among them or none, and a power of ten.
pub(crate) fn number_len(bytes: &[u8]) -> usize {
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut at = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let whole = digits(at);
    at += whole;
    let mut fraction = 0;
    if bytes.get(at) == Some(&b'.') {
        fraction = digits(at + 1);
        if whole + fraction > 0 {
            at += 1 + fraction;
        }
    }
    if whole + fraction == 0 {
        return 0;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        let signed = usize::from(matches!(bytes.get(at + 1), Some(b'+' | b'-')));
        let power = digits(at + 1 + signed);
        if power > 0 {
            at += 1 + signed + power;
        }
    }
    at
}

/// The number written at the start of `bytes`, after any blanks, as awk
/// takes a string for a number: 0 when none is.
#[must_use]
pub fn leading(bytes: &[u8]) -> f64 {
    let from = bytes.iter().take_while(|&&b| blank(b)).count();
    let len = number_len(&bytes[from..]);
    parsed(&bytes[from..from + len])
}

/// The number `bytes`, which [`number_len`] took whole, writes.
fn parsed(bytes: &[u8]) -> f64 {
    core::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0.0)
}

/// The number `bytes` writes when they write one and nothing else but
/// blanks around it, as a numeric string must.
#[must_use]
pub fn numeric(bytes: &[u8]) -> Option<f64> {
    let from = bytes.iter().take_while(|&&b| blank(b)).count();
    let len = number_len(&bytes[from..]);
    let rest = &bytes[from + len..];
    (len > 0 && rest.iter().all(|&b| blank(b))).then(|| parsed(&bytes[from..from + len]))
}

/// 2^63: the doubles from its negative up to it, but not it, are whole
/// numbers an `i64` holds when they have no fraction.
const WHOLE_BELOW: f64 = 9_223_372_036_854_775_808.0;

/// `x` as an `i64`, when it is a whole number one holds.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]
fn whole(x: f64) -> Option<i64> {
    // A cast goes toward zero, so it gives x back when x has no fraction.
    let n = x as i64;
    ((-WHOLE_BELOW..WHOLE_BELOW).contains(&x) && n as f64 == x).then_some(n)
}

/// `x` as a string: a whole number as an integer, infinities and the
/// value that is no number as words, and the rest as `form`, `CONVFMT`
/// or `OFMT`, writes it.
#[must_use]
pub fn number_text(x: f64, form: &[u8]) -> Vec<u8> {
    if x.is_nan() {
        return Vec::from(if x.is_sign_negative() { "-nan" } else { "nan" });
    }
    if x.is_infinite() {
        return Vec::from(if x < 0.0 { "-inf" } else { "inf" });
    }
    match whole(x) {
        Some(n) => Vec::from(alloc::format!("{n}")),
        None => formatted(x, form),
    }
}

/// `x` as `form` writes it, when `form` is text around one conversion
/// of a number; as `%.6g` does when it is anything else.
fn formatted(x: f64, form: &[u8]) -> Vec<u8> {
    let mut written = Vec::new();
    let mut at = 0;
    let mut converted = false;
    while at < form.len() {
        if form[at] != b'%' {
            written.push(form[at]);
            at += 1;
            continue;
        }
        if form.get(at + 1) == Some(&b'%') {
            written.push(b'%');
            at += 2;
            continue;
        }
        let Some((spec, stars, len)) = format::parse(&form[at + 1..]) else {
            return general(x);
        };
        if converted || stars.width || stars.precision || matches!(spec.letter, b'c' | b's') {
            return general(x);
        }
        let shown = if b"eEfFgG".contains(&spec.letter) {
            format::real(&spec, x)
        } else {
            format::integer(&spec, format::whole(x))
        };
        written.extend_from_slice(shown.as_bytes());
        converted = true;
        at += 1 + len;
    }
    if converted { written } else { general(x) }
}

/// `x` as `%.6g` writes it.
fn general(x: f64) -> Vec<u8> {
    let spec = format::Spec {
        letter: b'g',
        ..format::Spec::default()
    };
    Vec::from(format::real(&spec, x))
}

impl Value {
    /// The value of a string from the input: a numeric string when it
    /// looks like a number, else a string.
    #[must_use]
    pub fn input(bytes: Vec<u8>) -> Value {
        match numeric(&bytes) {
            Some(n) => Value::Numeric(bytes, n),
            None => Value::Text(bytes),
        }
    }

    /// The value as a number.
    #[must_use]
    pub fn number(&self) -> f64 {
        match self {
            Value::Unset => 0.0,
            Value::Number(n) | Value::Numeric(_, n) => *n,
            Value::Text(bytes) => leading(bytes),
        }
    }

    /// The value as a string, a number written as `form`, `CONVFMT`,
    /// says.
    #[must_use]
    pub fn text(&self, form: &[u8]) -> Vec<u8> {
        match self {
            Value::Unset => Vec::new(),
            Value::Number(n) => number_text(*n, form),
            Value::Text(bytes) | Value::Numeric(bytes, _) => bytes.clone(),
        }
    }

    /// Whether the value counts as true: a number other than 0, a
    /// string other than "", a numeric string by its number.
    #[must_use]
    pub fn truth(&self) -> bool {
        match self {
            Value::Unset => false,
            Value::Number(n) | Value::Numeric(_, n) => *n != 0.0,
            Value::Text(bytes) => !bytes.is_empty(),
        }
    }

    /// Whether the value compares as a number.
    fn numeric(&self) -> bool {
        !matches!(self, Value::Text(_))
    }
}

/// How `a` compares with `b`: as numbers when both are numbers, numeric
/// strings or unset, else as strings, a number written as `form`,
/// `CONVFMT`, says; none when either is the value that is no number.
#[must_use]
pub fn compare(a: &Value, b: &Value, form: &[u8]) -> Option<Ordering> {
    if a.numeric() && b.numeric() {
        return a.number().partial_cmp(&b.number());
    }
    Some(a.text(form).cmp(&b.text(form)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONVFMT: &[u8] = b"%.6g";

    #[test]
    fn a_string_is_the_number_at_its_start() {
        for (written, n) in [
            ("12", 12.0),
            ("  -3.5e2xyz", -350.0),
            ("+.5", 0.5),
            ("5.", 5.0),
            ("1e", 1.0),
            ("1e+", 1.0),
            ("2E-1 ", 0.2),
            (".", 0.0),
            ("-", 0.0),
            ("abc", 0.0),
            ("", 0.0),
            ("\t\n7", 7.0),
            ("0x1A", 0.0),
            ("inf", 0.0),
            ("1.5.5", 1.5),
            ("--1", 0.0),
        ] {
            assert_eq!(leading(written.as_bytes()), n, "{written:?}");
        }
    }

    #[test]
    fn a_numeric_string_is_a_number_and_blanks_only() {
        for (written, n) in [
            ("12", Some(12.0)),
            (" 12 ", Some(12.0)),
            ("\t-1e3\n", Some(-1000.0)),
            ("+.5", Some(0.5)),
            ("12a", None),
            ("1 2", None),
            ("", None),
            ("  ", None),
            (".", None),
            ("1e", None),
            ("e5", None),
            ("0x10", None),
        ] {
            assert_eq!(numeric(written.as_bytes()), n, "{written:?}");
        }
    }

    #[test]
    fn numbers_are_written_whole_or_as_the_form_says() {
        for (n, written) in [
            (3.0, "3"),
            (-0.0, "0"),
            (1e15, "1000000000000000"),
            (0.1, "0.1"),
            (1.0 / 3.0, "0.333333"),
            (1e30, "1e+30"),
            (9_223_372_036_854_774_784.0, "9223372036854774784"),
            (9_223_372_036_854_775_808.0, "9.22337e+18"),
            (-9_223_372_036_854_775_808.0, "-9223372036854775808"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-f64::NAN, "-nan"),
            (123_456.7, "123457"),
        ] {
            assert_eq!(number_text(n, CONVFMT), written.as_bytes(), "{n}");
        }
        assert_eq!(number_text(0.5, b"%.2f"), b"0.50");
        assert_eq!(number_text(2.5, b"<%5.1e>"), b"<2.5e+00>");
        assert_eq!(number_text(2.5, b"%d%%"), b"2%");
        assert_eq!(number_text(2.5, b"%x"), b"2");
        assert_eq!(number_text(0.5, b"%s"), b"0.5", "no string conversion");
        assert_eq!(number_text(0.5, b"%c"), b"0.5");
        assert_eq!(number_text(0.5, b"%*g"), b"0.5", "no stars");
        assert_eq!(number_text(0.5, b"%g%g"), b"0.5", "one conversion");
        assert_eq!(number_text(0.5, b"text"), b"0.5", "no conversion");
        assert_eq!(number_text(0.5, b"%y"), b"0.5", "no such conversion");
    }

    #[test]
    fn values_turn_into_numbers_strings_and_truth() {
        let cases = [
            (Value::Unset, 0.0, "", false),
            (Value::Number(2.5), 2.5, "2.5", true),
            (Value::Number(0.0), 0.0, "0", false),
            (Value::Text(b"3x".to_vec()), 3.0, "3x", true),
            (Value::Text(b"0".to_vec()), 0.0, "0", true),
            (Value::Text(Vec::new()), 0.0, "", false),
            (Value::input(b" 0.0 ".to_vec()), 0.0, " 0.0 ", false),
            (Value::input(b"0x".to_vec()), 0.0, "0x", true),
        ];
        for (value, n, text, truth) in cases {
            assert_eq!(value.number(), n, "{value:?}");
            assert_eq!(value.text(CONVFMT), text.as_bytes(), "{value:?}");
            assert_eq!(value.truth(), truth, "{value:?}");
        }
        assert_eq!(
            Value::input(b"1e2".to_vec()),
            Value::Numeric(b"1e2".to_vec(), 100.0)
        );
        assert_eq!(
            Value::input(b"1e2x".to_vec()),
            Value::Text(b"1e2x".to_vec())
        );
    }

    #[test]
    fn comparisons_take_numbers_when_both_are_and_strings_else() {
        use Ordering::{Equal, Greater, Less};
        let number = Value::Number;
        let text = |t: &str| Value::Text(t.as_bytes().to_vec());
        let input = |t: &str| Value::input(t.as_bytes().to_vec());
        for (a, b, ordering) in [
            (number(2.0), number(10.0), Less),
            (input("10"), input("9"), Greater),
            (input(" 10 "), number(10.0), Equal),
            (input("1e1"), input("10"), Equal),
            (text("10"), text("9"), Less),
            (text("10"), number(9.0), Less),
            (input("abc"), number(0.0), Greater),
            (Value::Unset, number(0.0), Equal),
            (Value::Unset, text(""), Equal),
            (Value::Unset, input("-1"), Greater),
            (number(0.1), text("0.1"), Equal),
            (number(1.0 / 3.0), text("0.333333"), Equal),
        ] {
            assert_eq!(compare(&a, &b, CONVFMT), Some(ordering), "{a:?} {b:?}");
        }
        assert_eq!(compare(&number(f64::NAN), &number(1.0), CONVFMT), None);
        assert_eq!(
            compare(&number(0.5), &text("0.50"), b"%.2f"),
            Some(Equal),
            "a number becomes a string as CONVFMT says"
        );
    }

    #[test]
    fn blanks_around_numbers_are_those_of_c() {
        for blank in [" ", "\t", "\n", "\r", "\x0b", "\x0c"] {
            let written = format!("{blank}5{blank}");
            assert_eq!(numeric(written.as_bytes()), Some(5.0), "{written:?}");
            assert_eq!(leading(written.as_bytes()), 5.0, "{written:?}");
        }
        assert_eq!(numeric(b"\x015"), None, "a control character is no blank");
        assert_eq!(leading(b"\x015"), 0.0);
    }

    #[test]
    fn number_lengths_take_points_signs_and_powers() {
        for (written, len) in [
            ("1", 1),
            ("-1", 2),
            ("+.5", 3),
            ("1.", 2),
            ("1.5e3x", 5),
            ("1e", 1),
            ("1e-", 1),
            ("1e-3", 4),
            ("1E+3", 4),
            ("e3", 0),
            ("-", 0),
            ("-.", 0),
            (".e3", 0),
            ("..1", 0),
            ("12.34.5", 5),
        ] {
            assert_eq!(number_len(written.as_bytes()), len, "{written:?}");
        }
    }

    #[test]
    fn forms_with_text_around_a_conversion_keep_it() {
        assert_eq!(number_text(1.5, b"[%g]"), b"[1.5]");
        assert_eq!(number_text(1.5, b"%5.2f"), b" 1.50");
        assert_eq!(number_text(1.5, b"%-6.1e|"), b"1.5e+00|");
        assert_eq!(number_text(1.5, b"%i"), b"1");
        assert_eq!(
            number_text(-1.5, b"%u"),
            b"18446744073709551615",
            "as C writes a number below zero"
        );
        assert_eq!(number_text(255.5, b"%#X"), b"0XFF");
        assert_eq!(number_text(0.5, b"%%%g"), b"%0.5");
        assert_eq!(
            number_text(0.5, b"%"),
            b"0.5",
            "a % at the end is no conversion"
        );
        assert_eq!(number_text(1e100, b"%.3G"), b"1E+100");
    }

    #[test]
    fn values_compare_alike_whichever_side_they_are_on() {
        let values = [
            Value::Unset,
            Value::Number(1.0),
            Value::Text(b"1".to_vec()),
            Value::input(b"1".to_vec()),
            Value::input(b"x".to_vec()),
            Value::Number(-2.5),
        ];
        for a in &values {
            for b in &values {
                let ab = compare(a, b, CONVFMT);
                let ba = compare(b, a, CONVFMT);
                assert_eq!(ab.map(Ordering::reverse), ba, "{a:?} {b:?}");
            }
            assert_eq!(compare(a, a, CONVFMT), Some(Ordering::Equal), "{a:?}");
        }
    }

    #[test]
    fn a_numeric_string_keeps_its_text_and_its_number() {
        let value = Value::input(b" +1.50 ".to_vec());
        assert_eq!(value.number(), 1.5);
        assert_eq!(value.text(CONVFMT), b" +1.50 ", "it is written as it came");
        assert!(value.truth());
        assert_eq!(
            compare(&value, &Value::Number(1.5), CONVFMT),
            Some(Ordering::Equal)
        );
        assert_eq!(
            compare(&value, &Value::Text(b"1.5".to_vec()), CONVFMT),
            Some(Ordering::Less),
            "against a string it is its text"
        );
        let zero = Value::input(b"-0".to_vec());
        assert!(!zero.truth(), "minus zero is false");
        assert_eq!(zero.text(CONVFMT), b"-0");
        assert_eq!(
            Value::Number(-0.0).text(CONVFMT),
            b"0",
            "a number minus zero is written 0"
        );
        assert_eq!(Value::Unset.text(b"%d"), b"");
        assert_eq!(Value::Text(b"abc".to_vec()).number(), 0.0);
    }
}
