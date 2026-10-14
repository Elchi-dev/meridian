//! Broken documents: the suite's documents cut short and with bytes
//! changed. Whatever the bytes say, the reader must end with events or an
//! error, never panic, and point errors inside the document.

#![allow(clippy::unwrap_used)]

mod common;

use common::{Rng, suite};
use meri::{Document, Error, Event, Reader, Value};

/// Bytes that change what a document means the most.
const PIECES: &[&[u8]] = &[
    b"\r",
    b"\n",
    b"\r\n",
    b"\x00",
    b"\x7f",
    b"\xff",
    b"\xc3",
    b"\xe2\x82",
    b"\xef\xbb\xbf",
    b"\"",
    b"\"\"\"",
    b"\\",
    b"\\u{",
    b"{",
    b"}",
    b"[",
    b"]",
    b",",
    b"#",
    b"=",
    b" ",
    b"\t",
    b"e",
    b"_",
    b".",
    b"%",
    b"-",
    b"0",
    b"0x",
    b"a",
    b"9",
];

fn documents() -> Vec<Vec<u8>> {
    let mut docs: Vec<Vec<u8>> = suite("valid", "dump").into_iter().map(|c| c.1).collect();
    docs.extend(suite("invalid", "error").into_iter().map(|c| c.1));
    docs
}

/// Reads a whole document and checks what the reader says about it.
fn check(src: &[u8]) -> Result<(), Error> {
    let lines = src.split(|&b| b == b'\n').count();
    let mut depth = 0usize;
    let mut buf = [0u8; 4096];
    for event in Reader::new(src) {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                assert!(err.at.line >= 1 && err.at.line <= lines, "{err:?}");
                assert!(err.at.col >= 1 && err.at.col <= src.len() + 1, "{err:?}");
                assert_eq!(Document::parse(src).unwrap_err(), err);
                return Err(err);
            }
        };
        let value = match event {
            Event::Block { label, .. } => {
                depth += 1;
                if let Some(label) = label {
                    assert_eq!(label.decode(&mut buf).unwrap(), label.to_string());
                }
                None
            }
            Event::End | Event::ListEnd => {
                depth -= 1;
                None
            }
            Event::Entry { value, .. } | Event::Element { value, .. } => Some(value),
        };
        match value {
            Some(Value::List) => depth += 1,
            Some(Value::Str(s)) => assert_eq!(s.decode(&mut buf).unwrap(), s.to_string()),
            Some(Value::Quantity(q)) => {
                let _ = (q.size(), q.duration());
            }
            _ => {}
        }
        assert!(depth <= meri::MAX_DEPTH);
    }
    assert_eq!(depth, 0, "every block and list that starts ends");
    assert!(Document::parse(src).is_ok());
    Ok(())
}

#[test]
fn every_prefix_of_every_document_reads_to_an_end() {
    for doc in documents() {
        for len in 0..=doc.len() {
            let _ = check(&doc[..len]);
        }
    }
}

#[test]
fn changed_documents_read_to_an_end() {
    let docs = documents();
    let mut rng = Rng::new(0x6d65_7269);
    let (mut valid, mut invalid) = (0, 0);
    for _ in 0..6000 {
        let mut doc = docs[rng.below(docs.len())].clone();
        for _ in 0..=rng.below(3) {
            let at = rng.below(doc.len() + 1);
            match rng.below(3) {
                0 => {
                    let piece = PIECES[rng.below(PIECES.len())];
                    doc.splice(at..at, piece.iter().copied());
                }
                1 => {
                    let end = (at + 1 + rng.below(3)).min(doc.len());
                    doc.drain(at..end);
                }
                _ => {
                    let from = rng.below(doc.len() + 1);
                    let piece: Vec<u8> = doc[from.min(at)..from.max(at)]
                        .iter()
                        .take(40)
                        .copied()
                        .collect();
                    doc.splice(at..at, piece);
                }
            }
        }
        match check(&doc) {
            Ok(()) => valid += 1,
            Err(_) => invalid += 1,
        }
    }
    assert!(
        valid > 300 && invalid > 3000,
        "{valid} valid, {invalid} invalid"
    );
}
