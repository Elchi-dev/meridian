//! The rules for names within one block: a key is set once, a name is
//! either a key or the name of blocks, and labels of blocks with the same
//! name differ. The reader checks them by reading the block again, so
//! these cases put strings, texts, lists and nested blocks in the way.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]

mod common;

use std::fmt::Write;

use common::error_line;

const ALLOWED: &[&[u8]] = &[
    b"a = 1\nb { a = 2 }",
    b"b { a = 1 }\na = 2",
    b"b { }\nb { }\nb x { }\nb \"y\" { }\nb { }",
    b"a \"x\" { }\nb \"x\" { }",
    b"t = \"\"\"\n  a = 1\n  }\n  \"\"\"\na = 2",
    b"s = \"a = 1 # } {\"\na = 2",
    b"l = [\n  1, # a = 1\n  [2],\n]\na = 3",
    b"a { a { a = 1 } }",
    b"x { a = 1 }\nx { a = 2 }",
    b"d { }\nd \"\" { }",
    b"d \"a\" { }\nd \"ab\" { }\nd \"b\" { }",
    b"d \"ab\" { }\nd \"a\" { }",
];

const REFUSED: &[(&[u8], &str)] = &[
    (b"o { p = 1\n  q = 2\n  p = 3 }", "3:3 duplicate-key"),
    (
        b"a = 1\nb { }\nc = [1]\nd = \"\"\"\n  x\n  \"\"\"\na = 2",
        "7:1 duplicate-key",
    ),
    (b"a { a = 1\n a = 2 }", "2:2 duplicate-key"),
    (b"a = 1\nb { a { } }\na { }", "3:1 name-conflict"),
    (b"b { a { } }\na = 1\na { }", "3:1 name-conflict"),
    (b"a { }\nb = 1\na = 2", "3:1 name-conflict"),
    (
        b"d \"x\\u{1F600}\" { }\nd \"x\xf0\x9f\x98\x80\" { }",
        "2:1 duplicate-label",
    ),
    (b"d \"\" { }\nd \"\" { }", "2:1 duplicate-label"),
    (b"d a { }\nd { }\nd b { }\nd a { }", "4:1 duplicate-label"),
    (b"\xef\xbb\xbfa = 1\na = 2", "2:1 duplicate-key"),
    (b"a = 1\r\nb { c = 1\r\n c = 2 }", "3:2 duplicate-key"),
    (b"a = 1\nb = 2\nc = 3\nb = 4", "4:1 duplicate-key"),
];

#[test]
fn names_that_follow_the_rules_are_allowed() {
    for src in ALLOWED {
        assert_eq!(error_line(src), None, "{:?}", String::from_utf8_lossy(src));
    }
}

#[test]
fn names_that_break_the_rules_are_refused_at_the_second_one() {
    for (src, want) in REFUSED {
        assert_eq!(
            error_line(src).as_deref(),
            Some(*want),
            "{:?}",
            String::from_utf8_lossy(src)
        );
    }
}

#[test]
fn many_keys_in_one_block() {
    let mut src = String::new();
    for k in 0..300 {
        writeln!(src, "k{k} = {k}").unwrap();
    }
    assert_eq!(error_line(src.as_bytes()), None);
    src.push_str("k150 = 1\n");
    assert_eq!(
        error_line(src.as_bytes()).as_deref(),
        Some("301:1 duplicate-key")
    );
}

#[test]
fn the_innermost_of_deep_blocks_checks_only_its_own_items() {
    let mut src = String::new();
    for _ in 0..63 {
        src.push_str("b {\n  a = 1\n");
    }
    src.push_str("a = 2\n");
    src.push_str(&"}\n".repeat(63));
    assert_eq!(
        error_line(src.as_bytes()).as_deref(),
        Some("127:1 duplicate-key")
    );
    let fixed = src.replace("a = 2", "c = 2");
    assert_eq!(error_line(fixed.as_bytes()), None);
}
