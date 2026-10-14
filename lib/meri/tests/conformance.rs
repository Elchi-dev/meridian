//! The conformance suite in `tests/meri`, which the Go reader in sextant
//! passes too: every valid document must dump byte for byte to its
//! `.dump`, and every invalid one must fail with the code and at the
//! position its `.error` names.

#![allow(clippy::unwrap_used)]

mod common;

use common::{dump, error_line, suite};

#[test]
fn every_valid_document_dumps_as_the_suite_says() {
    let cases = suite("valid", "dump");
    assert!(
        cases.len() >= 18,
        "found only {} valid cases; is the suite missing?",
        cases.len()
    );
    let mut failed = Vec::new();
    for (name, src, want) in &cases {
        match dump(src) {
            Ok(got) if got.as_bytes() == want.as_slice() => {}
            Ok(got) => failed.push(format!(
                "{name}: dump differs\ngot:\n{got}want:\n{}",
                String::from_utf8_lossy(want)
            )),
            Err(err) => failed.push(format!(
                "{name}: rejected with {} at {}",
                err.code.name(),
                err.at
            )),
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} valid cases fail:\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n")
    );
}

#[test]
fn every_invalid_document_fails_where_the_suite_says() {
    let cases = suite("invalid", "error");
    assert!(
        cases.len() >= 116,
        "found only {} invalid cases; is the suite missing?",
        cases.len()
    );
    let mut failed = Vec::new();
    for (name, src, want) in &cases {
        let want = String::from_utf8_lossy(want).trim_end().to_string();
        match error_line(src) {
            Some(got) if got == want => {}
            Some(got) => failed.push(format!("{name}: got {got}, want {want}")),
            None => failed.push(format!("{name}: accepted, want {want}")),
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} invalid cases fail:\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n")
    );
}
