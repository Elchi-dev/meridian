//! Helpers the test files share: the canonical dump of section 15 of the
//! specification built from the reader's events, the conformance suite,
//! and a small random number generator.

// Each test file uses a different part of this module.
#![allow(dead_code)]
// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;

use meri::{Error, Event, Number, Reader, Value};

/// Dumps a document the way the conformance suite writes it, or returns
/// the first error.
pub fn dump(src: &[u8]) -> Result<String, Error> {
    let mut lines: Vec<String> = Vec::new();
    // Each open block: its path and how many blocks of each name it holds.
    let mut blocks: Vec<(String, HashMap<String, usize>)> = vec![(String::new(), HashMap::new())];
    // Each open list: its path, the index of its line, and its length.
    let mut lists: Vec<(String, usize, usize)> = Vec::new();
    for event in Reader::new(src) {
        match event? {
            Event::Block { name, label, .. } => {
                let (parent, counts) = blocks.last_mut().unwrap();
                let count = counts.entry(name.to_string()).or_insert(0);
                let path = join(parent, &format!("{name}#{count}"));
                *count += 1;
                lines.push(match label {
                    Some(label) => format!("{path} block {}", quote(&label.to_string())),
                    None => format!("{path} block"),
                });
                blocks.push((path, HashMap::new()));
            }
            Event::End => {
                blocks.pop();
            }
            Event::Entry { key, value, .. } => {
                let path = join(&blocks.last().unwrap().0, key);
                push_value(&mut lines, &mut lists, &path, value);
            }
            Event::Element { value, .. } => {
                let (list, _, len) = lists.last_mut().unwrap();
                let path = format!("{list}[{len}]");
                *len += 1;
                push_value(&mut lines, &mut lists, &path, value);
            }
            Event::ListEnd => {
                let (path, line, len) = lists.pop().unwrap();
                lines[line] = format!("{path} list {len}");
            }
        }
    }
    Ok(lines.iter().fold(String::new(), |mut out, line| {
        writeln!(out, "{line}").unwrap();
        out
    }))
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

fn push_value(
    lines: &mut Vec<String>,
    lists: &mut Vec<(String, usize, usize)>,
    path: &str,
    value: Value,
) {
    if value == Value::List {
        lists.push((path.to_string(), lines.len(), 0));
    }
    lines.push(format!("{path} {}", value_text(value)));
}

/// A value the way the dump writes it after the path, except that a list
/// is only `list`, since its length is not known when it starts.
pub fn value_text(value: Value) -> String {
    match value {
        Value::Bool(b) => format!("bool {b}"),
        Value::Int(n) => format!("int {n}"),
        Value::Float(f) => format!("float {}", float_text(f.literal)),
        Value::Quantity(q) => match q.number {
            Number::Int(n) => format!("quantity {n} {}", q.unit),
            Number::Float(f) => format!("quantity {} {}", float_text(f.literal), q.unit),
        },
        Value::Str(s) => format!("string {}", quote(&s.to_string())),
        Value::Word(w) => format!("word {w}"),
        Value::List => "list".to_string(),
    }
}

/// The events of a document, one line each with their positions, ending
/// in the error if there is one.
pub fn trace(src: &[u8]) -> Vec<String> {
    Reader::new(src)
        .map(|event| match event {
            Ok(Event::Block { name, label, at }) => match label {
                Some(label) => format!("{at} block {name} {}", quote(&label.to_string())),
                None => format!("{at} block {name}"),
            },
            Ok(Event::End) => "end".to_string(),
            Ok(Event::Entry {
                key,
                at,
                value,
                value_at,
            }) => format!("{at} {key} = {} @{value_at}", value_text(value)),
            Ok(Event::Element { value, at }) => format!("{at} {}", value_text(value)),
            Ok(Event::ListEnd) => "list end".to_string(),
            Err(err) => format!("error {} {}", err.at, err.code.name()),
        })
        .collect()
}

/// A float the way the dump writes it: without `_`, with a lower-case `e`
/// and without a `+` in the exponent.
pub fn float_text(literal: &str) -> String {
    literal
        .chars()
        .filter(|&c| c != '_' && c != '+')
        .map(|c| if c == 'E' { 'e' } else { c })
        .collect()
}

/// A string or label with the escapes of the dump.
pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            '\u{1}'..='\u{1f}' | '\u{7f}' => write!(out, "\\u{{{:x}}}", u32::from(c)).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The first error of a document as the suite's `.error` files write it.
pub fn error_line(src: &[u8]) -> Option<String> {
    let err = Reader::new(src).find_map(Result::err)?;
    Some(format!(
        "{}:{} {}",
        err.at.line,
        err.at.col,
        err.code.name()
    ))
}

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The cases of one half of the conformance suite: each name with the
/// document and the expected `.dump` or `.error`.
pub fn suite(dir: &str, expected: &str) -> Vec<(String, Vec<u8>, Vec<u8>)> {
    let dir = repo().join("tests/meri").join(dir);
    let mut cases = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "meri") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let want = std::fs::read(path.with_extension(expected)).unwrap();
            cases.push((name, std::fs::read(&path).unwrap(), want));
        }
    }
    cases.sort();
    cases
}

/// xorshift64*, enough to vary test inputs reproducibly.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }
}
