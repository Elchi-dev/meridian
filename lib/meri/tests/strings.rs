//! Strings, texts and labels: what their characters are once decoded, and
//! the ways to get at them without allocating.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]

use meri::{Event, Reader, Str, Value};

/// The value of the first entry, which must be a string.
fn string(src: &str) -> Str<'_> {
    for event in Reader::new(src.as_bytes()) {
        if let Event::Entry {
            value: Value::Str(s),
            ..
        } = event.unwrap()
        {
            return s;
        }
    }
    panic!("no string in {src:?}");
}

fn label(src: &str) -> Str<'_> {
    for event in Reader::new(src.as_bytes()) {
        if let Event::Block {
            label: Some(label), ..
        } = event.unwrap()
        {
            return label;
        }
    }
    panic!("no label in {src:?}");
}

fn decoded(s: Str) -> String {
    s.chars().collect()
}

#[test]
fn escapes_decode() {
    let s = string(r#"s = "a\\b\"c\nd\re\tf\0g\u{e9}\u{1F600}\u{000041}\u{7f}""#);
    assert_eq!(decoded(s), "a\\b\"c\nd\re\tf\0g\u{e9}\u{1F600}A\u{7f}");
}

#[test]
fn plain_characters_stay() {
    let s = string("s = \"tab\there, é, 漢, 😀, # { } [ ] = '\"");
    assert_eq!(decoded(s), "tab\there, é, 漢, 😀, # { } [ ] = '");
    assert_eq!(decoded(string("s = \"\"")), "");
}

#[test]
fn a_text_loses_its_indentation() {
    let s = string("t = \"\"\"\n    one\n      two\n    three\n    \"\"\"");
    assert_eq!(decoded(s), "one\n  two\nthree");
}

#[test]
fn blank_text_lines_become_empty_whatever_their_length() {
    let s = string("t = \"\"\"\n    a\n\n  \n        \n\t\n    b\n    \"\"\"");
    assert_eq!(decoded(s), "a\n\n\n\n\nb");
}

#[test]
fn a_text_ends_in_a_line_feed_only_after_an_empty_line() {
    assert_eq!(decoded(string("t = \"\"\"\n  a\n  \"\"\"")), "a");
    assert_eq!(decoded(string("t = \"\"\"\n  a\n\n  \"\"\"")), "a\n");
    assert_eq!(decoded(string("t = \"\"\"\n\n\n  \"\"\"")), "\n");
    assert_eq!(decoded(string("t = \"\"\"\n\n  \"\"\"")), "");
    assert_eq!(decoded(string("t = \"\"\"\n  \"\"\"")), "");
    assert_eq!(decoded(string("t = \"\"\"\n\"\"\"")), "");
}

#[test]
fn text_lines_end_in_line_feeds_even_in_crlf_documents() {
    let s = string("t = \"\"\"\r\n  a\r\n\r\n   \r\n  b\r\n  \"\"\"\r\n");
    assert_eq!(decoded(s), "a\n\n\nb");
}

#[test]
fn a_text_without_indentation_keeps_every_line() {
    let s = string("t = \"\"\"\n  a\nb\n\"\"\"");
    assert_eq!(decoded(s), "  a\nb");
}

#[test]
fn tabs_indent_a_text_too() {
    let s = string("t = \"\"\"\n\t\tx\n\t\t\ty\n\t\t\"\"\"");
    assert_eq!(decoded(s), "x\n\ty");
}

#[test]
fn a_text_takes_no_escapes() {
    let s = string("t = \"\"\"\n  a\\nb \\u{41} \"q\" \"\" x \"\"\"\n  \"\"\"");
    assert_eq!(decoded(s), "a\\nb \\u{41} \"q\" \"\" x \"\"\"");
    assert_eq!(s.as_str(), None);
}

#[test]
fn texts_and_strings_in_lists() {
    let src = "l = [\"\"\"\n  x\n  \"\"\", \"y\"]";
    let values: Vec<String> = Reader::new(src.as_bytes())
        .filter_map(|event| match event.unwrap() {
            Event::Element {
                value: Value::Str(s),
                ..
            } => Some(decoded(s)),
            _ => None,
        })
        .collect();
    assert_eq!(values, ["x", "y"]);
}

#[test]
fn as_str_borrows_what_needs_no_decoding() {
    assert_eq!(string("s = \"plain é\"").as_str(), Some("plain é"));
    assert_eq!(string("s = \"\"").as_str(), Some(""));
    assert_eq!(string(r#"s = "a\nb""#).as_str(), None);
    assert_eq!(string("s = \"\"\"\n  a\n  \"\"\"").as_str(), None);
    assert_eq!(label("b word { }").as_str(), Some("word"));
    assert_eq!(label("b \"x y\" { }").as_str(), Some("x y"));
}

#[test]
fn decode_writes_into_the_buffer() {
    let s = string(r#"s = "é\u{1F600}x""#);
    let mut buf = [0u8; 7];
    assert_eq!(s.decode(&mut buf), Some("é😀x"));
    let mut big = [0xffu8; 32];
    assert_eq!(s.decode(&mut big), Some("é😀x"));
    assert_eq!(big[7], 0xff, "decode wrote past the string");
    let t = string("t = \"\"\"\n  a\n    b\n  \"\"\"");
    assert_eq!(t.decode(&mut big), Some("a\n  b"));
}

#[test]
fn decode_refuses_a_buffer_that_is_too_small() {
    let s = string(r#"s = "é\u{1F600}x""#);
    let mut short = [0u8; 6];
    assert_eq!(s.decode(&mut short), None);
    let mut cut = [0u8; 4];
    assert_eq!(s.decode(&mut cut), None, "the emoji does not fit");
    let mut one = [0u8; 1];
    assert_eq!(s.decode(&mut one), None, "half of the é does not fit");
    let mut none = [0u8; 0];
    assert_eq!(string("s = \"\"").decode(&mut none), Some(""));
    assert_eq!(string("s = \"x\"").decode(&mut none), None);
}

#[test]
fn strings_compare_by_their_characters() {
    let plain = label("b main { }");
    let escaped = label(r#"b "m\u{61}in" { }"#);
    let other = label("b mains { }");
    assert_eq!(plain, escaped);
    assert_ne!(plain, other);
    assert_ne!(other, plain);
    assert_eq!(escaped, "main");
    assert_eq!(escaped, *"main");
    assert_ne!(escaped, *"mai");
    assert_ne!(escaped, *"mainx");
    assert_ne!(escaped, "mai");
    assert_ne!(escaped, "mainx");
    let text = string("t = \"\"\"\n  main\n  \"\"\"");
    assert_eq!(text, plain);
}

#[test]
fn display_writes_the_decoded_characters() {
    let s = string(r#"s = "a\tb\u{e9}""#);
    assert_eq!(s.to_string(), "a\tb\u{e9}");
    assert_eq!(format!("[{}]", label(r#"b "x\"y" { }"#)), "[x\"y]");
}

#[test]
fn chars_can_be_walked_more_than_once() {
    let s = string(r#"s = "a\u{62}c""#);
    let mut chars = s.chars();
    assert_eq!(chars.next(), Some('a'));
    let copy = chars.clone();
    assert_eq!(chars.collect::<String>(), "bc");
    assert_eq!(copy.collect::<String>(), "bc");
    assert_eq!(s.chars().count(), 3);
}
