//! Looking up settings by path, the way the loader reads `boot.meri`.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]

use meri::{Code, Document, Error, Item, Pos, Value};

const BOOT: &str = r#"# The loader's settings.
timeout = 1s
boot {
    timeout = 3s
    default = meridian
    title = "Meridian"
    quiet = true
    tries = 3
    scale = 1.25
    memory = 512MiB
    order = [meridian, windows]
    screen { width = 1920 }
}
display "DP-1" { scale = 2 }
display { scale = 1 }
"#;

fn doc() -> Document<'static> {
    Document::parse(BOOT.as_bytes()).unwrap()
}

fn at(line: usize, col: usize) -> Pos {
    Pos { line, col }
}

fn wrong_type(line: usize, col: usize) -> Error {
    Error {
        code: Code::WrongType,
        at: at(line, col),
    }
}

#[test]
fn parse_reports_the_first_error() {
    let err = Document::parse(b"a = 1\na = 2").unwrap_err();
    assert_eq!(err.code, Code::DuplicateKey);
    assert_eq!(err.at, at(2, 1));
    assert!(Document::parse(b"").is_ok());
}

#[test]
fn paths_lead_through_unlabelled_blocks() {
    let d = doc();
    assert_eq!(d.duration("timeout"), Ok(Some(1_000_000_000)));
    assert_eq!(d.duration("boot.timeout"), Ok(Some(3_000_000_000)));
    assert_eq!(d.int("boot.screen.width"), Ok(Some(1920)));
    assert_eq!(
        d.int("display.scale"),
        Ok(Some(1)),
        "the labelled display is not on the path"
    );
}

#[test]
fn missing_settings_are_none() {
    let d = doc();
    for path in [
        "missing",
        "boot.missing",
        "screen.width",
        "boot.screen.height",
        "timeout.x",
        "boot.timeout.x",
        "boot.default.x",
        "",
        ".",
        "boot.",
    ] {
        assert_eq!(d.get(path), None, "{path}");
        assert_eq!(d.int(path), Ok(None), "{path}");
    }
}

#[test]
fn typed_lookups() {
    let d = doc();
    assert_eq!(d.word("boot.default"), Ok(Some("meridian")));
    assert_eq!(d.bool("boot.quiet"), Ok(Some(true)));
    assert_eq!(d.int("boot.tries"), Ok(Some(3)));
    assert_eq!(d.float("boot.scale"), Ok(Some(1.25)));
    assert_eq!(
        d.float("boot.tries"),
        Ok(Some(3.0)),
        "an integer serves as a float"
    );
    assert_eq!(d.size("boot.memory"), Ok(Some(512 * 1024 * 1024)));
    let title = d.text("boot.title").unwrap().unwrap();
    assert_eq!(title, "Meridian");
    let word = d.text("boot.default").unwrap().unwrap();
    assert_eq!(word, "meridian", "a word serves as text");
    assert_eq!(word.as_str(), Some("meridian"));
}

#[test]
fn a_value_of_the_wrong_type_is_an_error_at_the_value() {
    let d = doc();
    assert_eq!(d.int("boot.title"), Err(wrong_type(6, 13)));
    assert_eq!(d.word("boot.title"), Err(wrong_type(6, 13)));
    assert_eq!(d.bool("boot.tries"), Err(wrong_type(8, 13)));
    assert_eq!(d.float("boot.memory"), Err(wrong_type(10, 14)));
    assert_eq!(d.size("boot.tries"), Err(wrong_type(8, 13)));
    assert_eq!(d.duration("boot.memory"), Err(wrong_type(10, 14)));
    assert_eq!(d.size("boot.timeout"), Err(wrong_type(4, 15)));
    assert_eq!(d.duration("boot.default"), Err(wrong_type(5, 15)));
    assert!(d.text("boot.order").is_err());
    assert_eq!(d.int("boot.quiet"), Err(wrong_type(7, 13)));
}

#[test]
fn blocks_where_a_value_should_be_are_the_wrong_type() {
    let d = doc();
    assert_eq!(d.get("boot"), Some(Item::Block { at: at(3, 1) }));
    assert_eq!(d.get("boot.screen"), Some(Item::Block { at: at(12, 5) }));
    assert_eq!(d.int("boot.screen"), Err(wrong_type(12, 5)));
    assert_eq!(d.value("display"), Err(wrong_type(14, 1)));
}

#[test]
fn values_come_with_their_position() {
    let d = doc();
    let (value, pos) = d.value("boot.order").unwrap().unwrap();
    assert_eq!(value, Value::List);
    assert_eq!(pos, at(11, 13));
    assert_eq!(
        d.get("boot.tries"),
        Some(Item::Entry {
            value: Value::Int(3),
            at: at(8, 13)
        })
    );
}

#[test]
fn sizes_and_durations_out_of_range_point_at_the_value() {
    let d = Document::parse(b"s = 1.5B\nt = 0.5ns\nn = -1KiB").unwrap();
    let out = |line| Error {
        code: Code::OutOfRange,
        at: at(line, 5),
    };
    assert_eq!(d.size("s"), Err(out(1)));
    assert_eq!(d.duration("t"), Err(out(2)));
    assert_eq!(d.size("n"), Err(out(3)));
}

#[test]
fn the_first_of_repeated_blocks_that_has_the_setting_counts() {
    let d = Document::parse(b"b { }\nb { x = 1 }\nb { x = 2 }").unwrap();
    assert_eq!(d.int("b.x"), Ok(Some(1)));
}

#[test]
fn blocks_off_the_path_are_skipped_with_all_they_hold() {
    let src = b"z { a { k = 2 } }\nk = 0\na \"l\" { k = 3 }\na { b { k = 4 } }\na { k = 1 }";
    let d = Document::parse(src).unwrap();
    assert_eq!(d.int("a.k"), Ok(Some(1)));
    assert_eq!(d.int("k"), Ok(Some(0)));
    assert_eq!(d.int("a.b.k"), Ok(Some(4)));
    assert_eq!(d.int("z.a.k"), Ok(Some(2)));
    assert_eq!(d.int("b.k"), Ok(None));
}

#[test]
fn leaving_a_block_on_the_path_goes_back_up() {
    let src = b"a { b { } }\nc = 1\na { c = 2 }\nb { c = 3 }";
    let d = Document::parse(src).unwrap();
    assert_eq!(d.int("c"), Ok(Some(1)));
    assert_eq!(d.int("a.c"), Ok(Some(2)));
    assert_eq!(d.int("b.c"), Ok(Some(3)));
    assert_eq!(d.int("a.b.c"), Ok(None));
}

#[test]
fn lists_are_stepped_over() {
    let src = b"l = [[1], [k]]\nk = 1\nb { l = [2, [3]]\n k = 2 }";
    let d = Document::parse(src).unwrap();
    assert_eq!(d.int("k"), Ok(Some(1)));
    assert_eq!(d.int("b.k"), Ok(Some(2)));
}
