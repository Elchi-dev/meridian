//! sh scripts run, as decision record 0021 has it: their commands on a
//! machine, and their variables, functions, parameters and the builtins
//! POSIX calls special in a session.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::expand::{Char, failed};
use super::lex::Word;
use super::parse::{
    self, AndOr, Command, Compound, Function, How, Item, Join, Pipeline, Redirection as Written,
    Simple,
};
use crate::parse::is_name;
use crate::run::{Machine, Ready, Redirection, Target};
use crate::{Code, Error, Pos};

/// How a list ended: on to what follows, or out of `n` loops, or on with
/// the `n`th loop out, or out of a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Next,
    Break(usize),
    Continue(usize),
    Return,
}

/// What stops a script: an error at its place, `exit` with a status, or an
/// error the shell said already, with the status it ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Error(Error),
    Exit(u8),
    Said(u8),
}

impl From<Error> for Stop {
    fn from(e: Error) -> Stop {
        Stop::Error(e)
    }
}

/// A variable: its value, none for one marked before it was given one,
/// and whether it goes to the programs the shell starts and whether it
/// keeps its value.
#[derive(Clone, Debug, Default)]
struct Variable {
    value: Option<String>,
    exported: bool,
    readonly: bool,
}

/// How deep commands may run inside each other in all, compound commands,
/// function calls, `eval` and `$(...)` alike, before a call, an `eval` or
/// a `$(...)` going deeper is refused. Compound commands nest 16 deep at
/// most in one text, so the deepest script is some 80 deep, which the
/// shell's stack holds (decision record 0022).
const DEEPEST_RUNNING: usize = 64;

/// The builtins POSIX calls special: their errors end a script, and the
/// variables assigned before them stay.
const SPECIAL: [&str; 11] = [
    "break", "continue", "return", "exit", ":", "eval", "export", "readonly", "set", "shift",
    "unset",
];

/// A sh session: its variables, functions and parameters, its options,
/// and the status of its last command.
#[derive(Clone, Debug)]
pub struct Session {
    variables: BTreeMap<String, Variable>,
    functions: BTreeMap<String, Function>,
    positional: Vec<String>,
    zero: String,
    process: u64,
    status: u8,
    /// The letters of the options `set` turned on.
    options: BTreeSet<char>,
    /// How deep the command running stands inside others: compound
    /// commands, function calls, `eval` and `$(...)`.
    depth: usize,
    /// How many loops the command running stands inside.
    loops: usize,
    /// How many conditions, which `set -e` leaves alone, the command
    /// running stands in.
    conditions: usize,
    /// The status of the last command substitution of the command being
    /// expanded, which is the status of a command of assignments alone.
    substituted: Option<u8>,
}

/// `value` quoted so the shell reads it back as it is.
fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The text of a field: its characters, its quotes gone.
fn text(field: &[Char]) -> String {
    field.iter().map(|c| c.c).collect()
}

impl Session {
    /// A session for the script called `zero`, run by the process
    /// `process`, its positional parameters `positional`. `IFS` starts as
    /// space, tab and newline, as POSIX sets it.
    #[must_use]
    pub fn new(zero: String, process: u64, positional: Vec<String>) -> Session {
        let mut variables = BTreeMap::new();
        variables.insert(
            String::from("IFS"),
            Variable {
                value: Some(String::from(" \t\n")),
                ..Variable::default()
            },
        );
        Session {
            variables,
            functions: BTreeMap::new(),
            positional,
            zero,
            process,
            status: 0,
            options: BTreeSet::new(),
            depth: 0,
            loops: 0,
            conditions: 0,
            substituted: None,
        }
    }

    /// The status of the last command.
    #[must_use]
    pub fn status(&self) -> u8 {
        self.status
    }

    /// The value of the variable `name`, none when it is not set.
    #[must_use]
    pub fn variable(&self, name: &str) -> Option<String> {
        self.variables.get(name)?.value.clone()
    }

    pub(super) fn positional(&self) -> &[String] {
        &self.positional
    }

    pub(super) fn process(&self) -> u64 {
        self.process
    }

    pub(super) fn zero(&self) -> &str {
        &self.zero
    }

    /// The letters of the options that are on, as `$-` gives them.
    pub(super) fn flags(&self) -> String {
        self.options.iter().collect()
    }

    pub(super) fn flag(&self, letter: char) -> bool {
        self.options.contains(&letter)
    }

    /// Sets the variable `name` to `value`; an error when it is read only.
    pub(super) fn assign(&mut self, name: &str, value: String) -> Result<(), ()> {
        let variable = self.variables.entry(name.into()).or_default();
        if variable.readonly {
            return Err(());
        }
        variable.value = Some(value);
        Ok(())
    }

    /// Runs `items`, the commands of a script or a list, and answers how
    /// they ended.
    ///
    /// # Errors
    ///
    /// What stops the script: an error, `exit`, or one already said.
    pub fn run(&mut self, items: &[Item], machine: &mut impl Machine) -> Result<Flow, Stop> {
        for item in items {
            // A list run with & runs in a subshell and ends with 0, as
            // POSIX has it; this one runs to its end first.
            if item.background {
                self.subshell(None, machine, |copy, machine| {
                    copy.and_or(&item.and_or, machine)
                });
                self.status = 0;
                continue;
            }
            let flow = self.and_or(&item.and_or, machine)?;
            if flow != Flow::Next {
                return Ok(flow);
            }
        }
        Ok(Flow::Next)
    }

    fn and_or(&mut self, list: &AndOr, machine: &mut impl Machine) -> Result<Flow, Stop> {
        let mut flow = self.checked(&list.first, !list.rest.is_empty(), machine)?;
        for (i, (join, pipeline)) in list.rest.iter().enumerate() {
            if flow != Flow::Next {
                return Ok(flow);
            }
            let runs = match join {
                Join::And => self.status == 0,
                Join::Or => self.status != 0,
            };
            if runs {
                flow = self.checked(pipeline, i + 1 < list.rest.len(), machine)?;
            }
        }
        Ok(flow)
    }

    /// Runs `pipeline`, a condition when `condition`, and ends the script
    /// as `set -e` asks when it failed and was none.
    fn checked(
        &mut self,
        pipeline: &Pipeline,
        condition: bool,
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        let condition = condition || pipeline.not;
        self.conditions += usize::from(condition);
        let flow = self.pipeline(pipeline, machine);
        self.conditions -= usize::from(condition);
        let flow = flow?;
        if pipeline.not {
            self.status = u8::from(self.status == 0);
        }
        let fails = self.status != 0 && !condition && self.conditions == 0;
        if flow == Flow::Next && fails && self.flag('e') {
            return Err(Stop::Exit(self.status));
        }
        Ok(flow)
    }

    fn pipeline(&mut self, pipeline: &Pipeline, machine: &mut impl Machine) -> Result<Flow, Stop> {
        if let [command] = pipeline.commands.as_slice() {
            return self.command(command, machine);
        }
        self.stages(&pipeline.commands, machine);
        Ok(Flow::Next)
    }

    /// Runs a pipeline of more than one command, each in a copy of the
    /// session, so what one sets stays in it and what stops one ends only
    /// it: side by side on the machine when it runs each of them, or else
    /// in their order, what one writes the next one's input.
    fn stages(&mut self, commands: &[Command], machine: &mut impl Machine) {
        let mut stages = Vec::new();
        for command in commands {
            let mut copy = self.clone();
            let prepared = match command {
                Command::Simple(simple) => Some(copy.prepare(simple, machine)),
                _ => None,
            };
            stages.push((copy, prepared));
        }
        let readies: Vec<Ready> = stages
            .iter()
            .filter_map(|(copy, prepared)| match prepared {
                Some(Ok(prepared)) if copy.machines(prepared) => Some(prepared.ready.clone()),
                _ => None,
            })
            .collect();
        if readies.len() == commands.len() {
            self.status = machine.run(&readies);
            return;
        }
        let mut input: Option<Vec<u8>> = None;
        for (i, (command, (mut copy, prepared))) in commands.iter().zip(stages).enumerate() {
            let last = i + 1 == commands.len();
            let fed = input.take().is_some_and(|text| {
                machine.begin_redirections(&[Redirection {
                    stream: 0,
                    target: Target::Text(text),
                }])
            });
            if !last {
                machine.begin_capture();
            }
            let ran = match prepared {
                Some(Ok(prepared)) => copy.prepared(prepared, machine),
                Some(Err(stop)) => Err(stop),
                None => copy.command(command, machine),
            };
            if !last {
                input = Some(machine.end_capture());
            }
            if fed {
                machine.end_redirections();
            }
            self.status = copy.ended(ran, machine);
        }
    }

    /// The status a copy of the session ends with after `ran`, saying the
    /// error that ended it, which ends only the copy.
    fn ended(&self, ran: Result<Flow, Stop>, machine: &mut impl Machine) -> u8 {
        match ran {
            Ok(_) => self.status,
            Err(Stop::Exit(status) | Stop::Said(status)) => status,
            Err(Stop::Error(e)) => {
                let _ = machine.err(format!("{e}\n").as_bytes());
                2
            }
        }
    }

    fn command(&mut self, command: &Command, machine: &mut impl Machine) -> Result<Flow, Stop> {
        match command {
            Command::Simple(simple) => {
                let prepared = self.prepare(simple, machine)?;
                self.prepared(prepared, machine)
            }
            Command::Compound(compound, written) => {
                let made = self.redirections(written, machine)?;
                if made.is_empty() {
                    return self.compound(compound, machine);
                }
                if !machine.begin_redirections(&made) {
                    self.status = 1;
                    return Ok(Flow::Next);
                }
                let flow = self.compound(compound, machine);
                machine.end_redirections();
                flow
            }
            Command::Function(function) => {
                self.functions
                    .insert(function.name.clone(), function.clone());
                self.status = 0;
                Ok(Flow::Next)
            }
        }
    }

    /// `written` with their words expanded, ready for the machine.
    fn redirections(
        &mut self,
        written: &[Written],
        machine: &mut impl Machine,
    ) -> Result<Vec<Redirection>, Stop> {
        let mut made = Vec::new();
        for redirection in written {
            made.push(match redirection {
                Written::File { stream, how, word } => {
                    let path = self.text(word, machine)?;
                    let target = match how {
                        How::Read => Target::Read(path),
                        How::Write if self.flag('C') => Target::Create(path),
                        How::Write | How::Clobber => Target::Write(path),
                        How::Append => Target::Append(path),
                        How::ReadWrite => Target::ReadWrite(path),
                        How::CopyIn | How::CopyOut if path == "-" => Target::Close,
                        How::CopyIn | How::CopyOut => match path.parse() {
                            Ok(other) => Target::Copy(other),
                            Err(_) => return Err(failed(word.at, Code::BadCopy)),
                        },
                    };
                    Redirection {
                        stream: *stream,
                        target,
                    }
                }
                // A here-document's body expands as inside double quotes.
                Written::Here { stream, doc } => {
                    let body = self.characters(&doc.body, doc.word.at, true, machine)?;
                    Redirection {
                        stream: *stream,
                        target: Target::Text(text(&body).into_bytes()),
                    }
                }
            });
        }
        Ok(made)
    }

    /// `words` expanded into the fields they make, as a command's words.
    fn words(&mut self, words: &[Word], machine: &mut impl Machine) -> Result<Vec<String>, Stop> {
        let mut out = Vec::new();
        for word in words {
            out.extend(self.fields(word, machine)?.iter().map(|f| text(f)));
        }
        Ok(out)
    }

    /// A simple command with its words, redirections and assignments
    /// expanded, in that order, as POSIX expands them; each assignment is
    /// made while those after it are expanded, and taken back after.
    fn prepare(&mut self, simple: &Simple, machine: &mut impl Machine) -> Result<Prepared, Stop> {
        self.substituted = None;
        let words = self.words(&simple.words, machine)?;
        let redirections = self.redirections(&simple.redirections, machine)?;
        let mut assigned = Vec::new();
        let mut kept = Vec::new();
        for assignment in &simple.assignments {
            let value = match self.text(&assignment.value, machine) {
                Ok(value) => value,
                Err(stop) => {
                    self.restore(kept);
                    return Err(stop);
                }
            };
            let name = &assignment.name;
            kept.push((name.clone(), self.variables.get(name).cloned()));
            if self.assign(name, value.clone()).is_err() {
                self.restore(kept);
                return Err(failed(assignment.value.at, Code::ReadOnly));
            }
            assigned.push((name.clone(), value));
        }
        self.restore(kept);
        Ok(Prepared {
            ready: Ready {
                words,
                redirections,
            },
            assigned,
            at: simple.at,
            substituted: self.substituted,
        })
    }

    /// Whether the machine runs the command `prepared` names, not the
    /// session: not a function, nor one of the session's builtins.
    fn machines(&self, prepared: &Prepared) -> bool {
        prepared.ready.words.first().is_some_and(|name| {
            !SPECIAL.contains(&name.as_str()) && !self.functions.contains_key(name)
        })
    }

    /// Runs a simple command expanded: assignments alone, a special
    /// builtin, a function, or what the machine runs.
    fn prepared(&mut self, prepared: Prepared, machine: &mut impl Machine) -> Result<Flow, Stop> {
        let Prepared {
            ready,
            assigned,
            at,
            substituted,
        } = prepared;
        // prepare made each assignment once already, so none of them is to
        // a read-only variable.
        let Some(name) = ready.words.first().cloned() else {
            for (name, value) in assigned {
                let _ = self.assign(&name, value);
            }
            self.status = substituted.unwrap_or(0);
            if !ready.redirections.is_empty() {
                if machine.begin_redirections(&ready.redirections) {
                    machine.end_redirections();
                } else {
                    self.status = 1;
                }
            }
            return Ok(Flow::Next);
        };
        if SPECIAL.contains(&name.as_str()) {
            for (name, value) in assigned {
                let _ = self.assign(&name, value);
            }
            return self.around(&ready.redirections, machine, |session, machine| {
                session.special(&ready.words, at, machine)
            });
        }
        if let Some(function) = self.functions.get(&name).cloned() {
            return self.call(&function, &ready, assigned, at, machine);
        }
        // Meridian's programs get no environment yet, so the variables
        // assigned before one reach none of them.
        self.status = machine.run(&[ready]);
        Ok(Flow::Next)
    }

    /// Runs `run` with `redirections` made around it.
    fn around<M: Machine>(
        &mut self,
        redirections: &[Redirection],
        machine: &mut M,
        run: impl FnOnce(&mut Session, &mut M) -> Result<Flow, Stop>,
    ) -> Result<Flow, Stop> {
        if redirections.is_empty() {
            return run(self, machine);
        }
        if !machine.begin_redirections(redirections) {
            self.status = 1;
            return Ok(Flow::Next);
        }
        let flow = run(self, machine);
        machine.end_redirections();
        flow
    }

    /// Calls `function` with the words after its name as its parameters,
    /// the variables assigned before it set while it runs.
    fn call(
        &mut self,
        function: &Function,
        ready: &Ready,
        assigned: Vec<(String, String)>,
        at: Pos,
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        let mut kept = Vec::new();
        for (name, value) in assigned {
            kept.push((name.clone(), self.variables.get(&name).cloned()));
            let _ = self.assign(&name, value);
        }
        let outer = core::mem::replace(&mut self.positional, ready.words[1..].to_vec());
        let ran = self.deeper(at, machine, |session, machine| {
            session.around(&ready.redirections, machine, |session, machine| {
                let made = session.redirections(&function.redirections, machine)?;
                session.around(&made, machine, |session, machine| {
                    session.compound(&function.body, machine)
                })
            })
        });
        self.positional = outer;
        self.restore(kept);
        match ran? {
            Flow::Return | Flow::Next => Ok(Flow::Next),
            flow => Ok(flow),
        }
    }

    /// Runs `run` one deeper; refused at `at` when commands run
    /// [`DEEPEST_RUNNING`] deep already.
    fn deeper<M: Machine, T>(
        &mut self,
        at: Pos,
        machine: &mut M,
        run: impl FnOnce(&mut Session, &mut M) -> Result<T, Stop>,
    ) -> Result<T, Stop> {
        if self.depth >= DEEPEST_RUNNING {
            return Err(failed(at, Code::TooDeep));
        }
        self.depth += 1;
        let ran = run(self, machine);
        self.depth -= 1;
        ran
    }

    /// Puts the variables `kept` back as they were, the last kept first,
    /// so a name kept twice ends as it was the first time.
    fn restore(&mut self, kept: Vec<(String, Option<Variable>)>) {
        for (name, before) in kept.into_iter().rev() {
            match before {
                Some(variable) => self.variables.insert(name, variable),
                None => self.variables.remove(&name),
            };
        }
    }

    /// Runs `run` on a copy of the session, as a subshell, with the
    /// working directory set back after; answers its status. An error in
    /// it is said at `at` when there is one, the word a substitution
    /// stands in, as the text it ran has no place in the script.
    fn subshell<M: Machine>(
        &mut self,
        at: Option<Pos>,
        machine: &mut M,
        run: impl FnOnce(&mut Session, &mut M) -> Result<Flow, Stop>,
    ) -> u8 {
        let directory = machine.directory();
        let mut copy = self.clone();
        let ran = run(&mut copy, machine).map_err(|stop| match at {
            Some(at) => placed(stop, at),
            None => stop,
        });
        let status = copy.ended(ran, machine);
        machine.set_directory(&directory);
        status
    }

    /// What the commands `text` write, as `$(text)` stands for it: run in
    /// a subshell, its line ends at the end taken off.
    pub(super) fn substituted(
        &mut self,
        text: &str,
        at: Pos,
        machine: &mut impl Machine,
    ) -> Result<String, Stop> {
        let items = parse::script(text).map_err(|e| failed(at, e.code))?;
        let (status, said) = self.deeper(at, machine, |session, machine| {
            machine.begin_capture();
            let status =
                session.subshell(Some(at), machine, |copy, machine| copy.run(&items, machine));
            Ok((status, machine.end_capture()))
        })?;
        self.substituted = Some(status);
        let mut said = String::from_utf8_lossy(&said).into_owned();
        while said.ends_with('\n') {
            said.pop();
        }
        Ok(said)
    }

    /// Runs a loop's body, and answers whether the loop goes on and how it
    /// ends when not.
    fn body(&mut self, body: &[Item], machine: &mut impl Machine) -> Result<Option<Flow>, Stop> {
        Ok(looped(self.run(body, machine)?))
    }

    /// Runs `list` as a condition, which `set -e` leaves alone, and
    /// answers whether it held, or how it ended when it went out.
    fn holds(
        &mut self,
        list: &[Item],
        machine: &mut impl Machine,
    ) -> Result<Result<bool, Flow>, Stop> {
        self.conditions += 1;
        let flow = self.run(list, machine);
        self.conditions -= 1;
        Ok(match flow? {
            Flow::Next => Ok(self.status == 0),
            flow => Err(flow),
        })
    }

    fn compound(&mut self, compound: &Compound, machine: &mut impl Machine) -> Result<Flow, Stop> {
        self.depth += 1;
        let flow = self.compounded(compound, machine);
        self.depth -= 1;
        flow
    }

    fn compounded(
        &mut self,
        compound: &Compound,
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        match compound {
            Compound::Brace(list) => self.run(list, machine),
            Compound::Subshell(list) => {
                self.status = self.subshell(None, machine, |copy, machine| copy.run(list, machine));
                Ok(Flow::Next)
            }
            Compound::If {
                branches,
                otherwise,
            } => self.branches(branches, otherwise.as_deref(), machine),
            Compound::While {
                until,
                condition,
                body,
            } => {
                self.loops += 1;
                let flow = self.repeat(*until, condition, body, machine);
                self.loops -= 1;
                flow
            }
            Compound::For { name, words, body } => {
                let values = match words {
                    Some(words) => self.words(words, machine)?,
                    None => self.positional.clone(),
                };
                self.loops += 1;
                let flow = self.each(name, values, body, machine);
                self.loops -= 1;
                flow
            }
            Compound::Case { word, items } => self.case(word, items, machine),
        }
    }

    /// `if`: the body of the first condition that holds, or the `else`.
    fn branches(
        &mut self,
        branches: &[(Vec<Item>, Vec<Item>)],
        otherwise: Option<&[Item]>,
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        for (condition, body) in branches {
            match self.holds(condition, machine)? {
                Ok(true) => return self.run(body, machine),
                Ok(false) => {}
                Err(flow) => return Ok(flow),
            }
        }
        if let Some(otherwise) = otherwise {
            return self.run(otherwise, machine);
        }
        self.status = 0;
        Ok(Flow::Next)
    }

    /// `while`, or with `until` the other way round: the body as long as
    /// the condition holds, ending with the body's last status.
    fn repeat(
        &mut self,
        until: bool,
        condition: &[Item],
        body: &[Item],
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        let mut status = 0;
        let flow = loop {
            match self.holds(condition, machine)? {
                Ok(held) if held != until => {}
                Ok(_) => break Flow::Next,
                Err(flow) => match looped(flow) {
                    Some(flow) => break flow,
                    None => continue,
                },
            }
            match self.body(body, machine)? {
                None => status = self.status,
                Some(flow) => break flow,
            }
        };
        self.status = status;
        Ok(flow)
    }

    /// `for`: the body with `name` set to each of `values`.
    fn each(
        &mut self,
        name: &str,
        values: Vec<String>,
        body: &[Item],
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        self.status = 0;
        for value in values {
            if self.assign(name, value).is_err() {
                return Self::refused("for", &format!("{name}: read only"), machine);
            }
            if let Some(flow) = self.body(body, machine)? {
                return Ok(flow);
            }
        }
        Ok(Flow::Next)
    }

    /// `case`: the body of the first item a pattern of which matches the
    /// word, and with `;&` the bodies after it.
    fn case(
        &mut self,
        word: &Word,
        items: &[parse::CaseItem],
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        let word = self.text(word, machine)?;
        self.status = 0;
        let mut falling = false;
        for item in items {
            if !falling {
                let mut matched = false;
                for pattern in &item.patterns {
                    if self.pattern(pattern, machine)?.matches(&word) {
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    continue;
                }
            }
            let flow = self.run(&item.body, machine)?;
            if flow != Flow::Next || !item.falls {
                return Ok(flow);
            }
            falling = true;
        }
        Ok(Flow::Next)
    }

    /// Says `said` went wrong, as the special builtin `name` says it, and
    /// ends the script, as an error of a special builtin does.
    fn refused(name: &str, said: &str, machine: &mut impl Machine) -> Result<Flow, Stop> {
        let _ = machine.err(format!("{name}: {said}\n").as_bytes());
        Err(Stop::Said(1))
    }

    /// Runs the special builtin `words` names.
    fn special(
        &mut self,
        words: &[String],
        at: Pos,
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        let before = self.status;
        let (name, rest) = words
            .split_first()
            .map_or(("", &[][..]), |(n, r)| (n.as_str(), r));
        self.status = 0;
        match name {
            ":" => Ok(Flow::Next),
            "break" | "continue" => {
                let n = match count(rest, 1, 1) {
                    Ok(n) => n,
                    Err(said) => return Self::refused(name, &said, machine),
                };
                if self.loops == 0 {
                    return Ok(Flow::Next);
                }
                let n = n.min(self.loops);
                Ok(if name == "break" {
                    Flow::Break(n)
                } else {
                    Flow::Continue(n)
                })
            }
            "return" | "exit" => {
                let status = match rest {
                    [] => before,
                    [n] => match n.parse::<i64>() {
                        Ok(n) => n.to_le_bytes()[0],
                        Err(_) => {
                            return Self::refused(
                                name,
                                &format!("{n}: not a whole number"),
                                machine,
                            );
                        }
                    },
                    _ => return Self::refused(name, "takes one number at most", machine),
                };
                self.status = status;
                if name == "exit" {
                    Err(Stop::Exit(status))
                } else {
                    Ok(Flow::Return)
                }
            }
            "eval" => {
                let text = rest.join(" ");
                let items = parse::script(&text).map_err(|e| failed(at, e.code))?;
                self.deeper(at, machine, |session, machine| session.run(&items, machine))
                    .map_err(|stop| placed(stop, at))
            }
            "export" | "readonly" => self.mark(name, rest, machine),
            "unset" => self.unset(rest, machine),
            "set" => self.set(rest, machine),
            _ => {
                let n = match count(rest, 1, 0) {
                    Ok(n) => n,
                    Err(said) => return Self::refused(name, &said, machine),
                };
                if n > self.positional.len() {
                    let said = format!(
                        "{n}: more than the {} parameters there are",
                        self.positional.len()
                    );
                    return Self::refused(name, &said, machine);
                }
                self.positional.drain(..n);
                Ok(Flow::Next)
            }
        }
    }

    /// `export` or `readonly`: marks each name, giving it the value after
    /// an `=`; with `-p` or nothing, says those marked so.
    fn mark(
        &mut self,
        name: &str,
        rest: &[String],
        machine: &mut impl Machine,
    ) -> Result<Flow, Stop> {
        let exporting = name == "export";
        let rest: &[String] = match rest.split_first() {
            Some((p, rest)) if p == "-p" => rest,
            _ => rest,
        };
        if rest.is_empty() {
            let said: String = self
                .variables
                .iter()
                .filter(|(_, v)| if exporting { v.exported } else { v.readonly })
                .map(|(var, variable)| match &variable.value {
                    Some(value) => format!("{name} {var}={}\n", quoted(value)),
                    None => format!("{name} {var}\n"),
                })
                .collect();
            if !machine.out(said.as_bytes()) {
                self.status = 1;
            }
            return Ok(Flow::Next);
        }
        for word in rest {
            let (var, value) = match word.split_once('=') {
                Some((var, value)) => (var, Some(value)),
                None => (word.as_str(), None),
            };
            if !is_name(var) {
                return Self::refused(name, &format!("{var}: not a name"), machine);
            }
            if let Some(value) = value
                && self.assign(var, value.into()).is_err()
            {
                return Self::refused(name, &format!("{var}: read only"), machine);
            }
            let variable = self.variables.entry(var.into()).or_default();
            if exporting {
                variable.exported = true;
            } else {
                variable.readonly = true;
            }
        }
        Ok(Flow::Next)
    }

    /// `unset`: the variables named, or with `-f` the functions.
    fn unset(&mut self, rest: &[String], machine: &mut impl Machine) -> Result<Flow, Stop> {
        let (functions, names) = match rest.split_first() {
            Some((f, names)) if f == "-f" => (true, names),
            Some((v, names)) if v == "-v" => (false, names),
            _ => (false, rest),
        };
        for name in names {
            if functions {
                self.functions.remove(name);
                continue;
            }
            if self.variables.get(name).is_some_and(|v| v.readonly) {
                return Self::refused("unset", &format!("{name}: read only"), machine);
            }
            self.variables.remove(name);
        }
        Ok(Flow::Next)
    }

    /// `set`: options on with `-` and off with `+`, and the positional
    /// parameters after them or after `--`; alone, says every variable.
    fn set(&mut self, rest: &[String], machine: &mut impl Machine) -> Result<Flow, Stop> {
        if rest.is_empty() {
            let said: String = self
                .variables
                .iter()
                .filter_map(|(name, v)| Some(format!("{name}={}\n", quoted(v.value.as_ref()?))))
                .collect();
            if !machine.out(said.as_bytes()) {
                self.status = 1;
            }
            return Ok(Flow::Next);
        }
        let mut words = rest.iter();
        let mut parameters = None;
        while let Some(word) = words.next() {
            if word == "--" {
                parameters = Some(words.by_ref().cloned().collect());
                break;
            }
            let (on, letters) = match word.as_bytes().first() {
                Some(b'-') => (true, &word[1..]),
                Some(b'+') => (false, &word[1..]),
                _ => {
                    let mut all = Vec::from([word.clone()]);
                    all.extend(words.by_ref().cloned());
                    parameters = Some(all);
                    break;
                }
            };
            for letter in letters.chars() {
                if !"efuC".contains(letter) {
                    return Self::refused("set", &format!("-{letter}: no such option"), machine);
                }
                if on {
                    self.options.insert(letter);
                } else {
                    self.options.remove(&letter);
                }
            }
        }
        if let Some(parameters) = parameters {
            self.positional = parameters;
        }
        Ok(Flow::Next)
    }
}

/// `stop` with the error in it, if any, said at `at`: commands run from a
/// text, which has no place in the script, are said at the word or the
/// command that ran them.
fn placed(stop: Stop, at: Pos) -> Stop {
    match stop {
        Stop::Error(e) => failed(at, e.code),
        stop => stop,
    }
}

/// How a loop goes on after its body or its condition ended with `flow`:
/// none to go round again, or how the loop itself ends.
fn looped(flow: Flow) -> Option<Flow> {
    match flow {
        Flow::Next | Flow::Continue(1) => None,
        Flow::Break(1) => Some(Flow::Next),
        Flow::Break(n) => Some(Flow::Break(n - 1)),
        Flow::Continue(n) => Some(Flow::Continue(n - 1)),
        Flow::Return => Some(Flow::Return),
    }
}

/// The count the words after a builtin give: `default` without one, and
/// an error for one that is no whole number of at least `least`, or for
/// more than one.
fn count(rest: &[String], default: usize, least: usize) -> Result<usize, String> {
    match rest {
        [] => Ok(default),
        [n] => n
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= least)
            .ok_or_else(|| format!("{n}: not a whole number of {least} or more")),
        _ => Err(String::from("takes one number at most")),
    }
}

/// A simple command expanded: what the machine would run, the variables
/// assigned before its name, where it starts, and the status of its last
/// command substitution.
#[derive(Clone, Debug)]
struct Prepared {
    ready: Ready,
    assigned: Vec<(String, String)>,
    at: Pos,
    substituted: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh::desk::{out, ran, said, stopped};
    use alloc::vec;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| String::from(*w)).collect()
    }

    #[test]
    fn a_command_runs_on_the_machine_with_its_words() {
        let (ended, session, desk) = ran("echo a   'b  c'", &[]);
        assert_eq!(ended, Ok(Flow::Next));
        assert_eq!(desk.out, b"a b  c\n");
        assert_eq!(desk.ran, [words(&["echo", "a", "b  c"])]);
        assert_eq!(session.status(), 0);
    }

    #[test]
    fn a_program_that_is_not_there_ends_with_127() {
        assert_eq!(
            said("nope x; echo $?", &[]),
            ("127\n".into(), "nope: not found\n".into())
        );
    }

    #[test]
    fn the_status_is_the_last_command_s() {
        assert_eq!(out("false; echo $?; true; echo $?"), "1\n0\n");
        assert_eq!(ran("false", &[]).1.status(), 1);
        assert_eq!(ran("false; true", &[]).1.status(), 0);
    }

    #[test]
    fn a_bang_turns_the_status_round() {
        assert_eq!(out("! true; echo $?; ! false; echo $?"), "1\n0\n");
        assert_eq!(out("! nope 2>&1; echo $?"), "nope: not found\n0\n");
    }

    #[test]
    fn and_and_or_run_what_follows_by_the_status() {
        assert_eq!(
            out("false && echo a; true && echo b; false || echo c; true || echo d"),
            "b\nc\n"
        );
        assert_eq!(out("false && echo x || echo y"), "y\n");
        assert_eq!(out("true || echo x && echo y"), "y\n");
        assert_eq!(out("false || false || echo z"), "z\n");
        assert_eq!(out("true && false && echo no; echo $?"), "1\n");
    }

    #[test]
    fn set_e_ends_the_script_at_a_command_that_fails() {
        let (ended, _, desk) = ran("set -e; echo a; false; echo b", &[]);
        assert_eq!((ended, desk.out), (Err(Stop::Exit(1)), b"a\n".to_vec()));
        let (ended, _, desk) = ran("set -e; true && nope; echo b", &[]);
        assert_eq!((ended, desk.out), (Err(Stop::Exit(127)), Vec::new()));
        assert_eq!(ran("set -e; (false); echo b", &[]).0, Err(Stop::Exit(1)));
        assert_eq!(ran("set -e; { false; }; echo b", &[]).0, Err(Stop::Exit(1)));
        assert_eq!(ran("set -e; echo a | false", &[]).0, Err(Stop::Exit(1)));
    }

    #[test]
    fn set_e_leaves_conditions_alone() {
        let script = "set -e; false || true; false && true; ! true; \
                      if false; then :; fi; while false; do :; done; \
                      until true; do :; done; echo ok";
        let (ended, _, desk) = ran(script, &[]);
        assert_eq!((ended, desk.out), (Ok(Flow::Next), b"ok\n".to_vec()));
        assert_eq!(
            out("set -e; f() { false; echo in; }; if f; then echo yes; fi; echo end"),
            "in\nyes\nend\n",
            "a function called as a condition is one all through"
        );
        assert_eq!(
            ran("set -e; f() { false; echo in; }; f; echo end", &[])
                .2
                .out,
            b"",
            "called as no condition, it is none"
        );
        assert_eq!(out("set -e; set +e; false; echo on"), "on\n");
    }

    #[test]
    fn set_e_holds_again_after_a_condition() {
        for script in [
            "set -e; if true; then :; fi; false; echo no",
            "set -e; while false; do :; done; false; echo no",
            "set -e; false || true; false; echo no",
            "set -e; ! true; false; echo no",
        ] {
            let (ended, _, desk) = ran(script, &[]);
            assert_eq!(
                (ended, desk.out),
                (Err(Stop::Exit(1)), Vec::new()),
                "{script}"
            );
        }
    }

    #[test]
    fn a_pipeline_of_commands_the_machine_runs_runs_on_it_whole() {
        let (_, session, desk) = ran("echo a | echo b", &[]);
        assert_eq!(desk.pipelines, [2]);
        assert_eq!(desk.out, b"b\n");
        assert_eq!(desk.fed, [b"a\n".to_vec()]);
        assert_eq!(desk.ran, [words(&["echo", "a"]), words(&["echo", "b"])]);
        assert_eq!(session.status(), 0);
        assert_eq!(out("echo a | false; echo $?"), "1\n");
        assert_eq!(out("false | true; echo $?"), "0\n");
        assert_eq!(out("! echo a | false; echo $?"), "0\n");
    }

    #[test]
    fn a_pipeline_with_a_function_or_a_builtin_runs_in_order() {
        let (_, _, desk) = ran("f() { echo x; }; f | echo y", &[]);
        assert_eq!(desk.pipelines, [1, 1]);
        assert_eq!(desk.out, b"y\n");
        assert_eq!(desk.fed, [b"x\n".to_vec()]);
        let (_, _, desk) = ran("echo a | : | echo b", &[]);
        assert_eq!(desk.pipelines, [1, 1]);
        assert_eq!(desk.out, b"b\n");
        assert_eq!(desk.fed, [b"a\n".to_vec(), Vec::new()]);
        assert_eq!(out("echo a | { echo b; echo c; }"), "b\nc\n");
        assert_eq!(out("f() { return 3; }; echo a | f; echo $?"), "3\n");
    }

    #[test]
    fn each_stage_of_a_pipeline_keeps_what_it_sets() {
        assert_eq!(out("a=1; a=2 | echo $a; echo $a"), "1\n1\n");
        assert_eq!(out("f() { b=2; }; f | echo; echo \"[$b]\""), "\n[]\n");
        assert_eq!(out("echo $((c = 5)) | echo; echo \"[$c]\""), "\n[]\n");
        assert_eq!(out("set -- a b | echo; echo $#"), "\n0\n");
    }

    #[test]
    fn what_stops_a_stage_ends_only_it() {
        assert_eq!(
            said("echo ${a?gone} | echo b; echo $?", &[]),
            ("b\n0\n".into(), "a: gone\n".into())
        );
        assert_eq!(out("exit 3 | echo b; echo $?"), "b\n0\n");
        let (ended, _, desk) = ran("echo a | exit 3; echo $?", &[]);
        assert_eq!((ended, desk.out), (Ok(Flow::Next), b"3\n".to_vec()));
        let (_, _, desk) = ran("echo $((1 / 0)) | echo b", &[]);
        assert_eq!(desk.out, b"b\n");
        assert_eq!(
            desk.err,
            b"1:6: division-by-zero: divide by something other than 0\n"
        );
        assert_eq!(out("echo a | echo $((1 / 0)) 2>&-; echo $?"), "2\n");
    }

    #[test]
    fn a_subshell_keeps_what_it_changes() {
        assert_eq!(out("a=1; (a=2; echo $a); echo $a"), "2\n1\n");
        assert_eq!(out("(exit 4); echo $?"), "4\n");
        assert_eq!(out("cd /x; (cd /y; pwd); pwd"), "/y\n/x\n");
        assert_eq!(
            said("(f() { echo f; }); f", &[]),
            (String::new(), "f: not found\n".into())
        );
        assert_eq!(out("set -- a; (shift; echo $#); echo $#"), "0\n1\n");
        assert_eq!(out("(set -e; false; echo no); echo $?"), "1\n");
    }

    #[test]
    fn a_brace_group_runs_in_the_session_itself() {
        assert_eq!(out("{ a=1; echo $a; }; echo $a"), "1\n1\n");
        assert_eq!(out("{ cd /z; }; pwd"), "/z\n");
        assert_eq!(out("{ false; }; echo $?"), "1\n");
    }

    #[test]
    fn a_compound_command_s_redirections_hold_for_all_inside() {
        assert_eq!(
            said("{ echo a; echo b >&2; } >&2", &[]),
            (String::new(), "a\nb\n".into())
        );
        assert_eq!(
            said("{ echo a >&2; } 2>&1", &[]),
            ("a\n".into(), String::new())
        );
        assert_eq!(
            said("{ echo a; echo $? >&2; } >&-", &[]),
            (String::new(), "1\n".into())
        );
        assert_eq!(
            said("for i in 1; do echo $i; done >&2; echo b", &[]),
            ("b\n".into(), "1\n".into())
        );
        assert_eq!(
            said("{ echo a >&2; } 2>&1 >&-", &[]),
            ("a\n".into(), String::new()),
            "2>&1 takes stream 1 as it is before >&- closes it"
        );
        assert_eq!(said("(echo a) >&2", &[]), (String::new(), "a\n".into()));
    }

    #[test]
    fn redirections_that_cannot_be_made_run_nothing_and_fail() {
        assert_eq!(
            said("{ echo a; } > f; echo $?", &[]),
            ("1\n".into(), "f: no file here to write\n".into())
        );
        assert_eq!(
            said("f() { echo a; }; f > g; echo $?", &[]),
            ("1\n".into(), "g: no file here to write\n".into())
        );
        assert_eq!(
            said("f() { echo a; } > g; f; echo $?", &[]),
            ("1\n".into(), "g: no file here to write\n".into())
        );
        assert_eq!(
            said(": 3>&1; echo $?", &[]),
            ("1\n".into(), "3: there is no such stream here\n".into())
        );
        assert_eq!(
            said("a=1 > f; echo $? $a", &[]),
            ("1 1\n".into(), "f: no file here to write\n".into())
        );
    }

    #[test]
    fn each_redirection_asks_for_what_its_operator_says() {
        for (script, said_) in [
            (": < f", "f: no file here to read\n"),
            (": > f", "f: no file here to write\n"),
            (": >| f", "f: no file here to write\n"),
            (": >> f", "f: no file here to add to\n"),
            (": <> f", "f: no file here to read and write\n"),
            ("set -C; : > f", "f: no file here to make\n"),
            ("set -C; : >| f", "f: no file here to write\n"),
            ("set -C; : >> f", "f: no file here to add to\n"),
            ("set -C; set +C; : > f", "f: no file here to write\n"),
        ] {
            assert_eq!(said(script, &[]).1, said_, "{script}");
        }
    }

    #[test]
    fn a_redirection_s_word_is_expanded_as_one_text() {
        assert_eq!(
            said("a='x y'; : > $a.txt", &[]).1,
            "x y.txt: no file here to write\n"
        );
        assert_eq!(
            said("set -- 3 4; : > \"$@\"", &[]).1,
            "3 4: no file here to write\n"
        );
    }

    #[test]
    fn a_copy_wants_a_stream_s_number_or_a_minus() {
        assert_eq!(stopped("echo a >&x"), (1, 10, Code::BadCopy));
        assert_eq!(stopped("echo a <&1x"), (1, 10, Code::BadCopy));
        assert_eq!(stopped("{ :; } >&-1"), (1, 10, Code::BadCopy));
        assert_eq!(
            said("echo a >&-; echo $? >&2", &[]),
            (String::new(), "1\n".into())
        );
        assert_eq!(said("n=2; echo a >&$n", &[]), (String::new(), "a\n".into()));
    }

    #[test]
    fn a_here_document_is_the_text_its_command_reads() {
        let (_, _, desk) = ran("a=1\n: <<EOF\nhello $a\n  $((a + 1))\nEOF\n", &[]);
        assert_eq!(desk.fed, [b"hello 1\n  2\n".to_vec()]);
        let (_, _, desk) = ran("a=1\n: <<'EOF'\nhello $a\nEOF\n", &[]);
        assert_eq!(desk.fed, [b"hello $a\n".to_vec()]);
        let (_, _, desk) = ran("echo x <<EOF | echo y\nin\nEOF\n", &[]);
        assert_eq!(desk.fed, [b"in\n".to_vec(), b"x\n".to_vec()]);
        let (_, _, desk) = ran("{ :; } <<EOF\nbraced\nEOF\n", &[]);
        assert_eq!(desk.fed, [b"braced\n".to_vec()]);
    }

    #[test]
    fn a_here_document_expands_as_inside_double_quotes() {
        let (_, _, desk) = ran("IFS=:; set -- a b\n: <<EOF\n$* $@ ~ \"q\" 'r'\nEOF\n", &[]);
        assert_eq!(desk.fed, [b"a:b a b ~ \"q\" 'r'\n".to_vec()]);
    }

    #[test]
    fn a_function_gets_its_words_as_its_parameters() {
        assert_eq!(
            said("f() { echo \"$1|$2|$#\"; }; f a b; echo \"$1 $#\"", &["p"]).0,
            "a|b|2\np 1\n"
        );
        assert_eq!(out("f() { echo $#; }; f; f ''; f a ''"), "0\n1\n2\n");
        assert_eq!(out("f() { shift; echo $1; }; f a b; echo $1"), "b\n\n");
    }

    #[test]
    fn a_function_is_defined_when_its_definition_runs() {
        assert_eq!(
            said("f; f() { echo in; }; f", &[]),
            ("in\n".into(), "f: not found\n".into())
        );
        assert_eq!(out("f() { echo 1; }; f() { echo 2; }; f"), "2\n");
        assert_eq!(out("false; f() { :; }; echo $?"), "0\n");
        assert_eq!(said("f() { :; }; unset -f f; f", &[]).1, "f: not found\n");
        assert_eq!(
            out("f() { :; }; unset f; f; echo $?"),
            "0\n",
            "unset alone is variables"
        );
    }

    #[test]
    fn return_ends_a_function_with_a_status() {
        assert_eq!(out("f() { return 3; echo no; }; f; echo $?"), "3\n");
        assert_eq!(out("f() { false; return; }; f; echo $?"), "1\n");
        assert_eq!(out("f() { return 257; }; f; echo $?"), "1\n");
        assert_eq!(out("f() { return -1; }; f; echo $?"), "255\n");
        assert_eq!(
            out("f() { for i in 1 2; do return 4; done; echo no; }; f; echo $?"),
            "4\n"
        );
        assert_eq!(out("f() { (return 5); echo $?; }; f"), "5\n");
    }

    #[test]
    fn return_and_exit_say_what_is_wrong_with_their_number() {
        let (ended, _, desk) = ran("f() { return x; }; f; echo no", &[]);
        assert_eq!(ended, Err(Stop::Said(1)));
        assert_eq!(desk.err, b"return: x: not a whole number\n");
        assert_eq!(said("exit 1 2", &[]).1, "exit: takes one number at most\n");
        assert_eq!(ran("exit 1 2", &[]).0, Err(Stop::Said(1)));
    }

    #[test]
    fn exit_ends_the_script_with_its_status() {
        assert_eq!(ran("echo a; exit 3; echo b", &[]).0, Err(Stop::Exit(3)));
        assert_eq!(ran("false; exit", &[]).0, Err(Stop::Exit(1)));
        assert_eq!(ran("exit 300", &[]).0, Err(Stop::Exit(44)));
        assert_eq!(ran("exit -1", &[]).0, Err(Stop::Exit(255)));
        assert_eq!(
            ran("f() { exit 2; }; for i in 1; do { f; }; done", &[]).0,
            Err(Stop::Exit(2))
        );
        assert_eq!(ran("exit 7", &[]).1.status(), 7);
    }

    #[test]
    fn return_outside_a_function_ends_the_script() {
        let (ended, _, desk) = ran("echo a; return; echo b", &[]);
        assert_eq!((ended, desk.out), (Ok(Flow::Return), b"a\n".to_vec()));
    }

    #[test]
    fn variables_assigned_before_a_function_hold_while_it_runs() {
        assert_eq!(out("f() { echo $a; }; a=0; a=1 f; echo $a"), "1\n0\n");
        assert_eq!(out("f() { echo $b; }; b=5 f; echo \"[$b]\""), "5\n[]\n");
        assert_eq!(
            out("f() { echo $a; }; a=0; a=1 a=2 f; echo $a"),
            "2\n0\n",
            "a name assigned twice ends as it was before both"
        );
        assert_eq!(
            out("f() { a=9; }; a=0; a=1 f; echo $a"),
            "0\n",
            "what the function set goes with the assignment"
        );
    }

    #[test]
    fn assignments_are_made_in_their_order() {
        assert_eq!(out("a=1 b=$a; echo $b"), "1\n");
        assert_eq!(out("f() { echo $b; }; a=0; a=1 b=$a f; echo $a"), "1\n0\n");
        assert_eq!(out("a=1 a=2; echo $a"), "2\n");
        assert_eq!(out("a=0; a=1 b=$a echo x; echo $a \"[$b]\""), "x\n0 []\n");
    }

    #[test]
    fn variables_assigned_before_a_program_reach_nothing() {
        assert_eq!(out("a=1 echo x; echo \"[$a]\""), "x\n[]\n");
        assert_eq!(ran("a=1 echo x", &[]).2.ran, [words(&["echo", "x"])]);
    }

    #[test]
    fn variables_assigned_before_a_special_builtin_stay() {
        assert_eq!(out("a=1 :; echo $a"), "1\n");
        assert_eq!(out("a=1 eval 'echo $a'; echo $a"), "1\n1\n");
    }

    #[test]
    fn a_read_only_variable_keeps_its_value() {
        assert_eq!(stopped("readonly r=1; r=2"), (1, 15, Code::ReadOnly));
        assert_eq!(stopped("readonly r=1; r=2 echo x"), (1, 15, Code::ReadOnly));
        assert_eq!(stopped("readonly r; a=1 r=2 :"), (1, 17, Code::ReadOnly));
        let (_, session, desk) = ran("a=0; readonly r; a=1 r=2 echo x", &[]);
        assert_eq!(desk.ran, Vec::<Vec<String>>::new(), "nothing ran");
        assert_eq!(session.variable("a").as_deref(), Some("0"));
        assert_eq!(
            said("readonly r=1; for r in 2; do :; done", &[]).1,
            "for: r: read only\n"
        );
        assert_eq!(
            ran("readonly r=1; for r in 2; do :; done", &[]).0,
            Err(Stop::Said(1))
        );
        assert_eq!(out("readonly r=1; echo $((r = 2)) $r"), "2 1\n");
    }

    #[test]
    fn functions_call_each_other_only_so_deep() {
        // Each call here stands in the function's braces and its if, so
        // the 22nd runs 63 deep and the 23rd would run 66 deep.
        assert_eq!(
            out("n=0; f() { n=$((n + 1)); if [ $n -lt 22 ]; then f; fi; }; f; echo $n"),
            "22\n"
        );
        assert_eq!(
            stopped("n=0; f() { n=$((n + 1)); if [ $n -lt 23 ]; then f; fi; }; f"),
            (1, 49, Code::TooDeep)
        );
        let (ended, session, _) = ran("n=0; f() { n=$((n + 1)); f; }\nf", &[]);
        let Err(Stop::Error(e)) = ended else {
            panic!("{ended:?}");
        };
        assert_eq!(
            (e.at, e.code),
            (
                Pos {
                    line: 1,
                    column: 26
                },
                Code::TooDeep
            )
        );
        assert_eq!(
            session.variable("n").as_deref(),
            Some("32"),
            "the 33rd ran 64 deep"
        );
        assert_eq!(
            out("n=0; f() { n=$((n + 1)); if [ $n -lt 22 ]; then f; fi; }; f; n=0; f; echo $n"),
            "22\n",
            "the calls that ended count no more"
        );
    }

    #[test]
    fn eval_and_substitutions_count_as_deep_as_calls() {
        let (ended, session, _) = ran("n=0; e='n=$((n + 1)); eval \"$e\"'; eval \"$e\"", &[]);
        let Err(Stop::Error(e)) = ended else {
            panic!("{ended:?}");
        };
        assert_eq!(
            (e.at, e.code),
            (
                Pos {
                    line: 1,
                    column: 35
                },
                Code::TooDeep
            ),
            "said at the outermost eval"
        );
        assert_eq!(session.variable("n").as_deref(), Some("64"));
        let (ended, _, desk) = ran("s='echo \"[$(eval \"$s\")]\"'; eval \"$s\"; echo on", &[]);
        assert_eq!(
            ended,
            Ok(Flow::Next),
            "the error ends only the substitution it is in"
        );
        let err = String::from_utf8(desk.err).unwrap();
        assert!(err.ends_with(": too-deep: commands run inside each other too deep, as a function calling itself without end does\n"), "{err}");
        assert_eq!(err.lines().count(), 1);
        assert!(String::from_utf8(desk.out).unwrap().ends_with("]]]\non\n"));
        assert_eq!(
            out("eval 'eval \"eval echo deep\"'; f() { echo $(echo $(echo in)); }; f"),
            "deep\nin\n"
        );
    }

    #[test]
    fn for_sets_its_name_to_each_word() {
        assert_eq!(out("for i in a b 'c d'; do echo $i; done"), "a\nb\nc d\n");
        assert_eq!(said("for i; do echo $i; done", &["x", "y z"]).0, "x\ny z\n");
        assert_eq!(out("v='1 2'; for i in $v 3; do echo $i; done"), "1\n2\n3\n");
        assert_eq!(out("for i in a b; do :; done; echo $i"), "b\n");
        assert_eq!(out("false; for i in; do :; done; echo $?"), "0\n");
        assert_eq!(out("for i in a; do false; done; echo $?"), "1\n");
    }

    #[test]
    fn while_and_until_loop_as_their_condition_says() {
        assert_eq!(
            out("i=0; while [ $i -lt 3 ]; do echo $i; i=$((i + 1)); done"),
            "0\n1\n2\n"
        );
        assert_eq!(
            out("i=0; until [ $i -ge 2 ]; do i=$((i + 1)); done; echo $i"),
            "2\n"
        );
        assert_eq!(out("false; while false; do :; done; echo $?"), "0\n");
        assert_eq!(
            out("i=0; while [ $i -lt 1 ]; do i=1; false; done; echo $?"),
            "1\n"
        );
        assert_eq!(out("until false; do echo once; break; done"), "once\n");
    }

    #[test]
    fn break_and_continue_leave_as_many_loops_as_asked() {
        assert_eq!(
            out(
                "for a in 1 2; do for b in x y; do if [ $b = y ]; then continue 2; fi; \
                 echo $a$b; done; echo never; done"
            ),
            "1x\n2x\n"
        );
        assert_eq!(
            out("for a in 1 2; do for b in x y; do echo $a$b; break 2; done; done; echo end"),
            "1x\nend\n"
        );
        assert_eq!(out("for a in 1 2; do break 9; done; echo $a"), "1\n");
        assert_eq!(
            out("for a in 1 2; do continue 9; echo no; done; echo $a"),
            "2\n"
        );
        assert_eq!(out("break; continue; echo on"), "on\n");
        assert_eq!(out("for i in 1; do :; done; break; echo on"), "on\n");
        assert_eq!(out("while false; do :; done; continue; echo on"), "on\n");
        assert_eq!(
            out("for i in 1 2; do for j in 1; do :; done; break 2; done; echo $i"),
            "1\n",
            "a loop that ended leaves the count of those around it"
        );
        assert_eq!(
            out("for a in 1 2 3; do for b in x; do continue 2; done; echo no; done; echo $a"),
            "3\n"
        );
        assert_eq!(
            out(
                "for a in 1 2; do for b in x y z; do for c in q; do break 2; done; echo no; \
                 done; echo $a$b; done"
            ),
            "1x\n2x\n"
        );
    }

    #[test]
    fn break_in_a_loop_s_condition_leaves_that_loop() {
        assert_eq!(out("while break; do echo no; done; echo end"), "end\n");
        assert_eq!(
            out("for a in 1 2; do while break 2; do :; done; echo no; done; echo $a"),
            "1\n"
        );
        assert_eq!(
            out("for a in 1 2; do until continue 2; do :; done; echo no; done; echo $a"),
            "2\n"
        );
        assert_eq!(
            out(
                "i=0; while i=$((i + 1)); [ $i -lt 3 ] || break; [ $i != 1 ] || continue; \
                 do echo $i; done; echo end $i"
            ),
            "2\nend 3\n"
        );
    }

    #[test]
    fn break_in_a_function_leaves_the_loop_it_was_called_in() {
        assert_eq!(
            out("f() { break; }; for i in 1 2 3; do echo $i; f; done; echo end"),
            "1\nend\n"
        );
    }

    #[test]
    fn break_and_continue_say_what_is_wrong_with_their_number() {
        for (script, said_) in [
            (
                "for a in 1; do break 0; done",
                "break: 0: not a whole number of 1 or more\n",
            ),
            (
                "for a in 1; do continue x; done",
                "continue: x: not a whole number of 1 or more\n",
            ),
            (
                "for a in 1; do break 1 2; done",
                "break: takes one number at most\n",
            ),
            ("break -1", "break: -1: not a whole number of 1 or more\n"),
        ] {
            let (ended, _, desk) = ran(script, &[]);
            assert_eq!(ended, Err(Stop::Said(1)), "{script}");
            assert_eq!(desk.err, said_.as_bytes(), "{script}");
        }
    }

    #[test]
    fn if_runs_the_first_branch_whose_condition_holds() {
        assert_eq!(
            out("if false; then echo a; elif true; then echo b; else echo c; fi"),
            "b\n"
        );
        assert_eq!(
            out("if false; then echo a; elif false; then echo b; else echo c; fi"),
            "c\n"
        );
        assert_eq!(
            out("if true; then echo a; elif true; then echo b; fi"),
            "a\n"
        );
        assert_eq!(out("if false; then :; fi; echo $?"), "0\n");
        assert_eq!(out("if false; then :; else echo $?; fi"), "1\n");
        assert_eq!(out("if true; then false; fi; echo $?"), "1\n");
        assert_eq!(
            out("for i in 1 2; do if break; then echo no; fi; done; echo $i"),
            "1\n"
        );
    }

    #[test]
    fn case_runs_the_first_item_whose_pattern_matches() {
        assert_eq!(out("case abc in a*) echo 1;; *) echo 2;; esac"), "1\n");
        assert_eq!(out("case b in a|b) echo ab;; b) echo b;; esac"), "ab\n");
        assert_eq!(out("false; case x in y) echo y;; esac; echo $?"), "0\n");
        assert_eq!(out("case x in x) false;; esac; echo $?"), "1\n");
        assert_eq!(
            out("case a in a) echo 1;& b) echo 2;; c) echo 3;; esac"),
            "1\n2\n"
        );
        assert_eq!(
            out("case a in a) echo 1;& b) echo 2;& c) echo 3;; esac"),
            "1\n2\n3\n"
        );
        assert_eq!(out("case z in a) echo 1;& z) echo 2;& esac"), "2\n");
        assert_eq!(
            out("for i in 1 2; do case x in x) break;& y) echo no;; esac; done; echo $i"),
            "1\n"
        );
    }

    #[test]
    fn a_case_pattern_s_quoted_characters_stand_for_themselves() {
        assert_eq!(out("case '*' in \"*\") echo star;; esac"), "star\n");
        assert_eq!(
            out("case a in \"*\") echo no;; *) echo yes;; esac"),
            "yes\n"
        );
        assert_eq!(out("p='a*'; case abc in $p) echo yes;; esac"), "yes\n");
        assert_eq!(
            out("p='a*'; case abc in \"$p\") echo no;; *) echo lit;; esac"),
            "lit\n"
        );
        assert_eq!(
            out("w='a b'; case $w in 'a b') echo whole;; esac"),
            "whole\n"
        );
        assert_eq!(
            out("case ab in a\\*) echo no;; a?) echo yes;; esac"),
            "yes\n"
        );
    }

    #[test]
    fn shift_drops_parameters_from_the_front() {
        assert_eq!(
            out("set -- x y z; shift; echo $@; shift 2; echo $#"),
            "y z\n0\n"
        );
        assert_eq!(out("set -- x; shift 0; echo $1"), "x\n");
        let (ended, _, desk) = ran("shift 5; echo no", &[]);
        assert_eq!(ended, Err(Stop::Said(1)));
        assert_eq!(
            desk.err,
            b"shift: 5: more than the 0 parameters there are\n"
        );
        assert_eq!(
            said("shift", &[]).1,
            "shift: 1: more than the 0 parameters there are\n"
        );
        assert_eq!(
            said("shift x", &["a"]).1,
            "shift: x: not a whole number of 0 or more\n"
        );
        assert_eq!(said("shift 2; echo $#", &["a", "b"]).0, "0\n");
    }

    #[test]
    fn eval_runs_its_words_as_a_script() {
        assert_eq!(out("eval 'a=1; echo $a'"), "1\n");
        assert_eq!(out("b=2; eval echo '$b'"), "2\n");
        assert_eq!(out("eval 'false'; echo $?"), "1\n");
        assert_eq!(out("eval; echo $?"), "0\n");
        assert_eq!(out("for i in 1 2; do eval break; done; echo $i"), "1\n");
        assert_eq!(stopped("echo a\n  eval 'fi'"), (2, 3, Code::Unexpected));
        assert_eq!(
            stopped("echo a\n  eval 'echo ${a;b}'"),
            (2, 3, Code::BadSubstitution)
        );
        assert_eq!(
            stopped("echo a\n  eval 'eval \"echo ${a;b}\"'"),
            (2, 3, Code::BadSubstitution)
        );
        assert_eq!(ran("eval 'exit 4'", &[]).0, Err(Stop::Exit(4)));
        assert_eq!(out("f() { eval 'return 5'; echo no; }; f; echo $?"), "5\n");
    }

    #[test]
    fn export_marks_variables_and_says_them() {
        assert_eq!(out("export a=1 b; export -p"), "export a='1'\nexport b\n");
        assert_eq!(out("export q=\"it's\"; export"), "export q='it'\\''s'\n");
        assert_eq!(
            out("a=1; export a; export -p; echo $a"),
            "export a='1'\n1\n"
        );
        assert_eq!(out("export a=1; readonly -p"), "");
        let (ended, _, desk) = ran("export 1a=2", &[]);
        assert_eq!(ended, Err(Stop::Said(1)));
        assert_eq!(desk.err, b"export: 1a: not a name\n");
        assert_eq!(
            said("readonly r=1; export r=2", &[]).1,
            "export: r: read only\n"
        );
        assert_eq!(out("readonly r=1; export r; export -p"), "export r='1'\n");
    }

    #[test]
    fn readonly_marks_variables_and_says_them() {
        assert_eq!(
            out("readonly a=1 b; readonly -p"),
            "readonly a='1'\nreadonly b\n"
        );
        assert_eq!(out("readonly a=1; export -p"), "");
        assert_eq!(said("readonly -x", &[]).1, "readonly: -x: not a name\n");
        assert_eq!(out("b=1; readonly b; readonly"), "readonly b='1'\n");
    }

    #[test]
    fn unset_takes_variables_or_with_f_functions() {
        assert_eq!(out("a=1; unset a; echo \"[$a]\""), "[]\n");
        assert_eq!(out("a=1; unset -v a; echo \"[$a]\""), "[]\n");
        assert_eq!(
            out("a=1; f() { echo f; }; unset -f a f; echo $a; f 2>&1"),
            "1\nf: not found\n"
        );
        assert_eq!(out("unset nothing; echo $?"), "0\n");
        let (ended, session, desk) = ran("readonly r=1; unset r", &[]);
        assert_eq!(ended, Err(Stop::Said(1)));
        assert_eq!(desk.err, b"unset: r: read only\n");
        assert_eq!(session.variable("r").as_deref(), Some("1"));
        assert_eq!(
            out("unset IFS; v='a b'; for i in $v; do echo $i; done"),
            "a\nb\n"
        );
    }

    #[test]
    fn set_alone_says_every_variable() {
        assert_eq!(
            out("a=1; b=\"it's\"; set"),
            "IFS=' \t\n'\na='1'\nb='it'\\''s'\n"
        );
        assert_eq!(
            out("export c; set"),
            "IFS=' \t\n'\n",
            "one with no value is left out"
        );
    }

    #[test]
    fn set_turns_options_on_and_off() {
        assert_eq!(
            out("echo \"[$-]\"; set -e -u; echo $-; set +e; echo $-"),
            "[]\neu\nu\n"
        );
        assert_eq!(out("set -Cfu; echo $-; set +uC; echo $-"), "Cfu\nf\n");
        let (ended, session, desk) = ran("set -ex; echo no", &[]);
        assert_eq!(ended, Err(Stop::Said(1)));
        assert_eq!(desk.err, b"set: -x: no such option\n");
        assert_eq!(session.flags(), "e", "the letters before it were set");
        assert_eq!(said("set +v", &[]).1, "set: -v: no such option\n");
    }

    #[test]
    fn set_sets_the_positional_parameters() {
        assert_eq!(out("set a b; echo $# $2"), "2 b\n");
        assert_eq!(out("set -- -a; echo $1"), "-a\n");
        assert_eq!(out("set -e a -u; echo $1 $2 $-"), "a -u e\n");
        assert_eq!(said("set --; echo $#", &["x", "y"]).0, "0\n");
        assert_eq!(said("set -e; echo $#", &["x", "y"]).0, "2\n");
        assert_eq!(said("set -- -- x; echo $1 $#", &[]).0, "-- 2\n");
    }

    #[test]
    fn a_special_builtin_s_status_starts_at_0() {
        assert_eq!(out("false; :; echo $?"), "0\n");
        assert_eq!(out("false; export a; echo $?"), "0\n");
        assert_eq!(out("false; set -- a; echo $?"), "0\n");
    }

    #[test]
    fn a_special_builtin_writes_where_its_redirections_say() {
        assert_eq!(
            said("export -p a=1 >&2", &[]),
            (String::new(), String::new())
        );
        assert_eq!(
            said("export a=1; export >&2", &[]),
            (String::new(), "export a='1'\n".into())
        );
        assert_eq!(said("export a; export >&-; echo $?", &[]).0, "1\n");
        assert_eq!(said("set >&-; echo $?", &[]).0, "1\n");
        assert_eq!(said(": > f; echo $?", &[]).0, "1\n");
    }

    #[test]
    fn a_list_run_in_the_background_ends_with_0() {
        assert_eq!(out("false & echo $?"), "0\n");
        assert_eq!(out("echo a & echo \"[$!]\""), "a\n[]\n");
        assert_eq!(out("false && echo no || echo yes & echo $?"), "yes\n0\n");
    }

    #[test]
    fn a_list_run_in_the_background_runs_in_a_subshell() {
        assert_eq!(out("a=1 & echo \"[$a]\""), "[]\n");
        assert_eq!(out("cd /x & pwd"), "/\n");
        assert_eq!(out("f() { :; } & f 2>&-; echo $?"), "127\n");
        assert_eq!(out("set -e; false & echo on"), "on\n");
        assert_eq!(out("set -e; { false; echo no; } & echo on"), "on\n");
        let (ended, _, desk) = ran("exit 3 & echo on", &[]);
        assert_eq!((ended, desk.out), (Ok(Flow::Next), b"on\n".to_vec()));
        assert_eq!(
            out("for i in 1 2; do break & echo $i; done"),
            "1\n2\n",
            "a break in it leaves no loop of the session's"
        );
        assert_eq!(
            said("echo ${u?gone} & echo on", &[]),
            ("on\n".into(), "u: gone\n".into())
        );
    }

    #[test]
    fn a_command_of_assignments_alone_ends_as_its_last_substitution() {
        assert_eq!(out("a=$(false); echo $?"), "1\n");
        assert_eq!(out("a=$(exit 3) b=1; echo $?"), "3\n");
        assert_eq!(out("a=$(exit 3) b=$(true); echo $?"), "0\n");
        assert_eq!(out("false; a=1; echo $?"), "0\n");
        assert_eq!(out("echo $(false); echo $?"), "\n0\n");
        assert_eq!(out("a=$(exit 4) nope 2>&-; echo $?"), "127\n");
    }

    #[test]
    fn a_command_substitution_runs_in_a_subshell() {
        assert_eq!(out("a=1; echo $(a=2; echo $a) $a"), "2 1\n");
        assert_eq!(out("echo $(cd /q; pwd); pwd"), "/q\n/\n");
        assert_eq!(out("a=$(echo a; exit 3; echo b); echo $a $?"), "a 3\n");
        assert_eq!(out("f() { echo in; }; echo \"[$(f)]\""), "[in]\n");
        assert_eq!(out("echo \"$(export a=1; export -p)\""), "export a='1'\n");
        assert_eq!(
            said("a=$(echo x >&2); echo \"[$a]\"", &[]),
            ("[]\n".into(), "x\n".into())
        );
        assert_eq!(out("echo $(echo $(echo deep))"), "deep\n");
    }

    #[test]
    fn what_stops_a_substitution_ends_only_it() {
        let (ended, _, desk) = ran("a=$(echo ${u?gone}); echo $? \"[$a]\"", &[]);
        assert_eq!(ended, Ok(Flow::Next));
        assert_eq!(desk.out, b"1 []\n");
        assert_eq!(desk.err, b"u: gone\n");
        let (_, _, desk) = ran("a=$(echo $((1 / 0))); echo $?", &[]);
        assert_eq!(desk.out, b"2\n");
        assert_eq!(
            desk.err, b"1:1: division-by-zero: divide by something other than 0\n",
            "said at the word the substitution stands in"
        );
    }

    #[test]
    fn the_count_of_a_builtin_is_one_whole_number() {
        assert_eq!(count(&[], 7, 1), Ok(7));
        assert_eq!(count(&words(&["3"]), 7, 1), Ok(3));
        assert_eq!(count(&words(&["1"]), 7, 1), Ok(1));
        assert_eq!(count(&words(&["0"]), 7, 0), Ok(0));
        assert_eq!(
            count(&words(&["0"]), 7, 1),
            Err(String::from("0: not a whole number of 1 or more"))
        );
        assert_eq!(
            count(&words(&["+"]), 7, 0),
            Err(String::from("+: not a whole number of 0 or more"))
        );
        assert_eq!(
            count(&words(&["1", "2"]), 7, 0),
            Err(String::from("takes one number at most"))
        );
    }

    #[test]
    fn a_loop_goes_round_again_only_on_next_or_one_continue() {
        assert_eq!(looped(Flow::Next), None);
        assert_eq!(looped(Flow::Continue(1)), None);
        assert_eq!(looped(Flow::Break(1)), Some(Flow::Next));
        assert_eq!(looped(Flow::Break(3)), Some(Flow::Break(2)));
        assert_eq!(looped(Flow::Continue(2)), Some(Flow::Continue(1)));
        assert_eq!(looped(Flow::Return), Some(Flow::Return));
    }

    #[test]
    fn quoted_reads_back_as_the_value() {
        assert_eq!(quoted(""), "''");
        assert_eq!(quoted("a b"), "'a b'");
        assert_eq!(quoted("'"), "''\\'''");
        assert_eq!(
            out(&format!("echo {}", quoted("it's $a \\ \"x\""))),
            "it's $a \\ \"x\"\n"
        );
    }

    #[test]
    fn a_new_session_has_ifs_and_its_parameters() {
        let session = Session::new(String::from("me"), 9, vec![String::from("p")]);
        assert_eq!(session.variable("IFS").as_deref(), Some(" \t\n"));
        assert_eq!(session.variable("HOME"), None);
        assert_eq!(session.status(), 0);
        assert_eq!(said("echo $0 $$ $1 $#", &["one"]).0, "script 42 one 1\n");
    }

    #[test]
    fn an_error_says_where_it_is() {
        assert_eq!(stopped("echo ${a;b}"), (1, 6, Code::BadSubstitution));
        assert_eq!(
            stopped("a=1\n  echo $((a / 0))"),
            (2, 8, Code::DivisionByZero)
        );
        let (_, _, desk) = ran("(echo ${a!})", &[]);
        assert_eq!(
            desk.err,
            b"1:7: bad-substitution: write ${name}, or ${name} with an operator such as :- after it\n"
        );
    }

    #[test]
    fn a_compound_command_s_redirection_words_are_expanded_first() {
        assert_eq!(
            stopped("{ echo no; } > ${a;b}"),
            (1, 16, Code::BadSubstitution)
        );
        assert_eq!(
            said("n=2; { echo a; } >&$n; echo $?", &[]),
            ("0\n".into(), "a\n".into())
        );
        assert_eq!(
            ran("{ echo no; } > ${a;b}", &[]).2.out,
            b"",
            "nothing in it ran"
        );
    }

    #[test]
    fn a_loop_s_redirections_are_made_once_for_all_its_rounds() {
        let (_, _, desk) = ran(
            "i=0; while [ $i -lt 3 ]; do i=$((i + 1)); done <<EOF\nonce\nEOF\n",
            &[],
        );
        assert_eq!(desk.fed, [b"once\n".to_vec()]);
        let (_, _, desk) = ran("for i in 1 2; do :; done <<EOF\nonce\nEOF\n", &[]);
        assert_eq!(desk.fed, [b"once\n".to_vec()]);
    }

    #[test]
    fn a_function_s_own_redirections_hold_inside_those_of_its_call() {
        assert_eq!(
            said("f() { echo in; } >&2; f", &[]),
            (String::new(), "in\n".into())
        );
        assert_eq!(
            said("f() { echo in; } >&2; f 2>&-; echo $?", &[]),
            ("1\n".into(), String::new()),
            "the call closed stream 2 before the function's own copy of it"
        );
        assert_eq!(
            said("f() { echo $1 >&2; }; f a 2>&1", &[]),
            ("a\n".into(), String::new())
        );
    }

    #[test]
    fn a_function_may_define_and_unset_functions() {
        assert_eq!(out("f() { g() { echo g; }; }; f; g"), "g\n");
        assert_eq!(
            out("g() { echo g; }; f() { unset -f g; }; f; g 2>&-; echo $?"),
            "127\n"
        );
        assert_eq!(
            out("f() { echo outer; f() { echo inner; }; }; f; f"),
            "outer\ninner\n",
            "the call runs the body it started with"
        );
    }

    #[test]
    fn what_a_function_sets_stays_but_its_parameters() {
        assert_eq!(
            out("f() { a=1; set -u; readonly r=2; }; f; echo $a $- $r"),
            "1 u 2\n"
        );
        assert_eq!(
            said("f() { set -- x y; echo $#; }; f; echo $#", &["p"]).0,
            "2\n1\n"
        );
        assert_eq!(out("f() { cd /in; }; f; pwd"), "/in\n");
    }

    #[test]
    fn a_function_may_call_itself_and_come_back() {
        assert_eq!(
            out("count() { if [ $1 -gt 0 ]; then echo $1; count $(($1 - 1)); fi; }; count 3"),
            "3\n2\n1\n"
        );
        assert_eq!(
            out("f() { if [ $1 = x ]; then return 7; fi; f x; echo \"back $?\"; }; f y"),
            "back 7\n"
        );
    }

    #[test]
    fn variables_and_functions_keep_names_of_their_own() {
        assert_eq!(out("f=1; f() { echo fn; }; f; echo $f"), "fn\n1\n");
        assert_eq!(out("f=1; f() { :; }; unset -f f; echo $f"), "1\n");
        assert_eq!(
            out("f=1; f() { echo fn; }; unset f; f; echo \"[$f]\""),
            "fn\n[]\n"
        );
    }

    #[test]
    fn export_and_readonly_take_several_names() {
        assert_eq!(
            out("b=2; export a=1 b c; export -p"),
            "export a='1'\nexport b='2'\nexport c\n"
        );
        assert_eq!(out("export -p a=1; export"), "export a='1'\n");
        let (ended, session, _) = ran("export a=1 1b c=3", &[]);
        assert_eq!(ended, Err(Stop::Said(1)));
        assert_eq!(
            session.variable("a").as_deref(),
            Some("1"),
            "those before it were marked"
        );
        assert_eq!(session.variable("c"), None, "and none after it");
        assert_eq!(stopped("readonly a=1 b; b=2"), (1, 17, Code::ReadOnly));
    }

    #[test]
    fn a_marked_variable_keeps_its_mark_when_given_a_value() {
        assert_eq!(out("export a; a=1; export -p"), "export a='1'\n");
        assert_eq!(out("export a=1; a=2; export -p"), "export a='2'\n");
        assert_eq!(out("export a=1; unset a; export -p"), "");
    }

    #[test]
    fn set_e_leaves_a_loop_that_breaks_alone() {
        assert_eq!(out("set -e; while true; do break; done; echo ok"), "ok\n");
        assert_eq!(
            out("set -e; for i in 1; do continue; done; echo ok"),
            "ok\n"
        );
        assert_eq!(
            ran("set -e; for i in 1; do false; done; echo no", &[]).0,
            Err(Stop::Exit(1))
        );
    }

    #[test]
    fn a_case_s_word_is_one_text() {
        assert_eq!(out("v='a b'; case $v in a*b) echo whole;; esac"), "whole\n");
        assert_eq!(out("case $u in '') echo empty;; esac"), "empty\n");
        assert_eq!(
            out("case ~ in '~') echo kept;; *) echo home;; esac"),
            "home\n"
        );
        assert_eq!(out("case x in *) ;; esac; echo $?"), "0\n");
    }

    #[test]
    fn a_case_s_patterns_are_expanded_only_up_to_the_one_that_matches() {
        assert_eq!(
            out("case a in a) echo a;; $(echo b >&2)) echo b;; esac"),
            "a\n"
        );
        assert_eq!(said("case a in a) ;; $(echo b >&2)) ;; esac", &[]).1, "");
        assert_eq!(said("case z in $(echo b >&2)) ;; esac", &[]).1, "b\n");
    }

    #[test]
    fn eval_s_status_is_its_last_command_s() {
        assert_eq!(out("eval 'false; true'; echo $?"), "0\n");
        assert_eq!(out("eval 'true; false'; echo $?"), "1\n");
        assert_eq!(out("eval ''; echo $?"), "0\n");
        assert_eq!(out("eval 'a=1' 'b=2'; echo $a$b"), "12\n");
    }

    #[test]
    fn exit_in_a_condition_still_ends_the_script() {
        assert_eq!(
            ran("if exit 4; then echo no; fi", &[]).0,
            Err(Stop::Exit(4))
        );
        assert_eq!(ran("while exit 5; do :; done", &[]).0, Err(Stop::Exit(5)));
        assert_eq!(ran("false || exit 6", &[]).0, Err(Stop::Exit(6)));
    }
}
