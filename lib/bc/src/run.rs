//! A bc program run as it is read: each item as soon as it is whole,
//! over variables, arrays and functions, the functions' parameters and
//! `auto` variables standing in for others of their names while they
//! run, as POSIX has it. What it writes and says goes through a
//! [`World`].

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::base;
use crate::lex::{self, Op, Rel, Spot, Wrong};
use crate::number::{Fault, Number};
use crate::parse::{self, Argument, Expr, Function, Item, Place, Printed, Read, Statement};

/// What a bc program writes and says.
pub trait World {
    /// Writes `bytes` on the standard output.
    fn write(&mut self, bytes: &[u8]);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// The greatest `obase`, and index of an array.
pub const MOST: u32 = 16_777_215;

/// The greatest `scale`.
pub const MOST_SCALE: u32 = 2_147_483_647;

/// How deep calls may go, so that running them fits the stack a program
/// has.
pub const DEEPEST: usize = 500;

/// The characters a line takes before it is broken with a backslash.
const LINE: usize = 68;

type Array = BTreeMap<usize, Number>;

/// Why a statement stopped.
enum Flow {
    Normal,
    Break,
    Continue,
    Return(Number),
    Halt,
}

/// What went wrong running a statement, which ends the item it is in.
type Stopped = Result<Flow, String>;

/// A run of bc.
pub struct Bc {
    variables: BTreeMap<String, Number>,
    arrays: BTreeMap<String, Array>,
    functions: BTreeMap<String, Rc<Function>>,
    scale: usize,
    ibase: u32,
    obase: u32,
    /// The text read and not yet run.
    pending: Vec<u8>,
    /// The line the pending text begins on, and what it is read from.
    line: usize,
    source: String,
    /// The characters on the output's line so far.
    column: usize,
    depth: usize,
    ended: bool,
}

impl Default for Bc {
    fn default() -> Self {
        Self::new()
    }
}

fn says(fault: Fault) -> String {
    String::from(match fault {
        Fault::DivideByZero => "a division by zero",
        Fault::ModuloByZero => "a remainder of a division by zero",
        Fault::NegativeRoot => "the square root of a number below zero",
        Fault::TooGreat => "a power with more digits than bc holds",
    })
}

impl Bc {
    #[must_use]
    pub fn new() -> Bc {
        Bc {
            variables: BTreeMap::new(),
            arrays: BTreeMap::new(),
            functions: BTreeMap::new(),
            scale: 0,
            ibase: 10,
            obase: 10,
            pending: Vec::new(),
            line: 1,
            source: String::new(),
            column: 0,
            depth: 0,
            ended: false,
        }
    }

    /// Sets `scale`, as `-l` does.
    pub fn set_scale(&mut self, scale: usize) {
        self.scale = scale;
    }

    /// Whether `quit` or `halt` ended the run.
    #[must_use]
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Begins reading the text of `source`, a file's name or the
    /// standard input's.
    pub fn begin(&mut self, source: &str) {
        self.source = String::from(source);
        self.line = 1;
        self.pending.clear();
    }

    /// Reads `text`, the next whole lines of the program, so that no
    /// word is cut short, and runs each item they make whole; whether bc
    /// goes on, which it does not after `quit` or `halt`.
    pub fn feed(&mut self, text: &[u8], world: &mut impl World) -> bool {
        self.pending.extend_from_slice(text);
        while !self.ended {
            let (tokens, limit, bad) = self.whole_tokens();
            let taken = match parse::next_item(&tokens) {
                Read::More => {
                    // An item cut short by a bad character goes with its
                    // line; one cut short by the end waits for more.
                    let Some((c, line, end)) = bad else {
                        break;
                    };
                    self.mistake(line, &format!("a character bc does not take: {c}"), world);
                    self.drop_bytes(end);
                    continue;
                }
                Read::Quit => {
                    self.ended = true;
                    break;
                }
                Read::Nothing(taken) => taken,
                Read::Wrong(why, line, taken) => {
                    self.mistake(line, &why, world);
                    taken
                }
                Read::Item(item, taken) => {
                    self.drop_bytes(tokens.get(taken).map_or(limit, |s| s.at));
                    self.run_item(item, world);
                    continue;
                }
            };
            self.drop_bytes(tokens.get(taken).map_or(limit, |s| s.at));
        }
        !self.ended
    }

    /// The end of the text begun. A last line without its newline runs
    /// as if it had one; what is left after it is a statement cut short.
    pub fn end(&mut self, world: &mut impl World) {
        if !self.ended && self.pending.iter().any(|b| !b.is_ascii_whitespace()) {
            self.feed(b"\n", world);
        }
        let left = match lex::tokens(&self.pending, self.line) {
            Ok(tokens) => !tokens.is_empty(),
            Err(_) => true,
        };
        if !self.ended && left {
            let line = self.line;
            self.mistake(line, "the text ends inside a statement", world);
        }
        self.pending.clear();
    }

    /// The tokens of the pending text as far as they go whole, and
    /// where they end: before a string or comment not yet closed, or
    /// before the line with a character no token begins with, which
    /// comes with them, its line and where that line ends.
    fn whole_tokens(&self) -> (Vec<Spot>, usize, Option<(char, usize, usize)>) {
        // What comes before the trouble lexed whole on the way to it.
        let before = |end: usize| lex::tokens(&self.pending[..end], self.line).unwrap_or_default();
        match lex::tokens(&self.pending, self.line) {
            Ok(tokens) => (tokens, self.pending.len(), None),
            Err(Wrong::Unfinished(at)) => (before(at), at, None),
            Err(Wrong::Character(c, line, begins, end)) => {
                (before(begins), begins, Some((c, line, end)))
            }
        }
    }

    fn drop_bytes(&mut self, end: usize) {
        let gone: Vec<u8> = self.pending.drain(..end).collect();
        self.line += gone.iter().map(|&b| usize::from(b == b'\n')).sum::<usize>();
    }

    fn mistake(&mut self, line: usize, why: &str, world: &mut impl World) {
        world.say(&format!("bc: {} {line}: {why}\n", self.source));
    }

    fn run_item(&mut self, item: Item, world: &mut impl World) {
        match item {
            Item::Define(name, function) => {
                self.functions.insert(name, Rc::new(function));
            }
            Item::Run(statements) => {
                for statement in &statements {
                    match self.exec(statement, world) {
                        Ok(Flow::Halt) => {
                            self.ended = true;
                            return;
                        }
                        Ok(_) => {}
                        Err(why) => {
                            // A mistake ends the rest of its line; halt in
                            // a function ends it all, with nothing to say.
                            if !self.ended {
                                world.say(&format!("bc: {why}\n"));
                            }
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Writes `bytes`, breaking lines past [`LINE`] characters with a
    /// backslash.
    #[inline(never)]
    fn put(&mut self, bytes: &[u8], world: &mut impl World) {
        let mut out = Vec::with_capacity(bytes.len() + 2);
        for &b in bytes {
            if b == b'\n' {
                self.column = 0;
            } else {
                if self.column == LINE {
                    out.extend_from_slice(b"\\\n");
                    self.column = 0;
                }
                self.column += 1;
            }
            out.push(b);
        }
        world.write(&out);
    }

    #[inline(never)]
    fn put_number(&mut self, n: &Number, world: &mut impl World) {
        let text = base::write(n, self.obase);
        self.put(text.as_bytes(), world);
    }

    fn exec(&mut self, statement: &Statement, world: &mut impl World) -> Stopped {
        // Only what calls go through stays here; the rest is in functions
        // of their own, so that each level of a call takes little stack.
        match statement {
            Statement::Block(statements) => {
                for s in statements {
                    let flow = self.exec(s, world)?;
                    if !matches!(flow, Flow::Normal) {
                        return Ok(flow);
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::If(condition, then, otherwise) => {
                if self.test(condition, world)? {
                    return self.exec(then, world);
                }
                match otherwise {
                    Some(otherwise) => self.exec(otherwise, world),
                    None => Ok(Flow::Normal),
                }
            }
            Statement::Return(Some(expr)) => Ok(Flow::Return(self.eval(expr, world)?)),
            Statement::Return(None) => Ok(Flow::Return(Number::default())),
            Statement::Break => Ok(Flow::Break),
            Statement::Continue => Ok(Flow::Continue),
            Statement::Halt => Ok(Flow::Halt),
            _ => self.exec_rest(statement, world),
        }
    }

    #[inline(never)]
    fn exec_rest(&mut self, statement: &Statement, world: &mut impl World) -> Stopped {
        match statement {
            Statement::Expr(expr, shown) => {
                let value = self.eval(expr, world)?;
                if *shown {
                    self.put_number(&value, world);
                    self.put(b"\n", world);
                }
            }
            Statement::String(text) => self.put(text, world),
            Statement::Print(printed) => {
                for p in printed {
                    match p {
                        Printed::Value(expr) => {
                            let value = self.eval(expr, world)?;
                            self.put_number(&value, world);
                        }
                        Printed::String(text) => {
                            let text = escaped(text);
                            self.put(&text, world);
                        }
                    }
                }
            }
            Statement::While(condition, body) => {
                while self.test(condition, world)? {
                    match self.exec(body, world)? {
                        Flow::Break => break,
                        Flow::Normal | Flow::Continue => {}
                        flow => return Ok(flow),
                    }
                }
            }
            Statement::For(first, test, step, body) => {
                if let Some(first) = first {
                    self.eval(first, world)?;
                }
                loop {
                    if let Some(test) = test
                        && !self.test(test, world)?
                    {
                        break;
                    }
                    match self.exec(body, world)? {
                        Flow::Break => break,
                        Flow::Normal | Flow::Continue => {}
                        flow => return Ok(flow),
                    }
                    if let Some(step) = step {
                        self.eval(step, world)?;
                    }
                }
            }
            _ => return self.exec(statement, world),
        }
        Ok(Flow::Normal)
    }

    fn eval(&mut self, expr: &Expr, world: &mut impl World) -> Result<Number, String> {
        // Each kind of expression in a function of its own, so that the
        // recursion through here takes little stack at each level.
        match expr {
            Expr::Number(digits) => Ok(self.literal(digits)),
            Expr::Place(place) => self.load(place, world),
            Expr::Assign(place, op, value) => self.assign(place, *op, value, world),
            Expr::Binary(op, a, b) => self.binary(*op, a, b, world),
            Expr::Negate(a) => Ok(self.eval(a, world)?.negated()),
            Expr::Not(a) => Ok(truth(self.eval(a, world)?.is_zero())),
            Expr::And(..) | Expr::Or(..) | Expr::Compare(..) => self.test(expr, world).map(truth),
            Expr::Step { place, up, before } => self.step(place, *up, *before, world),
            Expr::Call(name, arguments) => self.call(name, arguments, world),
            Expr::Length(a) => Ok(count(self.eval(a, world)?.length())),
            Expr::ScaleOf(a) => Ok(count(self.eval(a, world)?.scale())),
            Expr::Sqrt(a) => self.eval(a, world)?.sqrt(self.scale).map_err(says),
        }
    }

    #[inline(never)]
    fn literal(&self, digits: &[u8]) -> Number {
        base::read(digits, self.ibase)
    }

    #[inline(never)]
    fn assign(
        &mut self,
        place: &Place,
        op: Option<Op>,
        value: &Expr,
        world: &mut impl World,
    ) -> Result<Number, String> {
        let mut value = self.eval(value, world)?;
        if let Some(op) = op {
            let old = self.load(place, world)?;
            value = self.apply(op, &old, &value, world)?;
        }
        self.store(place, value, world)
    }

    #[inline(never)]
    fn binary(
        &mut self,
        op: Op,
        a: &Expr,
        b: &Expr,
        world: &mut impl World,
    ) -> Result<Number, String> {
        let a = self.eval(a, world)?;
        let b = self.eval(b, world)?;
        self.apply(op, &a, &b, world)
    }

    /// Whether `&&`, `||` or a comparison holds.
    #[inline(never)]
    fn test(&mut self, expr: &Expr, world: &mut impl World) -> Result<bool, String> {
        Ok(match expr {
            Expr::And(a, b) => !self.eval(a, world)?.is_zero() && !self.eval(b, world)?.is_zero(),
            Expr::Or(a, b) => !self.eval(a, world)?.is_zero() || !self.eval(b, world)?.is_zero(),
            Expr::Compare(rel, a, b) => {
                let order = self.eval(a, world)?.compare(&self.eval(b, world)?);
                match rel {
                    Rel::Less => order == Ordering::Less,
                    Rel::LessEqual => order != Ordering::Greater,
                    Rel::Greater => order == Ordering::Greater,
                    Rel::GreaterEqual => order != Ordering::Less,
                    Rel::Equal => order == Ordering::Equal,
                    Rel::NotEqual => order != Ordering::Equal,
                }
            }
            _ => !self.eval(expr, world)?.is_zero(),
        })
    }

    #[inline(never)]
    fn step(
        &mut self,
        place: &Place,
        up: bool,
        before: bool,
        world: &mut impl World,
    ) -> Result<Number, String> {
        let old = self.load(place, world)?;
        let one = Number::from_i64(1);
        let new = if up { old.add(&one) } else { old.sub(&one) };
        let new = self.store(place, new, world)?;
        Ok(if before { new } else { old })
    }

    #[inline(never)]
    fn apply(
        &mut self,
        op: Op,
        a: &Number,
        b: &Number,
        world: &mut impl World,
    ) -> Result<Number, String> {
        let scale = self.scale;
        Ok(match op {
            Op::Add => a.add(b),
            Op::Sub => a.sub(b),
            Op::Mul => a.mul(b, scale),
            Op::Div => a.div(b, scale).map_err(says)?,
            Op::Rem => a.rem(b, scale).map_err(says)?,
            Op::Pow => {
                if b.scale() > 0 {
                    world.say("bc: only the whole part of an exponent counts\n");
                }
                a.pow(b, scale).map_err(says)?
            }
        })
    }

    /// The index of the element `index` names.
    #[inline(never)]
    fn index(&mut self, index: &Expr, world: &mut impl World) -> Result<usize, String> {
        let value = self.eval(index, world)?;
        value
            .to_i64()
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i <= MOST as usize)
            .ok_or_else(|| format!("an array's elements go from 0 to {MOST}"))
    }

    #[inline(never)]
    fn load(&mut self, place: &Place, world: &mut impl World) -> Result<Number, String> {
        Ok(match place {
            Place::Variable(name) => self.variables.get(name).cloned().unwrap_or_default(),
            Place::Element(name, index) => {
                let index = self.index(index, world)?;
                self.arrays
                    .get(name)
                    .and_then(|array| array.get(&index))
                    .cloned()
                    .unwrap_or_default()
            }
            Place::Scale => count(self.scale),
            Place::Ibase => count(self.ibase as usize),
            Place::Obase => count(self.obase as usize),
        })
    }

    /// Keeps `value` at `place`, and gives what was kept: for `scale`,
    /// `ibase` and `obase`, the whole part, brought within their bounds
    /// with a warning.
    #[inline(never)]
    fn store(
        &mut self,
        place: &Place,
        value: Number,
        world: &mut impl World,
    ) -> Result<Number, String> {
        match place {
            Place::Variable(name) => {
                self.variables.insert(name.clone(), value.clone());
            }
            Place::Element(name, index) => {
                let index = self.index(index, world)?;
                self.arrays
                    .entry(name.clone())
                    .or_default()
                    .insert(index, value.clone());
            }
            Place::Scale => {
                self.scale = bounded(&value, 0, MOST_SCALE, "scale", world) as usize;
                return Ok(count(self.scale));
            }
            Place::Ibase => {
                self.ibase = bounded(&value, 2, 16, "ibase", world);
                return Ok(count(self.ibase as usize));
            }
            Place::Obase => {
                self.obase = bounded(&value, 2, MOST, "obase", world);
                return Ok(count(self.obase as usize));
            }
        }
        Ok(value)
    }

    #[inline(never)]
    fn call(
        &mut self,
        name: &str,
        arguments: &[Argument],
        world: &mut impl World,
    ) -> Result<Number, String> {
        let function = self
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| format!("no function {name} is defined"))?;
        if arguments.len() != function.parameters.len() {
            return Err(format!(
                "{name} wants {} and was given {}",
                function.parameters.len(),
                arguments.len()
            ));
        }
        if self.depth == DEEPEST {
            return Err(format!("calls go deeper than {DEEPEST}"));
        }
        // What the parameters are given, worked out before any of them
        // stands in for its name.
        let mut given = Vec::with_capacity(arguments.len());
        for (argument, parameter) in arguments.iter().zip(&function.parameters) {
            given.push(match (argument, parameter.array) {
                (Argument::Value(expr), false) => Local::Value(self.eval(expr, world)?),
                (Argument::Array(array), true) => {
                    Local::Array(self.arrays.get(array).cloned().unwrap_or_default())
                }
                _ => {
                    return Err(format!(
                        "{name}'s {} is {}",
                        parameter.name,
                        if parameter.array {
                            "an array"
                        } else {
                            "not an array"
                        }
                    ));
                }
            });
        }
        let locals: Vec<(&parse::Local, Local)> = function
            .parameters
            .iter()
            .zip(given)
            .chain(function.autos.iter().map(|auto| {
                let fresh = if auto.array {
                    Local::Array(Array::new())
                } else {
                    Local::Value(Number::default())
                };
                (auto, fresh)
            }))
            .collect();
        let mut kept = Vec::with_capacity(locals.len());
        for (local, value) in locals {
            kept.push((local, self.swap(local, value)));
        }
        self.depth += 1;
        let mut ran = Ok(Flow::Normal);
        for statement in &function.body {
            ran = self.exec(statement, world);
            if !matches!(ran, Ok(Flow::Normal)) {
                break;
            }
        }
        self.depth -= 1;
        for (local, value) in kept.into_iter().rev() {
            self.swap(local, value);
        }
        match ran? {
            Flow::Return(value) => Ok(value),
            Flow::Halt => {
                self.ended = true;
                Err(String::from("halted"))
            }
            _ => Ok(Number::default()),
        }
    }

    /// Puts `value` in for the name `local`, giving what it held.
    #[inline(never)]
    fn swap(&mut self, local: &parse::Local, value: Local) -> Local {
        match value {
            Local::Value(value) => Local::Value(
                self.variables
                    .insert(local.name.clone(), value)
                    .unwrap_or_default(),
            ),
            Local::Array(array) => Local::Array(
                self.arrays
                    .insert(local.name.clone(), array)
                    .unwrap_or_default(),
            ),
        }
    }
}

/// What a parameter or `auto` variable holds.
enum Local {
    Value(Number),
    Array(Array),
}

/// The whole part of `value`, brought within `least` and `most` with a
/// warning when it was not.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn bounded(value: &Number, least: u32, most: u32, name: &str, world: &mut impl World) -> u32 {
    let whole = value.to_i64().unwrap_or(if value.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    });
    if whole < i64::from(least) {
        world.say(&format!("bc: {name} is at least {least}, so it is that\n"));
        return least;
    }
    if whole > i64::from(most) {
        world.say(&format!("bc: {name} is at most {most}, so it is that\n"));
        return most;
    }
    // Within least and most, both a u32's.
    whole as u32
}

fn truth(yes: bool) -> Number {
    Number::from_i64(i64::from(yes))
}

fn count(n: usize) -> Number {
    Number::from_i64(i64::try_from(n).unwrap_or(i64::MAX))
}

/// A string of `print` with its escapes: `\n`, `\t`, `\a`, `\b`, `\f`,
/// `\r`, `\e` for a backslash, `\q` for a quote, and `\\`.
fn escaped(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.iter();
    while let Some(&b) = bytes.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        let Some(&next) = bytes.next() else {
            out.push(b'\\');
            break;
        };
        let replaced = match next {
            b'n' => b'\n',
            b't' => b'\t',
            b'a' => 0x07,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'q' => b'"',
            b'e' | b'\\' => b'\\',
            other => {
                out.push(b'\\');
                other
            }
        };
        out.push(replaced);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a bc program writes and says, kept.
    #[derive(Default)]
    struct Kept {
        written: Vec<u8>,
        said: String,
    }

    impl World for Kept {
        fn write(&mut self, bytes: &[u8]) {
            self.written.extend_from_slice(bytes);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// What `program` writes, and what it says, read all at once.
    fn bc(program: &str) -> (String, String) {
        let mut run = Bc::new();
        let mut kept = Kept::default();
        run.begin("-");
        run.feed(program.as_bytes(), &mut kept);
        run.end(&mut kept);
        (
            String::from_utf8_lossy(&kept.written).into_owned(),
            kept.said,
        )
    }

    /// Programs and what GNU bc 1.08 writes for them.
    const GNU: [(&str, &str); 36] = [
        (
            r"1+2
3-5
2*3.5
7/2
scale=3; 7/2
",
            r"3
-2
7.0
3
3.500
",
        ),
        (
            r"define f(n) { if (n <= 1) return (1); return (n * f(n - 1)) }
f(5)
f(25)
",
            r"120
15511210043330985984000000
",
        ),
        (
            r"define fib(n) {
  auto a, b, c, i
  a = 0; b = 1
  for (i = 0; i < n; i++) { c = a + b; a = b; b = c }
  return a
}
fib(10)
fib(90)
",
            r"55
2880067194370816120
",
        ),
        (
            r"define gcd(a, b) { auto t; while (b != 0) { t = b; b = a % b; a = t }; return a }
gcd(1071, 462)
",
            r"21
",
        ),
        (
            r"for (i = 2; i < 30; i++) p[i] = 1
for (i = 2; i < 30; i++) if (p[i]) { i; for (j = i * i; j < 30; j += i) p[j] = 0 }
",
            r"2
3
5
7
11
13
17
19
23
29
",
        ),
        (
            r"i = 0
while (1) { i += 1; if (i == 3) continue; if (i > 5) break; i }
",
            r"1
2
4
5
",
        ),
        (
            r"x = 5
x++
x
++x
x--
--x
x
",
            r"5
6
7
7
5
5
",
        ),
        (
            r"a = 10; a += 5; a; a -= 3; a; a *= 2; a; a /= 4; a; a %= 4; a; a ^= 3; a
",
            r"15
12
24
6
2
8
",
        ),
        (
            r"1 < 2; 2 < 1; 1 <= 1; 1 >= 2; 1 == 1.0; 1 != 1
!0; !5; 1 && 0; 1 || 0; 0 || 0
",
            r"1
0
1
0
1
0
1
0
0
1
0
",
        ),
        (
            r"obase=16; 255; -255; 10.5
obase=2; 10
obase=10; ibase=16; FF; A
",
            r"FF
-FF
A.8
1010
255
10
",
        ),
        (
            r"scale=20; 1/7
scale=200; 1/7
",
            r".14285714285714285714
.1428571428571428571428571428571428571428571428571428571428571428571\
42857142857142857142857142857142857142857142857142857142857142857142\
85714285714285714285714285714285714285714285714285714285714285714
",
        ),
        (
            r"2^500
",
            r"32733906078961418700131896968275991522166420460430647894832913680961\
33796404674554883270092325904157150886684127560071009217256545885393\
053328527589376
",
        ),
        (
            r#"print "a\tb\n", 1.5, " ", -2, "\n"
"plain\n"
"#,
            r"a	b
1.5 -2
plain\n",
        ),
        (
            r"define f(x) { x = 9; y = 4 }
x = 1; y = 1; f(2); x; y
",
            r"0
1
4
",
        ),
        (
            r"define g() { return (z) }
define h() { auto z; z = 7; return (g()) }
h(); z
",
            r"7
0
",
        ),
        (
            r"define f(a[]) { a[0] = 99; return (a[0] + a[1]) }
b[0] = 1; b[1] = 2; f(b[]); b[0]
",
            r"101
1
",
        ),
        (
            r"define f(n) { auto a[]; a[n] = n; return (a[n] * 2) }
f(3); a[3]
",
            r"6
0
",
        ),
        (
            r"if (1) 5
if (0) 5 else 6
if (0) { 7 } else { 8 }
",
            r"5
6
8
",
        ),
        (
            r"/* comment */ 1 # another
2
",
            r"1
2
",
        ),
        (
            r"scale = 5
sqrt(2)
sqrt(144)
length(123.456); scale(1.25)
",
            r"1.41421
12.00000
6
2
",
        ),
        (
            r"a[2] = 4; a[2] + a[5]
",
            r"4
",
        ),
        (
            r"(a = 5)
b = 6
",
            r"5
",
        ),
        (
            r"-2^2
2^-2
scale=4; 2^-2
2^3^2
",
            r"4
0
.2500
512
",
        ),
        (
            r"x = 3 < 5; x
y = 1 == 1
y
",
            r"1
3
1
1
",
        ),
        (
            r"1; 2
quit
3
",
            r"1
2
",
        ),
        (
            r"define f() { halt }
1
f()
2
",
            r"1
",
        ),
        (
            r"for (i = 0; i < 3; i++) {
  i
}
",
            r"0
1
2
",
        ),
        (
            r"for (;;) { if (++i > 2) break }
i
",
            r"3
",
        ),
        (
            r"x = 0; for (i = 1; i <= 100; i++) x += i; x
",
            r"5050
",
        ),
        (
            r"scale = 2
x = 10 / 3
x * 3
scale(x)
",
            r"9.99
2
",
        ),
        (
            r#""ab
c"; 2^250
"#,
            r"ab
c1809251394333065553493296640760748560207343510400633813116524750123\
642650624
",
        ),
        (
            r"obase=100; 123456789; obase=17; 300; obase=1000; 1234567
",
            r" 01 23 45 67 89
 01 00 11
 001 234 567
",
        ),
        (
            r"ibase=2; 1010; ibase=1010; 15
",
            r"10
15
",
        ),
        (
            r#"define r(n) { if (n == 0) return; print n, " "; r(n - 1) }
r(5)
"#,
            r"5 4 3 2 1 0
0
0
0
0
0
",
        ),
        (
            r"t = 1; for (i = 1; i <= 60; i++) t *= 3; t
",
            r"42391158275216203514294433201
",
        ),
        (
            r"scale=10; e = 1; f = 1; for (i = 1; i < 30; i++) { f *= i; e += 1 / f }; e
",
            r"2.7182818277
",
        ),
    ];

    #[test]
    fn programs_write_what_gnu_bc_writes() {
        for (program, written) in GNU {
            assert_eq!(
                bc(program),
                (String::from(written), String::new()),
                "{program}"
            );
        }
    }

    /// What `lines` write and say, fed a line at a time as the standard
    /// input gives them.
    fn fed(lines: &str) -> (String, String) {
        let mut run = Bc::new();
        let mut kept = Kept::default();
        run.begin("-");
        for line in lines.split_inclusive('\n') {
            run.feed(line.as_bytes(), &mut kept);
        }
        run.end(&mut kept);
        (
            String::from_utf8_lossy(&kept.written).into_owned(),
            kept.said,
        )
    }

    fn pair(written: &str, said: &str) -> (String, String) {
        (written.into(), said.into())
    }

    #[test]
    fn a_program_fed_a_line_at_a_time_runs_as_it_does_whole() {
        for (program, written) in GNU {
            assert_eq!(fed(program), pair(written, ""), "{program}");
        }
        assert_eq!(
            fed("{\n1\n/* a\nb */ 2\n}\n\"x\ny\"\n"),
            pair("1\n2\nx\ny", "")
        );
    }

    #[test]
    fn a_mistake_while_running_ends_the_rest_of_its_line() {
        assert_eq!(bc("1/0; 7\n8\n"), pair("8\n", "bc: a division by zero\n"));
        assert_eq!(
            bc("3%0\n"),
            pair("", "bc: a remainder of a division by zero\n")
        );
        assert_eq!(
            bc("sqrt(-4); 1\n2\n"),
            pair("2\n", "bc: the square root of a number below zero\n")
        );
        assert_eq!(bc("0^-1\n"), pair("", "bc: a division by zero\n"));
        assert_eq!(
            bc("2^99999999999\n"),
            pair("", "bc: a power with more digits than bc holds\n")
        );
        assert_eq!(bc("f(1)\n"), pair("", "bc: no function f is defined\n"));
        assert_eq!(
            bc("define f(x) { return 1 }\nf()\n"),
            pair("", "bc: f wants 1 and was given 0\n")
        );
        assert_eq!(
            bc("define f(a[]) { return 1 }\nf(1)\n"),
            pair("", "bc: f's a is an array\n")
        );
        assert_eq!(
            bc("define f(a) { return 1 }\nb[0] = 1; f(b[])\n"),
            pair("", "bc: f's a is not an array\n")
        );
        let bounds = "bc: an array's elements go from 0 to 16777215\n";
        assert_eq!(bc("a[-1] = 1\n"), pair("", bounds));
        assert_eq!(bc("a[16777216]\n"), pair("", bounds));
        assert_eq!(bc("a[16777215] = 3; a[16777215]\n"), pair("3\n", ""));
        assert_eq!(
            bc("define f() { 1/0; return 5 }\nx = 2; f(); x = 3\nx\n"),
            pair("2\n", "bc: a division by zero\n"),
            "inside a function too"
        );
    }

    #[test]
    fn mistakes_in_the_text_are_said_with_their_lines() {
        assert_eq!(
            bc("1 2\n3\n"),
            pair("3\n", "bc: - 1: the number 2 cannot come here\n")
        );
        assert_eq!(
            bc("@\n4\n"),
            pair("4\n", "bc: - 1: a character bc does not take: @\n")
        );
        assert_eq!(
            bc("1\nbreak\n5\n"),
            pair("1\n5\n", "bc: - 2: break outside a loop\n")
        );
        assert_eq!(
            bc("continue\n"),
            pair("", "bc: - 1: continue outside a loop\n")
        );
        assert_eq!(
            bc("return 5\n6\n"),
            pair("6\n", "bc: - 1: return outside a function\n")
        );
        assert_eq!(
            bc("if (0) 5\nelse 6\n7\n"),
            pair("7\n", "bc: - 2: else cannot come here\n"),
            "else goes on the line its statement ends on"
        );
        assert_eq!(
            bc("1 +\n2\n"),
            pair("2\n", "bc: - 1: the end of the line cannot come here\n")
        );
        assert_eq!(
            bc("\"abc\n"),
            pair("", "bc: - 1: the text ends inside a statement\n")
        );
        assert_eq!(
            bc("1\n{ 2\n"),
            pair("1\n", "bc: - 2: the text ends inside a statement\n")
        );
        assert_eq!(
            bc("3 = 4\n"),
            pair(
                "",
                "bc: - 1: only a variable, an element or scale, ibase or obase takes a value\n"
            )
        );
        assert_eq!(
            bc("++3\n"),
            pair(
                "",
                "bc: - 1: ++ and -- take a variable, an element or scale, ibase or obase\n"
            )
        );
        assert_eq!(
            bc("f(1 2)\n"),
            pair("", "bc: - 1: the number 2 cannot come here\n")
        );
        assert_eq!(
            bc("(1\n"),
            pair("", "bc: - 1: ) was wanted, not the end of the line\n")
        );
    }

    #[test]
    fn scale_ibase_and_obase_stay_in_their_bounds() {
        assert_eq!(
            bc("scale=-1\nscale\n"),
            pair("0\n", "bc: scale is at least 0, so it is that\n")
        );
        assert_eq!(
            bc("ibase=1\nibase\n"),
            pair("2\n", "bc: ibase is at least 2, so it is that\n")
        );
        assert_eq!(
            bc("ibase=17\nibase\n"),
            pair("16\n", "bc: ibase is at most 16, so it is that\n")
        );
        assert_eq!(
            bc("obase=1\nobase\n"),
            pair("10\n", "bc: obase is at least 2, so it is that\n"),
            "2 in base 2"
        );
        assert_eq!(
            bc("obase=16777216\n"),
            pair("", "bc: obase is at most 16777215, so it is that\n")
        );
        assert_eq!(
            bc("scale=99999999999\n"),
            pair("", "bc: scale is at most 2147483647, so it is that\n")
        );
        assert_eq!(
            bc("2^1.5\n2^1.0\n"),
            pair(
                "2\n2\n",
                "bc: only the whole part of an exponent counts\nbc: only the whole part of an exponent counts\n"
            )
        );
        assert_eq!(bc("(scale = 3.9)\nscale\n"), pair("3\n3\n", ""));
        assert_eq!(bc("ibase=16\n(obase=A)\n"), pair("10\n", ""), "A is ten");
    }

    #[test]
    fn quit_ends_bc_as_it_is_read_and_halt_as_it_runs() {
        assert_eq!(bc("1; quit; 2\n"), pair("", ""));
        assert_eq!(bc("1\nif (0) quit\n2\n"), pair("1\n", ""));
        assert_eq!(bc("define f() { quit }\n1\n"), pair("", ""));
        assert_eq!(bc("1; halt; 2\n3\n"), pair("1\n", ""));
        assert_eq!(bc("if (0) halt\n4\n"), pair("4\n", ""));
        let mut run = Bc::new();
        let mut kept = Kept::default();
        run.begin("-");
        assert!(run.feed(b"1\n", &mut kept));
        assert!(!run.ended());
        assert!(!run.feed(b"quit\n", &mut kept));
        assert!(run.ended());
        assert!(!run.feed(b"2\n", &mut kept), "nothing runs after");
        assert_eq!(kept.written, b"1\n");
    }

    #[test]
    fn calls_go_no_deeper_than_their_limit_on_a_program_s_stack() {
        let deep = "define f(n) { if (n == 0) return (0); return (f(n - 1) + 1) }\n";
        let (most, past) = (format!("{deep}f(499)\n"), format!("{deep}f(500)\n"));
        // The most stack a program may ask for.
        let ran = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || (bc(&most), bc(&past)))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(ran.0, pair("499\n", ""));
        assert_eq!(ran.1, pair("", "bc: calls go deeper than 500\n"));
    }

    #[test]
    fn nesting_goes_no_deeper_than_its_limit() {
        let deep =
            |n: usize, open: &str, close: &str| format!("{}1{}\n", open.repeat(n), close.repeat(n));
        assert_eq!(bc(&deep(90, "(", ")")), pair("1\n", ""));
        let nested = "bc: - 1: expressions and statements nest deeper than 100\n";
        assert_eq!(bc(&deep(200, "(", ")")), pair("", nested));
        assert_eq!(bc(&deep(200, "- ", "")), pair("", nested));
        assert_eq!(bc(&deep(200, "!", "")), pair("", nested));
        assert_eq!(bc(&deep(200, "2^", "")), pair("", nested));
        assert_eq!(bc(&deep(200, "{", "}")), pair("", nested));
        assert_eq!(bc(&deep(200, "if (1) ", "")), pair("", nested));
        assert_eq!(
            bc(&deep(90, "- ", "")),
            pair("1\n", ""),
            "an even count of minus"
        );
    }

    #[test]
    fn long_lines_break_after_68_characters() {
        let ones = |n: usize| "1".repeat(n);
        assert_eq!(bc(&format!("{}\n", ones(68))).0, format!("{}\n", ones(68)));
        assert_eq!(
            bc(&format!("{}\n", ones(69))).0,
            format!("{}\\\n1\n", ones(68))
        );
        assert_eq!(
            bc(&format!("{}\n", ones(137))).0,
            format!("{}\\\n{}\\\n1\n", ones(68), ones(68))
        );
        assert_eq!(
            bc(&format!("\"{}\"; 12\n", "x".repeat(67))).0,
            format!("{}1\\\n2\n", "x".repeat(67)),
            "what is on the line before counts"
        );
        assert_eq!(
            bc(&format!("\"{}\n\"; {}\n", "x".repeat(67), ones(68))).0,
            format!("{}\n{}\n", "x".repeat(67), ones(68)),
            "a newline begins the count again"
        );
    }

    #[test]
    fn print_s_strings_take_their_escapes() {
        assert_eq!(escaped(br"a\nb\tc\\d\qe\ef\x\"), b"a\nb\tc\\d\"e\\f\\x\\");
        assert_eq!(escaped(br"\a\b\f\r"), b"\x07\x08\x0c\r");
        assert_eq!(escaped(b"plain"), b"plain");
        assert_eq!(bc("print \"\\q1\\q\"\n").0, "\"1\"");
        assert_eq!(bc("\"\\n\"\n").0, "\\n", "a string alone takes none");
    }

    #[test]
    fn functions_see_the_names_their_callers_made_their_own() {
        let program = "define g() { return (v) }\n\
                       define f() { auto v; v = 2; return (g()) }\n\
                       v = 1; f(); g(); v\n";
        assert_eq!(bc(program), pair("2\n1\n1\n", ""));
        let arrays = "define f(a[]) { auto b[]; b[0] = a[0] + 1; a[0] = 9; return (b[0]) }\n\
                      a[0] = 4; b[0] = 7; f(a[]); a[0]; b[0]\n";
        assert_eq!(
            bc(arrays),
            pair("5\n4\n7\n", ""),
            "arrays go by value, and come back"
        );
        let recursive = "define f(n) { auto x; x = n; if (n > 0) f(n - 1); return (x) }\nf(3)\n";
        assert_eq!(
            bc(recursive),
            pair(
                "0
1
2
3
", ""
            ),
            "each call its own, and a call alone written, in a function too"
        );
        let redefined = "define f() { return (1) }\ndefine f() { return (2) }\nf()\n";
        assert_eq!(bc(redefined), pair("2\n", ""), "the last definition holds");
    }

    #[test]
    fn values_of_places_and_steps_are_what_they_were_or_became() {
        assert_eq!(
            bc("a[1]++; a[1]; --a[1]; scale++; scale; obase++\n"),
            pair("0\n1\n0\n0\n1\nA\n", ""),
            "10, written in base 11 once obase is"
        );
        assert_eq!(bc("ibase--\nibase\n"), pair("10\n9\n", ""));
        assert_eq!(bc("x = y = 3; x + y\n"), pair("6\n", ""));
        assert_eq!(bc("a[2] += 5; a[2] *= 3; a[2]\n"), pair("15\n", ""));
    }

    /// Longer programs, and what GNU bc 1.08 writes for them.
    const LONGER: [(&str, &str); 31] = [
        (
            r"scale = 30
define r(x) {
  auto g, h
  g = x / 2
  while (1) {
    h = (g + x / g) / 2
    if (h == g) return (g)
    g = h
  }
}
r(2)
sqrt(2)
r(2) - sqrt(2)
",
            r"1.414213562373095048801688724209
1.414213562373095048801688724209
0
",
        ),
        (
            r"define c(n) {
  auto s
  for (s = 0; n != 1; s++) {
    if (n % 2 == 0) n = n / 2 else n = 3 * n + 1
  }
  return (s)
}
c(27); c(97); c(871)
",
            r"111
118
178
",
        ),
        (
            r"define a(m, n) {
  if (m == 0) return (n + 1)
  if (n == 0) return (a(m - 1, 1))
  return (a(m - 1, a(m, n - 1)))
}
a(2, 3); a(3, 3)
",
            r"9
61
",
        ),
        (
            r"define s() { scale = 7; return (1 / 3) }
s()
scale
1 / 3
",
            r".3333333
7
.3333333
",
        ),
        (
            r"define f() { return (10) }
ibase = 16
f()
ibase = A
f()
",
            r"16
10
",
        ),
        (
            r"obase = 16
-1 / 4
scale = 4
-1 / 4
-255.75
obase = 8
-0.5
",
            r"0
-.4000
-FF.C0
-.40
",
        ),
        (
            r"define f(n) { if (n < 2) return (1); return (n * f(n - 1)) }
f(100)
",
            r"93326215443944152681699238856266700490715968264381621468592963895217\
59999322991560894146397615651828625369792082722375825118521091686400\
0000000000000000000000
",
        ),
        (
            r"for (i = 0; i < 6; i++) {
  if (i % 2) continue
  i
}
",
            r"0
2
4
",
        ),
        (
            r#"for (i = 1; i <= 3; i++) {
  for (j = 1; j <= 3; j++) {
    if (j > i) break
    print i, j, " "
  }
}
print "\n"
"#,
            r"11 21 22 31 32 33 
",
        ),
        (
            r"define g(x) {
  if (x < 0) return (-1) else if (x == 0) return (0) else return (1)
}
g(-5); g(0); g(7)
",
            r"-1
0
1
",
        ),
        (
            r#"print "a\tb\\c\qd\n"
obase = 2
print 5, " ", 6, "\n"
"#,
            r#"a	b\c"d
101 110
"#,
        ),
        (
            r"i = 0
while (i++ < 3) i
i
",
            r"1
2
3
4
",
        ),
        (
            r"scale = 5
x = 22 / 7
x; length(x); scale(x)
length(x * 1000)
scale(x * 1.000000)
",
            r"3.14285
6
5
9
6
",
        ),
        (
            r"scale = 150
1 / 97
",
            r".0103092783505154639175257731958762886597938144329896907216494845360\
82474226804123711340206185567010309278350515463917525773195876288659\
793814432989690
",
        ),
        (
            r"/* a */ define f(x) { /* b */
  return (x * x) # c
}
f(9) /* d */
",
            r"81
",
        ),
        (
            r#"x = 3; "x is "; x; "done
"
"#,
            r"x is 3
done
",
        ),
        (
            r"define sum(a[], n) { auto i, s; for (i = 0; i < n; i++) s += a[i]; return (s) }
for (i = 0; i < 10; i++) b[i] = i * i
sum(b[], 10)
",
            r"285
",
        ),
        (
            r#"n = 60
for (i = 2; i <= n; i++) s[i] = 1
for (i = 2; i * i <= n; i++) if (s[i]) for (j = i * i; j <= n; j += i) s[j] = 0
for (i = 2; i <= n; i++) if (s[i]) print i, " "
print "\n"
"#,
            r"2 3 5 7 11 13 17 19 23 29 31 37 41 43 47 53 59 
",
        ),
        (
            r"scale = 0
2 ^ 64 % 1000007
(-7) % 3; 7 % -3; -7 % -3
scale = 3
-7.5 % 2
",
            r"919788
-1
1
-1
0
",
        ),
        (
            r"(1 < 2) + (2 < 1) + (3 == 3)
!(1 == 2) * 5
",
            r"2
1
",
        ),
        (
            r"obase = 60
3661.5
obase = 24
scale = 6
1 / 3
",
            r" 01 01 01.30
.07 23 23 23 21
",
        ),
        (
            r"sqrt(10 ^ 40)
scale = 20; sqrt(10 ^ 41)
",
            r"100000000000000000000
316227766016837933199.88935444327185337195
",
        ),
        (
            r"define g(a, b) { if (b == 0) return (a); return (g(b, a % b)) }
define l(a, b) { return (a * b / g(a, b)) }
g(462, 1071); l(4, 6); l(21, 6)
",
            r"21
12
42
",
        ),
        (
            r"a = 0; b = 1
for (i = 0; i < 200; i++) { c = a + b; a = b; b = c }
a
",
            r"280571172992510140037611932413038677189525
",
        ),
        (
            r"(x = 5) + (y = 6)
x * y
(scale = 3)
",
            r"11
30
3
",
        ),
        (
            r"a[5] = 2; a[5] ^= 10; a[5]
scale = 1; scale += 2; scale
",
            r"1024
3
",
        ),
        (
            r"scale = 10
2 ^ -10
(-2) ^ -3
0.5 ^ -2
",
            r".0009765625
-.1250000000
4.0000000000
",
        ),
        (
            r"000.0100
00100
.000
",
            r".0100
100
0
",
        ),
        (
            r"for (i = 0; i < 10; i++) { if (i == 3) halt; i }
",
            r"0
1
2
",
        ),
        (
            r"define f(n) { auto a[]; a[0] += n; return (a[0]) }
f(1); f(2)
",
            r"1
2
",
        ),
        (
            r"define p(n) { auto r; if (n == 0) return (1); r = 2 * p(n - 1); return (r) }
p(30)
",
            r"1073741824
",
        ),
    ];

    #[test]
    fn longer_programs_write_what_gnu_bc_writes() {
        for (program, written) in LONGER {
            assert_eq!(bc(program), pair(written, ""), "{program}");
            assert_eq!(fed(program), pair(written, ""), "fed: {program}");
        }
    }

    #[test]
    fn each_text_names_its_mistakes_and_definitions_last_across_them() {
        let mut run = Bc::new();
        let mut kept = Kept::default();
        run.begin("a.bc");
        run.feed(b"define f(x) { return (x * 2) }\n1 2\n", &mut kept);
        run.end(&mut kept);
        run.begin("-");
        run.feed(b"f(4)\n\n@\n", &mut kept);
        run.end(&mut kept);
        assert_eq!(kept.written, b"8\n");
        assert_eq!(
            kept.said,
            "bc: a.bc 2: the number 2 cannot come here\nbc: - 3: a character bc does not take: @\n"
        );
    }

    #[test]
    fn the_last_line_runs_without_its_newline() {
        assert_eq!(bc("1+2"), pair("3\n", ""));
        assert_eq!(bc("1+2 # c"), pair("3\n", ""));
        assert_eq!(bc("1\n# only a comment"), pair("1\n", ""));
        assert_eq!(bc("define f() { return 5 }"), pair("", ""));
        assert_eq!(bc("1\n  \t\n"), pair("1\n", ""), "blanks are nothing");
        let cut = "bc: - 2: the text ends inside a statement\n";
        assert_eq!(bc("1\n/* open"), pair("1\n", cut));
        assert_eq!(bc("1\n{ 2"), pair("1\n", cut));
        assert_eq!(bc("1\n\"open"), pair("1\n", cut));
        assert_eq!(bc("quit"), pair("", ""), "and quit, nothing after");
    }

    #[test]
    fn a_line_may_come_in_pieces() {
        let mut run = Bc::new();
        let mut kept = Kept::default();
        run.begin("-");
        for piece in ["1", "2", " + ", "3", "\n", "x", " = 4\nx\n"] {
            run.feed(piece.as_bytes(), &mut kept);
        }
        run.end(&mut kept);
        assert_eq!(kept.written, b"15\n4\n");
        assert_eq!(kept.said, "");
    }

    #[test]
    fn a_bad_character_ends_its_line_alone() {
        assert_eq!(
            bc("1\n2 @ 3\n4\n"),
            pair("1\n4\n", "bc: - 2: a character bc does not take: @\n")
        );
        assert_eq!(
            bc("{\n1\n$\n}\n5\n"),
            pair(
                "5\n",
                "bc: - 3: a character bc does not take: $\nbc: - 4: } cannot come here\n"
            ),
            "the block before it goes too"
        );
    }

    #[test]
    fn scale_is_set_as_l_sets_it() {
        let mut run = Bc::default();
        let mut kept = Kept::default();
        run.set_scale(20);
        run.begin("-");
        run.feed(b"scale\n1/3\n", &mut kept);
        assert_eq!(kept.written, b"20\n.33333333333333333333\n");
    }

    #[test]
    fn zero_is_0_in_every_base_and_long_lines_break_in_all() {
        assert_eq!(
            bc("obase=2; 0; obase=16; 0.000; obase=100; -0\n"),
            pair("0\n0\n0\n", "")
        );
        let (written, _) = bc("obase=1000; 10^75\n");
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 69, "68 and the backslash");
        assert!(lines[0].ends_with('\\'));
        assert_eq!(
            lines.concat().replace('\\', ""),
            " 001".to_string() + &" 000".repeat(25)
        );
    }

    #[test]
    fn a_mistake_inside_an_index_or_a_call_ends_the_line() {
        assert_eq!(
            bc("a[-1] += 1; 2\n3\n"),
            pair("3\n", "bc: an array's elements go from 0 to 16777215\n")
        );
        assert_eq!(
            bc("define f(x) { return (x) }\nf(1/0); 2\n3\n"),
            pair("3\n", "bc: a division by zero\n"),
            "an argument's"
        );
        assert_eq!(bc("a[1/0] = 1\n"), pair("", "bc: a division by zero\n"));
        assert_eq!(
            bc("define f() { auto x; x = 1/0 }\nx = 5; f(); x = 6\nx\n"),
            pair("5\n", "bc: a division by zero\n"),
            "and the names a call took are given back"
        );
    }

    #[test]
    fn conditions_and_loops_test_their_values() {
        assert_eq!(
            bc("if (0.0001) 1\nif (-1) 2\nif (0.000) 3\n"),
            pair("1\n2\n", "")
        );
        assert_eq!(
            bc("while (0) 1\nfor (i = 0; 0; i++) 2\ni\n"),
            pair("0\n", ""),
            "a loop that never runs"
        );
        assert_eq!(
            bc("for (i = 0; i < 3; i++) { if (i == 1) continue; print i }\nprint \"\\n\"\n"),
            pair("02\n", "")
        );
        assert_eq!(
            bc("define f() { for (i = 0; ; i++) if (i == 4) return (i) }\nf()\n"),
            pair("4\n", ""),
            "return goes through a loop"
        );
        assert_eq!(
            bc("define f() { while (1) { while (1) return (7) } }\nf()\n"),
            pair("7\n", ""),
            "and through loops in loops"
        );
        assert_eq!(
            bc("i = 0; while (1) { if (++i == 3) break }; i\n"),
            pair("3\n", "")
        );
    }

    #[test]
    fn and_and_or_work_out_only_what_they_need() {
        assert_eq!(bc("0 && x++; x; 1 || y++; y\n"), pair("0\n0\n1\n0\n", ""));
        assert_eq!(bc("1 && x++; x; 0 || y++; y\n"), pair("0\n1\n0\n1\n", ""));
        assert_eq!(bc("2 && 3; 0 || 0.5; !2; !0\n"), pair("1\n1\n0\n1\n", ""));
    }

    /// POSIX's example and programs of numbers written every way, and
    /// what GNU bc 1.08 writes for them.
    const WRITTEN: [(&str, &str); 15] = [
        (
            r"scale = 20
define e(x){
	auto a, b, c, i, s
	a = 1
	b = 1
	s = 1
	for(i=1; 1==1; i++){
		a = a*x
		b = b*i
		c = a/b
		if(c == 0) return(s)
		s = s+c
	}
}
for(i=1; i<=10; i++) e(i)
",
            r"2.71828182845904523526
7.38905609893065022713
20.08553692318766774083
54.59815003314423907790
148.41315910257660342091
403.42879349273512260821
1096.63315842845859926350
2980.95798704172827474335
8103.08392757538400770974
22026.46579480671651695759
",
        ),
        (
            r"scale = 50
define t(x) {
  auto s, p, k, q
  s = x; p = x; k = 1
  while (1) {
    p = -p * x * x
    k += 2
    q = p / k
    if (q == 0) return (s)
    s += q
  }
}
4 * (4 * t(1/5) - t(1/239))
",
            r"3.14159265358979323846264338327950288419716939937496
",
        ),
        (
            r"x = 255; obase = 16; x; obase = 8; x; obase = 2; x; obase = 10; x
",
            r"FF
377
11111111
255
",
        ),
        (
            r"ibase = 16; 1F; ibase = 2; 101; ibase = 1010; 99
",
            r"31
5
99
",
        ),
        (
            r#""a"; 1; "b
c"; 2
"#,
            r"a1
b
c2
",
        ),
        (
            r"define f() {}
define g() { return }
f(); g()
",
            r"0
0
",
        ),
        (
            r"a[1000000] = 7; a[999999] + a[1000000]
b[0] = 1; b[16777215] = 2; b[0] + b[16777215]
",
            r"7
3
",
        ),
        (
            r"scale = 0
1.25 * 1.25
scale = 1
1.25 * 1.25
scale = 10
1.25 * 1.25
0.1 * 0.1 * 0.1
",
            r"1.56
1.56
1.5625
.001
",
        ),
        (
            r"scale = 2
-5 / 3
5 / -3
-5 / -3
-0.01 / 3
",
            r"-1.66
-1.66
1.66
0
",
        ),
        (
            r"scale = 0; sqrt(99); sqrt(100); sqrt(0.99)
scale = 3; sqrt(99); sqrt(1); sqrt(0.0001)
",
            r"9
10
.99
9.949
1
.0100
",
        ),
        (
            r"define m(a[], n) { auto i, b; b = a[0]; for (i = 1; i < n; i++) if (a[i] > b) b = a[i]; return (b) }
define f() { auto a[]; a[0] = 3; a[1] = 9; a[2] = 4; return (m(a[], 3)) }
f()
",
            r"9
",
        ),
        (
            r"for (i = 1; i < 100; i *= 3) i
",
            r"1
3
9
27
81
",
        ),
        (
            r"length(2 ^ 1000)
scale(2 ^ 1000)
length(0.5 ^ 10)
scale = 40; length(0.5 ^ 10)
",
            r"302
0
1
10
",
        ),
        (
            r#"obase = 20
print 399, " ", 400, "\n"
"#,
            r" 19 19  01 00 00
",
        ),
        (
            r"for (i = 2; i > -3; i--) i
",
            r"2
1
0
-1
-2
",
        ),
    ];

    #[test]
    fn posix_s_example_and_numbers_written_every_way_come_out_as_gnu_s() {
        for (program, written) in WRITTEN {
            assert_eq!(bc(program), pair(written, ""), "{program}");
        }
    }

    #[test]
    fn tokens_go_as_far_as_they_are_whole() {
        let mut run = Bc::new();
        run.begin("-");
        run.pending = b"1\n\"open".to_vec();
        let (tokens, limit, bad) = run.whole_tokens();
        assert_eq!(
            (tokens.len(), limit, bad),
            (2, 2, None),
            "up to the open string"
        );
        run.pending = b"1\n/* open".to_vec();
        assert_eq!(run.whole_tokens().1, 2, "up to the open comment");
        run.pending = b"1\n2 @ 3\n4\n".to_vec();
        let (tokens, limit, bad) = run.whole_tokens();
        assert_eq!(
            (tokens.len(), limit, bad),
            (2, 2, Some(('@', 2, 8))),
            "up to the line with the bad character, which ends at 8"
        );
        run.pending = b"1 + 2\n".to_vec();
        let (tokens, limit, bad) = run.whole_tokens();
        assert_eq!((tokens.len(), limit, bad), (4, 6, None), "all of it");
    }

    #[test]
    fn the_column_goes_on_from_one_feed_to_the_next() {
        let mut run = Bc::new();
        let mut kept = Kept::default();
        run.begin("-");
        run.feed(format!("\"{}\"\n", "x".repeat(60)).as_bytes(), &mut kept);
        run.feed(b"1234567890\n", &mut kept);
        let written = String::from_utf8(kept.written).unwrap();
        assert_eq!(
            written,
            format!("{}12345678\\\n90\n", "x".repeat(60)),
            "the number goes on the line the string began"
        );
    }

    #[test]
    fn each_fault_is_said_in_words() {
        assert_eq!(says(Fault::DivideByZero), "a division by zero");
        assert_eq!(
            says(Fault::ModuloByZero),
            "a remainder of a division by zero"
        );
        assert_eq!(
            says(Fault::NegativeRoot),
            "the square root of a number below zero"
        );
        assert_eq!(
            says(Fault::TooGreat),
            "a power with more digits than bc holds"
        );
    }
}
