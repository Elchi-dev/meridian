//! Numbers: integers in every base up to the limits of 64 bits, floats
//! rounded exactly as Rust's own parser rounds them, and quantities.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]
// Every float here must be exactly the double the specification asks for.
#![allow(clippy::float_cmp)]

use meri::{Code, Event, Number, Reader, Value};

/// The value of `a = text`. The document is leaked so that the value,
/// which borrows from it, can outlive this function.
fn value(text: &str) -> Value<'static> {
    let src: &'static str = Box::leak(format!("a = {text}").into_boxed_str());
    match Reader::new(src.as_bytes()).next() {
        Some(Ok(Event::Entry { value, .. })) => value,
        other => panic!("{text}: {other:?}"),
    }
}

fn int(text: &str) -> i64 {
    match value(text) {
        Value::Int(n) => n,
        other => panic!("{text}: {other:?}"),
    }
}

fn float(text: &str) -> f64 {
    match value(text) {
        Value::Float(f) => {
            assert_eq!(f.literal, text);
            f.value
        }
        other => panic!("{text}: {other:?}"),
    }
}

fn error(text: &str) -> Code {
    let src = format!("a = {text}");
    Reader::new(src.as_bytes())
        .find_map(Result::err)
        .unwrap()
        .code
}

/// What Rust's parser makes of the literal once the `_` are gone.
fn std_float(text: &str) -> f64 {
    text.replace('_', "").parse().unwrap()
}

#[test]
fn decimal_integers() {
    assert_eq!(int("0"), 0);
    assert_eq!(int("-0"), 0);
    assert_eq!(int("7"), 7);
    assert_eq!(int("-12"), -12);
    assert_eq!(int("1_000_000"), 1_000_000);
    assert_eq!(int("9223372036854775807"), i64::MAX);
    assert_eq!(int("-9223372036854775808"), i64::MIN);
    assert_eq!(int("9_223_372_036_854_775_807"), i64::MAX);
}

#[test]
fn integers_in_other_bases() {
    assert_eq!(int("0xff"), 255);
    assert_eq!(int("0xFF"), 255);
    assert_eq!(int("0xaBcD_eF09"), 0xabcd_ef09);
    assert_eq!(int("-0x10"), -16);
    assert_eq!(int("0o755"), 0o755);
    assert_eq!(int("0o0"), 0);
    assert_eq!(int("0b1010"), 10);
    assert_eq!(int("0b1_0"), 2);
    assert_eq!(int("-0b1"), -1);
    assert_eq!(int("0x7fff_ffff_ffff_ffff"), i64::MAX);
    assert_eq!(int("-0x8000_0000_0000_0000"), i64::MIN);
    assert_eq!(int("0o777777777777777777777"), i64::MAX);
    assert_eq!(int("-0o1000000000000000000000"), i64::MIN);
    assert_eq!(int(&format!("0b{}", "1".repeat(63))), i64::MAX);
    assert_eq!(int(&format!("-0b1{}", "0".repeat(63))), i64::MIN);
}

#[test]
fn integers_beyond_64_bits_overflow() {
    for text in [
        "9223372036854775808",
        "-9223372036854775809",
        "99999999999999999999999",
        "0x8000_0000_0000_0000",
        "-0x8000_0000_0000_0001",
        "0x1_0000_0000_0000_0000",
        "0o1000000000000000000000",
        "-0o1000000000000000000001",
        "0b1000000000000000000000000000000000000000000000000000000000000000",
        "9223372036854775808KiB",
    ] {
        assert_eq!(error(text), Code::NumberOverflow, "{text}");
    }
}

#[test]
fn floats_round_like_rusts_parser() {
    for text in [
        "1.5",
        "-0.25",
        "6.02e23",
        "6.02E23",
        "1e+5",
        "1e-5",
        "1_000.000_1",
        "1.7976931348623157e308",
        "4.9e-324",
        "2.4e-324",
        "1e-400",
        "0.1",
        "0.3",
        "123456789012345678901234567890.0",
        "9007199254740993.0",
        "9007199254740993.0000000000000000001",
        "2.2250738585072011e-308",
        "0.000_000_000_000_000_000_000_000_1e30",
        "1_2.3_4e5_6",
    ] {
        assert_eq!(float(text).to_bits(), std_float(text).to_bits(), "{text}");
    }
}

#[test]
fn a_negative_zero_keeps_its_sign() {
    assert!(float("-0.0").is_sign_negative());
    assert!(float("0.0").is_sign_positive());
    assert!(float("-0e999").is_sign_negative());
}

#[test]
fn long_floats_round_exactly() {
    // 2^53 + 1 lies halfway between two doubles: on its own it rounds to
    // the even one, with any digit beyond it to the one above.
    let halfway = "9007199254740993.";
    let down = format!("{halfway}{}", "0".repeat(1500));
    let up = format!("{down}1");
    assert_eq!(float(&down), 9_007_199_254_740_992.0);
    assert_eq!(float(&up), 9_007_199_254_740_994.0);
    let with_underscores = format!("9_007_199_254_740_993.{}1", "0_".repeat(900));
    assert_eq!(float(&with_underscores), 9_007_199_254_740_994.0);
    let long_and_small = format!(
        "0.{}9007199254740993{}1e320",
        "0".repeat(320),
        "0".repeat(1000)
    );
    assert_eq!(
        float(&long_and_small).to_bits(),
        std_float(&long_and_small).to_bits()
    );
    let many_digits: String = (0..2000)
        .map(|i| char::from(b"0123456789"[i * 7 % 10]))
        .collect();
    for text in [
        format!("1.{many_digits}"),
        format!("{}.5e-1990", &many_digits[1..]),
        format!("0.{many_digits}e-300"),
        format!("-1_{}.0e-1990", "0".repeat(1990)),
    ] {
        assert_eq!(float(&text).to_bits(), std_float(&text).to_bits(), "{text}");
    }
}

#[test]
fn floats_too_large_for_a_double_overflow() {
    for text in ["1e309", "-1e309", "1.8e308", "1e99999999999999999999999"] {
        assert_eq!(error(text), Code::NumberOverflow, "{text}");
    }
    let just_above_max = format!("17976931348623159{}.0", "0".repeat(292));
    assert_eq!(error(&just_above_max), Code::NumberOverflow);
    let max = format!("17976931348623157{}.0", "0".repeat(292));
    assert_eq!(float(&max), f64::MAX);
    assert_eq!(float("1e-99999999999999999999999"), 0.0);
    assert_eq!(float(&format!("0.{}1", "0".repeat(5000))), 0.0);
    assert_eq!(
        float(&format!("1{}.0e-1000", "0".repeat(1000))),
        1.0,
        "the exponent pulls a long number back"
    );
}

#[test]
fn quantities_carry_their_unit() {
    let q = |text| match value(text) {
        Value::Quantity(q) => q,
        other => panic!("{text}: {other:?}"),
    };
    assert_eq!(q("512MiB").number, Number::Int(512));
    assert_eq!(q("512MiB").unit, "MiB");
    assert_eq!(q("125%").unit, "%");
    assert_eq!(q("-3x").number, Number::Int(-3));
    assert_eq!(q("0bit").unit, "bit");
    assert_eq!(q("0B").unit, "B");
    assert_eq!(q("0x").unit, "x");
    assert_eq!(q("0o").unit, "o");
    assert_eq!(q("5em").unit, "em");
    assert_eq!(q("5e").unit, "e");
    assert_eq!(q("5e+").unit, "e");
    let q15 = q("1.5h");
    assert_eq!(q15.unit, "h");
    assert!(matches!(q15.number, Number::Float(f) if f.value == 1.5 && f.literal == "1.5"));
    let exp = q("1E+5Hz");
    assert!(matches!(exp.number, Number::Float(f) if f.value == 1e5 && f.literal == "1E+5"));
    assert_eq!(exp.unit, "Hz");
    assert!(matches!(q("1e5x").number, Number::Float(f) if f.value == 1e5));
    assert!(matches!(q("0.5%").number, Number::Float(f) if f.value == 0.5));
}

#[test]
fn a_quantity_ends_before_the_end_of_its_item() {
    let src = b"a = [1KiB, 2.5s]\nb = 3x # c\nc { d = 4% }";
    let units: Vec<&str> = Reader::new(src)
        .filter_map(|event| match event.unwrap() {
            Event::Entry {
                value: Value::Quantity(q),
                ..
            }
            | Event::Element {
                value: Value::Quantity(q),
                ..
            } => Some(q.unit),
            _ => None,
        })
        .collect();
    assert_eq!(units, ["KiB", "s", "x", "%"]);
}

#[test]
fn malformed_numbers() {
    for text in [
        "-", "--1", "-x", "-_1", "1_", "1__2", "01", "-01", "0_1", "00", "1.", "1.e5", "1._5",
        "1.5_", "1e5_", "1e_5", "1e5.5", "1.5.2", "1%%", "1%a", "1%_", "1a1", "1a_", "1a.", "1a%",
        "0x1.", "0x1_", "0x1__2", "0b12", "0b2", "0o8", "0o78", "0X1", "0B1", "0o1_8", "1E5_",
    ] {
        assert_eq!(error(text), Code::InvalidNumber, "{text}");
    }
}

#[test]
fn units_only_follow_decimal_numbers() {
    for text in [
        "0x1g", "0xfG", "0o7a", "0b1%", "-0b1z", "0x1_2Z", "0b1e5", "0o1E",
    ] {
        assert_eq!(error(text), Code::UnitNotAllowed, "{text}");
    }
}
