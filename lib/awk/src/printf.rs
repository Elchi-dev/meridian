//! awk's `printf` and `sprintf`: a format's text as it is, and each of
//! its conversions filled with the next value, as C's printf would fill
//! it with a number or a string.

use alloc::vec::Vec;

use crate::format::{self, Spec};
use crate::value::Value;

/// How many characters `bytes` hold, a byte that is no UTF-8 counting as
/// one, as `regex` counts them.
#[must_use]
pub fn chars(bytes: &[u8]) -> usize {
    bytes
        .utf8_chunks()
        .map(|chunk| chunk.valid().chars().count() + chunk.invalid().len())
        .sum()
}

/// The bytes of the first `n` characters of `bytes`.
#[must_use]
pub fn first_chars(bytes: &[u8], n: usize) -> &[u8] {
    let mut left = n;
    let mut at = 0;
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if left == 0 {
                return &bytes[..at];
            }
            left -= 1;
            at += c.len_utf8();
        }
        for _ in chunk.invalid() {
            if left == 0 {
                return &bytes[..at];
            }
            left -= 1;
            at += 1;
        }
    }
    bytes
}

/// `body` in the field `spec`'s width asks for, with blanks before or,
/// with `-`, after.
fn padded(spec: &Spec, body: &[u8]) -> Vec<u8> {
    let room = spec.width.unwrap_or(0).saturating_sub(chars(body));
    let blanks = core::iter::repeat_n(b' ', room);
    if spec.left {
        body.iter().copied().chain(blanks).collect()
    } else {
        blanks.chain(body.iter().copied()).collect()
    }
}

/// The character `%c` writes for `value`: the one a number is the code
/// of, or the first of a string.
fn character(value: &Value, convfmt: &[u8]) -> Vec<u8> {
    if let Value::Number(n) | Value::Numeric(_, n) = value {
        let code = u32::try_from(format::whole(*n).clamp(0, i128::from(u32::MAX))).unwrap_or(0);
        return match char::from_u32(code) {
            Some(c) => Vec::from(c.encode_utf8(&mut [0; 4]).as_bytes()),
            None => Vec::from([u8::try_from(code & 0xff).unwrap_or(0)]),
        };
    }
    let text = value.text(convfmt);
    Vec::from(first_chars(&text, 1))
}

/// What `printf` writes with the format `form` and the values `args`;
/// a value missing is "" and 0, and a `%` with no conversion after it is
/// written as it is. Numbers become strings for `%s` as `convfmt` says.
#[must_use]
pub fn sprintf(form: &[u8], args: &[Value], convfmt: &[u8]) -> Vec<u8> {
    let mut written = Vec::new();
    let mut args = args.iter();
    let mut next = || args.next().cloned().unwrap_or(Value::Unset);
    let mut at = 0;
    while at < form.len() {
        let byte = form[at];
        at += 1;
        if byte != b'%' {
            written.push(byte);
            continue;
        }
        if form.get(at) == Some(&b'%') {
            written.push(b'%');
            at += 1;
            continue;
        }
        let Some((mut spec, stars, len)) = format::parse(&form[at..]) else {
            written.push(b'%');
            continue;
        };
        at += len;
        if stars.width {
            let width = format::whole(next().number());
            spec.left |= width < 0;
            spec.width = Some(
                usize::try_from(width.unsigned_abs())
                    .unwrap_or(format::MOST)
                    .min(format::MOST),
            );
        }
        if stars.precision {
            let precision = format::whole(next().number());
            spec.precision = usize::try_from(precision).ok().map(|p| p.min(format::MOST));
        }
        let value = next();
        let shown = match spec.letter {
            b'd' | b'i' | b'o' | b'u' | b'x' | b'X' => {
                Vec::from(format::integer(&spec, format::whole(value.number())))
            }
            b'c' => padded(&spec, &character(&value, convfmt)),
            b's' => {
                let text = value.text(convfmt);
                let text = match spec.precision {
                    Some(precision) => first_chars(&text, precision),
                    None => &text,
                };
                padded(&spec, text)
            }
            _ => Vec::from(format::real(&spec, value.number())),
        };
        written.extend_from_slice(&shown);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONVFMT: &[u8] = b"%.6g";

    fn printed(form: &str, args: &[Value]) -> Vec<u8> {
        sprintf(form.as_bytes(), args, CONVFMT)
    }

    fn text(t: &str) -> Value {
        Value::Text(t.as_bytes().to_vec())
    }

    #[test]
    fn characters_count_as_regex_counts_them() {
        assert_eq!(chars(b"abc"), 3);
        assert_eq!(chars("äöü".as_bytes()), 3);
        assert_eq!(chars(b"a\xffb\xc3"), 4, "a byte that is no UTF-8 is one");
        assert_eq!(chars(b""), 0);
        assert_eq!(first_chars("äöü".as_bytes(), 2), "äö".as_bytes());
        assert_eq!(first_chars(b"a\xffb", 2), b"a\xff");
        assert_eq!(first_chars(b"ab", 0), b"");
        assert_eq!(first_chars(b"ab", 5), b"ab");
        assert_eq!(first_chars(b"\xff\xfe", 1), b"\xff");
    }

    #[test]
    fn conversions_take_the_values_in_turn() {
        let args = [Value::Number(42.0), text("hi"), Value::Number(2.5)];
        assert_eq!(printed("%d|%s|%.2f|", &args), b"42|hi|2.50|");
        assert_eq!(printed("100%% %d", &[Value::Number(7.0)]), b"100% 7");
        assert_eq!(
            printed("[%5s][%-5s]", &[text("ab"), text("ab")]),
            b"[   ab][ab   ]"
        );
        assert_eq!(
            printed("%.1s", &[text("äb")]),
            "ä".as_bytes(),
            "a precision counts characters"
        );
        assert_eq!(
            printed("%3s|", &[text("äb")]),
            " äb|".as_bytes(),
            "so does a width"
        );
        assert_eq!(
            printed(
                "%x %X %o %u",
                &[
                    Value::Number(255.0),
                    Value::Number(255.0),
                    Value::Number(8.0),
                    Value::Number(-1.0)
                ]
            ),
            b"ff FF 10 18446744073709551615"
        );
        assert_eq!(
            printed("%i %d", &[Value::Number(-3.9), text("12abc")]),
            b"-3 12"
        );
        assert_eq!(
            printed(
                "%e %G",
                &[Value::Number(1234.5), Value::Number(0.000_012_5)]
            ),
            b"1.234500e+03 1.25E-05"
        );
        assert_eq!(
            printed("%s %s", &[Value::Number(0.1), Value::Number(3.0)]),
            b"0.1 3"
        );
        assert_eq!(
            sprintf(b"%s", &[Value::Number(0.123_456_789)], b"%.2g"),
            b"0.12",
            "numbers are strings as CONVFMT says"
        );
    }

    #[test]
    fn stars_take_a_width_and_a_precision_from_the_values() {
        assert_eq!(
            printed("[%*d]", &[Value::Number(5.0), Value::Number(42.0)]),
            b"[   42]"
        );
        assert_eq!(
            printed("[%*d]", &[Value::Number(-5.0), Value::Number(42.0)]),
            b"[42   ]",
            "below zero it is to the left"
        );
        assert_eq!(
            printed("[%.*f]", &[Value::Number(1.0), Value::Number(2.25)]),
            b"[2.2]"
        );
        assert_eq!(
            printed("[%.*s]", &[Value::Number(-1.0), text("abc")]),
            b"[abc]",
            "a precision below zero is none"
        );
        assert_eq!(
            printed(
                "[%*.*s]",
                &[Value::Number(4.0), Value::Number(2.0), text("abc")]
            ),
            b"[  ab]"
        );
        assert_eq!(
            printed("[%*d]", &[Value::Number(1e9), Value::Number(1.0)]).len(),
            format::MOST + 2,
            "a width stops at the most"
        );
    }

    #[test]
    fn c_writes_the_character_of_a_code_or_the_first_of_a_string() {
        assert_eq!(
            printed(
                "%c%c%c",
                &[Value::Number(65.0), text("bcd"), Value::Number(228.0)]
            ),
            "Abä".as_bytes()
        );
        assert_eq!(
            printed("%c", &[Value::input(b"66".to_vec())]),
            b"B",
            "a numeric string is a number"
        );
        assert_eq!(
            printed("%c", &[Value::Number(55296.0)]),
            b"\x00",
            "no character: its lowest byte"
        );
        assert_eq!(printed("%c", &[Value::Number(-5.0)]), b"\x00");
        assert_eq!(printed("[%3c]", &[text("äx")]), "[  ä]".as_bytes());
        assert_eq!(printed("%c", &[text("")]), b"");
    }

    #[test]
    fn what_is_missing_is_empty_and_what_is_no_conversion_stays() {
        assert_eq!(printed("%d %s|", &[]), b"0 |");
        assert_eq!(printed("%y %", &[Value::Number(1.0)]), b"%y %");
        assert_eq!(
            printed("%5%", &[]),
            b"%5%",
            "% with a width is no conversion"
        );
        assert_eq!(printed("a\x00b", &[]), b"a\x00b");
        assert_eq!(
            printed("%s %s", &[text("a"), text("b"), text("c")]),
            b"a b",
            "values left over are not written"
        );
        assert_eq!(
            sprintf(b"\xff%s", &[Value::Text(b"\xfe".to_vec())], CONVFMT),
            b"\xff\xfe"
        );
    }
}
