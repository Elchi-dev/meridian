//! Random valid documents, written by a generator that knows which events
//! the reader must report for them and where: every kind of value, labels,
//! escapes, texts, nested lists, comments, tabs, CR LF and characters of
//! several bytes.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests; and generated numbers convert between types freely.
#![allow(
    clippy::unwrap_used,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

mod common;

use std::fmt::Write;

use common::Rng;
use meri::{Event, Number, Reader, Value};

/// Writes a document and the events it must produce.
struct Writer {
    rng: Rng,
    out: String,
    line: usize,
    col: usize,
    crlf: bool,
    events: Vec<String>,
    labels: usize,
}

impl Writer {
    fn push(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
        self.out.push_str(s);
    }

    fn newline(&mut self) {
        let comment = ["", " # note", "\t# é {", "  #"][self.rng.below(4)];
        self.push(comment);
        self.out.push_str(if self.crlf { "\r" } else { "" });
        self.push("\n");
        if self.rng.below(5) == 0 {
            let blank = self.pick(&["\n", "  \n", "# comment\n"]);
            self.push(blank);
        }
    }

    fn at(&self) -> String {
        format!("{}:{}", self.line, self.col)
    }

    fn pick<'s>(&mut self, from: &[&'s str]) -> &'s str {
        from[self.rng.below(from.len())]
    }

    fn indent(&mut self, depth: usize) {
        let unit = self.pick(&["  ", "\t", "    "]);
        self.push(&unit.repeat(depth));
    }

    fn block(&mut self, depth: usize) {
        let items = self.rng.below(6) + if depth == 0 { 4 } else { 0 };
        for k in 0..items {
            self.indent(depth);
            if depth < 4 && self.rng.below(3) == 0 {
                self.nested(depth, k);
            } else {
                let key = format!("k{k}-{}", self.pick(&["x", "_", "Y9"]));
                let at = self.at();
                self.push(&key);
                let equals = self.pick(&[" = ", "=", "\t=  "]);
                self.push(equals);
                let value_at = self.at();
                let value = self.value(depth);
                self.events.insert(
                    self.events.len() - value.1,
                    format!("{at} {key} = {} @{value_at}", value.0),
                );
            }
            self.newline();
        }
    }

    fn nested(&mut self, depth: usize, k: usize) {
        let name = if self.rng.below(2) == 0 {
            "rep".to_string()
        } else {
            format!("b{k}")
        };
        let at = self.at();
        self.push(&name);
        let label = if self.rng.below(3) == 0 {
            None
        } else {
            self.labels += 1;
            let label = format!("L{}é", self.labels);
            let written = match self.rng.below(3) {
                0 => format!(" \"{label}\""),
                1 => format!(" \"\\u{{4c}}{}\"", &label[1..]),
                _ => format!(" L{}", self.labels),
            };
            self.push(&written);
            Some(if written.ends_with('"') {
                label
            } else {
                format!("L{}", self.labels)
            })
        };
        self.events.push(format!("{at} block {name} {label:?}"));
        if self.rng.below(4) == 0 {
            self.push(" { ");
            let key_at = self.at();
            self.push("one = ");
            let value_at = self.at();
            self.push("1 }");
            self.events
                .push(format!("{key_at} one = int 1 @{value_at}"));
        } else {
            self.push(" {");
            self.newline();
            self.block(depth + 1);
            self.indent(depth);
            self.push("}");
        }
        self.events.push("end".to_string());
    }

    /// Writes a value and returns how it shows in an event, and how many
    /// events of its own (list elements) it has added after it.
    fn value(&mut self, depth: usize) -> (String, usize) {
        let before = self.events.len();
        let shown = match self.rng.below(8) {
            0 => {
                let b = self.rng.below(2) == 0;
                self.push(if b { "true" } else { "false" });
                format!("bool {b}")
            }
            1 => {
                let w = self.pick(&["kvm", "x86_64", "a-b", "True"]);
                self.push(w);
                format!("word {w}")
            }
            2 => self.int(true),
            3 => self.float(),
            4 => {
                let unit = self.pick(&["MiB", "s", "%", "Hz", "e"]);
                let shown = self.int(false).replace("int ", "");
                self.push(unit);
                format!("quantity int {shown} {unit}")
            }
            5 => self.string(),
            6 => self.text(depth),
            _ if depth < 6 => self.list(depth),
            _ => self.int(true),
        };
        (shown, self.events.len() - before)
    }

    /// An integer in any base, or only in decimal for a quantity.
    fn int(&mut self, any_base: bool) -> String {
        let n = (self.rng.next() >> self.rng.below(64)) as i64;
        let n = if self.rng.below(2) == 0 {
            n
        } else {
            n.wrapping_neg()
        };
        let sign = if n < 0 { "-" } else { "" };
        let m = n.unsigned_abs();
        let base = if any_base { self.rng.below(5) } else { 4 };
        let text = match base {
            0 => format!("{sign}0x{m:x}"),
            1 => format!("{sign}0X{m:X}").replacen("0X", "0x", 1),
            2 => format!("{sign}0o{m:o}"),
            3 => format!("{sign}0b{m:b}"),
            _ => {
                let digits = m.to_string();
                let mut with = String::new();
                for (i, c) in digits.chars().enumerate() {
                    if i > 0 && (digits.len() - i) % 3 == 0 && self.rng.below(2) == 0 {
                        with.push('_');
                    }
                    with.push(c);
                }
                format!("{sign}{with}")
            }
        };
        self.push(&text);
        format!("int {n}")
    }

    fn float(&mut self) -> String {
        let f = loop {
            let f = f64::from_bits(self.rng.next());
            if f.is_finite() {
                break f;
            }
        };
        let text = match self.rng.below(3) {
            0 => format!("{f:e}"),
            1 => format!("{f:E}").replace('E', "E+").replace("E+-", "E-"),
            _ => format!("{f:?}"),
        };
        self.push(&text);
        format!("float {:x}", f.to_bits())
    }

    fn string(&mut self) -> String {
        let mut decoded = String::new();
        let mut written = String::from("\"");
        for _ in 0..self.rng.below(8) {
            let c = [
                'a', 'é', '漢', '😀', ' ', '\t', '"', '\\', '\n', '\r', '\0', '\u{1}', '#', '{',
                '\u{7f}',
            ][self.rng.below(15)];
            decoded.push(c);
            match c {
                '"' => written.push_str("\\\""),
                '\\' => written.push_str("\\\\"),
                '\n' => written.push_str("\\n"),
                '\r' => written.push_str("\\r"),
                '\0' => written.push_str("\\0"),
                '\u{1}' | '\u{7f}' => write!(written, "\\u{{{:x}}}", u32::from(c)).unwrap(),
                '\t' if self.rng.below(2) == 0 => written.push_str("\\t"),
                c if self.rng.below(4) == 0 => {
                    write!(written, "\\u{{{:X}}}", u32::from(c)).unwrap();
                }
                c => written.push(c),
            }
        }
        written.push('"');
        self.push(&written);
        format!("str {decoded:?}")
    }

    fn text(&mut self, depth: usize) -> String {
        let opening = self.pick(&["\"\"\"", "\"\"\"  "]);
        self.push(opening);
        self.newline_bare();
        let indent = self
            .pick(&["", "  ", "\t", "    "])
            .repeat(depth.min(2) + 1);
        let mut lines = Vec::new();
        for _ in 0..self.rng.below(4) {
            let line = self.pick(&["", "word", "  more", "a \"q\" \\n", "x = 1 }", "é漢"]);
            lines.push(line);
            if line.is_empty() {
                let blank = self.pick(&["", " ", "\t\t", "      "]);
                self.push(blank);
            } else {
                self.push(&format!("{indent}{line}"));
            }
            self.newline_bare();
        }
        self.push(&format!("{indent}\"\"\""));
        format!("str {:?}", lines.join("\n"))
    }

    /// A line ending without a comment, as texts need.
    fn newline_bare(&mut self) {
        self.out.push_str(if self.crlf { "\r" } else { "" });
        self.push("\n");
    }

    fn list(&mut self, depth: usize) -> String {
        self.push("[");
        let multi = self.rng.below(2) == 0;
        let n = self.rng.below(4);
        for i in 0..n {
            if multi {
                self.newline();
                self.indent(depth + 1);
            } else if self.rng.below(2) == 0 {
                self.push(" ");
            }
            let at = self.at();
            let value = self.value(depth + 1);
            self.events
                .insert(self.events.len() - value.1, format!("{at} {}", value.0));
            if i + 1 < n || self.rng.below(2) == 0 {
                self.push(",");
            }
        }
        if multi {
            self.newline();
            self.indent(depth);
        }
        self.push("]");
        self.events.push("list end".to_string());
        "list".to_string()
    }
}

/// An event the way the generator writes them down.
fn show(event: Event) -> String {
    let value = |value: Value| match value {
        Value::Bool(b) => format!("bool {b}"),
        Value::Word(w) => format!("word {w}"),
        Value::Int(n) => format!("int {n}"),
        Value::Float(f) => format!("float {:x}", f.value.to_bits()),
        Value::Quantity(q) => match q.number {
            Number::Int(n) => format!("quantity int {n} {}", q.unit),
            Number::Float(f) => format!("quantity float {:x} {}", f.value.to_bits(), q.unit),
        },
        Value::Str(s) => format!("str {:?}", s.to_string()),
        Value::List => "list".to_string(),
    };
    match event {
        Event::Block { name, label, at } => {
            format!("{at} block {name} {:?}", label.map(|l| l.to_string()))
        }
        Event::End => "end".to_string(),
        Event::Entry {
            key,
            at,
            value: v,
            value_at,
        } => format!("{at} {key} = {} @{value_at}", value(v)),
        Event::Element { value: v, at } => format!("{at} {}", value(v)),
        Event::ListEnd => "list end".to_string(),
    }
}

#[test]
fn generated_documents_read_back_exactly() {
    let mut events = 0;
    for seed in 1..=400u64 {
        let mut rng = Rng::new(seed);
        let crlf = rng.below(3) == 0;
        let bom = rng.below(5) == 0;
        let mut w = Writer {
            rng,
            out: String::new(),
            line: 1,
            col: 1,
            crlf,
            events: Vec::new(),
            labels: 0,
        };
        if bom {
            w.out.push('\u{feff}');
        }
        w.block(0);
        let got: Vec<String> = Reader::new(w.out.as_bytes())
            .map(|event| match event {
                Ok(event) => show(event),
                Err(err) => format!("error {} {} in\n{}", err.at, err.code.name(), w.out),
            })
            .collect();
        assert_eq!(got, w.events, "seed {seed}, document:\n{}", w.out);
        events += got.len();
    }
    assert!(events > 8000, "only {events} events in all documents");
}
