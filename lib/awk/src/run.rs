//! An awk program run, as POSIX has it: the `BEGIN` actions, then each
//! record of the input through the rules whose patterns select it, then
//! the `END` actions. What the program reaches beyond its variables, its
//! input, files, commands, the clock, it reaches through a [`World`].

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use regex::{Regex, Syntax, Text};

use crate::lex::{self, Builtin};
use crate::parse::{
    self, Arith, Expr, Output, Pattern, Place, Program, Relation, Source, Stmt, Var,
};
use crate::record::{Ends, Splitter};
use crate::value::{self, Value};
use crate::{format, math, printf};

/// A stream awk reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream<'a> {
    /// The standard input, or the standard output.
    Standard,
    /// The standard error.
    Error,
    /// A file by its name, made anew at its first write.
    File(&'a [u8]),
    /// A file written at its end.
    Append(&'a [u8]),
    /// A command: its output read, or its input written.
    Command(&'a [u8]),
}

/// What awk reads from and writes to besides its variables. A file or a
/// command named stays open from its first use until [`World::close`]
/// closes it, so that each read goes on from the last and each write
/// adds to the last.
pub trait World {
    /// More of what `from` gives, nothing at its end.
    ///
    /// # Errors
    ///
    /// What went wrong, in words, when it cannot be read.
    fn read(&mut self, from: Stream) -> Result<Vec<u8>, String>;

    /// Writes `bytes` to `to`.
    ///
    /// # Errors
    ///
    /// What went wrong, in words, when it cannot be written.
    fn write(&mut self, to: Stream, bytes: &[u8]) -> Result<(), String>;

    /// Closes the file or command open by `name`: its status, a command's
    /// as it ended; none when nothing is open by that name.
    fn close(&mut self, name: &[u8]) -> Option<i32>;

    /// Writes out what is held for `name`, or for everything open when
    /// none; whether that went.
    fn flush(&mut self, name: Option<&[u8]>) -> bool;

    /// Runs `command` and waits for it to end: its status.
    fn system(&mut self, command: &[u8]) -> i32;

    /// A number that changes from one run to the next, to seed `rand`
    /// when `srand` is given none.
    fn ticks(&mut self) -> u64;
}

/// Why a run stopped before its end: what went wrong, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fatal(pub String);

/// How a statement left the code around it, besides going on.
#[derive(Debug)]
enum Stop {
    Fatal(Fatal),
    Next,
    Nextfile,
    /// `exit`, with its status if it gave one.
    Exit(Option<i32>),
}

impl From<Fatal> for Stop {
    fn from(fatal: Fatal) -> Stop {
        Stop::Fatal(fatal)
    }
}

fn fatal<T>(what: String) -> Result<T, Stop> {
    Err(Stop::Fatal(Fatal(what)))
}

/// How a statement ended within the code around it.
enum Flow {
    Normal,
    Break,
    Continue,
    Return(Value),
}

/// Where a variable that was given to a function unset came from, so
/// that when the function makes it an array, the caller's is that array.
#[derive(Clone, Copy, Debug)]
struct Home {
    /// The call it is in, none for the program's own.
    frame: Option<usize>,
    slot: usize,
}

#[derive(Clone, Debug)]
enum Cell {
    Unset(Option<Home>),
    Value(Value),
    Array(usize),
}

/// The variables of one call of a function.
struct Frame {
    cells: Vec<Cell>,
    /// The arrays it made of its own, which go when it returns.
    made: Vec<usize>,
}

type Array = BTreeMap<Vec<u8>, Value>;

/// The record at hand: `$0` and its fields, each made from the other
/// only when it is wanted.
struct Record {
    /// `$0`, unless a field changed since.
    whole: Vec<u8>,
    stale: bool,
    fields: Vec<Value>,
    split: bool,
    /// The `FS`, and whether `RS` was empty, when the record came.
    fs: Vec<u8>,
    paragraph: bool,
}

/// Bytes read from a stream and not yet taken as records.
#[derive(Default)]
struct Reader {
    data: Vec<u8>,
    ended: bool,
}

/// How far the operands have been read as the input.
#[derive(Default)]
struct Main {
    /// The next operand to look at, from ARGV[1].
    next: usize,
    /// The file being read; `-` for the standard input.
    current: Option<Vec<u8>>,
    reader: Reader,
    /// Whether an operand named a file, so that the standard input is
    /// not read when none did.
    any: bool,
}

/// How deep calls of the program's functions may go before the run
/// stops, so that the stack of a program on Meridian holds them.
pub const DEEPEST: usize = 500;

/// The most fields a record may be given, so that a mistaken number
/// does not fill the memory.
pub const MOST_FIELDS: usize = 65535;

/// A run of a program.
pub struct Awk<'p> {
    program: &'p Program,
    globals: Vec<Cell>,
    arrays: Vec<Array>,
    free: Vec<usize>,
    frames: Vec<Frame>,
    record: Record,
    /// For each rule, whether a range it selects is going on.
    ranges: Vec<bool>,
    main: Main,
    /// Readers of files and commands, by `kind`, 0 or 1, and name.
    readers: BTreeMap<(u8, Vec<u8>), Reader>,
    /// Regular expressions made of strings, kept for when they come again.
    dynamic: Vec<(Vec<u8>, Regex)>,
    /// The splitter for the `FS` and paragraph mode it was made for.
    splitter: Option<(Vec<u8>, bool, Splitter)>,
    ends: Option<(Vec<u8>, Ends)>,
    seed: f64,
    random: u64,
    status: i32,
}

/// A string value.
fn text(bytes: &[u8]) -> Value {
    Value::Text(bytes.to_vec())
}

/// Whether `name` could name a variable.
fn is_name(name: &[u8]) -> bool {
    name.first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// `x` rounded down to a whole number.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn floor(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 4_503_599_627_370_496.0 {
        return x;
    }
    let cut = (x as i64) as f64;
    if cut > x { cut - 1.0 } else { cut }
}

/// `x` with its fraction cut off, as `int` does.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn truncated(x: f64) -> f64 {
    if !x.is_finite() || x.abs() >= 4_503_599_627_370_496.0 {
        return x;
    }
    (x as i64) as f64
}

/// The bytes the characters of `bytes` from the character `start` take,
/// up to the character `end`, counted from 0.
fn char_span(bytes: &[u8], start: usize, end: usize) -> &[u8] {
    let skipped = printf::first_chars(bytes, start).len();
    let rest = &bytes[skipped..];
    &rest[..printf::first_chars(rest, end.saturating_sub(start)).len()]
}

/// The next of the numbers `rand` gives, from `state`.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// What `repl`, the replacement of `sub` and `gsub`, makes of a match of
/// `matched`: `&` is the match, `\&` a `&`, `\\` a backslash.
fn replaced(repl: &[u8], matched: &[u8], into: &mut Vec<u8>) {
    let mut at = 0;
    while at < repl.len() {
        match (repl[at], repl.get(at + 1)) {
            (b'\\', Some(b'&')) => {
                into.push(b'&');
                at += 2;
            }
            (b'\\', Some(b'\\')) => {
                into.push(b'\\');
                at += 2;
            }
            (b'&', _) => {
                into.extend_from_slice(matched);
                at += 1;
            }
            (byte, _) => {
                into.push(byte);
                at += 1;
            }
        }
    }
}

impl<'p> Awk<'p> {
    /// A run of `program` whose operands, ARGV[1] on, are `operands`.
    #[must_use]
    pub fn new(program: &'p Program, operands: &[Vec<u8>]) -> Awk<'p> {
        let mut globals = alloc::vec![Cell::Unset(None); program.globals.len()];
        let defaults: [(usize, Value); 12] = [
            (parse::NR, Value::Number(0.0)),
            (parse::NF, Value::Number(0.0)),
            (parse::FNR, Value::Number(0.0)),
            (parse::FS, text(b" ")),
            (parse::OFS, text(b" ")),
            (parse::ORS, text(b"\n")),
            (parse::RS, text(b"\n")),
            (parse::SUBSEP, text(b"\x1c")),
            (parse::RSTART, Value::Number(0.0)),
            (parse::RLENGTH, Value::Number(-1.0)),
            (parse::CONVFMT, text(b"%.6g")),
            (parse::OFMT, text(b"%.6g")),
        ];
        for (slot, value) in defaults {
            globals[slot] = Cell::Value(value);
        }
        let mut argv = Array::new();
        argv.insert(b"0".to_vec(), text(b"awk"));
        for (at, operand) in operands.iter().enumerate() {
            argv.insert(
                format!("{}", at + 1).into_bytes(),
                Value::input(operand.clone()),
            );
        }
        #[allow(clippy::cast_precision_loss)]
        let count = (operands.len() + 1) as f64;
        globals[parse::ARGC] = Cell::Value(Value::Number(count));
        globals[parse::ARGV] = Cell::Array(0);
        globals[parse::ENVIRON] = Cell::Array(1);
        let mut random = 0;
        splitmix(&mut random);
        Awk {
            program,
            globals,
            arrays: alloc::vec![argv, Array::new()],
            free: Vec::new(),
            frames: Vec::new(),
            record: Record {
                whole: Vec::new(),
                stale: false,
                fields: Vec::new(),
                split: true,
                fs: b" ".to_vec(),
                paragraph: false,
            },
            ranges: alloc::vec![false; program.rules.len()],
            main: Main {
                next: 1,
                ..Main::default()
            },
            readers: BTreeMap::new(),
            dynamic: Vec::new(),
            splitter: None,
            ends: None,
            seed: 0.0,
            random,
            status: 0,
        }
    }

    /// Gives the variable `name` the value `value`, its escapes taken, as
    /// `-v` and an operand `name=value` do; whether `name` is one.
    pub fn assign(&mut self, name: &[u8], value: &[u8]) -> bool {
        if !is_name(name) {
            return false;
        }
        let Some(slot) = self
            .program
            .globals
            .iter()
            .position(|global| global.as_bytes() == name)
        else {
            // A variable the program never names cannot be seen.
            return !self
                .program
                .functions
                .iter()
                .any(|f| f.name.as_bytes() == name);
        };
        if matches!(self.globals[slot], Cell::Array(_)) {
            return false;
        }
        let value = Value::input(lex::unescaped(value));
        self.set_var(Var::Global(slot), value).is_ok()
    }

    /// Runs the program: its exit status.
    ///
    /// # Errors
    ///
    /// When it cannot go on: what went wrong, after which awk ends with 2.
    pub fn run(&mut self, world: &mut impl World) -> Result<i32, Fatal> {
        let program = self.program;
        let exited = match self.actions(&program.begin, world) {
            Ok(()) => false,
            Err(Stop::Exit(code)) => {
                self.status = code.unwrap_or(self.status);
                true
            }
            Err(Stop::Fatal(fatal)) => return Err(fatal),
            Err(Stop::Next | Stop::Nextfile) => {
                return Err(Fatal(String::from("next goes only in a rule")));
            }
        };
        // A program of BEGIN actions alone reads no input.
        if !exited && (!program.rules.is_empty() || !program.end.is_empty()) {
            match self.rules(world) {
                Ok(()) | Err(Stop::Next | Stop::Nextfile) => {}
                Err(Stop::Exit(code)) => self.status = code.unwrap_or(self.status),
                Err(Stop::Fatal(fatal)) => return Err(fatal),
            }
        }
        match self.actions(&program.end, world) {
            Ok(()) => {}
            Err(Stop::Exit(code)) => self.status = code.unwrap_or(self.status),
            Err(Stop::Fatal(fatal)) => return Err(fatal),
            Err(Stop::Next | Stop::Nextfile) => {
                return Err(Fatal(String::from("next goes only in a rule")));
            }
        }
        world.flush(None);
        Ok(self.status)
    }

    /// Runs `BEGIN` or `END` actions.
    fn actions(&mut self, statements: &[Stmt], world: &mut impl World) -> Result<(), Stop> {
        for statement in statements {
            self.statement(statement, world)?;
        }
        Ok(())
    }

    /// Each record of the input through the rules.
    fn rules(&mut self, world: &mut impl World) -> Result<(), Stop> {
        let program = self.program;
        while let Some(record) = self.main_record(world)? {
            self.count_record(true);
            self.set_record(record);
            for (at, rule) in program.rules.iter().enumerate() {
                if !self.selects(at, &rule.pattern, world)? {
                    continue;
                }
                let done = if let Some(action) = &rule.action {
                    self.actions(action, world)
                } else {
                    let mut line = self.whole();
                    line.extend_from_slice(&self.global_text(parse::ORS));
                    write(Stream::Standard, &line, world)
                };
                match done {
                    Ok(()) => {}
                    Err(Stop::Next) => break,
                    Err(Stop::Nextfile) => {
                        self.end_file(world);
                        break;
                    }
                    Err(stop) => return Err(stop),
                }
            }
        }
        Ok(())
    }

    /// Whether the rule `at` with `pattern` selects the record.
    fn selects(
        &mut self,
        at: usize,
        pattern: &Pattern,
        world: &mut impl World,
    ) -> Result<bool, Stop> {
        match pattern {
            Pattern::All => Ok(true),
            Pattern::Expr(expr) => Ok(self.eval(expr, world)?.truth()),
            Pattern::Range(start, end) => {
                if !self.ranges[at] {
                    if !self.eval(start, world)?.truth() {
                        return Ok(false);
                    }
                    self.ranges[at] = true;
                }
                if self.eval(end, world)?.truth() {
                    self.ranges[at] = false;
                }
                Ok(true)
            }
        }
    }

    /// Adds one to NR, and to FNR when the record is from the input's
    /// files.
    fn count_record(&mut self, file: bool) {
        for slot in [parse::NR, parse::FNR]
            .into_iter()
            .take(1 + usize::from(file))
        {
            let n = self.global(slot).number();
            self.globals[slot] = Cell::Value(Value::Number(n + 1.0));
        }
    }

    // The input.

    /// The next record of the operands' files, or of the standard input
    /// when they name none; none at the end of the last.
    fn main_record(&mut self, world: &mut impl World) -> Result<Option<Vec<u8>>, Stop> {
        loop {
            let Some(name) = self.main.current.clone() else {
                let Some(name) = self.next_operand()? else {
                    return Ok(None);
                };
                self.globals[parse::FILENAME] = Cell::Value(Value::input(name.clone()));
                self.globals[parse::FNR] = Cell::Value(Value::Number(0.0));
                self.main.current = Some(name);
                self.main.reader = Reader::default();
                continue;
            };
            let stream = if name == b"-" || name == b"/dev/stdin" {
                Stream::Standard
            } else {
                Stream::File(&name)
            };
            let mut reader = core::mem::take(&mut self.main.reader);
            let record = self.next_record(&mut reader, stream, world);
            self.main.reader = reader;
            match record? {
                Ok(Some(record)) => return Ok(Some(record)),
                Ok(None) => self.end_file(world),
                Err(why) => {
                    world
                        .write(
                            Stream::Error,
                            format!("awk: {}: {why}\n", String::from_utf8_lossy(&name)).as_bytes(),
                        )
                        .ok();
                    self.status = 2;
                    self.main.current = None;
                }
            }
        }
    }

    /// Leaves the file being read as the input.
    fn end_file(&mut self, world: &mut impl World) {
        if let Some(name) = self.main.current.take()
            && name != b"-"
        {
            world.close(&name);
        }
        self.main.reader = Reader::default();
    }

    /// The next operand that names a file, after making the assignments
    /// before it; `-` for the standard input when none is left and none
    /// named one.
    fn next_operand(&mut self) -> Result<Option<Vec<u8>>, Stop> {
        loop {
            let argc = self.global(parse::ARGC).number();
            #[allow(clippy::cast_precision_loss)]
            if (self.main.next as f64) >= argc {
                if self.main.any {
                    return Ok(None);
                }
                self.main.any = true;
                return Ok(Some(b"-".to_vec()));
            }
            let key = format!("{}", self.main.next).into_bytes();
            self.main.next += 1;
            let convfmt = self.global_text(parse::CONVFMT);
            let operands = self.argv()?;
            let Some(operand) = self.arrays[operands].get(&key).map(|v| v.text(&convfmt)) else {
                continue;
            };
            if operand.is_empty() {
                continue;
            }
            if let Some(equals) = operand.iter().position(|&b| b == b'=')
                && is_name(&operand[..equals])
            {
                self.assign(&operand[..equals], &operand[equals + 1..]);
                continue;
            }
            self.main.any = true;
            return Ok(Some(operand));
        }
    }

    /// The array ARGV is, which a program may have made something else.
    fn argv(&mut self) -> Result<usize, Stop> {
        self.array(Var::Global(parse::ARGV))
    }

    /// The record separator `RS` names now.
    fn ends(&mut self) -> Result<&Ends, Stop> {
        let rs = self.global_text(parse::RS);
        let kept = self.ends.take().filter(|(made, _)| *made == rs);
        let made = if let Some(made) = kept {
            made
        } else {
            let ends = Ends::new(&rs).or_else(|e| fatal(format!("RS: {}", e.code.says())))?;
            (rs, ends)
        };
        Ok(&self.ends.insert(made).1)
    }

    /// The next record of `stream`, read on from `reader`; none at its
    /// end, and what went wrong when it could not be read.
    fn next_record(
        &mut self,
        reader: &mut Reader,
        stream: Stream,
        world: &mut impl World,
    ) -> Result<Result<Option<Vec<u8>>, String>, Stop> {
        loop {
            let found = self.ends()?.first(&reader.data, reader.ended);
            if let Some((start, end, taken)) = found {
                let record = reader.data[start..end].to_vec();
                reader.data.drain(..taken);
                return Ok(Ok(Some(record)));
            }
            if reader.ended {
                reader.data.clear();
                return Ok(Ok(None));
            }
            match world.read(stream) {
                Ok(more) if more.is_empty() => reader.ended = true,
                Ok(more) => reader.data.extend_from_slice(&more),
                Err(why) => return Ok(Err(why)),
            }
        }
    }

    // Records and fields.

    /// Makes `bytes` the record, split later with the `FS` of now.
    fn set_record(&mut self, bytes: Vec<u8>) {
        let paragraph = self.global_text(parse::RS).is_empty();
        self.record = Record {
            whole: bytes,
            stale: false,
            fields: Vec::new(),
            split: false,
            fs: self.global_text(parse::FS),
            paragraph,
        };
    }

    /// Splits the record into its fields if it is not yet.
    fn split(&mut self) -> Result<(), Stop> {
        if self.record.split {
            return Ok(());
        }
        let (fs, paragraph) = (self.record.fs.clone(), self.record.paragraph);
        self.splitter(&fs, paragraph)?;
        let Some((_, _, splitter)) = &self.splitter else {
            return Ok(());
        };
        let fields: Vec<Value> = splitter
            .split(&self.record.whole)
            .into_iter()
            .map(|field| Value::input(field.to_vec()))
            .collect();
        self.record.fields = fields;
        self.record.split = true;
        Ok(())
    }

    /// The splitter for `fs`, kept while it stays the same.
    fn splitter(&mut self, fs: &[u8], paragraph: bool) -> Result<&Splitter, Stop> {
        let kept = self
            .splitter
            .take()
            .filter(|(made, mode, _)| made == fs && *mode == paragraph);
        let made = if let Some(made) = kept {
            made
        } else {
            let splitter = Splitter::new(fs, paragraph)
                .or_else(|e| fatal(format!("FS: {}", e.code.says())))?;
            (fs.to_vec(), paragraph, splitter)
        };
        Ok(&self.splitter.insert(made).2)
    }

    /// `$0`, made anew from the fields if one changed.
    fn whole(&mut self) -> Vec<u8> {
        if self.record.stale {
            let ofs = self.global_text(parse::OFS);
            let convfmt = self.global_text(parse::CONVFMT);
            let mut whole = Vec::new();
            for (at, field) in self.record.fields.iter().enumerate() {
                if at > 0 {
                    whole.extend_from_slice(&ofs);
                }
                whole.extend_from_slice(&field.text(&convfmt));
            }
            self.record.whole = whole;
            self.record.stale = false;
        }
        self.record.whole.clone()
    }

    fn field(&mut self, n: usize) -> Result<Value, Stop> {
        if n == 0 {
            return Ok(Value::input(self.whole()));
        }
        self.split()?;
        Ok(self
            .record
            .fields
            .get(n - 1)
            .cloned()
            .unwrap_or(Value::Unset))
    }

    fn set_field(&mut self, n: usize, value: Value) -> Result<(), Stop> {
        if n == 0 {
            let convfmt = self.global_text(parse::CONVFMT);
            self.set_record(value.text(&convfmt));
            return Ok(());
        }
        self.split()?;
        if n > MOST_FIELDS {
            return fatal(format!("no field past {MOST_FIELDS} can be made"));
        }
        if self.record.fields.len() < n {
            self.record.fields.resize(n, Value::Unset);
        }
        self.record.fields[n - 1] = value;
        self.record.stale = true;
        Ok(())
    }

    /// The field number `index` names.
    fn field_number(index: &Value) -> Result<usize, Stop> {
        let n = index.number();
        if n < 0.0 || n.is_nan() {
            return fatal(format!(
                "there is no field {}",
                String::from_utf8_lossy(&value::number_text(n, b"%.6g"))
            ));
        }
        Ok(usize::try_from(format::whole(n))
            .unwrap_or(usize::MAX)
            .min(1 << 24))
    }

    fn nf(&mut self) -> Result<Value, Stop> {
        self.split()?;
        #[allow(clippy::cast_precision_loss)]
        Ok(Value::Number(self.record.fields.len() as f64))
    }

    fn set_nf(&mut self, value: &Value) -> Result<(), Stop> {
        let n = value.number();
        if n < 0.0 || n.is_nan() {
            return fatal(String::from("NF cannot be below 0"));
        }
        self.split()?;
        let n = usize::try_from(format::whole(n)).unwrap_or(usize::MAX);
        if n > MOST_FIELDS {
            return fatal(format!("no field past {MOST_FIELDS} can be made"));
        }
        self.record.fields.resize(n, Value::Unset);
        self.record.stale = true;
        Ok(())
    }

    // Variables and arrays.

    /// The value of the program's variable at `slot`, as a value.
    fn global(&self, slot: usize) -> Value {
        match &self.globals[slot] {
            Cell::Value(value) => value.clone(),
            _ => Value::Unset,
        }
    }

    /// A variable of awk's own as a string.
    fn global_text(&self, slot: usize) -> Vec<u8> {
        let convfmt = match &self.globals[parse::CONVFMT] {
            Cell::Value(Value::Text(t) | Value::Numeric(t, _)) => t.clone(),
            _ => b"%.6g".to_vec(),
        };
        self.global(slot).text(&convfmt)
    }

    fn cell(&mut self, var: Var) -> &mut Cell {
        match var {
            Var::Global(slot) => &mut self.globals[slot],
            Var::Local(slot) => {
                let frame = self.frames.len() - 1;
                &mut self.frames[frame].cells[slot]
            }
        }
    }

    fn name(&self, var: Var) -> String {
        match var {
            Var::Global(slot) => self.program.globals[slot].clone(),
            Var::Local(slot) => format!("parameter {}", slot + 1),
        }
    }

    fn get_var(&mut self, var: Var) -> Result<Value, Stop> {
        if var == Var::Global(parse::NF) {
            return self.nf();
        }
        match self.cell(var) {
            Cell::Value(value) => Ok(value.clone()),
            Cell::Unset(_) => Ok(Value::Unset),
            Cell::Array(_) => fatal(format!("{} is an array, used as a value", self.name(var))),
        }
    }

    fn set_var(&mut self, var: Var, value: Value) -> Result<(), Stop> {
        if var == Var::Global(parse::NF) {
            self.set_nf(&value)?;
        }
        let cell = self.cell(var);
        if matches!(cell, Cell::Array(_)) {
            return fatal(format!("{} is an array, given a value", self.name(var)));
        }
        *cell = Cell::Value(value);
        Ok(())
    }

    /// The array `var` is, made one if it was unset, and so for the
    /// variable it was given for, when it was given to a function.
    fn array(&mut self, var: Var) -> Result<usize, Stop> {
        let home = match self.cell(var) {
            Cell::Array(id) => return Ok(*id),
            Cell::Value(_) => {
                return fatal(format!("{} is a value, used as an array", self.name(var)));
            }
            Cell::Unset(home) => *home,
        };
        let id = self.free.pop().unwrap_or_else(|| {
            self.arrays.push(Array::new());
            self.arrays.len() - 1
        });
        *self.cell(var) = Cell::Array(id);
        // Up the calls to where the variable was first given, which keeps
        // the array; a function's own goes when it returns.
        let mut home = home;
        let mut owner = match var {
            Var::Global(_) => None,
            Var::Local(_) => Some(self.frames.len() - 1),
        };
        while let Some(Home { frame, slot }) = home {
            let cell = match frame {
                Some(frame) => &mut self.frames[frame].cells[slot],
                None => &mut self.globals[slot],
            };
            home = match cell {
                Cell::Unset(next) => *next,
                _ => None,
            };
            *cell = Cell::Array(id);
            owner = frame;
        }
        if let Some(frame) = owner {
            self.frames[frame].made.push(id);
        }
        Ok(id)
    }

    /// The subscript `subscripts` make, joined by SUBSEP.
    fn subscript(&mut self, subscripts: &[Expr], world: &mut impl World) -> Result<Vec<u8>, Stop> {
        let convfmt = self.global_text(parse::CONVFMT);
        let mut key = Vec::new();
        for (at, subscript) in subscripts.iter().enumerate() {
            if at > 0 {
                key.extend_from_slice(&self.global_text(parse::SUBSEP));
            }
            key.extend_from_slice(&self.eval(subscript, world)?.text(&convfmt));
        }
        Ok(key)
    }

    /// Where `place` is, its subscripts and field number worked out once.
    fn spot(&mut self, place: &Place, world: &mut impl World) -> Result<Spot, Stop> {
        Ok(match place {
            Place::Var(var) => Spot::Var(*var),
            Place::Element(var, subscripts) => {
                let key = self.subscript(subscripts, world)?;
                Spot::Element(self.array(*var)?, key)
            }
            Place::Field(index) => {
                let index = self.eval(index, world)?;
                Spot::Field(Self::field_number(&index)?)
            }
        })
    }

    fn read_spot(&mut self, spot: &Spot) -> Result<Value, Stop> {
        match spot {
            Spot::Var(var) => self.get_var(*var),
            // Naming an element makes it.
            Spot::Element(id, key) => Ok(self.arrays[*id]
                .entry(key.clone())
                .or_insert(Value::Unset)
                .clone()),
            Spot::Field(n) => self.field(*n),
        }
    }

    fn write_spot(&mut self, spot: Spot, value: Value) -> Result<(), Stop> {
        match spot {
            Spot::Var(var) => self.set_var(var, value),
            Spot::Element(id, key) => {
                self.arrays[id].insert(key, value);
                Ok(())
            }
            Spot::Field(n) => self.set_field(n, value),
        }
    }

    fn get(&mut self, place: &Place, world: &mut impl World) -> Result<Value, Stop> {
        let spot = self.spot(place, world)?;
        self.read_spot(&spot)
    }

    // Statements.

    // As with `eval`, what is not on the way of every call is done in
    // functions of its own.
    fn statement(&mut self, statement: &Stmt, world: &mut impl World) -> Result<Flow, Stop> {
        match statement {
            Stmt::Expr(expr) => {
                self.eval(expr, world)?;
                Ok(Flow::Normal)
            }
            Stmt::If(test, then, otherwise) => {
                if self.truth_of(test, world)? {
                    self.statement(then, world)
                } else if let Some(otherwise) = otherwise {
                    self.statement(otherwise, world)
                } else {
                    Ok(Flow::Normal)
                }
            }
            Stmt::Block(statements) => {
                for statement in statements {
                    match self.statement(statement, world)? {
                        Flow::Normal => {}
                        other => return Ok(other),
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::Return(Some(value)) => Ok(Flow::Return(self.eval(value, world)?)),
            Stmt::Return(None) => Ok(Flow::Return(Value::Unset)),
            Stmt::While(..) | Stmt::Do(..) | Stmt::For { .. } | Stmt::ForIn(..) => {
                self.looping(statement, world)
            }
            Stmt::Break => Ok(Flow::Break),
            Stmt::Continue => Ok(Flow::Continue),
            Stmt::Next => Err(Stop::Next),
            Stmt::Nextfile => Err(Stop::Nextfile),
            _ => self.effect(statement, world),
        }
    }

    /// `print`, `printf`, `exit` and `delete`.
    #[inline(never)]
    fn effect(&mut self, statement: &Stmt, world: &mut impl World) -> Result<Flow, Stop> {
        match statement {
            Stmt::Print(args, output) => {
                let line = self.printed(args, world)?;
                self.output(output, &line, world)?;
            }
            Stmt::Printf(args, output) => {
                let mut values = Vec::new();
                for arg in args {
                    values.push(self.eval(arg, world)?);
                }
                let convfmt = self.global_text(parse::CONVFMT);
                let form = values[0].text(&convfmt);
                let written = printf::sprintf(&form, &values[1..], &convfmt);
                self.output(output, &written, world)?;
            }
            Stmt::Exit(code) => {
                let code = match code {
                    Some(code) => {
                        let n = format::whole(self.eval(code, world)?.number());
                        Some(i32::try_from(n & 0xff).unwrap_or(0))
                    }
                    None => None,
                };
                return Err(Stop::Exit(code));
            }
            Stmt::Delete(array, subscripts) => {
                let key = match subscripts {
                    Some(subscripts) => Some(self.subscript(subscripts, world)?),
                    None => None,
                };
                let id = self.array(*array)?;
                match key {
                    Some(key) => {
                        self.arrays[id].remove(&key);
                    }
                    None => self.arrays[id].clear(),
                }
            }
            _ => return self.statement(statement, world),
        }
        Ok(Flow::Normal)
    }

    /// Runs a loop.
    #[inline(never)]
    fn looping(&mut self, statement: &Stmt, world: &mut impl World) -> Result<Flow, Stop> {
        match statement {
            Stmt::While(test, body) => {
                while self.eval(test, world)?.truth() {
                    match self.statement(body, world)? {
                        Flow::Break => break,
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                        Flow::Normal | Flow::Continue => {}
                    }
                }
            }
            Stmt::Do(body, test) => loop {
                match self.statement(body, world)? {
                    Flow::Break => break,
                    Flow::Return(value) => return Ok(Flow::Return(value)),
                    Flow::Normal | Flow::Continue => {}
                }
                if !self.eval(test, world)?.truth() {
                    break;
                }
            },
            Stmt::For {
                start,
                test,
                step,
                body,
            } => {
                if let Some(start) = start {
                    self.eval(start, world)?;
                }
                loop {
                    if let Some(test) = test
                        && !self.eval(test, world)?.truth()
                    {
                        break;
                    }
                    match self.statement(body, world)? {
                        Flow::Break => break,
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                        Flow::Normal | Flow::Continue => {}
                    }
                    if let Some(step) = step {
                        self.eval(step, world)?;
                    }
                }
            }
            Stmt::ForIn(var, array, body) => {
                let id = self.array(*array)?;
                let keys: Vec<Vec<u8>> = self.arrays[id].keys().cloned().collect();
                for key in keys {
                    // An element taken out on the way is passed over.
                    if !self.arrays[id].contains_key(&key) {
                        continue;
                    }
                    self.set_var(*var, Value::input(key))?;
                    match self.statement(body, world)? {
                        Flow::Break => break,
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                        Flow::Normal | Flow::Continue => {}
                    }
                }
            }
            _ => {}
        }
        Ok(Flow::Normal)
    }

    /// What `print` writes for `args`: `$0` when there are none, else each
    /// parted by OFS, a number as OFMT writes it, and ORS after.
    fn printed(&mut self, args: &[Expr], world: &mut impl World) -> Result<Vec<u8>, Stop> {
        let mut line = Vec::new();
        if args.is_empty() {
            line = self.whole();
        }
        let ofmt = self.global_text(parse::OFMT);
        for (at, arg) in args.iter().enumerate() {
            if at > 0 {
                line.extend_from_slice(&self.global_text(parse::OFS));
            }
            line.extend_from_slice(&self.eval(arg, world)?.text(&ofmt));
        }
        line.extend_from_slice(&self.global_text(parse::ORS));
        Ok(line)
    }

    /// Writes `bytes` where `output` says.
    fn output(
        &mut self,
        output: &Output,
        bytes: &[u8],
        world: &mut impl World,
    ) -> Result<(), Stop> {
        let (named, kind) = match output {
            Output::Standard => return write(Stream::Standard, bytes, world),
            Output::File(name) => (name, 0),
            Output::Append(name) => (name, 1),
            Output::Pipe(command) => (command, 2),
        };
        let convfmt = self.global_text(parse::CONVFMT);
        let name = self.eval(named, world)?.text(&convfmt);
        if name.is_empty() {
            return fatal(String::from("output to a name that is empty"));
        }
        let stream = match (kind, name.as_slice()) {
            (2, _) => Stream::Command(&name),
            (_, b"/dev/stdout" | b"-") => Stream::Standard,
            (_, b"/dev/stderr") => Stream::Error,
            (0, _) => Stream::File(&name),
            _ => Stream::Append(&name),
        };
        write(stream, bytes, world)
    }

    // Expressions.

    /// The regular expression `expr` is: one written in the program, or
    /// one its value as a string makes.
    fn regex(&mut self, expr: &Expr, world: &mut impl World) -> Result<RegexAt, Stop> {
        if let Expr::Regex(at) = expr {
            return Ok(RegexAt::Program(*at));
        }
        let convfmt = self.global_text(parse::CONVFMT);
        let pattern = self.eval(expr, world)?.text(&convfmt);
        if let Some(at) = self.dynamic.iter().position(|(made, _)| *made == pattern) {
            return Ok(RegexAt::Dynamic(at));
        }
        let regex = Regex::new(&String::from_utf8_lossy(&pattern), Syntax::Extended, false)
            .or_else(|e| {
                fatal(format!(
                    "/{}/: {}",
                    String::from_utf8_lossy(&pattern),
                    e.code.says()
                ))
            })?;
        // A few dozen kept is plenty for the expressions a program builds.
        if self.dynamic.len() >= 64 {
            self.dynamic.remove(0);
        }
        self.dynamic.push((pattern, regex));
        Ok(RegexAt::Dynamic(self.dynamic.len() - 1))
    }

    fn regex_of(&self, at: RegexAt) -> &Regex {
        match at {
            RegexAt::Program(at) => &self.program.regexes[at],
            RegexAt::Dynamic(at) => &self.dynamic[at].1,
        }
    }

    /// Where `regex` first matches `bytes` from the byte `from` on.
    fn find(
        &self,
        regex: RegexAt,
        text: &Text,
        from: usize,
    ) -> Result<Option<(usize, usize)>, Stop> {
        match self.regex_of(regex).find(text, from) {
            Ok(found) => Ok(found.map(|span| (span.start, span.end))),
            Err(e) => fatal(String::from(e.says())),
        }
    }

    fn matches(&mut self, bytes: &[u8], regex: RegexAt) -> Result<bool, Stop> {
        Ok(self.find(regex, &Text::new(bytes), 0)?.is_some())
    }

    // Each kind of expression is worked out in a function of its own, so
    // that the frame of this one, which every nested expression and call
    // adds to the stack, stays small.
    fn eval(&mut self, expr: &Expr, world: &mut impl World) -> Result<Value, Stop> {
        match expr {
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::Var(var) => self.get_var(*var),
            Expr::Place(place) => self.get(place, world),
            Expr::Arith(op, a, b) => self.arithmetic(*op, a, b, world),
            Expr::Cond(test, yes, no) => {
                let chosen = if self.truth_of(test, world)? { yes } else { no };
                self.eval(chosen, world)
            }
            Expr::Call(function, args) => self.call(*function, args, world),
            Expr::Builtin(builtin, args) => self.builtin(*builtin, args, world),
            Expr::Getline { from, into } => self.getline(from, into.as_deref(), world),
            _ => self.operation(expr, world),
        }
    }

    fn truth_of(&mut self, expr: &Expr, world: &mut impl World) -> Result<bool, Stop> {
        Ok(self.eval(expr, world)?.truth())
    }

    fn arithmetic(
        &mut self,
        op: Arith,
        a: &Expr,
        b: &Expr,
        world: &mut impl World,
    ) -> Result<Value, Stop> {
        let a = self.eval(a, world)?.number();
        let b = self.eval(b, world)?.number();
        Ok(Value::Number(arith(op, a, b)?))
    }

    /// The expressions `eval` leaves to others.
    #[inline(never)]
    fn operation(&mut self, expr: &Expr, world: &mut impl World) -> Result<Value, Stop> {
        let truth = |t: bool| Value::Number(if t { 1.0 } else { 0.0 });
        Ok(match expr {
            Expr::String(bytes) => Value::Text(bytes.clone()),
            Expr::Regex(at) => {
                let whole = self.whole();
                truth(self.matches(&whole, RegexAt::Program(*at))?)
            }
            Expr::Assign(op, place, value) => {
                let spot = self.spot(place, world)?;
                let value = self.eval(value, world)?;
                let value = match op {
                    None => value,
                    Some(op) => {
                        let old = self.read_spot(&spot)?.number();
                        Value::Number(arith(*op, old, value.number())?)
                    }
                };
                self.write_spot(spot, value.clone())?;
                value
            }
            Expr::Step { place, by, before } => {
                let spot = self.spot(place, world)?;
                let old = self.read_spot(&spot)?.number();
                self.write_spot(spot, Value::Number(old + by))?;
                Value::Number(if *before { old + by } else { old })
            }
            Expr::Negate(e) => Value::Number(-self.eval(e, world)?.number()),
            Expr::Plus(e) => Value::Number(self.eval(e, world)?.number()),
            Expr::Not(e) => truth(!self.eval(e, world)?.truth()),
            Expr::Concat(a, b) => {
                let convfmt = self.global_text(parse::CONVFMT);
                let mut joined = self.eval(a, world)?.text(&convfmt);
                joined.extend_from_slice(&self.eval(b, world)?.text(&convfmt));
                Value::Text(joined)
            }
            Expr::Compare(relation, a, b) => {
                let a = self.eval(a, world)?;
                let b = self.eval(b, world)?;
                let convfmt = self.global_text(parse::CONVFMT);
                let order = value::compare(&a, &b, &convfmt);
                truth(match relation {
                    Relation::Less => order == Some(Ordering::Less),
                    Relation::LessEqual => matches!(order, Some(Ordering::Less | Ordering::Equal)),
                    Relation::Equal => order == Some(Ordering::Equal),
                    Relation::NotEqual => order != Some(Ordering::Equal),
                    Relation::Greater => order == Some(Ordering::Greater),
                    Relation::GreaterEqual => {
                        matches!(order, Some(Ordering::Greater | Ordering::Equal))
                    }
                })
            }
            Expr::Match { not, text, regex } => {
                let convfmt = self.global_text(parse::CONVFMT);
                let bytes = self.eval(text, world)?.text(&convfmt);
                let regex = self.regex(regex, world)?;
                truth(self.matches(&bytes, regex)? != *not)
            }
            Expr::And(a, b) => truth(self.eval(a, world)?.truth() && self.eval(b, world)?.truth()),
            Expr::Or(a, b) => truth(self.eval(a, world)?.truth() || self.eval(b, world)?.truth()),
            Expr::In(subscripts, array) => {
                let key = self.subscript(subscripts, world)?;
                let id = self.array(*array)?;
                truth(self.arrays[id].contains_key(&key))
            }
            Expr::Number(_)
            | Expr::Var(_)
            | Expr::Place(_)
            | Expr::Arith(..)
            | Expr::Cond(..)
            | Expr::Call(..)
            | Expr::Builtin(..)
            | Expr::Getline { .. } => self.eval(expr, world)?,
        })
    }

    /// Calls the program's function `function` with `args`: a variable
    /// that is an array, or unset, goes as itself, anything else as its
    /// value.
    #[inline(never)]
    fn call(
        &mut self,
        function: usize,
        args: &[Expr],
        world: &mut impl World,
    ) -> Result<Value, Stop> {
        let program = self.program;
        let defined = &program.functions[function];
        if args.len() > defined.params {
            return fatal(format!(
                "{} takes {} arguments at most",
                defined.name, defined.params
            ));
        }
        if self.frames.len() >= DEEPEST {
            return fatal(format!("functions called more than {DEEPEST} deep"));
        }
        let mut cells = Vec::with_capacity(defined.params);
        for arg in args {
            let by_name = match arg {
                Expr::Var(var) => {
                    let frame = match var {
                        Var::Global(_) => None,
                        Var::Local(_) => Some(self.frames.len() - 1),
                    };
                    let slot = match var {
                        Var::Global(slot) | Var::Local(slot) => *slot,
                    };
                    match self.cell(*var) {
                        Cell::Array(id) => Some(Cell::Array(*id)),
                        Cell::Unset(_) => Some(Cell::Unset(Some(Home { frame, slot }))),
                        Cell::Value(_) => None,
                    }
                }
                _ => None,
            };
            let cell = match by_name {
                Some(cell) => cell,
                None => Cell::Value(self.eval(arg, world)?),
            };
            cells.push(cell);
        }
        cells.resize(defined.params, Cell::Unset(None));
        self.frames.push(Frame {
            cells,
            made: Vec::new(),
        });
        let mut returned = Ok(Value::Unset);
        for statement in &defined.body {
            match self.statement(statement, world) {
                Ok(Flow::Return(value)) => {
                    returned = Ok(value);
                    break;
                }
                Ok(_) => {}
                Err(stop) => {
                    returned = Err(stop);
                    break;
                }
            }
        }
        if let Some(frame) = self.frames.pop() {
            for id in frame.made {
                self.arrays[id].clear();
                self.free.push(id);
            }
        }
        returned
    }

    #[inline(never)]
    fn getline(
        &mut self,
        from: &Source,
        into: Option<&Place>,
        world: &mut impl World,
    ) -> Result<Value, Stop> {
        // The place is worked out before the read, whether a line comes
        // or not.
        let spot = match into {
            Some(place) => Some(self.spot(place, world)?),
            None => None,
        };
        let record = match from {
            Source::Main => self.main_record(world)?.ok_or(()),
            Source::File(name) | Source::Command(name) => {
                let convfmt = self.global_text(parse::CONVFMT);
                let name = self.eval(name, world)?.text(&convfmt);
                let command = matches!(from, Source::Command(_));
                let stream = match (command, name.as_slice()) {
                    (true, _) => Stream::Command(&name),
                    (false, b"-" | b"/dev/stdin") => Stream::Standard,
                    (false, _) => Stream::File(&name),
                };
                let key = (u8::from(command), name.clone());
                let mut reader = self.readers.remove(&key).unwrap_or_default();
                let record = self.next_record(&mut reader, stream, world)?;
                self.readers.insert(key, reader);
                match record {
                    Ok(Some(record)) => Ok(record),
                    Ok(None) => Err(()),
                    Err(_) => return Ok(Value::Number(-1.0)),
                }
            }
        };
        let Ok(record) = record else {
            return Ok(Value::Number(0.0));
        };
        // A record from the input, or from a command, counts in NR.
        match from {
            Source::Main => self.count_record(true),
            Source::Command(_) => self.count_record(false),
            Source::File(_) => {}
        }
        match spot {
            Some(spot) => self.write_spot(spot, Value::input(record))?,
            None => self.set_record(record),
        }
        Ok(Value::Number(1.0))
    }

    /// The value of `args[at]` as a string.
    fn text_arg(
        &mut self,
        args: &[Expr],
        at: usize,
        world: &mut impl World,
    ) -> Result<Vec<u8>, Stop> {
        let convfmt = self.global_text(parse::CONVFMT);
        Ok(match args.get(at) {
            Some(arg) => self.eval(arg, world)?.text(&convfmt),
            None => self.whole(),
        })
    }

    fn number_arg(
        &mut self,
        args: &[Expr],
        at: usize,
        world: &mut impl World,
    ) -> Result<f64, Stop> {
        Ok(self.eval(&args[at], world)?.number())
    }

    #[inline(never)]
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    fn builtin(
        &mut self,
        builtin: Builtin,
        args: &[Expr],
        world: &mut impl World,
    ) -> Result<Value, Stop> {
        let number = Value::Number;
        Ok(match builtin {
            Builtin::Length => {
                if let Some(Expr::Var(var)) = args.first()
                    && let Cell::Array(id) = self.cell(*var)
                {
                    let id = *id;
                    return Ok(number(self.arrays[id].len() as f64));
                }
                number(printf::chars(&self.text_arg(args, 0, world)?) as f64)
            }
            Builtin::Substr => {
                let bytes = self.text_arg(args, 0, world)?;
                let start = floor(self.number_arg(args, 1, world)? + 0.5);
                let end = match args.get(2) {
                    Some(len) => start + floor(self.eval(len, world)?.number() + 0.5),
                    None => f64::INFINITY,
                };
                // What is no number gives no characters; min and max
                // would pass over it.
                if start.is_nan() || end.is_nan() {
                    return Ok(text(b""));
                }
                let len = printf::chars(&bytes) as f64;
                let (from, to) = (start.max(1.0), end.min(len + 1.0));
                if from.is_nan() || to.is_nan() || to <= from {
                    return Ok(text(b""));
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let (from, to) = (from as usize - 1, to as usize - 1);
                text(char_span(&bytes, from, to))
            }
            Builtin::Index => {
                let haystack = self.text_arg(args, 0, world)?;
                let needle = self.text_arg(args, 1, world)?;
                let found = (!needle.is_empty())
                    .then(|| {
                        haystack
                            .windows(needle.len())
                            .position(|w| w == needle.as_slice())
                    })
                    .flatten();
                number(found.map_or(0.0, |at| (printf::chars(&haystack[..at]) + 1) as f64))
            }
            Builtin::Split => self.split_into(args, world)?,
            Builtin::Sub | Builtin::Gsub => {
                self.substitute(builtin == Builtin::Gsub, args, world)?
            }
            Builtin::Match => {
                let bytes = self.text_arg(args, 0, world)?;
                let regex = self.regex(&args[1], world)?;
                let found = self.find(regex, &Text::new(&bytes), 0)?;
                let (start, length) = match found {
                    Some((start, end)) => (
                        (printf::chars(&bytes[..start]) + 1) as f64,
                        printf::chars(&bytes[start..end]) as f64,
                    ),
                    None => (0.0, -1.0),
                };
                self.globals[parse::RSTART] = Cell::Value(number(start));
                self.globals[parse::RLENGTH] = Cell::Value(number(length));
                number(start)
            }
            Builtin::Sprintf => {
                let mut values = Vec::new();
                for arg in args {
                    values.push(self.eval(arg, world)?);
                }
                let convfmt = self.global_text(parse::CONVFMT);
                let form = values[0].text(&convfmt);
                Value::Text(printf::sprintf(&form, &values[1..], &convfmt))
            }
            Builtin::Sin => number(math::sin(self.number_arg(args, 0, world)?)),
            Builtin::Cos => number(math::cos(self.number_arg(args, 0, world)?)),
            Builtin::Atan2 => {
                let y = self.number_arg(args, 0, world)?;
                number(math::atan2(y, self.number_arg(args, 1, world)?))
            }
            Builtin::Exp => number(math::exp(self.number_arg(args, 0, world)?)),
            Builtin::Log => number(math::log(self.number_arg(args, 0, world)?)),
            Builtin::Sqrt => number(math::sqrt(self.number_arg(args, 0, world)?)),
            Builtin::Int => number(truncated(self.number_arg(args, 0, world)?)),
            Builtin::Rand => {
                number((splitmix(&mut self.random) >> 11) as f64 / (1u64 << 53) as f64)
            }
            Builtin::Srand => {
                let seed = match args.first() {
                    Some(seed) => self.eval(seed, world)?.number(),
                    None => world.ticks() as f64,
                };
                let before = core::mem::replace(&mut self.seed, seed);
                self.random = seed.to_bits();
                splitmix(&mut self.random);
                number(before)
            }
            Builtin::Tolower | Builtin::Toupper => {
                let bytes = self.text_arg(args, 0, world)?;
                let mut changed = Vec::with_capacity(bytes.len());
                for chunk in bytes.utf8_chunks() {
                    for c in chunk.valid().chars() {
                        let mut room = [0; 4];
                        if builtin == Builtin::Tolower {
                            for l in c.to_lowercase() {
                                changed.extend_from_slice(l.encode_utf8(&mut room).as_bytes());
                            }
                        } else {
                            for u in c.to_uppercase() {
                                changed.extend_from_slice(u.encode_utf8(&mut room).as_bytes());
                            }
                        }
                    }
                    changed.extend_from_slice(chunk.invalid());
                }
                Value::Text(changed)
            }
            Builtin::Close => {
                let name = self.text_arg(args, 0, world)?;
                let read = self.readers.remove(&(0, name.clone())).is_some();
                let ran = self.readers.remove(&(1, name.clone())).is_some();
                match world.close(&name) {
                    Some(status) => number(f64::from(status)),
                    None if read || ran => number(0.0),
                    None => number(-1.0),
                }
            }
            Builtin::System => {
                let command = self.text_arg(args, 0, world)?;
                world.flush(None);
                number(f64::from(world.system(&command)))
            }
            Builtin::Fflush => {
                let flushed = match args.first() {
                    Some(name) => {
                        let convfmt = self.global_text(parse::CONVFMT);
                        let name = self.eval(name, world)?.text(&convfmt);
                        world.flush(Some(&name))
                    }
                    None => world.flush(None),
                };
                number(if flushed { 0.0 } else { -1.0 })
            }
        })
    }

    /// `split(s, a, fs)`: the fields of `s` as `fs`, or FS, splits them,
    /// into `a` from 1 on, numeric strings when they look like numbers.
    fn split_into(&mut self, args: &[Expr], world: &mut impl World) -> Result<Value, Stop> {
        let bytes = self.text_arg(args, 0, world)?;
        let Expr::Var(array) = args[1] else {
            return fatal(String::from("split puts what it splits into an array"));
        };
        let pieces: Vec<Vec<u8>> = match args.get(2) {
            Some(Expr::Regex(at)) => {
                let regex = &self.program.regexes[*at];
                let mut pieces = Vec::new();
                if !bytes.is_empty() {
                    let searched = Text::new(&bytes);
                    let mut at = 0;
                    loop {
                        let found = regex
                            .find(&searched, at)
                            .or_else(|e| fatal(String::from(e.says())))?;
                        match found {
                            Some(span) if span.end > span.start => {
                                pieces.push(bytes[at..span.start].to_vec());
                                at = span.end;
                            }
                            Some(span) if span.start < bytes.len() => {
                                // An empty match parts nothing.
                                let next =
                                    span.start + printf::first_chars(&bytes[span.start..], 1).len();
                                if let Some(more) = regex
                                    .find(&searched, next)
                                    .ok()
                                    .flatten()
                                    .filter(|s| s.end > s.start)
                                {
                                    pieces.push(bytes[at..more.start].to_vec());
                                    at = more.end;
                                } else {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    pieces.push(bytes[at..].to_vec());
                }
                pieces
            }
            Some(fs) => {
                let convfmt = self.global_text(parse::CONVFMT);
                let fs = self.eval(fs, world)?.text(&convfmt);
                let splitter = Splitter::new(&fs, false).or_else(|e| {
                    fatal(format!(
                        "{}: {}",
                        String::from_utf8_lossy(&fs),
                        e.code.says()
                    ))
                })?;
                splitter
                    .split(&bytes)
                    .into_iter()
                    .map(<[u8]>::to_vec)
                    .collect()
            }
            None => {
                let fs = self.global_text(parse::FS);
                self.splitter(&fs, false)?
                    .split(&bytes)
                    .into_iter()
                    .map(<[u8]>::to_vec)
                    .collect()
            }
        };
        let id = self.array(array)?;
        let made = &mut self.arrays[id];
        made.clear();
        for (at, piece) in pieces.iter().enumerate() {
            made.insert(
                format!("{}", at + 1).into_bytes(),
                Value::input(piece.clone()),
            );
        }
        #[allow(clippy::cast_precision_loss)]
        Ok(Value::Number(pieces.len() as f64))
    }

    /// `sub` and, when `all`, `gsub`: the first match of the expression
    /// in the place, or each, replaced; how many were.
    fn substitute(
        &mut self,
        all: bool,
        args: &[Expr],
        world: &mut impl World,
    ) -> Result<Value, Stop> {
        let regex = self.regex(&args[0], world)?;
        let repl = self.text_arg(args, 1, world)?;
        let whole = Place::Field(alloc::boxed::Box::new(Expr::Number(0.0)));
        let place = match args.get(2) {
            Some(Expr::Var(var)) => Place::Var(*var),
            Some(Expr::Place(place)) => (**place).clone(),
            _ => whole,
        };
        let spot = self.spot(&place, world)?;
        let convfmt = self.global_text(parse::CONVFMT);
        let bytes = self.read_spot(&spot)?.text(&convfmt);
        let searched = Text::new(&bytes);
        let mut changed = Vec::new();
        let mut copied = 0;
        let mut at = 0;
        let mut last_end = None;
        let mut count = 0;
        while at <= bytes.len() {
            let Some((start, end)) = self.find(regex, &searched, at)? else {
                break;
            };
            let step = || start + printf::first_chars(&bytes[start..], 1).len().max(1);
            if start == end && last_end == Some(start) {
                // An empty match right after another is none.
                if start >= bytes.len() {
                    break;
                }
                at = step();
                continue;
            }
            changed.extend_from_slice(&bytes[copied..start]);
            replaced(&repl, &bytes[start..end], &mut changed);
            copied = end;
            count += 1;
            last_end = Some(end);
            if !all {
                break;
            }
            if start == end {
                if start >= bytes.len() {
                    break;
                }
                at = step();
            } else {
                at = end;
            }
        }
        if count > 0 {
            changed.extend_from_slice(&bytes[copied..]);
            self.write_spot(spot, Value::Text(changed))?;
        }
        Ok(Value::Number(f64::from(count)))
    }
}

/// A place with its subscripts and field number worked out, so that it is
/// read and written as the one place it named.
enum Spot {
    Var(Var),
    Element(usize, Vec<u8>),
    Field(usize),
}

/// Where a regular expression is kept.
#[derive(Clone, Copy)]
enum RegexAt {
    Program(usize),
    Dynamic(usize),
}

/// Writes `bytes` to `to`, what went wrong a reason to stop.
fn write(to: Stream, bytes: &[u8], world: &mut impl World) -> Result<(), Stop> {
    world.write(to, bytes).or_else(|why| {
        let name = match to {
            Stream::Standard => b"the standard output".as_slice(),
            Stream::Error => b"the standard error".as_slice(),
            Stream::File(name) | Stream::Append(name) | Stream::Command(name) => name,
        };
        fatal(format!("{}: {why}", String::from_utf8_lossy(name)))
    })
}

fn arith(op: Arith, a: f64, b: f64) -> Result<f64, Stop> {
    Ok(match op {
        Arith::Add => a + b,
        Arith::Sub => a - b,
        Arith::Mul => a * b,
        Arith::Div if b == 0.0 => return fatal(String::from("division by zero")),
        Arith::Div => a / b,
        Arith::Mod if b == 0.0 => return fatal(String::from("division by zero in %")),
        Arith::Mod => math::fmod(a, b),
        Arith::Pow => math::pow(a, b),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in awk reaches the host's own files.
    fn confined(root: &std::path::Path, path: &[u8]) -> std::path::PathBuf {
        let path = String::from_utf8_lossy(path);
        let mut depth = 0usize;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .unwrap_or_else(|| panic!("{path} climbs out of the test's directory"));
                }
                _ => depth += 1,
            }
        }
        root.join(path.trim_start_matches('/'))
    }

    /// What a command written to was given, run when it is closed.
    enum Writer {
        File(std::fs::File),
        Command(Vec<u8>),
    }

    /// awk's world on the host: files in a directory of their own, gone
    /// once dropped, commands run by the host's sh in it, the standard
    /// input given in pieces of a few bytes as a pipe gives them.
    struct Host {
        root: std::path::PathBuf,
        stdin: Vec<u8>,
        out: Vec<u8>,
        err: Vec<u8>,
        writers: BTreeMap<Vec<u8>, Writer>,
        readers: BTreeMap<Vec<u8>, std::io::Cursor<Vec<u8>>>,
        files: BTreeMap<Vec<u8>, std::fs::File>,
    }

    /// How many bytes each read gives at most, so that records come in
    /// pieces.
    const PIECE: usize = 5;

    impl Host {
        fn new(name: &str) -> Host {
            static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("meridian-awk-{name}-{}-{made}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                stdin: Vec::new(),
                out: Vec::new(),
                err: Vec::new(),
                writers: BTreeMap::new(),
                readers: BTreeMap::new(),
                files: BTreeMap::new(),
            }
        }

        fn put(&self, name: &str, bytes: &str) {
            std::fs::write(self.root.join(name), bytes).unwrap();
        }

        fn holds(&self, name: &str) -> String {
            std::fs::read_to_string(self.root.join(name)).unwrap()
        }

        /// Runs `command` with `input`, its output to awk's: its status.
        fn sh(&mut self, command: &[u8], input: &[u8]) -> i32 {
            let mut child = std::process::Command::new("sh")
                .arg("-c")
                .arg(String::from_utf8_lossy(command).as_ref())
                .current_dir(&self.root)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(input).unwrap();
            let done = child.wait_with_output().unwrap();
            self.out.extend_from_slice(&done.stdout);
            done.status.code().unwrap_or(-1)
        }

        /// Closes what is still open, as a program's end does.
        fn finish(&mut self) {
            self.files.clear();
            self.readers.clear();
            let names: Vec<Vec<u8>> = self.writers.keys().cloned().collect();
            for name in names {
                self.close(&name);
            }
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl World for Host {
        fn read(&mut self, from: Stream) -> Result<Vec<u8>, String> {
            match from {
                Stream::Standard => {
                    let taken = self.stdin.len().min(PIECE);
                    Ok(self.stdin.drain(..taken).collect())
                }
                Stream::File(name) => {
                    if !self.files.contains_key(name) {
                        let file = std::fs::File::open(confined(&self.root, name))
                            .map_err(|_| String::from("no such file"))?;
                        self.files.insert(name.to_vec(), file);
                    }
                    let mut room = [0; PIECE];
                    let len = self
                        .files
                        .get_mut(name)
                        .map_or(Ok(0), |f| f.read(&mut room))
                        .map_err(|e| format!("{e}"))?;
                    Ok(room[..len].to_vec())
                }
                Stream::Command(command) => {
                    if !self.readers.contains_key(command) {
                        let done = std::process::Command::new("sh")
                            .arg("-c")
                            .arg(String::from_utf8_lossy(command).as_ref())
                            .current_dir(&self.root)
                            .output()
                            .unwrap();
                        self.readers
                            .insert(command.to_vec(), std::io::Cursor::new(done.stdout));
                    }
                    let mut room = [0; PIECE];
                    let len = self
                        .readers
                        .get_mut(command)
                        .map_or(0, |r| r.read(&mut room).unwrap());
                    Ok(room[..len].to_vec())
                }
                Stream::Error | Stream::Append(_) => Err(String::from("not for reading")),
            }
        }

        fn write(&mut self, to: Stream, bytes: &[u8]) -> Result<(), String> {
            match to {
                Stream::Standard => self.out.extend_from_slice(bytes),
                Stream::Error => self.err.extend_from_slice(bytes),
                Stream::File(name) | Stream::Append(name) => {
                    if !self.writers.contains_key(name) {
                        let file = std::fs::OpenOptions::new()
                            .create(true)
                            .write(true)
                            .append(matches!(to, Stream::Append(_)))
                            .truncate(matches!(to, Stream::File(_)))
                            .open(confined(&self.root, name))
                            .map_err(|_| String::from("cannot be written"))?;
                        self.writers.insert(name.to_vec(), Writer::File(file));
                    }
                    if let Some(Writer::File(file)) = self.writers.get_mut(name) {
                        file.write_all(bytes).map_err(|e| format!("{e}"))?;
                    }
                }
                Stream::Command(command) => {
                    let Writer::Command(input) = self
                        .writers
                        .entry(command.to_vec())
                        .or_insert_with(|| Writer::Command(Vec::new()))
                    else {
                        return Err(String::from("open as a file"));
                    };
                    input.extend_from_slice(bytes);
                }
            }
            Ok(())
        }

        fn close(&mut self, name: &[u8]) -> Option<i32> {
            let read = self.files.remove(name).is_some() | self.readers.remove(name).is_some();
            match self.writers.remove(name) {
                Some(Writer::Command(input)) => Some(self.sh(name, &input)),
                Some(Writer::File(_)) => Some(0),
                None => read.then_some(0),
            }
        }

        fn flush(&mut self, _: Option<&[u8]>) -> bool {
            true
        }

        fn system(&mut self, command: &[u8]) -> i32 {
            self.sh(command, b"")
        }

        fn ticks(&mut self) -> u64 {
            42
        }
    }

    /// How awk ran: its status, what it wrote and what it said.
    #[derive(Debug, PartialEq)]
    struct Ran {
        status: Result<i32, String>,
        out: String,
        err: String,
    }

    /// Runs the program `text` on `host` with `operands`, its standard
    /// input `input`.
    fn run_on(host: &mut Host, text: &str, operands: &[&str], input: &str) -> Ran {
        let program = parse::parse(text).unwrap();
        let operands: Vec<Vec<u8>> = operands.iter().map(|o| o.as_bytes().to_vec()).collect();
        host.stdin = input.as_bytes().to_vec();
        host.out.clear();
        host.err.clear();
        let mut awk = Awk::new(&program, &operands);
        let status = awk.run(host).map_err(|Fatal(why)| why);
        host.finish();
        Ran {
            status,
            out: String::from_utf8_lossy(&host.out).into_owned(),
            err: String::from_utf8_lossy(&host.err).into_owned(),
        }
    }

    /// What the program `text` writes, run on `input` alone.
    fn awk(text: &str, input: &str) -> String {
        let ran = run_on(&mut Host::new("plain"), text, &[], input);
        assert_eq!(ran.status, Ok(0), "{text}: {ran:?}");
        ran.out
    }

    /// Why the program `text` stopped.
    fn stopped(text: &str, input: &str) -> String {
        run_on(&mut Host::new("stopped"), text, &[], input)
            .status
            .unwrap_err()
    }

    #[test]
    fn begin_alone_reads_no_input() {
        let mut host = Host::new("begin");
        let ran = run_on(&mut host, "BEGIN { print \"hi\" }", &[], "left\n");
        assert_eq!(
            ran,
            Ran {
                status: Ok(0),
                out: "hi\n".into(),
                err: String::new()
            }
        );
        assert_eq!(host.stdin, b"left\n", "the input stays unread");
        assert_eq!(
            awk("END { print NR }", "a\nb\nc"),
            "3\n",
            "an END action reads it all"
        );
    }

    #[test]
    fn fields_split_at_blanks_and_make_the_record_again() {
        assert_eq!(awk("{ print $2, $1 }", "a b\n  c \t d  \n"), "b a\nd c\n");
        assert_eq!(awk("{ print NF, $NF, $(NF-1) }", "a b c\n"), "3 c b\n");
        assert_eq!(
            awk("{ print $5 \"|\" }", "a b\n"),
            "|\n",
            "a field past the last is empty"
        );
        assert_eq!(
            awk(
                "BEGIN { OFS = \"-\" } { $2 = \"X\"; print; print NF }",
                "a b c\n"
            ),
            "a-X-c\n3\n"
        );
        assert_eq!(
            awk(
                "BEGIN { OFS = \":\" } { $5 = \"e\"; print; print NF }",
                "a b\n"
            ),
            "a:b:::e\n5\n"
        );
        assert_eq!(awk("{ NF = 2; print }", "a b c d\n"), "a b\n");
        assert_eq!(awk("{ NF = 4; $1 = $1; print }", "a b\n"), "a b  \n");
        assert_eq!(awk("{ $0 = \"x y z\"; print NF, $3 }", "a\n"), "3 z\n");
        assert_eq!(awk("{ $3 = \"\"; print NF }", "a b c d\n"), "4\n");
        assert_eq!(awk("{ $2++; print }", "a 5\n"), "a 6\n");
        assert_eq!(
            awk("{ print; $1 = $1; print }", "  a   b  \n"),
            "  a   b  \na b\n",
            "a field set makes the record of the fields"
        );
        assert_eq!(stopped("{ print $(-1) }", "a\n"), "there is no field -1");
        assert_eq!(stopped("{ NF = -1 }", "a\n"), "NF cannot be below 0");
    }

    #[test]
    fn fs_splits_the_next_record_as_it_is_when_that_comes() {
        assert_eq!(
            awk("{ FS = \":\"; print $1 }", "a:b c\nd:e f\n"),
            "a:b\nd\n"
        );
        assert_eq!(awk("BEGIN { FS = \",\" } { print $2 }", "a,b,c\n"), "b\n");
        assert_eq!(
            awk(
                "BEGIN { FS = \"[0-9]+\" } { print $1 \"-\" $2 }",
                "ab12cd\n"
            ),
            "ab-cd\n"
        );
        assert_eq!(
            awk("BEGIN { FS = \"\\t\" } { print $2 }", "a b\tc\n"),
            "c\n"
        );
        assert_eq!(
            awk("BEGIN { FS = \"\" } { print NF, $2 }", "abc\n"),
            "3 b\n"
        );
        assert_eq!(
            awk("{ FS = \":\"; $0 = \"x:y\"; print $2 }", "a\n"),
            "y\n",
            "a record set takes the FS of then"
        );
        assert_eq!(
            stopped("BEGIN { FS = \"a(\" } { print $1 }", "x\n"),
            "FS: a parenthesis without its other half"
        );
    }

    #[test]
    fn records_end_as_rs_says() {
        assert_eq!(
            awk("BEGIN { RS = \";\" } { print NR \": \" $0 }", "a;b;c"),
            "1: a\n2: b\n3: c\n"
        );
        assert_eq!(
            awk(
                "BEGIN { RS = \"\" } { print NR, NF, $2 }",
                "\n\na b\nc\n\n\nd\n"
            ),
            "1 3 b\n2 1 \n",
            "in paragraphs a line feed parts fields too"
        );
        assert_eq!(
            awk("BEGIN { RS = \"\"; FS = \":\" } { print $2 }", "a:b\nc\n"),
            "b\n"
        );
        assert_eq!(
            awk("BEGIN { RS = \"[0-9]+\" } { print }", "a12b345c"),
            "a\nb\nc\n"
        );
        assert_eq!(
            awk("{ print }", "no end"),
            "no end\n",
            "the last record needs no line feed"
        );
        assert_eq!(
            awk("NR == 1 { RS = \";\" } { print }", "a\nb;c"),
            "a\nb\nc\n",
            "a new RS ends the next record"
        );
        assert_eq!(
            stopped("BEGIN { RS = \"a(\" } { print }", "x\n"),
            "RS: a parenthesis without its other half"
        );
    }

    #[test]
    fn patterns_select_records_and_ranges_run_on() {
        let input = "a\nstart\nb\nend\nc\nstart\nd\n";
        assert_eq!(awk("/start/, /end/", input), "start\nb\nend\nstart\nd\n");
        assert_eq!(
            awk("NR == 2, NR == 2", input),
            "start\n",
            "a range may start and end on one record"
        );
        assert_eq!(awk("/^[a-c]$/ { n++ } END { print n }", input), "3\n");
        assert_eq!(awk("NR % 2", input), "a\nb\nc\nd\n");
        assert_eq!(
            awk("$0 ~ \"ta+r\"", input),
            "start\nstart\n",
            "a string is a regular expression where one goes"
        );
        assert_eq!(awk("!/a/", "a\nb\n"), "b\n");
        assert_eq!(awk("{ print } { print \"again\" }", "x\n"), "x\nagain\n");
    }

    #[test]
    fn comparisons_take_numbers_from_the_input() {
        assert_eq!(
            awk("$1 == 10 { print \"ten\" }", "10.0\n1e1\nten\n"),
            "ten\nten\n"
        );
        assert_eq!(
            awk("{ print ($1 < $2) }", "9 10\nabc abd\n10 9x\n"),
            "1\n1\n1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { print (2 < \"10\"), (2 < 10), (\"a\" < \"b\") }",
                ""
            ),
            "0 1 1\n",
            "a string constant compares as a string"
        );
        assert_eq!(
            awk("BEGIN { print (x == 0), (x == \"\"), length(x) }", ""),
            "1 1 0\n"
        );
        assert_eq!(
            awk("BEGIN { x = \"3\" + 4; y = \"3x\" * 2; print x, y }", ""),
            "7 6\n"
        );
        assert_eq!(
            awk("{ x = $1; print (x == 1) }", "1.0\n"),
            "1\n",
            "a numeric string stays one when assigned"
        );
        assert_eq!(
            awk(
                "BEGIN { print 1 == 1.0, \"a\" != \"a\", 3 >= 3, 2 <= 1, (2 > 1) }",
                ""
            ),
            "1 0 1 0 1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { n = log(-1); print (n == n), (n != n), (n < 1) }",
                ""
            ),
            "0 1 0\n"
        );
    }

    #[test]
    fn numbers_are_written_as_ofmt_and_convfmt_say() {
        assert_eq!(
            awk("BEGIN { print 0.1 + 0.2, 1e6, 1e30, 3/2, -0 }", ""),
            "0.3 1000000 1e+30 1.5 0\n"
        );
        assert_eq!(
            awk(
                "BEGIN { OFMT = \"%.2f\"; print 3.14159; x = 3.14159 \"\"; print x }",
                ""
            ),
            "3.14\n3.14159\n"
        );
        assert_eq!(
            awk(
                "BEGIN { CONVFMT = \"%.2f\"; x = 3.14159 \"\"; print x; print 3.14159 }",
                ""
            ),
            "3.14\n3.14159\n"
        );
        assert_eq!(
            awk(
                "BEGIN { CONVFMT = \"%d\"; a[1.5] = 1; for (k in a) print k }",
                ""
            ),
            "1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { a[01] = 1; a[\"01\"] = 2; for (k in a) print k, a[k] }",
                ""
            ),
            "01 2\n1 1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { print 2^10, 2^0.5, -2^2, 2^3^2, 7 % 3, -7 % 3, 7.5 % 2 }",
                ""
            ),
            "1024 1.41421 -4 512 1 -1 1.5\n"
        );
        assert_eq!(awk("BEGIN { print 1/3 }", ""), "0.333333\n");
        assert_eq!(
            awk("BEGIN { print length(12345), length(1/4) }", ""),
            "5 4\n"
        );
        assert_eq!(stopped("BEGIN { print 1/0 }", ""), "division by zero");
        assert_eq!(stopped("BEGIN { print 1%0 }", ""), "division by zero in %");
        assert_eq!(stopped("BEGIN { x = 0; x /= 0 }", ""), "division by zero");
    }

    #[test]
    fn arrays_count_delete_and_take_more_subscripts() {
        assert_eq!(
            awk(
                "{ for (i = 1; i <= NF; i++) c[$i]++ } END { for (w in c) print w, c[w] }",
                "b a b\nc b\n"
            ),
            "a 1\nb 3\nc 1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1]; print length(a); if (2 in a) print \"no\"; print length(a) }",
                ""
            ),
            "1\n1\n",
            "in makes no element"
        );
        assert_eq!(
            awk("BEGIN { if (a[2] == \"\") print length(a) }", ""),
            "1\n",
            "naming an element makes it"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1]; a[2]; delete a[1]; for (k in a) print k; delete a; print length(a) }",
                ""
            ),
            "2\n0\n"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1, \"x\"] = 3; for (k in a) { split(k, p, SUBSEP); print p[1], p[2] } if ((1, \"x\") in a) print \"in\" }",
                ""
            ),
            "1 x\nin\n"
        );
        assert_eq!(
            awk(
                "BEGIN { SUBSEP = \":\"; a[1, 2]; for (k in a) print k }",
                ""
            ),
            "1:2\n"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1]; a[2]; a[3]; for (k in a) { delete a[3]; n++ } print n }",
                ""
            ),
            "2\n",
            "an element taken out on the way is passed over"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1]; for (k in a) a[k \"x\"]; print length(a) }",
                ""
            ),
            "2\n"
        );
        assert_eq!(
            stopped("BEGIN { x = 1; x[1] = 2 }", ""),
            "x is a value, used as an array"
        );
        assert_eq!(
            stopped("BEGIN { x[1] = 2; print x }", ""),
            "x is an array, used as a value"
        );
        assert_eq!(
            stopped("BEGIN { x[1] = 2; x = 3 }", ""),
            "x is an array, given a value"
        );
    }

    #[test]
    fn functions_take_values_and_arrays_and_recurse() {
        assert_eq!(
            awk(
                "function f(n) { return n <= 1 ? 1 : n * f(n - 1) } BEGIN { print f(10) }",
                ""
            ),
            "3628800\n"
        );
        assert_eq!(
            awk(
                "function f(a, b,   t) { t = a; a = b; return t + a } BEGIN { x = 1; print f(x, 2), x, t }",
                ""
            ),
            "3 1 \n",
            "values are copied and locals are new"
        );
        assert_eq!(
            awk(
                "function fill(a) { a[\"x\"] = 1; a[\"y\"] = 2 } BEGIN { fill(arr); print length(arr) }",
                ""
            ),
            "2\n",
            "an unset variable becomes the array the function makes"
        );
        assert_eq!(
            awk(
                "function f(a) { g(a) } function g(b) { b[1] = 7 } BEGIN { f(x); print x[1] }",
                ""
            ),
            "7\n",
            "through calls too"
        );
        assert_eq!(
            awk(
                "function f(a) { a[1] = 9 } BEGIN { z[1] = 1; f(z); print z[1] }",
                ""
            ),
            "9\n",
            "an array goes as itself"
        );
        assert_eq!(
            awk(
                "function f(  local) { local[1] = 1; return length(local) } BEGIN { print f(), f() }",
                ""
            ),
            "1 1\n",
            "an array of a call goes when it returns"
        );
        assert_eq!(
            awk(
                "function f() { } BEGIN { x = f(); print x == 0, x == \"\" }",
                ""
            ),
            "1 1\n"
        );
        assert_eq!(
            awk(
                "function f() { return } BEGIN { print \"[\" f() \"]\" }",
                ""
            ),
            "[]\n"
        );
        assert_eq!(
            awk(
                "function f(n) { while (1) { if (n-- < 0) return \"out\" } } BEGIN { print f(3) }",
                ""
            ),
            "out\n"
        );
        assert_eq!(
            awk(
                "function f() { next } { f(); print \"no\" } END { print NR }",
                "a\nb\n"
            ),
            "2\n",
            "next from a function goes to the next record"
        );
        assert_eq!(
            stopped("function f(n) { return f(n + 1) } BEGIN { f(1) }", ""),
            "functions called more than 500 deep"
        );
        assert_eq!(
            stopped("function f(a) { } BEGIN { f(1, 2) }", ""),
            "f takes 1 arguments at most"
        );
        assert_eq!(
            stopped("function f(a) { a = 1; a[1] = 2 } BEGIN { f() }", ""),
            "parameter 1 is a value, used as an array"
        );
    }

    #[test]
    fn control_flow_goes_as_c_s_does() {
        assert_eq!(
            awk(
                "BEGIN { for (i = 0; i < 10; i++) { if (i == 2) continue; if (i == 5) break; print i } }",
                ""
            ),
            "0\n1\n3\n4\n"
        );
        assert_eq!(
            awk("BEGIN { i = 0; while (i < 3) print i++ }", ""),
            "0\n1\n2\n"
        );
        assert_eq!(awk("BEGIN { do print \"once\"; while (0) }", ""), "once\n");
        assert_eq!(
            awk(
                "BEGIN { do { i++; if (i == 2) continue; if (i > 3) break } while (1); print i }",
                ""
            ),
            "4\n"
        );
        assert_eq!(
            awk(
                "BEGIN { while (1) { while (1) break; n++; if (n == 3) break } print n }",
                ""
            ),
            "3\n"
        );
        assert_eq!(
            awk("BEGIN { for (;;) { k++; if (k > 2) break } print k }", ""),
            "3\n"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1]; a[2]; for (k in a) { if (k == 1) continue; print k } for (k in a) break; print k }",
                ""
            ),
            "2\n1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { if (0) print 1; else if (\"\") print 2; else print 3 }",
                ""
            ),
            "3\n"
        );
        assert_eq!(
            awk(
                "BEGIN { x = 1 ? \"y\" : \"n\"; print x, (0 || \"\"), (1 && \"a\"), !\"\" }",
                ""
            ),
            "y 0 1 1\n"
        );
        assert_eq!(
            awk(
                "function f(n) { for (;;) return n } function g() { do return 2; while (1) } function h(a) { for (k in a) return k } BEGIN { b[5]; print f(1), g(), h(b) }",
                ""
            ),
            "1 2 5\n"
        );
    }

    #[test]
    fn next_nextfile_and_exit() {
        assert_eq!(awk("/b/ { next } { print }", "a\nb\nc\n"), "a\nc\n");
        assert_eq!(
            awk("{ print; exit } END { print \"end\" }", "a\nb\n"),
            "a\nend\n"
        );
        let ran = run_on(
            &mut Host::new("exit"),
            "{ exit 3 } END { print \"end\" }",
            &[],
            "a\n",
        );
        assert_eq!((ran.status, ran.out.as_str()), (Ok(3), "end\n"));
        let ran = run_on(
            &mut Host::new("exit"),
            "BEGIN { exit 4 } { print } END { print NR; exit }",
            &[],
            "a\n",
        );
        assert_eq!(
            (ran.status, ran.out.as_str()),
            (Ok(4), "0\n"),
            "exit in BEGIN reads no input but runs END, whose exit keeps the status"
        );
        let ran = run_on(
            &mut Host::new("exit"),
            "END { exit 5; print \"no\" }",
            &[],
            "",
        );
        assert_eq!((ran.status, ran.out.as_str()), (Ok(5), ""));
        assert_eq!(awk("BEGIN { exit } END { print \"end\" }", ""), "end\n");
        let ran = run_on(&mut Host::new("exit"), "BEGIN { exit 257 }", &[], "");
        assert_eq!(ran.status, Ok(1), "a status is its low byte");
        let mut host = Host::new("nextfile");
        host.put("one", "a\nb\n");
        host.put("two", "c\nd\n");
        assert_eq!(
            run_on(
                &mut host,
                "FNR == 2 { nextfile } { print FILENAME, $0 }",
                &["one", "two"],
                ""
            )
            .out,
            "one a\ntwo c\n"
        );
        assert_eq!(stopped("BEGIN { next }", ""), "next goes only in a rule");
        assert_eq!(stopped("END { nextfile }", ""), "next goes only in a rule");
    }

    #[test]
    fn operands_are_files_and_assignments() {
        let mut host = Host::new("operands");
        host.put("one", "a\nb\n");
        host.put("two", "c\n");
        let ran = run_on(
            &mut host,
            "{ print FILENAME, NR, FNR, x }",
            &["one", "x=5", "two"],
            "",
        );
        assert_eq!(ran.out, "one 1 1 \none 2 2 \ntwo 3 1 5\n");
        let ran = run_on(&mut host, "{ print }", &["one", "-"], "in\n");
        assert_eq!(ran.out, "a\nb\nin\n", "- is the standard input");
        let ran = run_on(&mut host, "{ print }", &["x=1"], "in\n");
        assert_eq!(
            ran.out, "in\n",
            "with assignments alone the standard input is read"
        );
        let ran = run_on(
            &mut host,
            "BEGIN { ARGV[1] = \"\" } { print }",
            &["one", "two"],
            "",
        );
        assert_eq!(ran.out, "c\n", "an operand made empty is passed over");
        let ran = run_on(
            &mut host,
            "BEGIN { ARGV[ARGC++] = \"two\" } { print }",
            &[],
            "",
        );
        assert_eq!(ran.out, "c\n");
        let ran = run_on(
            &mut host,
            "BEGIN { print ARGC, ARGV[0], ARGV[1] }",
            &["one"],
            "",
        );
        assert_eq!(ran.out, "2 awk one\n");
        let ran = run_on(&mut host, "{ print v }", &["v=a\\tb", "one"], "");
        assert_eq!(
            ran.out, "a\tb\na\tb\n",
            "an assignment's value takes the escapes of a string"
        );
        let ran = run_on(&mut host, "{ print }", &["nope", "two"], "");
        assert_eq!(
            ran,
            Ran {
                status: Ok(2),
                out: "c\n".into(),
                err: "awk: nope: no such file\n".into()
            }
        );
        let ran = run_on(&mut host, "{ print }", &["1x=2"], "in\n");
        assert_eq!(
            (ran.status, ran.err.as_str()),
            (Ok(2), "awk: 1x=2: no such file\n"),
            "no name, so a file"
        );
    }

    #[test]
    fn assign_sets_a_variable_before_begin() {
        let program = parse::parse("BEGIN { print x, FS, y + 1 }").unwrap();
        let mut awk = Awk::new(&program, &[]);
        assert!(awk.assign(b"x", b"a\\nb"));
        assert!(awk.assign(b"FS", b":"));
        assert!(awk.assign(b"y", b"41"));
        assert!(
            awk.assign(b"unused", b"1"),
            "a variable the program never names"
        );
        assert!(!awk.assign(b"1x", b"1"));
        assert!(!awk.assign(b"", b"1"));
        assert!(!awk.assign(b"ARGV", b"1"), "an array");
        let mut host = Host::new("assign");
        assert_eq!(awk.run(&mut host), Ok(0));
        assert_eq!(String::from_utf8_lossy(&host.out), "a\nb : 42\n");
        let program = parse::parse("function f() {} BEGIN {}").unwrap();
        assert!(
            !Awk::new(&program, &[]).assign(b"f", b"1"),
            "a function is no variable"
        );
    }

    #[test]
    fn getline_reads_the_input_files_and_commands() {
        let mut host = Host::new("getline");
        host.put("f", "one\ntwo\n");
        let ran = run_on(
            &mut host,
            "BEGIN { while ((getline line < \"f\") > 0) print \"got\", line; print NR, (getline x < \"nope\") }",
            &[],
            "",
        );
        assert_eq!(ran.out, "got one\ngot two\n0 -1\n", "from a file NR stays");
        let ran = run_on(
            &mut host,
            "BEGIN { getline < \"f\"; print $0, NF; close(\"f\"); getline < \"f\"; print }",
            &[],
            "",
        );
        assert_eq!(
            ran.out, "one 1\none\n",
            "closed, a file is read again from its start"
        );
        let ran = run_on(
            &mut host,
            "BEGIN { \"echo a b; echo c\" | getline; print $2, NR; \"echo a b; echo c\" | getline x; print x, NR }",
            &[],
            "",
        );
        assert_eq!(ran.out, "b 1\nc 2\n", "from a command NR counts");
        let ran = run_on(
            &mut host,
            "NR == 1 { getline; print \"now\", $0, NR } NR == 3 { getline x; print \"then\", x, NR, $0 }",
            &[],
            "a\nb\nc\nd\n",
        );
        assert_eq!(ran.out, "now b 2\nthen d 4 c\n");
        let ran = run_on(
            &mut host,
            "{ while ((getline line) > 0) n++ } END { print n, NR }",
            &[],
            "a\nb\nc\n",
        );
        assert_eq!(ran.out, "2 3\n");
        let ran = run_on(
            &mut host,
            "BEGIN { getline; print \"first\", $0 } { print }",
            &[],
            "a\nb\n",
        );
        assert_eq!(ran.out, "first a\nb\n", "BEGIN may read the input");
        let ran = run_on(
            &mut host,
            "BEGIN { print (getline line < \"-\"), line }",
            &[],
            "in\n",
        );
        assert_eq!(ran.out, "1 in\n");
        let ran = run_on(
            &mut host,
            "BEGIN { getline line < \"f\"; print (line == \"one\") }",
            &[],
            "",
        );
        assert_eq!(ran.out, "1\n");
        let ran = run_on(&mut host, "END { print (getline), $0 }", &[], "last\n");
        assert_eq!(
            ran.out, "0 last\n",
            "at the end there is none and the last record stays"
        );
    }

    #[test]
    fn output_goes_to_files_and_commands_named() {
        let mut host = Host::new("output");
        let ran = run_on(
            &mut host,
            "{ print > \"out\" } END { close(\"out\"); while ((getline l < \"out\") > 0) print \"read\", l }",
            &[],
            "a\nb\n",
        );
        assert_eq!(ran.out, "read a\nread b\n", "one file, opened once");
        assert_eq!(host.holds("out"), "a\nb\n");
        run_on(
            &mut host,
            "BEGIN { print \"c\" >> \"out\"; printf \"%d\\n\", 4 >> \"out\" }",
            &[],
            "",
        );
        assert_eq!(host.holds("out"), "a\nb\nc\n4\n");
        run_on(&mut host, "BEGIN { print \"new\" > \"out\" }", &[], "");
        assert_eq!(host.holds("out"), "new\n", "> makes a file anew");
        let ran = run_on(
            &mut host,
            "{ print | \"sort\" } END { print \"before\"; close(\"sort\"); print \"after\" }",
            &[],
            "b\na\n",
        );
        assert_eq!(ran.out, "before\na\nb\nafter\n");
        let ran = run_on(
            &mut host,
            "BEGIN { print \"x\" | \"cat; exit 3\"; print close(\"cat; exit 3\"), close(\"nothing\") }",
            &[],
            "",
        );
        assert_eq!(ran.out, "x\n3 -1\n", "close gives a command's status");
        let ran = run_on(
            &mut host,
            "BEGIN { print \"to error\" > \"/dev/stderr\"; print \"to out\" > \"/dev/stdout\"; print \"dash\" > \"-\" }",
            &[],
            "",
        );
        assert_eq!(
            (ran.out.as_str(), ran.err.as_str()),
            ("to out\ndash\n", "to error\n")
        );
        let ran = run_on(
            &mut host,
            "BEGIN { printf \"%s-%d\", \"a\", 3; printf(\"[%5.1f]\\n\", 2.25) }",
            &[],
            "",
        );
        assert_eq!(ran.out, "a-3[  2.2]\n");
        let ran = run_on(
            &mut host,
            "BEGIN { OFS = \"-\"; ORS = \"|\"; print \"a\", \"b\"; print }",
            &[],
            "",
        );
        assert_eq!(ran.out, "a-b||");
        assert_eq!(
            stopped("BEGIN { print > \"\" }", ""),
            "output to a name that is empty"
        );
        assert_eq!(
            stopped("BEGIN { print > \"no/such/dir\" }", ""),
            "no/such/dir: cannot be written"
        );
    }

    #[test]
    fn system_runs_a_command_and_gives_its_status() {
        let mut host = Host::new("system");
        let ran = run_on(
            &mut host,
            "BEGIN { s = system(\"echo hi; exit 7\"); print s }",
            &[],
            "",
        );
        assert_eq!(ran.out, "hi\n7\n");
        let ran = run_on(
            &mut host,
            "BEGIN { print fflush(), fflush(\"x\") }",
            &[],
            "",
        );
        assert_eq!(ran.out, "0 0\n");
    }

    #[test]
    fn sub_and_gsub_replace_matches() {
        assert_eq!(
            awk("{ n = gsub(/o/, \"0\"); print n, $0 }", "foo boo\n"),
            "4 f00 b00\n"
        );
        assert_eq!(awk("{ sub(/o/, \"[&]\"); print }", "foo\n"), "f[o]o\n");
        assert_eq!(
            awk("{ gsub(/o/, \"\\\\&\"); print }", "foo\n"),
            "f&&\n",
            "\\& is a &"
        );
        assert_eq!(
            awk("{ gsub(/o/, \"\\\\\\\\&\"); print }", "fo\n"),
            "f\\o\n",
            "\\\\& is a backslash and the match"
        );
        assert_eq!(awk("{ gsub(/x*/, \"-\"); print }", "abc\n"), "-a-b-c-\n");
        assert_eq!(awk("{ gsub(/b*/, \"-\"); print }", "abc\n"), "-a-c-\n");
        assert_eq!(awk("{ gsub(/^/, \">\"); print }", "aaa\n"), ">aaa\n");
        assert_eq!(awk("{ gsub(/$/, \"<\"); print }", "aaa\n"), "aaa<\n");
        assert_eq!(
            awk("{ gsub(/a/, \"b\", $2); print; print NF }", "a a a\n"),
            "a b a\n3\n"
        );
        assert_eq!(
            awk(
                "BEGIN { s = \"hello\"; print sub(/l+/, \"L\", s), s, sub(/z/, \"\", s), s }",
                ""
            ),
            "1 heLo 0 heLo\n"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1] = \"xx\"; gsub(\"x\", \"y\", a[1]); print a[1] }",
                ""
            ),
            "yy\n"
        );
        assert_eq!(awk("{ gsub(/ä/, \"a\"); print }", "äbä\n"), "aba\n");
        assert_eq!(
            awk("{ gsub(//, \"-\"); print }", "äb\n"),
            "-ä-b-\n",
            "an empty match goes a character on, not a byte"
        );
        assert_eq!(
            awk("{ n = gsub(/z/, \"y\"); print n; print }", "  a  \n"),
            "0\n  a  \n",
            "no match leaves the record as it was"
        );
        assert_eq!(
            stopped("BEGIN { s = \"a\"; sub(\"(\", \"\", s) }", ""),
            "/(/: a parenthesis without its other half"
        );
    }

    #[test]
    fn string_functions_count_characters() {
        assert_eq!(
            awk("BEGIN { print length(\"äbc\"), length(), length }", ""),
            "3 0 0\n"
        );
        assert_eq!(awk("{ print length() }", "abcd\n"), "4\n");
        assert_eq!(awk("BEGIN { a[1]; a[2]; print length(a) }", ""), "2\n");
        for (call, got) in [
            ("substr(\"hello\", 2, 3)", "ell"),
            ("substr(\"hello\", 2)", "ello"),
            ("substr(\"hello\", 0, 2)", "h"),
            ("substr(\"hello\", -1, 3)", "h"),
            ("substr(\"hello\", 1.5)", "ello"),
            ("substr(\"hello\", 1.4)", "hello"),
            ("substr(\"hello\", 4, 100)", "lo"),
            ("substr(\"hello\", 6)", ""),
            ("substr(\"hello\", 2, -1)", ""),
            ("substr(\"äöü\", 2, 1)", "ö"),
            ("substr(\"hello\", log(-1))", ""),
            ("index(\"hello\", \"ll\")", "3"),
            ("index(\"äöü\", \"ü\")", "3"),
            ("index(\"hello\", \"z\")", "0"),
            ("index(\"hello\", \"\")", "0"),
            ("toupper(\"aäb\") tolower(\"XÖ\")", "AÄBxö"),
            ("sprintf(\"%03d|%s\", 7, \"x\")", "007|x"),
            ("int(3.9) int(-3.9) int(\"4x\")", "3-34"),
            (
                "match(\"foobar\", /o+/) \" \" RSTART \" \" RLENGTH",
                "2 2 2",
            ),
            (
                "match(\"foobar\", \"z\") \" \" RSTART \" \" RLENGTH",
                "0 0 -1",
            ),
            ("match(\"äöö\", /ö+/) \" \" RLENGTH", "2 2"),
        ] {
            assert_eq!(
                awk(&format!("BEGIN {{ print {call} }}"), ""),
                format!("{got}\n"),
                "{call}"
            );
        }
    }

    #[test]
    fn split_parts_a_string_into_an_array() {
        assert_eq!(
            awk(
                "BEGIN { n = split(\"a b  c\", p); print n, p[1], p[3] }",
                ""
            ),
            "3 a c\n"
        );
        assert_eq!(
            awk(
                "BEGIN { n = split(\"a:b::c\", p, \":\"); print n, p[3] == \"\", p[4] }",
                ""
            ),
            "4 1 c\n"
        );
        assert_eq!(
            awk(
                "BEGIN { n = split(\"a1b22c\", p, /[0-9]+/); print n, p[1] p[2] p[3] }",
                ""
            ),
            "3 abc\n"
        );
        assert_eq!(
            awk(
                "BEGIN { n = split(\"a1b\", p, \"[0-9]\"); print n, p[2] }",
                ""
            ),
            "2 b\n"
        );
        assert_eq!(
            awk("BEGIN { n = split(\"abc\", p, \"\"); print n, p[2] }", ""),
            "3 b\n"
        );
        assert_eq!(
            awk("BEGIN { n = split(\"\", p); print n, length(p) }", ""),
            "0 0\n"
        );
        assert_eq!(
            awk("BEGIN { p[9] = 1; split(\"x\", p); print length(p) }", ""),
            "1\n",
            "the array is emptied first"
        );
        assert_eq!(
            awk("BEGIN { split(\"10 9\", p); print (p[1] > p[2]) }", ""),
            "1\n",
            "elements that look like numbers are"
        );
        assert_eq!(
            awk("BEGIN { FS = \",\" } { split($0, p); print p[2] }", "a,b\n"),
            "b\n"
        );
        assert_eq!(
            awk("BEGIN { n = split(\"axxb\", p, /x*/); print n, p[2] }", ""),
            "2 b\n"
        );
        assert_eq!(
            awk("BEGIN { n = split(\"ab\", p, /x*/); print n, p[1] }", ""),
            "1 ab\n"
        );
        assert_eq!(
            awk(
                "BEGIN { n = split(\"1a\", p, /[0-9]/); print n, p[1] == \"\", p[2] }",
                ""
            ),
            "2 1 a\n"
        );
        assert_eq!(
            stopped("BEGIN { split(\"a\", p, \"a(\") }", ""),
            "a(: a parenthesis without its other half"
        );
    }

    #[test]
    fn numeric_functions_and_rand() {
        assert_eq!(
            awk(
                "BEGIN { printf \"%.4f %.4f %.4f %.4f %.4f %.4f\\n\", sin(1), cos(1), atan2(1, 1), exp(1), log(10), sqrt(2) }",
                ""
            ),
            "0.8415 0.5403 0.7854 2.7183 2.3026 1.4142\n"
        );
        let first = awk("BEGIN { srand(1); print rand(), rand() }", "");
        assert_eq!(
            awk("BEGIN { srand(1); print rand(), rand() }", ""),
            first,
            "a seed gives the same numbers"
        );
        assert_ne!(awk("BEGIN { srand(2); print rand(), rand() }", ""), first);
        assert_eq!(
            awk("BEGIN { print srand(5), srand(), srand() }", ""),
            "0 5 42\n",
            "srand gives the seed before, the clock's ticks when given none"
        );
        assert_eq!(
            awk(
                "BEGIN { for (i = 0; i < 1000; i++) { r = rand(); if (r < 0 || r >= 1) bad++ } print bad + 0 }",
                ""
            ),
            "0\n"
        );
        assert_eq!(
            awk("BEGIN { x = rand(); y = rand(); print (x != y) }", ""),
            "1\n"
        );
    }

    #[test]
    fn assignments_steps_and_their_values() {
        assert_eq!(
            awk(
                "BEGIN { x = y = 3; x += 2; y -= 1; print x, y, x *= 2, x /= 4, x %= 2, x ^= 3 }",
                ""
            ),
            "5 2 10 2.5 0.5 0.125\n"
        );
        assert_eq!(
            awk("BEGIN { i = 5; print i++, i, ++i, i--, --i }", ""),
            "5 6 7 7 5\n"
        );
        assert_eq!(
            awk("BEGIN { a[\"k\"]++; a[\"k\"] += 2; print a[\"k\"] }", ""),
            "3\n"
        );
        assert_eq!(
            awk("BEGIN { print -\"3\", +\"4x\", !0, !1, !\"0\" }", ""),
            "-3 4 1 0 0\n",
            "the string \"0\" is true"
        );
        assert_eq!(
            awk("{ print !$1 }", "0\n"),
            "1\n",
            "but a field 0 is a number"
        );
        assert_eq!(awk("BEGIN { x = \"a\" \"b\" 1 + 2; print x }", ""), "ab3\n");
        assert_eq!(awk("BEGIN { NR = 10 } { print NR }", "a\n"), "11\n");
    }

    #[test]
    fn regular_expressions_made_of_strings_are_kept_and_read() {
        let mut program = String::from("BEGIN { ");
        for n in 0..80 {
            core::fmt::Write::write_fmt(
                &mut program,
                format_args!("if (\"x{n}\" ~ \"x{n}$\") c++; "),
            )
            .unwrap();
        }
        program.push_str("print c; print (\"x1\" ~ \"x1$\") }");
        assert_eq!(awk(&program, ""), "80\n1\n", "more than are kept");
        assert_eq!(
            stopped("BEGIN { x = \"a\" ~ \"[\" }", ""),
            "/[/: a [ without the ] that ends it"
        );
    }

    #[test]
    fn the_deepest_programs_run_on_a_stack_as_small_as_meridian_s() {
        // A program on Meridian asks for 256 pages of stack; here a thread
        // of that much reads and runs the deepest of each kind awk takes.
        let deepest = [
            format!(
                "function f(n) {{ if (n < {DEEPEST}) return f(n + 1) + (1 + (2 * (3 - (4 / (5 ^ 1))))); return 0 }} BEGIN {{ print (f(1) > 0) }}"
            ),
            format!(
                "BEGIN {{ x = {}1{}; print x }}",
                "(".repeat(90),
                ")".repeat(90)
            ),
            format!(
                "BEGIN {{ x = 1{}; print (x > 1000) }}",
                "+1".repeat(parse::CHAIN)
            ),
            format!("BEGIN {{ x = 1; {} print x }}", "if (x) ".repeat(90)),
            format!("BEGIN {{ print {}1 }}", "- ".repeat(90)),
        ];
        let ran = std::thread::Builder::new()
            .stack_size(256 * 4096)
            .spawn(move || {
                let mut host = Host::new("deep");
                deepest.map(|text| run_on(&mut host, &text, &[], "").out)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(
            ran,
            [
                "1
", "1
", "1
", "1
", "1
"
            ]
        );
    }

    #[test]
    fn unset_values_are_empty_and_zero_alike() {
        assert_eq!(
            awk("BEGIN { print x + 0, \"[\" x \"]\", length(x), !x }", ""),
            "0 [] 0 1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1]; print a[1] + 0, \"[\" a[1] \"]\", length(a[2]), length(a) }",
                ""
            ),
            "0 [] 0 2\n"
        );
        assert_eq!(
            awk(
                "{ print ($5 == 0), ($5 == \"\"), \"[\" $(NF + 2) \"]\" }",
                "a b\n"
            ),
            "1 1 []\n"
        );
        assert_eq!(
            awk(
                "BEGIN { FS = \":\" } { print ($2 == 0), ($2 == \"\") }",
                "a::b\n"
            ),
            "0 1\n",
            "an empty field is a string, which 0 is not"
        );
        assert_eq!(
            awk("BEGIN { print length() }", ""),
            "0\n",
            "there is no record before the input"
        );
        assert_eq!(awk("BEGIN { if (x == y) print \"same\" }", ""), "same\n");
        assert_eq!(awk("BEGIN { x++; y--; print x, y }", ""), "1 -1\n");
    }

    #[test]
    fn awk_s_own_variables_start_as_posix_says() {
        assert_eq!(
            awk(
                "BEGIN { print NR, NF, FNR, \"[\" FS \"]\", \"[\" OFS \"]\", RSTART, RLENGTH, CONVFMT, OFMT, length(ENVIRON), \"[\" FILENAME \"]\" }",
                ""
            ),
            "0 0 0 [ ] [ ] 0 -1 %.6g %.6g 0 []\n"
        );
        assert_eq!(
            awk(
                "BEGIN { printf \"%d %d\", length(SUBSEP), SUBSEP == \"\\034\"; print \"\" }",
                ""
            ),
            "1 1\n"
        );
        assert_eq!(awk("BEGIN { printf \"%s\", ORS RS }", ""), "\n\n");
        assert_eq!(
            awk("{ FNR = 10; NR = 20 } END { print FNR, NR }", "a\nb\n"),
            "10 20\n"
        );
        assert_eq!(
            awk("END { print NF, $0, $2 }", "a b\nc d e\n"),
            "3 c d e d\n",
            "END keeps the last record"
        );
        assert_eq!(awk("END { $0 = \"x y\"; print NF, $2 }", "a\n"), "2 y\n");
    }

    #[test]
    fn getline_fills_fields_elements_and_records() {
        let mut host = Host::new("into");
        host.put("f", "one two\nthree\n");
        let ran = run_on(
            &mut host,
            "{ getline $2 < \"f\"; print; print NF }",
            &[],
            "a b c\n",
        );
        assert_eq!(
            ran.out, "a one two c\n3\n",
            "the field takes the line and the record is made anew"
        );
        let ran = run_on(
            &mut host,
            "BEGIN { while ((getline a[++n] < \"f\") > 0); print n, a[1], a[2], \"[\" a[3] \"]\" }",
            &[],
            "",
        );
        assert_eq!(ran.out, "3 one two three []\n");
        let ran = run_on(
            &mut host,
            "{ getline line < \"f\"; print $0, NR, line }",
            &[],
            "rec\n",
        );
        assert_eq!(
            ran.out, "rec 1 one two\n",
            "a variable read leaves the record and NR"
        );
        let ran = run_on(
            &mut host,
            "BEGIN { while ((\"echo a; echo b\" | getline line) > 0) n++; print n, NR, line }",
            &[],
            "",
        );
        assert_eq!(ran.out, "2 2 b\n");
        let ran = run_on(
            &mut host,
            "BEGIN { getline x; print x, NR } { print \"main\", $0 }",
            &[],
            "first\nsecond\n",
        );
        assert_eq!(ran.out, "first 1\nmain second\n");
        let ran = run_on(
            &mut host,
            "BEGIN { r = getline; print r, \"[\" $0 \"]\" }",
            &[],
            "",
        );
        assert_eq!(ran.out, "0 []\n", "an empty input has no record");
        let ran = run_on(
            &mut host,
            "BEGIN { getline x < \"f\"; x = x \"!\"; print x }",
            &[],
            "",
        );
        assert_eq!(ran.out, "one two!\n");
        let ran = run_on(
            &mut host,
            "BEGIN { getline l < \"f\"; print (l < \"p\"), (l > 1) }",
            &[],
            "",
        );
        assert_eq!(
            ran.out, "1 1\n",
            "a line read is a string unless it looks like a number"
        );
        std::fs::create_dir(host.root.join("dir")).unwrap();
        let ran = run_on(&mut host, "BEGIN { print (getline x < \"dir\") }", &[], "");
        assert_eq!(ran.out, "-1\n", "a directory cannot be read");
    }

    #[test]
    fn closing_a_command_runs_it_again_next_time() {
        let mut host = Host::new("again");
        let ran = run_on(
            &mut host,
            "BEGIN { c = \"echo x\"; c | getline a; r = (c | getline b); close(c); c | getline d; print a, r, \"[\" b \"]\", d }",
            &[],
            "",
        );
        assert_eq!(ran.out, "x 0 [] x\n");
        let ran = run_on(
            &mut host,
            "BEGIN { print \"a\" > \"o\"; printf \"b\\n\" > \"o\"; close(\"o\"); while ((getline l < \"o\") > 0) print \"got\", l }",
            &[],
            "",
        );
        assert_eq!(
            ran.out, "got a\ngot b\n",
            "print and printf to one name write one file"
        );
        let ran = run_on(
            &mut host,
            "BEGIN { print \"c\" >> \"o\"; print \"d\" > \"o\"; close(\"o\"); while ((getline l < \"o\") > 0) print l }",
            &[],
            "",
        );
        assert_eq!(
            ran.out, "a\nb\nc\nd\n",
            "a name opened to add to stays open so"
        );
        let ran = run_on(
            &mut host,
            "BEGIN { print \"z\" | \"sort\"; print \"y\" | \"sort\" } END { print \"end\" }",
            &[],
            "",
        );
        assert_eq!(
            ran.out, "end\ny\nz\n",
            "a command's output comes when it ends, at awk's end"
        );
        let ran = run_on(&mut host, "BEGIN { print close(\"o\") }", &[], "");
        assert_eq!(ran.out, "-1\n", "nothing is open by a name of another run");
    }

    #[test]
    fn numbers_are_written_whole_where_they_are_whole() {
        assert_eq!(
            awk(
                "BEGIN { print 2^53, 2^53 + 1, 1e20, -0.0, 1e-300 * 1e-300 }",
                ""
            ),
            "9007199254740992 9007199254740992 1e+20 0 0\n"
        );
        assert_eq!(
            awk("BEGIN { print log(0), -log(0), log(-1) }", ""),
            "-inf inf nan\n"
        );
        assert_eq!(
            awk(
                "BEGIN { x = 0.1 * 3; print x, x \"\"; CONVFMT = \"%.17g\"; print x \"\" }",
                ""
            ),
            "0.3 0.3\n0.30000000000000004\n"
        );
        assert_eq!(
            awk("BEGIN { OFMT = \"%.2e\"; print 1234.5, 1234 }", ""),
            "1.23e+03 1234\n"
        );
        assert_eq!(awk("BEGIN { print 1e300 * 1e300 }", ""), "inf\n");
        assert_eq!(
            awk("BEGIN { print 100 / 3 * 3, 0.1 + 0.7 }", ""),
            "100 0.8\n"
        );
        assert_eq!(
            awk("BEGIN { x = 17; print x / 4, int(x / 4), x % 4 }", ""),
            "4.25 4 1\n"
        );
    }

    #[test]
    fn strings_and_numbers_compare_as_their_kinds_say() {
        assert_eq!(
            awk("{ print ($1 == $2) }", "1 1.0\na a\n1 a\n"),
            "1\n1\n0\n"
        );
        assert_eq!(
            awk("{ print ($1 < $2) }", "10 9\n"),
            "0\n",
            "fields that look like numbers compare as numbers"
        );
        assert_eq!(
            awk("{ print ($1 \"\" < $2 \"\") }", "10 9\n"),
            "1\n",
            "made strings, they compare as strings"
        );
        assert_eq!(awk("{ print ($1 + 0 < $2 + 0) }", "abc 1\n"), "1\n");
        assert_eq!(
            awk(
                "BEGIN { print (\"10\" < \"9\"), (10 < 9), (\"a\" < 1) }",
                ""
            ),
            "1 0 0\n"
        );
        assert_eq!(
            awk("BEGIN { x = \"3.0\"; y = 3; print (x == y) }", ""),
            "0\n",
            "a string constant stays a string"
        );
        assert_eq!(
            awk("BEGIN { split(\"3.0\", a); print (a[1] == 3) }", ""),
            "1\n"
        );
        assert_eq!(
            awk("{ print ($1 == 1e2) }", "100\n+1e2\n 100 \n"),
            "1\n1\n1\n"
        );
        assert_eq!(
            awk(
                "BEGIN { CONVFMT = \"%.2g\"; x = 0.123; print (x == \"0.12\") }",
                ""
            ),
            "1\n",
            "a number meets a string as CONVFMT writes it"
        );
    }

    #[test]
    fn functions_work_on_globals_and_arrays_down_their_calls() {
        assert_eq!(
            awk("function f() { g = 5 } BEGIN { f(); print g }", ""),
            "5\n"
        );
        assert_eq!(
            awk("function f(g) { g = 5 } BEGIN { g = 1; f(); print g }", ""),
            "1\n",
            "a parameter hides the global"
        );
        assert_eq!(
            awk(
                "function fill(a, n) { if (n == 0) return; a[n] = n * n; fill(a, n - 1) } BEGIN { fill(sq, 4); for (k in sq) s += sq[k]; print s, length(sq) }",
                ""
            ),
            "30 4\n"
        );
        assert_eq!(
            awk(
                "function f(v) { v = v \"x\"; return v } BEGIN { a[1] = \"y\"; print f(a[1]), a[1] }",
                ""
            ),
            "yx y\n",
            "an element goes as its value"
        );
        assert_eq!(
            awk("BEGIN { print f(2) } function f(n) { return n * 3 }", ""),
            "6\n",
            "a function may come after its call"
        );
        assert_eq!(
            awk(
                "function f(a, b) { return b == \"\" } BEGIN { print f(1) }",
                ""
            ),
            "1\n",
            "a parameter given nothing is unset"
        );
        assert_eq!(
            awk(
                "function f(x) { return x } BEGIN { y = f(); print length(y), y + 1 }",
                ""
            ),
            "0 1\n"
        );
        assert_eq!(
            awk(
                "function f(x) { x[1] = 1 } BEGIN { f(z); f(z); print length(z) }",
                ""
            ),
            "1\n"
        );
        assert_eq!(
            awk(
                "function f(x) { return length(x) } BEGIN { print f(unseen), length(unseen) }",
                ""
            ),
            "0 0\n",
            "an unset variable given stays one"
        );
        let ran = run_on(
            &mut Host::new("exit"),
            "function f(n) { if (n) exit n; return 1 } BEGIN { f(0); f(6); print \"no\" } END { print \"end\" }",
            &[],
            "",
        );
        assert_eq!(
            (ran.status, ran.out.as_str()),
            (Ok(6), "end\n"),
            "exit in a function ends the run"
        );
        assert_eq!(
            awk(
                "function f(a) { delete a; a[\"n\"] = 1 } BEGIN { b[1]; b[2]; f(b); for (k in b) print k }",
                ""
            ),
            "n\n"
        );
    }

    #[test]
    fn next_in_a_loop_and_nextfile_in_the_last_file() {
        assert_eq!(
            awk(
                "{ for (i = 1; i <= NF; i++) if ($i == \"x\") next; print }",
                "a b\nx y\nc\n"
            ),
            "a b\nc\n"
        );
        let mut host = Host::new("lastfile");
        host.put("only", "a\nb\nc\n");
        let ran = run_on(
            &mut host,
            "{ print FNR, $0; nextfile } END { print NR }",
            &["only"],
            "",
        );
        assert_eq!(ran.out, "1 a\n1\n");
        let ran = run_on(
            &mut host,
            "FNR == 1 { print FILENAME }",
            &["only", "only"],
            "",
        );
        assert_eq!(ran.out, "only\nonly\n", "a file named twice is read twice");
        let ran = run_on(
            &mut host,
            "{ n++ } END { print n }",
            &["only", "x=1", "only"],
            "",
        );
        assert_eq!(ran.out, "6\n");
    }

    #[test]
    fn exit_takes_its_status_from_a_value() {
        let status = |text: &str| run_on(&mut Host::new("status"), text, &[], "").status;
        assert_eq!(status("BEGIN { exit \"3abc\" }"), Ok(3));
        assert_eq!(status("BEGIN { exit 2.9 }"), Ok(2));
        assert_eq!(status("BEGIN { exit -1 }"), Ok(255));
        assert_eq!(status("BEGIN { exit }"), Ok(0));
        assert_eq!(status("BEGIN { exit 1 } END { exit }"), Ok(1));
        assert_eq!(status("BEGIN { exit 1 } END { exit 0 }"), Ok(0));
        let ran = run_on(
            &mut Host::new("status"),
            "{ print } END { print \"end\" }",
            &["nope"],
            "",
        );
        assert_eq!(
            (ran.status, ran.out.as_str()),
            (Ok(2), "end\n"),
            "a file that is not there makes the status 2"
        );
    }

    #[test]
    fn concatenation_and_signs_run_as_they_parse() {
        assert_eq!(awk("BEGIN { print 1 \" \" 2 + 3 }", ""), "1 5\n");
        assert_eq!(
            awk("BEGIN { print -1 \" \" -2 }", ""),
            "-1-2\n",
            "the second minus takes 2 off the blank"
        );
        assert_eq!(awk("BEGIN { x = 1; print x x++ x }", ""), "112\n");
        assert_eq!(awk("BEGIN { print 1 2, 3 4 }", ""), "12 34\n");
        assert_eq!(awk("BEGIN { print \"a\" (1 < 2) \"b\" }", ""), "a1b\n");
        assert_eq!(
            awk("BEGIN { $0 = \"3 4\"; print $1 $2, $1 * $2, -$1 }", ""),
            "34 12 -3\n"
        );
    }

    #[test]
    fn membership_and_subscripts_are_strings() {
        assert_eq!(
            awk(
                "BEGIN { a[\"1\"] = 1; print (1 in a), (\"1\" in a), (1.0 in a), (01 in a) }",
                ""
            ),
            "1 1 1 1\n"
        );
        assert_eq!(awk("BEGIN { a[0.5] = 1; print (\"0.5\" in a) }", ""), "1\n");
        assert_eq!(
            awk(
                "BEGIN { a[1, 2] = 1; SUBSEP = \":\"; print ((1, 2) in a); a[1, 2] = 2; print length(a) }",
                ""
            ),
            "0\n2\n",
            "a subscript is joined by the SUBSEP of then"
        );
        assert_eq!(
            awk("BEGIN { a[x] = 1; print (\"\" in a) }", ""),
            "1\n",
            "unset is the empty subscript"
        );
        assert_eq!(
            awk(
                "BEGIN { a[1]; a[2]; for (k in a) delete a[k]; print length(a) }",
                ""
            ),
            "0\n"
        );
    }

    #[test]
    fn records_join_over_reads_of_a_few_bytes() {
        // The host gives five bytes a read; these records run over them.
        assert_eq!(
            awk("{ print NR \": \" $0 }", "abcdefghijk\nl\nmnopqrstuvwxyz\n"),
            "1: abcdefghijk\n2: l\n3: mnopqrstuvwxyz\n"
        );
        assert_eq!(
            awk("BEGIN { RS = \"--+\" } { print }", "ab---cdefg--h-i----"),
            "ab\ncdefg\nh-i\n"
        );
        assert_eq!(
            awk(
                "BEGIN { RS = \"\" } { print NR, NF }",
                "a b c d e\nf\n\n\n\n\ng h i j k l\n"
            ),
            "1 6\n2 6\n"
        );
        assert_eq!(
            awk("BEGIN { RS = \"x\" } END { print NR, $0 }", "axbxc"),
            "3 c\n"
        );
        assert_eq!(
            awk("BEGIN { RS = \"x\" } END { print NR, $0 }", "axbx"),
            "2 b\n",
            "the last record ends at its separator"
        );
    }

    #[test]
    fn fields_split_on_tabs_regexes_and_single_blanks() {
        assert_eq!(
            awk("BEGIN { FS = \"[ ]\" } { print NF }", " a  b \n"),
            "5\n",
            "a bracketed blank is every blank"
        );
        assert_eq!(
            awk(
                "BEGIN { FS = \" +\" } { print NF, \"[\" $1 \"]\" }",
                "  a b\n"
            ),
            "3 []\n"
        );
        assert_eq!(
            awk("BEGIN { FS = \"\\t\" } { print NF, $2 }", "a\t\tb\n"),
            "3 \n"
        );
        assert_eq!(
            awk("BEGIN { FS = \"ab\" } { print NF, $2 }", "xabyabz\n"),
            "3 y\n"
        );
        assert_eq!(awk("{ print NF }", " \t \n"), "0\n");
        assert_eq!(
            awk("BEGIN { FS = \",\" } { $3 = \"c\"; print }", "a\n"),
            "a  c\n",
            "OFS, not FS, joins the fields made"
        );
    }

    #[test]
    fn printf_writes_where_print_does() {
        let mut host = Host::new("printf");
        let ran = run_on(
            &mut host,
            "BEGIN { printf \"%s=%d\\n\", \"x\", 3 > \"/dev/stderr\"; printf(\"%c%c\\n\", 65, \"bc\") }",
            &[],
            "",
        );
        assert_eq!((ran.out.as_str(), ran.err.as_str()), ("Ab\n", "x=3\n"));
        let ran = run_on(
            &mut host,
            "BEGIN { printf \"%s\\n\", 1 | \"cat\" }",
            &[],
            "",
        );
        assert_eq!(ran.out, "1\n");
        let ran = run_on(
            &mut host,
            "BEGIN { CONVFMT = \"%.1f\"; printf \"%s %s\\n\", 2.25, 3 }",
            &[],
            "",
        );
        assert_eq!(
            ran.out, "2.2 3\n",
            "%s writes a number as CONVFMT says, unless it is whole"
        );
        let ran = run_on(&mut host, "{ printf $0 }", &[], "%d|\n");
        assert_eq!(ran.out, "0|", "a format with nothing for it");
    }

    #[test]
    fn the_case_functions_and_int_take_any_value() {
        assert_eq!(
            awk(
                "BEGIN { print toupper(12), tolower(\"ÄB\"), toupper(\"ß\") }",
                ""
            ),
            "12 äb SS\n"
        );
        assert_eq!(
            awk("BEGIN { print toupper(\"a\\377b\") == \"A\\377B\" }", ""),
            "1\n",
            "a byte that is no UTF-8 stays"
        );
        assert_eq!(
            awk(
                "BEGIN { print int(1e30) == 1e30, int(-0.5), int(\"x\"), int(2^53 + 0.5) }",
                ""
            ),
            "1 0 0 9007199254740992\n"
        );
        assert_eq!(awk("BEGIN { print int(log(-1)) }", ""), "nan\n");
    }

    #[test]
    fn srand_without_a_seed_takes_the_ticks() {
        let a = awk("BEGIN { srand(); print rand() }", "");
        let b = awk("BEGIN { srand(42); print rand() }", "");
        assert_eq!(a, b, "the host's ticks are 42");
        let fresh = awk(
            "BEGIN { srand(7); x = rand(); srand(7); print (rand() == x) }",
            "",
        );
        assert_eq!(fresh, "1\n");
        assert_ne!(
            awk("BEGIN { print rand() }", ""),
            b,
            "the seed before any srand is 0"
        );
        assert_eq!(
            awk("BEGIN { print rand() }", ""),
            awk("BEGIN { srand(0); print rand() }", "")
        );
    }

    #[test]
    fn what_cannot_go_on_says_why() {
        assert_eq!(
            awk("{ print \"[\" $(1e30) \"]\" }", "a\n"),
            "[]\n",
            "a field far past the last is empty"
        );
        assert_eq!(
            stopped("{ $70000 = 1 }", "a\n"),
            "no field past 65535 can be made"
        );
        assert_eq!(
            stopped("{ NF = 70000 }", "a\n"),
            "no field past 65535 can be made"
        );
        assert_eq!(awk("{ $65535 = 1; print NF }", "a\n"), "65535\n");
        assert_eq!(
            stopped("BEGIN { a[1]; a = 1 }", ""),
            "a is an array, given a value"
        );
        assert_eq!(
            stopped("BEGIN { a[1]; print length(a) a }", ""),
            "a is an array, used as a value"
        );
        assert_eq!(
            stopped("function f(x) { x[1] } BEGIN { y = 1; f(y) }", ""),
            "parameter 1 is a value, used as an array"
        );
        assert_eq!(
            stopped("BEGIN { x = 1; for (k in x) print k }", ""),
            "x is a value, used as an array"
        );
        assert_eq!(
            stopped("BEGIN { x = 1; delete x }", ""),
            "x is a value, used as an array"
        );
        assert_eq!(
            stopped("BEGIN { x = 1; if (1 in x) print }", ""),
            "x is a value, used as an array"
        );
        assert_eq!(
            stopped("BEGIN { NF = log(-1) }", ""),
            "NF cannot be below 0"
        );
        assert_eq!(
            stopped("BEGIN { $(log(-1)) = 1 }", ""),
            "there is no field nan"
        );
        assert_eq!(
            stopped("BEGIN { x = 2; x %= 0 }", ""),
            "division by zero in %"
        );
    }

    #[test]
    fn a_place_s_subscript_is_worked_out_once() {
        assert_eq!(
            awk("BEGIN { i = 1; a[i++] += 5; print i, a[1], length(a) }", ""),
            "2 5 1\n"
        );
        assert_eq!(awk("BEGIN { i = 1; a[i++]++; print i, a[1] }", ""), "2 1\n");
        assert_eq!(
            awk(
                "BEGIN { i = 1; a[1] = \"xx\"; gsub(/x/, \"y\", a[i++]); print i, a[1] }",
                ""
            ),
            "2 yy\n"
        );
        assert_eq!(
            awk("{ i = 1; $(i++) += 2; print i, $0 }", "3 4\n"),
            "2 5 4\n"
        );
        assert_eq!(
            awk("BEGIN { a[1] = 1; a[1] = a[1] + 1; print a[1] }", ""),
            "2\n"
        );
        assert_eq!(
            awk(
                "BEGIN { n = 0; while ((getline x[++n] < \"nothing\") > 0); print n }",
                ""
            ),
            "1\n"
        );
    }

    #[test]
    fn common_programs_do_what_they_are_known_for() {
        // Lines once each, in their first order.
        assert_eq!(awk("!seen[$0]++", "b\na\nb\nc\na\n"), "b\na\nc\n");
        // Each line backwards.
        assert_eq!(
            awk(
                "{ s = \"\"; for (i = length($0); i > 0; i--) s = s substr($0, i, 1); print s }",
                "abc\näbc\n"
            ),
            "cba\ncbä\n"
        );
        // Columns summed and averaged.
        assert_eq!(
            awk(
                "{ sum += $2; n++ } END { printf \"%d %.2f\\n\", sum, sum / n }",
                "a 1\nb 2\nc 4\n"
            ),
            "7 2.33\n"
        );
        // The longest line.
        assert_eq!(
            awk(
                "length($0) > max { max = length($0); line = $0 } END { print max, line }",
                "ab\nabcd\nabc\n"
            ),
            "4 abcd\n"
        );
        // Lines in reverse.
        assert_eq!(
            awk(
                "{ l[NR] = $0 } END { for (i = NR; i > 0; i--) print l[i] }",
                "1\n2\n3\n"
            ),
            "3\n2\n1\n"
        );
        // Fields the other way round.
        assert_eq!(
            awk(
                "{ for (i = NF; i > 0; i--) printf \"%s%s\", $i, (i > 1 ? OFS : ORS) }",
                "a b c\nd\n"
            ),
            "c b a\nd\n"
        );
        // A count of words, most first.
        assert_eq!(
            awk(
                "{ for (i = 1; i <= NF; i++) n[tolower($i)]++ } END { for (w in n) if (n[w] > 1) print n[w], w }",
                "The cat and the hat\nand THE end\n"
            ),
            "2 and\n3 the\n"
        );
        // Lines between two marks, without the marks.
        assert_eq!(
            awk(
                "/^end$/ { on = 0 } on { print } /^start$/ { on = 1 }",
                "x\nstart\na\nb\nend\ny\n"
            ),
            "a\nb\n"
        );
        // Every third line.
        assert_eq!(awk("NR % 3 == 0", "1\n2\n3\n4\n5\n6\n"), "3\n6\n");
    }

    #[test]
    fn two_files_are_joined_by_their_first_field() {
        let mut host = Host::new("join");
        host.put("names", "1 ann\n2 bob\n3 cid\n");
        host.put("ages", "2 40\n1 30\n4 50\n");
        let ran = run_on(
            &mut host,
            "NR == FNR { name[$1] = $2; next } $1 in name { print name[$1], $2 }",
            &["names", "ages"],
            "",
        );
        assert_eq!(ran.out, "bob 40\nann 30\n");
        let ran = run_on(
            &mut host,
            "FNR == 1 && NR != 1 { print \"--\" } { print FILENAME \":\" FNR }",
            &["names", "ages"],
            "",
        );
        assert_eq!(
            ran.out,
            "names:1\nnames:2\nnames:3\n--\nages:1\nages:2\nages:3\n"
        );
    }

    #[test]
    fn reports_line_up_with_printf() {
        assert_eq!(
            awk(
                "BEGIN { printf \"%-6s|%5s\\n\", \"name\", \"count\" } { printf \"%-6s|%5d\\n\", $1, $2 } END { printf \"%-6s|%5.1f\\n\", \"mean\", 7.5 }",
                "ab 3\nlonger 12\n"
            ),
            "name  |count\nab    |    3\nlonger|   12\nmean  |  7.5\n"
        );
        assert_eq!(
            awk("{ printf \"%05.1f%%\\n\", $1 * 100 }", "0.125\n"),
            "012.5%\n"
        );
        assert_eq!(
            awk("BEGIN { printf \"%x %o %e\\n\", 255, 8, 0.000123 }", ""),
            "ff 10 1.230000e-04\n"
        );
    }

    #[test]
    fn strings_are_built_and_cut() {
        assert_eq!(
            awk(
                "{ n = split($0, w, \",\"); s = w[n]; for (i = n - 1; i > 0; i--) s = s \",\" w[i]; print s }",
                "a,b,c\n"
            ),
            "c,b,a\n"
        );
        assert_eq!(
            awk(
                "{ gsub(/^[ \\t]+|[ \\t]+$/, \"\"); print \"[\" $0 \"]\" }",
                "  a b \t\n"
            ),
            "[a b]\n"
        );
        assert_eq!(
            awk(
                "{ while (match($0, /[0-9]+/)) { print substr($0, RSTART, RLENGTH); $0 = substr($0, RSTART + RLENGTH) } }",
                "a12b345c6\n"
            ),
            "12\n345\n6\n"
        );
        assert_eq!(
            awk(
                "{ print toupper(substr($0, 1, 1)) substr($0, 2) }",
                "word\n"
            ),
            "Word\n"
        );
        assert_eq!(
            awk(
                "BEGIN { s = sprintf(\"%s-%s\", \"a\", \"b\"); print s, length(s) }",
                ""
            ),
            "a-b 3\n"
        );
        assert_eq!(
            awk(
                "{ print index($0, \"=\") ? substr($0, index($0, \"=\") + 1) : \"none\" }",
                "k=v\nnone here\n"
            ),
            "v\nnone\n"
        );
        assert_eq!(
            awk("{ $1 = \"\"; sub(/^ /, \"\"); print }", "drop keep this\n"),
            "keep this\n"
        );
    }

    #[test]
    fn numbers_are_counted_and_compared() {
        assert_eq!(
            awk(
                "$1 > max || NR == 1 { max = $1 } END { print max }",
                "3\n10\n9\n"
            ),
            "10\n",
            "fields compare as numbers"
        );
        assert_eq!(
            awk("{ s += $0 } END { print s }", "0x10\n1e1\n.5\n-3\n"),
            "7.5\n",
            "0x10 is 0"
        );
        assert_eq!(
            awk("{ print $1 + $2, $1 * $2, $1 / $2, $1 - $2 }", "6 4\n"),
            "10 24 1.5 2\n"
        );
        assert_eq!(
            awk("{ print ($1 > $2) ? \"more\" : \"less\" }", "2 10\nb a\n"),
            "less\nmore\n"
        );
        assert_eq!(
            awk("BEGIN { for (i = 0; i <= 1; i += 0.25) n++; print n }", ""),
            "5\n"
        );
        assert_eq!(
            awk("BEGIN { x = 7; x = x \"\" + 1; print x }", ""),
            "71\n",
            "concatenation binds below +"
        );
    }

    #[test]
    fn ofs_and_ors_join_what_print_writes() {
        assert_eq!(
            awk(
                "BEGIN { OFS = \",\"; ORS = \";\" } { $1 = $1; print; print $1, $2 }",
                "a b\n"
            ),
            "a,b;a,b;"
        );
        assert_eq!(
            awk("{ OFS = \"-\"; print $1, $2; $1 = $1; print }", "a b\n"),
            "a-b\na-b\n"
        );
        assert_eq!(
            awk("BEGIN { OFS = \"\" } { $2 = $2; print }", "a b c\n"),
            "abc\n"
        );
        assert_eq!(
            awk(
                "BEGIN { ORS = \"\" } { print } END { print \"\\n\" }",
                "a\nb\n"
            ),
            "ab\n"
        );
    }

    #[test]
    fn rs_may_change_between_records() {
        assert_eq!(awk("{ print; RS = \",\" }", "a\nb,c,d\n"), "a\nb\nc\nd\n\n");
        assert_eq!(
            awk("BEGIN { RS = \",\" } { print \"[\" $0 \"]\" }", "a,,b,\n"),
            "[a]\n[]\n[b]\n[\n]\n"
        );
        assert_eq!(
            awk("BEGIN { RS = \"\\n\" } END { print NR }", "a\n\n\nb\n"),
            "4\n"
        );
    }

    #[test]
    fn argv_and_argc_choose_what_is_read() {
        let mut host = Host::new("argv");
        host.put("a", "1\n");
        host.put("b", "2\n");
        let ran = run_on(
            &mut host,
            "BEGIN { delete ARGV[1] } { print }",
            &["a", "b"],
            "",
        );
        assert_eq!(ran.out, "2\n", "an operand taken out is passed over");
        let ran = run_on(&mut host, "BEGIN { ARGC = 2 } { print }", &["a", "b"], "");
        assert_eq!(ran.out, "1\n", "operands past ARGC are not read");
        let ran = run_on(&mut host, "BEGIN { ARGV[1] = \"b\" } { print }", &["a"], "");
        assert_eq!(ran.out, "2\n");
        let ran = run_on(
            &mut host,
            "BEGIN { ARGV[2] = \"a\"; ARGC = 3 } { print }",
            &["b"],
            "",
        );
        assert_eq!(ran.out, "2\n1\n");
        let ran = run_on(&mut host, "{ print FILENAME, $0 }", &["-"], "in\n");
        assert_eq!(ran.out, "- in\n");
        let ran = run_on(&mut host, "END { print FILENAME }", &["a", "b"], "");
        assert_eq!(ran.out, "b\n", "FILENAME stays the last file's");
        let ran = run_on(&mut host, "BEGIN { ARGV = 1 }", &[], "");
        assert_eq!(
            ran.status,
            Err(String::from("ARGV is an array, given a value"))
        );
        let ran = run_on(&mut host, "{ print x }", &["x=1", "a", "x=2", "b"], "");
        assert_eq!(
            ran.out, "1\n2\n",
            "each assignment is made when its operand comes"
        );
        let ran = run_on(&mut host, "END { print x }", &["a", "x=3"], "");
        assert_eq!(
            ran.out, "3\n",
            "an assignment after the last file is made before END"
        );
    }

    #[test]
    fn what_awk_keeps_follows_the_variables_that_set_it() {
        assert_eq!(
            awk("BEGIN { CONVFMT = 5; x = 0.5 \"\"; print x }", ""),
            "0.5\n",
            "a CONVFMT that is no string writes as %.6g"
        );
        assert_eq!(
            awk(
                "{ FS = (NR == 1 ? \",\" : \";\"); print $1 }",
                "a,b\nc,d;e\nf;g\n"
            ),
            "a,b\nc\nf\n"
        );
        assert_eq!(
            awk("{ RS = (NR == 1 ? \";\" : \",\"); print }", "a\nb;c,d"),
            "a\nb\nc\nd\n"
        );
        assert_eq!(
            awk(
                "BEGIN { FS = \",\" } { print $2; FS = \",\"; print $2 }",
                "a,b\n"
            ),
            "b\nb\n"
        );
        assert_eq!(
            awk(
                "BEGIN { SUBSEP = \"\" } { a[$1, $2]; for (k in a) print k }",
                "x y\n"
            ),
            "xy\n"
        );
    }

    #[test]
    fn builtins_take_what_they_are_given_at_the_edges() {
        assert_eq!(
            awk(
                "BEGIN { print \"[\" substr(\"abc\", 1, log(-1)) \"]\", \"[\" substr(\"abc\", 2, 1e30) \"]\" }",
                ""
            ),
            "[] [bc]\n"
        );
        assert_eq!(
            awk(
                "BEGIN { print substr(\"abc\", -1e30, 1e30), substr(12345, 2, 2) }",
                ""
            ),
            " 23\n",
            "a start far before the first and a length that ends there give nothing"
        );
        assert_eq!(
            awk(
                "BEGIN { print length(12.5), length(1e6), index(12345, 34) }",
                ""
            ),
            "4 7 3\n"
        );
        assert_eq!(
            awk(
                "BEGIN { print sqrt(-1), exp(1000), log(0), atan2(0, -0) }",
                ""
            ),
            "nan inf -inf 3.14159\n"
        );
        assert_eq!(
            awk("BEGIN { print sin(0), cos(0), int(-0) }", ""),
            "0 1 0\n"
        );
        assert_eq!(awk("BEGIN { print fflush(\"o\"), fflush() }", ""), "0 0\n");
        assert_eq!(
            awk("BEGIN { print match(\"\", /x*/), RSTART, RLENGTH }", ""),
            "1 1 0\n",
            "an empty match is at 1"
        );
        assert_eq!(
            awk("BEGIN { print split(\"a b\", arr, \" \"), arr[2] }", ""),
            "2 b\n",
            "a blank parts at runs of blanks"
        );
        assert_eq!(
            awk("BEGIN { print sprintf(\"%d%%\", 50), sprintf(\"x\") }", ""),
            "50% x\n"
        );
    }

    #[test]
    fn the_world_s_failures_are_said() {
        assert_eq!(
            stopped("BEGIN { print \"x\" > \"dir/nope\" }", ""),
            "dir/nope: cannot be written"
        );
        let mut host = Host::new("fail");
        std::fs::create_dir(host.root.join("d")).unwrap();
        let ran = run_on(&mut host, "{ print }", &["d", "-"], "in\n");
        assert_eq!(ran.status, Ok(2));
        assert!(ran.err.starts_with("awk: d: "), "{}", ran.err);
        assert_eq!(ran.out, "in\n", "the next operand is read all the same");
    }

    /// Programs, their input and what POSIX says they write.
    fn table(cases: &[(&str, &str, &str)]) {
        for &(program, input, written) in cases {
            assert_eq!(awk(program, input), written, "{program}");
        }
    }

    #[test]
    fn expressions_give_what_posix_says() {
        table(&[
            (
                "BEGIN { print 1 + 2 * 3, (1 + 2) * 3, 2 ^ 3 ^ 2, -2 ^ 2 }",
                "",
                "7 9 512 -4\n",
            ),
            (
                "BEGIN { print 7 % -3, -7 % -3, 0 % 5, 5.5 % 1 }",
                "",
                "1 -1 0 0.5\n",
            ),
            (
                "BEGIN { print 1 / 4, 1e3 / 1e3, 2 ^ -1, 4 ^ 0.5 }",
                "",
                "0.25 1 0.5 2\n",
            ),
            (
                "BEGIN { print !0 + !0, !!5, !\"\", !\"a\" }",
                "",
                "2 1 1 0\n",
            ),
            ("BEGIN { print 1 < 2 ? \"y\" : \"n\" }", "", "y\n"),
            (
                "BEGIN { x = 3; x ^= 2; x -= 1; x /= 2; print x }",
                "",
                "4\n",
            ),
            ("BEGIN { print (1, 2) in a, length(a) }", "", "0 0\n"),
            (
                "BEGIN { print \"a\" < \"b\", \"B\" < \"a\", \"\" < \"a\" }",
                "",
                "1 1 1\n",
            ),
            (
                "BEGIN { print 1 == \"1\", 1 == \"1.0\", \"a\" == \"a \" }",
                "",
                "1 0 0\n",
            ),
            (
                "BEGIN { a = \"x\"; b = a a a; print b, length(b) }",
                "",
                "xxx 3\n",
            ),
            (
                "BEGIN { print 1e3, 1E3, .5e1, 5e-1 }",
                "",
                "1000 1000 5 0.5\n",
            ),
            (
                "BEGIN { print 100000 * 100000, 123456789 * 1000 }",
                "",
                "10000000000 123456789000\n",
            ),
            (
                "BEGIN { print 0.1 + 0.2 == 0.3, 0.5 + 0.25 == 0.75 }",
                "",
                "0 1\n",
            ),
            ("BEGIN { i = 5; print i++ + ++i, i }", "", "12 7\n"),
            (
                "BEGIN { print length(\"\"), length(\" \"), length(\"\\t\\n\") }",
                "",
                "0 1 2\n",
            ),
        ]);
    }

    #[test]
    fn fields_and_records_give_what_posix_says() {
        table(&[
            ("{ print NF }", "\n", "0\n"),
            ("{ print $1 }", "   lead\n", "lead\n"),
            ("{ $0 = $2; print $1 }", "a b c\n", "b\n"),
            ("{ $2 = \"\"; print NF \":\" $0 }", "a b c\n", "3:a  c\n"),
            ("{ NF--; print }", "a b c\n", "a b\n"),
            ("{ NF++; $NF = \"z\"; print }", "a b\n", "a b z\n"),
            ("{ print $NF; NF = 1; print $NF }", "a b c\n", "c\na\n"),
            ("{ $(NF + 1) = NR; print }", "x\ny\n", "x 1\ny 2\n"),
            ("{ print $1 + 0 }", "3abc\n", "3\n"),
            ("{ print ($1 == 3) }", "3abc\n", "0\n"),
            ("BEGIN { FS = \"x\" } { print NF, $2 }", "axbxc\n", "3 b\n"),
            ("BEGIN { FS = \"\\\\|\" } { print $2 }", "a|b\n", "b\n"),
            ("{ print NR \"/\" NF }", "a\n\nb c\n", "1/1\n2/0\n3/2\n"),
            ("END { print NR }", "", "0\n"),
            ("END { print $0 }", "", "\n"),
        ]);
    }

    #[test]
    fn control_and_arrays_give_what_posix_says() {
        table(&[
            (
                "BEGIN { while (i < 3) { i++; if (i == 2) continue; s = s i } print s }",
                "",
                "13\n",
            ),
            (
                "BEGIN { for (i = 3; i; i--) s = s i; print s }",
                "",
                "321\n",
            ),
            ("BEGIN { do { i++ } while (i < 0); print i }", "", "1\n"),
            (
                "BEGIN { a[\"x\"] = 1; delete a[\"x\"]; print (\"x\" in a), length(a) }",
                "",
                "0 0\n",
            ),
            (
                "BEGIN { n = split(\"c b a\", w); for (i = n; i >= 1; i--) printf \"%s\", w[i]; print \"\" }",
                "",
                "abc\n",
            ),
            (
                "BEGIN { a[1]; a[10]; a[2]; for (k in a) s = s k \",\"; print s }",
                "",
                "1,10,2,\n",
            ),
            (
                "BEGIN { a[\"b\"] = 2; a[\"a\"] = 1; for (k in a) print k, a[k] }",
                "",
                "a 1\nb 2\n",
            ),
            (
                "BEGIN { if (!(3 in a)) print \"no\"; print length(a) }",
                "",
                "no\n0\n",
            ),
            (
                "{ c[$1] += $2 } END { for (k in c) print k, c[k] }",
                "a 1\nb 2\na 3\n",
                "a 4\nb 2\n",
            ),
            (
                "BEGIN { x[1] = 1; y = x[1] x[2]; print y, length(x) }",
                "",
                "1 2\n",
            ),
        ]);
    }

    #[test]
    fn functions_give_what_posix_says() {
        table(&[
            (
                "function max(a, b) { return a > b ? a : b } BEGIN { print max(3, 7), max(\"b\", \"a\") }",
                "",
                "7 b\n",
            ),
            (
                "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2) } BEGIN { print fib(20) }",
                "",
                "6765\n",
            ),
            (
                "function swap(a, i, j,  t) { t = a[i]; a[i] = a[j]; a[j] = t } BEGIN { v[1] = \"x\"; v[2] = \"y\"; swap(v, 1, 2); print v[1] v[2] }",
                "",
                "yx\n",
            ),
            (
                "function count(a,  k, n) { for (k in a) n++; return n + 0 } BEGIN { print count(none); z[1]; print count(z) }",
                "",
                "0\n1\n",
            ),
            (
                "function f(s) { sub(/a/, \"b\", s); return s } BEGIN { t = \"aa\"; print f(t), t }",
                "",
                "ba aa\n",
            ),
            (
                "function g() { return \"x\" } BEGIN { print g() g() }",
                "",
                "xx\n",
            ),
            (
                "function h(n,  i, s) { for (i = 0; i < n; i++) s = s \"*\"; return s } { print h($1) }",
                "3\n1\n",
                "***\n*\n",
            ),
        ]);
    }

    #[test]
    fn output_functions_give_what_posix_says() {
        table(&[
            (
                "BEGIN { printf \"%s %s %s\\n\", \"a\", 1, 1.5 }",
                "",
                "a 1 1.5\n",
            ),
            (
                "BEGIN { printf \"[%3d][%-3d][%03d]\\n\", 5, 5, 5 }",
                "",
                "[  5][5  ][005]\n",
            ),
            (
                "BEGIN { printf \"[%.3s][%5.1s]\\n\", \"abcdef\", \"xyz\" }",
                "",
                "[abc][    x]\n",
            ),
            ("BEGIN { printf \"%c%c%c\\n\", 72, \"i\", 33 }", "", "Hi!\n"),
            (
                "BEGIN { printf \"%d %d\\n\", \"12abc\", -7.9 }",
                "",
                "12 -7\n",
            ),
            (
                "BEGIN { printf \"%5.2f|%-8.3e|%G\\n\", 3.14159, 1234.5, 0.00001 }",
                "",
                " 3.14|1.234e+03|1E-05\n",
            ),
            (
                "BEGIN { x = sprintf(\"%*d|\", 4, 7); print x }",
                "",
                "   7|\n",
            ),
            ("BEGIN { print \"a\" \"b\", \"c\" }", "", "ab c\n"),
            ("BEGIN { print }", "", "\n"),
            ("BEGIN { print \"\" }", "", "\n"),
        ]);
    }

    #[test]
    fn regular_expressions_give_what_posix_says() {
        table(&[
            ("/^a.c$/", "abc\naXc\nabbc\n", "abc\naXc\n"),
            ("/a|b/", "a\nc\nb\n", "a\nb\n"),
            ("/^(ab)+$/", "abab\naba\n", "abab\n"),
            ("/[[:digit:]]{2}/", "a1\nb22\n", "b22\n"),
            ("$2 ~ /^[0-9]+$/", "a 12\nb x\n", "a 12\n"),
            ("$0 !~ /x/", "x\ny\n", "y\n"),
            ("{ print ($0 ~ \"^\" $1) }", "ab\n", "1\n"),
            ("BEGIN { r = \"^[a-z]+$\" } $0 ~ r", "abc\nab1\n", "abc\n"),
            ("/\\./", "a.b\nab\n", "a.b\n"),
            ("/\\//", "a/b\nab\n", "a/b\n"),
            ("/ä+/", "äää\na\n", "äää\n"),
            ("BEGIN { print match(\"aaa\", /a*/), RLENGTH }", "", "1 3\n"),
            ("BEGIN { print (\"\" ~ //), (\"x\" ~ //) }", "", "1 1\n"),
        ]);
    }

    #[test]
    fn substitutions_give_what_posix_says() {
        table(&[
            ("{ sub(/b+/, \"[&]\"); print }", "abbbc\n", "a[bbb]c\n"),
            ("{ gsub(/[aeiou]/, \"\"); print }", "education\n", "dctn\n"),
            ("{ gsub(/./, \"&&\"); print }", "ab\n", "aabb\n"),
            ("{ gsub(\"a\", \"b\"); print }", "aXa\n", "bXb\n"),
            ("{ n = gsub(/a/, \"a\"); print n }", "banana\n", "3\n"),
            ("{ sub(/$/, \"!\"); print }", "hi\n", "hi!\n"),
            ("{ gsub(/ +/, \" \"); print }", "a   b    c\n", "a b c\n"),
            ("{ gsub(/a/, \"\\\\\\\\\"); print }", "aXa\n", "\\X\\\n"),
            (
                "{ x = $0; gsub(/o/, \"0\", x); print x, $0 }",
                "foo\n",
                "f00 foo\n",
            ),
            ("{ gsub(/^x/, \"y\"); print }", "xxx\n", "yxx\n"),
            ("{ gsub(/x$/, \"y\"); print }", "xxx\n", "xxy\n"),
        ]);
    }

    #[test]
    fn split_gives_what_posix_says() {
        table(&[
            (
                "BEGIN { n = split(\"a:b:c\", a, \":\"); print n, a[1] a[2] a[3] }",
                "",
                "3 abc\n",
            ),
            (
                "BEGIN { n = split(\":a:\", a, \":\"); print n, \"[\" a[1] \"]\" a[2] \"[\" a[3] \"]\" }",
                "",
                "3 []a[]\n",
            ),
            (
                "BEGIN { n = split(\"  a  b  \", a); print n, a[1] a[2] }",
                "",
                "2 ab\n",
            ),
            (
                "BEGIN { n = split(\"a1b2c\", a, /[0-9]/); print n, a[3] }",
                "",
                "3 c\n",
            ),
            (
                "BEGIN { n = split(\"abc\", a, \"\"); print n, a[1] a[3] }",
                "",
                "3 ac\n",
            ),
            (
                "BEGIN { n = split(123.5, a, \".\"); print n, a[1], a[2] }",
                "",
                "2 123 5\n",
            ),
            (
                "BEGIN { FS = \"-\"; n = split(\"a-b\", a); print n }",
                "",
                "2\n",
            ),
        ]);
    }

    #[test]
    fn getline_and_output_give_what_posix_says() {
        let mut host = Host::new("io");
        host.put("lines", "1\n2\n3\n");
        for (program, written) in [
            (
                "BEGIN { while ((getline n < \"lines\") > 0) s += n; print s }",
                "6\n",
            ),
            (
                "BEGIN { getline < \"lines\"; getline < \"lines\"; print $0 }",
                "2\n",
            ),
            (
                "BEGIN { while ((\"cat lines\" | getline) > 0) print \"=\" $0 }",
                "=1\n=2\n=3\n",
            ),
            ("BEGIN { \"echo 5 6\" | getline; print $2 + NF }", "8\n"),
            (
                "BEGIN { print \"c\\nb\\na\" | \"sort\"; close(\"sort\"); print \"done\" }",
                "a\nb\nc\ndone\n",
            ),
            (
                "BEGIN { print 1 > \"o\"; close(\"o\"); getline x < \"o\"; print x + 1 }",
                "2\n",
            ),
            (
                "BEGIN { system(\"echo from sh\"); print \"after\" }",
                "from sh\nafter\n",
            ),
        ] {
            assert_eq!(
                run_on(&mut host, program, &[], "").out,
                written,
                "{program}"
            );
        }
    }

    #[test]
    fn getline_counts_records_as_posix_says() {
        let mut host = Host::new("counts");
        host.put("f", "x\ny\n");
        for (program, input, written) in [
            (
                "NR == 1 { getline; print NR, FNR, $0 }",
                "a\nb\n",
                "2 2 b\n",
            ),
            (
                "NR == 1 { getline v; print NR, FNR, $0, v }",
                "a\nb\n",
                "2 2 a b\n",
            ),
            (
                "NR == 1 { getline < \"f\"; print NR, FNR, $0 }",
                "a\nb\n",
                "1 1 x\n",
            ),
            (
                "NR == 1 { getline v < \"f\"; print NR, FNR, $0, v }",
                "a\nb\n",
                "1 1 a x\n",
            ),
            (
                "NR == 1 { \"echo c\" | getline; print NR, FNR, $0 }",
                "a\nb\n",
                "2 1 c\n",
            ),
            (
                "NR == 1 { \"echo c\" | getline v; print NR, FNR, $0, v }",
                "a\nb\n",
                "2 1 a c\n",
            ),
        ] {
            assert_eq!(
                run_on(&mut host, program, &[], input).out,
                written,
                "{program}"
            );
        }
    }

    #[test]
    fn printf_and_print_go_to_one_file_by_its_name() {
        let mut host = Host::new("onefile");
        run_on(
            &mut host,
            "{ printf \"%s;\", $0 > \"o\"; print NR > \"o\" }",
            &[],
            "a\nb\n",
        );
        assert_eq!(host.holds("o"), "a;1\nb;2\n");
        run_on(
            &mut host,
            "BEGIN { print \"x\" > \"o\"; close(\"o\"); print \"y\" > \"o\" }",
            &[],
            "",
        );
        assert_eq!(
            host.holds("o"),
            "y\n",
            "closed, a file named with > is made anew"
        );
        run_on(
            &mut host,
            "BEGIN { print \"z\" >> \"o\"; close(\"o\"); print \"w\" >> \"o\" }",
            &[],
            "",
        );
        assert_eq!(host.holds("o"), "y\nz\nw\n", ">> adds each time");
    }
}
