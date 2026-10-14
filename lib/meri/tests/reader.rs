//! The reader's events with their positions, and the code and position of
//! errors beyond what the conformance suite covers. Every expectation here
//! was checked against the Go reader in sextant as well.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]

mod common;

use common::{error_line, trace};
use meri::{Code, Event, Reader, Value};

fn lines(src: &[u8]) -> Vec<String> {
    trace(src)
}

#[test]
fn events_follow_the_document() {
    let src = b"a = 1\nb { c = [2, [true], \"x\"] }\nd \"l\" {\n}\n";
    let want = [
        "1:1 a = int 1 @1:5",
        "2:1 block b",
        "2:5 c = list @2:9",
        "2:10 int 2",
        "2:13 list",
        "2:14 bool true",
        "list end",
        "2:21 string \"x\"",
        "list end",
        "end",
        "3:1 block d \"l\"",
        "end",
    ];
    assert_eq!(lines(src), want);
}

#[test]
fn columns_count_characters_not_bytes() {
    let src = "k = \"äöü\" # 漢字\nl = [ \"😀\", x ]";
    let want = [
        "1:1 k = string \"äöü\" @1:5",
        "2:1 l = list @2:5",
        "2:7 string \"😀\"",
        "2:12 word x",
        "list end",
    ];
    assert_eq!(lines(src.as_bytes()), want);
}

#[test]
fn a_byte_order_mark_takes_no_column() {
    let src = b"\xef\xbb\xbfa = 1\nb = 2";
    assert_eq!(lines(src), ["1:1 a = int 1 @1:5", "2:1 b = int 2 @2:5"]);
    assert_eq!(lines(b"\xef\xbb\xbf"), Vec::<String>::new());
}

#[test]
fn crlf_ends_lines_like_lf() {
    let src = b"a = 1\r\nb = [\r\n  2,\r\n]\r\n";
    let want = [
        "1:1 a = int 1 @1:5",
        "2:1 b = list @2:5",
        "3:3 int 2",
        "list end",
    ];
    assert_eq!(lines(src), want);
}

#[test]
fn a_tab_is_one_column() {
    assert_eq!(lines(b"\ta =\t1"), ["1:2 a = int 1 @1:6"]);
}

#[test]
fn blocks_fit_on_one_line() {
    let want = [
        "1:1 block outer",
        "1:9 block inner",
        "1:17 level = int 2 @1:25",
        "end",
        "end",
    ];
    assert_eq!(lines(b"outer { inner { level = 2 } }"), want);
    let want = [
        "1:1 block a",
        "1:5 block b",
        "end",
        "end",
        "2:1 c = int 2 @2:5",
    ];
    assert_eq!(lines(b"a { b { } }\nc = 2"), want);
}

#[test]
fn lists_span_lines_with_comments() {
    let src = b"l = [ # start\n  1, # one\n  [ 2 ],\n]";
    let want = [
        "1:1 l = list @1:5",
        "2:3 int 1",
        "3:3 list",
        "3:5 int 2",
        "list end",
        "list end",
    ];
    assert_eq!(lines(src), want);
}

#[test]
fn empty_lists_still_end() {
    let want = [
        "1:1 e = list @1:5",
        "list end",
        "2:1 f = list @2:5",
        "2:6 list",
        "list end",
        "list end",
    ];
    assert_eq!(lines(b"e = []\nf = [[]]"), want);
}

#[test]
fn labels_are_words_or_strings() {
    let src = b"b true { }\nc \"a\\\"b\" { }\nd _x-1 {}";
    let want = [
        "1:1 block b \"true\"",
        "end",
        "2:1 block c \"a\\\"b\"",
        "end",
        "3:1 block d \"_x-1\"",
        "end",
    ];
    assert_eq!(lines(src), want);
}

#[test]
fn a_text_is_one_value() {
    let src = b"t = \"\"\"\n    one\n\n      two\n    \"\"\"\nu = 1";
    let want = [
        "1:1 t = string \"one\\n\\n  two\" @1:5",
        "6:1 u = int 1 @6:5",
    ];
    assert_eq!(lines(src), want);
}

#[test]
fn every_kind_of_value() {
    let src = b"t = true\nf = false\nw = kvm\ni = -12\nh = 0xff\no = 0o755\nb = 0b1010\nx = 6.02e23\nq = 1.5h\np = 125%\ns = \"x\"";
    let want = [
        "1:1 t = bool true @1:5",
        "2:1 f = bool false @2:5",
        "3:1 w = word kvm @3:5",
        "4:1 i = int -12 @4:5",
        "5:1 h = int 255 @5:5",
        "6:1 o = int 493 @6:5",
        "7:1 b = int 10 @7:5",
        "8:1 x = float 6.02e23 @8:5",
        "9:1 q = quantity 1.5 h @9:5",
        "10:1 p = quantity 125 % @10:5",
        "11:1 s = string \"x\" @11:5",
    ];
    assert_eq!(lines(src), want);
}

#[test]
fn events_before_an_error_still_come() {
    let want = [
        "1:1 a = int 1 @1:5",
        "2:1 b = list @2:5",
        "2:6 int 2",
        "error 2:9 unexpected-character",
    ];
    assert_eq!(lines(b"a = 1\nb = [2, @]"), want);
}

#[test]
fn documents_without_items_have_no_events() {
    for src in [&b""[..], b"\n\n", b"# only a comment", b"  \t\r\n# x\r\n"] {
        assert_eq!(lines(src), Vec::<String>::new(), "{src:?}");
    }
}

#[test]
fn reading_ends_after_an_error() {
    let mut reader = Reader::new(b"a = 1\n@\nb = 2");
    assert!(matches!(reader.next(), Some(Ok(Event::Entry { .. }))));
    let err = reader.next().unwrap().unwrap_err();
    assert_eq!(
        (err.code, err.at.line, err.at.col),
        (Code::UnexpectedCharacter, 2, 1)
    );
    assert!(reader.next().is_none());
    assert!(reader.next().is_none());
}

#[test]
fn reading_ends_after_the_last_event() {
    let mut reader = Reader::new(b"a = 1");
    assert!(matches!(
        reader.next(),
        Some(Ok(Event::Entry {
            value: Value::Int(1),
            ..
        }))
    ));
    assert!(reader.next().is_none());
    assert!(reader.next().is_none());
}

#[test]
fn columns_stay_right_along_a_long_line() {
    let mut src = String::from("l = [");
    for _ in 0..2000 {
        src.push_str("1, ");
    }
    src.push(']');
    let cols: Vec<usize> = Reader::new(src.as_bytes())
        .filter_map(|event| match event.unwrap() {
            Event::Element { at, .. } => Some(at.col),
            _ => None,
        })
        .collect();
    assert_eq!(cols.len(), 2000);
    for (k, col) in cols.iter().enumerate() {
        assert_eq!(*col, 6 + 3 * k);
    }
}

#[test]
fn sixty_four_levels_of_blocks_and_lists_together_are_allowed() {
    let blocks = "b {\n".repeat(63);
    let ok = format!("{blocks}l = [1]\n{}", "}\n".repeat(63));
    assert_eq!(error_line(ok.as_bytes()), None);
    let deeper = format!("{blocks}b {{ l = [1] }}\n{}", "}\n".repeat(63));
    assert_eq!(
        error_line(deeper.as_bytes()).as_deref(),
        Some("64:9 too-deep")
    );
    let lists = format!("l = {}{}", "[".repeat(64), "]".repeat(64));
    assert_eq!(error_line(lists.as_bytes()), None);
    let lists = format!("b {{ l = {}{} }}", "[".repeat(64), "]".repeat(64));
    assert_eq!(
        error_line(lists.as_bytes()).as_deref(),
        Some("1:72 too-deep")
    );
}

/// Documents with the first error each must report, by line, column and
/// code.
const ERRORS: &[(&[u8], &str)] = &[
    (b"a {\n  b {\n", "2:3 unclosed-block"),
    (b"a {\n  b { }\n", "1:1 unclosed-block"),
    (b"}", "1:1 unexpected-close"),
    (b"a = 1 }", "1:7 unexpected-close"),
    (b"a", "1:2 expected-equals"),
    (b"a   ", "1:5 expected-equals"),
    (b"a\n= 1", "1:2 expected-equals"),
    (b"a # c", "1:3 expected-equals"),
    (b"a b c", "1:5 expected-brace"),
    (b"a \"x\"", "1:6 expected-brace"),
    (b"a b", "1:4 expected-brace"),
    (b"a b\n{ }", "1:4 expected-brace"),
    (b"a =", "1:4 expected-value"),
    (b"a = # c", "1:5 expected-value"),
    (b"a = }", "1:5 expected-value"),
    (b"a =\r\n1", "1:4 expected-value"),
    (b"a = ,", "1:5 unexpected-character"),
    (b"a = ]", "1:5 unexpected-character"),
    (b"a = [,]", "1:6 expected-value"),
    (b"a = [1,,]", "1:8 expected-value"),
    (b"a = [1 2]", "1:8 expected-comma"),
    (b"a = [1\n  2]", "2:3 expected-comma"),
    (b"a = [[1] 2]", "1:10 expected-comma"),
    (b"a = [1 }", "1:8 expected-comma"),
    (b"a = [", "1:5 unterminated-list"),
    (b"a = [1,\n", "1:5 unterminated-list"),
    (b"a = [[1]", "1:5 unterminated-list"),
    (b"a = [1 # c", "1:5 unterminated-list"),
    (b"a = 1 2", "1:7 expected-newline"),
    (b"a = \"x\" y", "1:9 expected-newline"),
    (b"a { } b { }", "1:7 expected-newline"),
    (b"a = [1] x", "1:9 expected-newline"),
    (b"a = true\tfalse", "1:10 expected-newline"),
    (b"a = @", "1:5 unexpected-character"),
    (b"@", "1:1 unexpected-character"),
    (b"  1 = 2", "1:3 unexpected-character"),
    (b"a = .5", "1:5 unexpected-character"),
    (b"a = +1", "1:5 unexpected-character"),
    (b"a\r", "1:2 lone-carriage-return"),
    (b"a = 1\r", "1:6 lone-carriage-return"),
    (b"a = 1 # c\r", "1:10 lone-carriage-return"),
    (b"a = [\r1]", "1:6 lone-carriage-return"),
    (b"\r\n\r", "2:1 lone-carriage-return"),
    (b"a = \"\"\"\r", "1:8 lone-carriage-return"),
    (b"a = \"\"\"\nx\ry\n\"\"\"", "2:2 lone-carriage-return"),
    (b"a = \"x\r\"", "1:7 lone-carriage-return"),
    (b"\x01", "1:1 control-character"),
    (b"a \x01", "1:3 control-character"),
    (b"a = \x01", "1:5 control-character"),
    (b"a = 1\x7f", "1:6 control-character"),
    (b"a = [1, \x0b]", "1:9 control-character"),
    (b"a = [1 \x0c]", "1:8 control-character"),
    (b"# \x1f", "1:3 control-character"),
    (b"a = \"\x00\"", "1:6 control-character"),
    (b"a = \"\"\" \x0e", "1:9 control-character"),
    (b"a b \x08", "1:5 control-character"),
    (b"\xff", "1:1 invalid-utf8"),
    (b"\xc3\xa4", "1:1 unexpected-character"),
    (b"a\xff", "1:2 invalid-utf8"),
    (b"a = \xff", "1:5 invalid-utf8"),
    (b"a \xff {", "1:3 invalid-utf8"),
    (b"a b \xff", "1:5 invalid-utf8"),
    (b"a = \"\xc3\xa4\xc3\"", "1:7 invalid-utf8"),
    (b"a = 1\xff", "1:6 invalid-utf8"),
    (b"a = [1\xff]", "1:7 invalid-utf8"),
    (b"a = [1, \xff]", "1:9 invalid-utf8"),
    (b"a = 1 # \xe6\xbc\xa2\xed\xa0\x80", "1:10 invalid-utf8"),
    (b"a = \"\"\"\n  \xc0\x80\n\"\"\"", "2:3 invalid-utf8"),
    (b"a = \"\\\xff\"", "1:6 invalid-escape"),
    (b"a = \"\\u{\xff}\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\"\" \xff", "1:9 invalid-utf8"),
    (b"a = { \xff", "1:5 unexpected-character"),
    (b"a = \"x", "1:5 unterminated-string"),
    (b"a = \"x\ny\"", "1:5 unterminated-string"),
    (b"a \"x", "1:3 unterminated-string"),
    (b"a = [\"x\", \"y\r\n\"]", "1:11 unterminated-string"),
    (b"a = \"\\q\"", "1:6 invalid-escape"),
    (b"a = \"x\\", "1:7 invalid-escape"),
    (b"a = \"\\\n\"", "1:6 invalid-escape"),
    (b"a = \"\\U{41}\"", "1:6 invalid-escape"),
    (b"a = \"\\u41\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\\u{}\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\\u{0000041}\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\\u{110000}\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\\u{dfff}\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\\u{d7ff}\\u{DC00}\"", "1:14 invalid-unicode-escape"),
    (b"a = \"\\u{g}\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\\u{41\"", "1:6 invalid-unicode-escape"),
    (b"a = \"\\u{41", "1:6 invalid-unicode-escape"),
    (b"a = \"\"\" x", "1:9 text-opening"),
    (b"a = \"\"\"\"", "1:8 text-opening"),
    (b"a = \"\"\"", "1:5 unterminated-text"),
    (b"a = \"\"\"\n", "1:5 unterminated-text"),
    (b"a = \"\"\"\nx", "1:5 unterminated-text"),
    (b"a = \"\"\"\n  x\n", "1:5 unterminated-text"),
    (b"a = \"\"\"\n  x\n \"\"", "1:5 unterminated-text"),
    (b"a = \"\"\"\n    a\n  b\n  \"\"\" x", "4:7 expected-newline"),
    (b"a = \"\"\"\n  a\n b\n  \"\"\"", "3:1 text-indent"),
    (b"a = \"\"\"\n  a\n\n   \n  b\n x\n  \"\"\"", "6:1 text-indent"),
    (b"a = \"\"\"\n \ta\n  \"\"\"", "2:1 text-indent"),
    (b"a = \"\"\"\n x\n \x01\n  \"\"\"", "3:2 control-character"),
    (b"a = -", "1:5 invalid-number"),
    (b"a = --1", "1:5 invalid-number"),
    (b"a = -x", "1:5 invalid-number"),
    (b"a = 1_", "1:5 invalid-number"),
    (b"a = 1__2", "1:5 invalid-number"),
    (b"a = 01", "1:5 invalid-number"),
    (b"a = -01", "1:5 invalid-number"),
    (b"a = 0_1", "1:5 invalid-number"),
    (b"a = 00", "1:5 invalid-number"),
    (b"a = 1.", "1:5 invalid-number"),
    (b"a = 1.e5", "1:5 invalid-number"),
    (b"a = 1._5", "1:5 invalid-number"),
    (b"a = 1.5_", "1:5 invalid-number"),
    (b"a = 1e5_", "1:5 invalid-number"),
    (b"a = 1e_5", "1:5 invalid-number"),
    (b"a = 1.5.2", "1:5 invalid-number"),
    (b"a = 1%%", "1:5 invalid-number"),
    (b"a = 1%a", "1:5 invalid-number"),
    (b"a = 1a1", "1:5 invalid-number"),
    (b"a = 1a_", "1:5 invalid-number"),
    (b"a = 1a.", "1:5 invalid-number"),
    (b"a = 0x1.", "1:5 invalid-number"),
    (b"a = 0x1_", "1:5 invalid-number"),
    (b"a = 0x1__2", "1:5 invalid-number"),
    (b"a = 0b12", "1:5 invalid-number"),
    (b"a = 0b2", "1:5 invalid-number"),
    (b"a = 0o8", "1:5 invalid-number"),
    (b"a = 0o78", "1:5 invalid-number"),
    (b"a = 0X1", "1:5 invalid-number"),
    (b"a = 0B1", "1:5 invalid-number"),
    (b"a = 0x1g", "1:5 unit-not-allowed"),
    (b"a = 0xfG", "1:5 unit-not-allowed"),
    (b"a = 0o7a", "1:5 unit-not-allowed"),
    (b"a = 0b1%", "1:5 unit-not-allowed"),
    (b"a = -0b1z", "1:5 unit-not-allowed"),
    (b"a = 9223372036854775808", "1:5 number-overflow"),
    (b"a = -9223372036854775809", "1:5 number-overflow"),
    (b"a = 9_223_372_036_854_775_808B", "1:5 number-overflow"),
    (b"a = 0x8000_0000_0000_0000", "1:5 number-overflow"),
    (b"a = -0x8000_0000_0000_0001", "1:5 number-overflow"),
    (b"a = 0o1000000000000000000000", "1:5 number-overflow"),
    (b"a = 0b1000000000000000000000000000000000000000000000000000000000000000", "1:5 number-overflow"),
    (b"a = -0b10000000000000000000000000000000000000000000000000000000000000000", "1:5 number-overflow"),
    (b"a = 1e309", "1:5 number-overflow"),
    (b"a = -1.8e308", "1:5 number-overflow"),
    (b"a = 179769313486231590000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000.0", "1:5 number-overflow"),
    (b"a = [1, 1e999s]", "1:9 number-overflow"),
    (b"a = 1\na = 2", "2:1 duplicate-key"),
    (b"a = 1\na = @", "2:1 duplicate-key"),
    (b"a = 1\nb { a = 1 }\nb { a = 2 }\na = 3", "4:1 duplicate-key"),
    (b"a { }\na = 1", "2:1 name-conflict"),
    (b"a \"x\" { }\na = 1", "2:1 name-conflict"),
    (b"a = 1\na { }", "2:1 name-conflict"),
    (b"a = 1\na x { }", "2:1 name-conflict"),
    (b"a x { }\na \"x\" { }", "2:1 duplicate-label"),
    (b"a \"m\\u{61}in\" { }\na main { }", "2:1 duplicate-label"),
    (b"a \"\\t\" { }\na \"\\u{9}\" { }", "2:1 duplicate-label"),
];

#[test]
fn errors_point_at_the_problem() {
    let mut failed = Vec::new();
    for (src, want) in ERRORS {
        let got = error_line(src);
        if got.as_deref() != Some(*want) {
            failed.push(format!(
                "{:?}: got {got:?}, want {want}",
                String::from_utf8_lossy(src)
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
