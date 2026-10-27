//! Statements run, sections 4 to 6 of the specification: lists, variables
//! and the blocks they belong to, `if`, `for`, `while`, functions, `true`
//! and `false`. What runs a program, and finds the files a pattern
//! matches, is the [`Machine`]'s.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::expand::{self, Expanded, Piece};
use crate::lex::Word;
use crate::parse::{Command, Join, List, Pipeline, Redirect, Statement};
use crate::{Code, Error};

/// A command ready to run: its words, and the paths its redirections lead
/// to, all expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    pub words: Vec<String>,
    pub redirections: Vec<(Redirect, String)>,
}

/// What a session needs of the system it runs on.
pub trait Machine {
    /// Runs `pipeline`, its commands joined by pipes, and answers the exit
    /// status of its last command: 127 for one that names nothing there is.
    fn run(&mut self, pipeline: &[Ready]) -> u8;

    /// The paths of the files `pattern` matches, from `directory` when it
    /// does not start at the root, in the order of their names compared in
    /// capitals.
    fn matching(&mut self, directory: &str, pattern: &[Piece]) -> Vec<String>;
}

/// How statements ended: on to the next, or out of a loop, a function or
/// the script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Next,
    Break,
    Continue,
    Return,
}

/// How deep functions may call each other.
pub const DEPTH: usize = 64;

/// A session of the shell: its variables, block by block, its functions,
/// the status of its last pipeline, the arguments of each function it is
/// in, and the directory paths that do not start at the root start from.
pub struct Session {
    scopes: Vec<Vec<(String, String)>>,
    functions: Vec<(String, Vec<Statement>)>,
    status: u8,
    arguments: Vec<Vec<String>>,
    pub directory: String,
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

impl Session {
    #[must_use]
    pub fn new() -> Session {
        Session {
            scopes: Vec::from([Vec::new()]),
            functions: Vec::new(),
            status: 0,
            arguments: Vec::new(),
            directory: "/".into(),
        }
    }

    /// The exit status of the last pipeline.
    #[must_use]
    pub fn status(&self) -> u8 {
        self.status
    }

    /// The value of the variable `name`: one set with `let` or by a loop,
    /// or `?`, `#` and `1` on, the last status and the arguments of the
    /// function running, none outside one.
    #[must_use]
    pub fn variable(&self, name: &str) -> Option<String> {
        let arguments = self.arguments.last().map_or(&[][..], Vec::as_slice);
        match name {
            "?" => Some(self.status.to_string()),
            "#" => Some(arguments.len().to_string()),
            n if n.bytes().all(|b| b.is_ascii_digit()) => {
                let at = n.parse::<usize>().ok()?.checked_sub(1)?;
                arguments.get(at).cloned()
            }
            _ => self.scopes.iter().rev().find_map(|scope| {
                scope
                    .iter()
                    .find(|(set, _)| set == name)
                    .map(|(_, value)| value.clone())
            }),
        }
    }

    /// Sets the variable `name` where it was set before, or in the
    /// innermost block.
    fn set(&mut self, name: &str, value: String) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.iter_mut().find(|(set, _)| set == name) {
                slot.1 = value;
                return;
            }
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.into(), value));
        }
    }

    /// Runs `statements` and says how they ended.
    ///
    /// # Errors
    ///
    /// The first error running them: a variable never set, a pattern that
    /// matches nothing, a status that is no number from 0 to 255, or
    /// functions calling each other more than [`DEPTH`] deep.
    pub fn run(
        &mut self,
        statements: &[Statement],
        machine: &mut impl Machine,
    ) -> Result<Flow, Error> {
        for statement in statements {
            let flow = self.statement(statement, machine)?;
            if flow != Flow::Next {
                return Ok(flow);
            }
        }
        Ok(Flow::Next)
    }

    /// Runs `statements` in a block of their own, whose variables go with
    /// it, after setting `first` there, when it is some.
    fn block(
        &mut self,
        statements: &[Statement],
        first: Option<(&str, String)>,
        machine: &mut impl Machine,
    ) -> Result<Flow, Error> {
        let scope = first.map(|(n, v)| (n.into(), v)).into_iter().collect();
        self.scopes.push(scope);
        let flow = self.run(statements, machine);
        self.scopes.pop();
        flow
    }

    fn statement(
        &mut self,
        statement: &Statement,
        machine: &mut impl Machine,
    ) -> Result<Flow, Error> {
        match statement {
            Statement::List(list) => self.status = self.list(list, machine)?,
            Statement::Let { name, value } => {
                let value = self.text(value)?;
                self.set(name, value);
            }
            Statement::If {
                condition,
                then,
                otherwise,
            } => {
                let branch = if self.list(condition, machine)? == 0 {
                    then
                } else {
                    otherwise
                };
                let flow = self.block(branch, None, machine)?;
                if branch.is_empty() {
                    self.status = 0;
                }
                return Ok(flow);
            }
            Statement::For { name, words, body } => {
                let items = self.words(words, machine)?;
                if items.is_empty() {
                    self.status = 0;
                }
                for item in items {
                    match self.block(body, Some((name, item)), machine)? {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        Flow::Next | Flow::Continue => {}
                    }
                }
            }
            Statement::While { condition, body } => {
                // What the body ended with last, 0 while it never ran.
                let mut last = 0;
                while self.list(condition, machine)? == 0 {
                    let flow = self.block(body, None, machine)?;
                    last = self.status;
                    match flow {
                        Flow::Break => break,
                        Flow::Return => return Ok(Flow::Return),
                        Flow::Next | Flow::Continue => {}
                    }
                }
                self.status = last;
            }
            Statement::Fn { name, body } => {
                self.functions.retain(|(made, _)| made != name);
                self.functions.push((name.clone(), body.clone()));
            }
            Statement::Break => return Ok(Flow::Break),
            Statement::Continue => return Ok(Flow::Continue),
            Statement::Return(word) => {
                self.status = match word {
                    None => 0,
                    Some(w) => self.text(w)?.parse().map_err(|_| Error {
                        at: w.at,
                        code: Code::BadStatus,
                    })?,
                };
                return Ok(Flow::Return);
            }
        }
        Ok(Flow::Next)
    }

    /// Runs `list`, each pipeline after the first only as its join says,
    /// and answers the status of the last that ran.
    fn list(&mut self, list: &List, machine: &mut impl Machine) -> Result<u8, Error> {
        let mut status = self.pipeline(&list.first, machine)?;
        for (join, pipeline) in &list.rest {
            let runs = match join {
                Join::And => status == 0,
                Join::Or => status != 0,
            };
            if runs {
                status = self.pipeline(pipeline, machine)?;
            }
        }
        Ok(status)
    }

    fn pipeline(&mut self, pipeline: &Pipeline, machine: &mut impl Machine) -> Result<u8, Error> {
        let status = if let [one] = pipeline.commands.as_slice() {
            self.single(one, machine)?
        } else {
            let mut ready = Vec::new();
            for command in &pipeline.commands {
                ready.push(self.ready(command, machine)?);
            }
            machine.run(&ready)
        };
        let status = if pipeline.not {
            u8::from(status == 0)
        } else {
            status
        };
        self.status = status;
        Ok(status)
    }

    /// Runs a command alone: `true`, `false`, a function, or whatever the
    /// machine runs.
    fn single(&mut self, command: &Command, machine: &mut impl Machine) -> Result<u8, Error> {
        let ready = self.ready(command, machine)?;
        let name = ready.words.first().map_or("", String::as_str);
        if name == "true" {
            return Ok(0);
        }
        if name == "false" {
            return Ok(1);
        }
        let Some(body) = self
            .functions
            .iter()
            .find(|(made, _)| made == name)
            .map(|(_, body)| body.clone())
        else {
            return Ok(machine.run(&[ready]));
        };
        if self.arguments.len() == DEPTH {
            return Err(Error {
                at: command.words[0].at,
                code: Code::TooDeep,
            });
        }
        self.arguments.push(ready.words[1..].to_vec());
        self.status = 0;
        let flow = self.block(&body, None, machine);
        self.arguments.pop();
        flow?;
        Ok(self.status)
    }

    /// `command` with its words and its redirections' paths expanded.
    fn ready(&mut self, command: &Command, machine: &mut impl Machine) -> Result<Ready, Error> {
        let words = self.words(&command.words, machine)?;
        let mut redirections = Vec::new();
        for (how, path) in &command.redirections {
            redirections.push((*how, self.text(path)?));
        }
        Ok(Ready {
            words,
            redirections,
        })
    }

    /// `words` expanded, each pattern into the paths it matches.
    fn words(&mut self, words: &[Word], machine: &mut impl Machine) -> Result<Vec<String>, Error> {
        let mut out = Vec::new();
        for word in words {
            match expand::expand(word, |n| self.variable(n))? {
                Expanded::Text(text) => out.push(text),
                Expanded::Pattern(pattern) => {
                    let found = machine.matching(&self.directory, &pattern);
                    if found.is_empty() {
                        return Err(Error {
                            at: word.at,
                            code: Code::NoMatch,
                        });
                    }
                    out.extend(found);
                }
            }
        }
        Ok(out)
    }

    /// `word` expanded into one word, a pattern taken as it is written.
    fn text(&self, word: &Word) -> Result<String, Error> {
        Ok(match expand::expand(word, |n| self.variable(n))? {
            Expanded::Text(text) => text,
            Expanded::Pattern(pattern) => pattern
                .iter()
                .map(|p| match p {
                    Piece::Char(c) => *c,
                    Piece::One => '?',
                    Piece::Any => '*',
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pos;
    use crate::parse::script;

    /// A machine with no programs and no files, so only what the shell
    /// has itself runs: every program it is asked for is not there.
    struct Bare;

    impl Machine for Bare {
        fn run(&mut self, _: &[Ready]) -> u8 {
            127
        }

        fn matching(&mut self, _: &str, _: &[Piece]) -> Vec<String> {
            Vec::new()
        }
    }

    /// A session that ran `text` through to its end.
    fn ran(text: &str) -> Session {
        let mut session = Session::new();
        session.run(&script(text).unwrap(), &mut Bare).unwrap();
        session
    }

    fn value(text: &str, name: &str) -> Option<String> {
        ran(text).variable(name)
    }

    fn failure(text: &str) -> (usize, usize, Code) {
        let e = Session::new()
            .run(&script(text).unwrap(), &mut Bare)
            .unwrap_err();
        (e.at.line, e.at.column, e.code)
    }

    #[test]
    fn variables_hold_what_let_gives_them() {
        assert_eq!(value("let a = 1", "a").as_deref(), Some("1"));
        assert_eq!(
            value("let a = 1; let b = \"$a and $a\"", "b").as_deref(),
            Some("1 and 1")
        );
        assert_eq!(value("let a = 1; let a = 2", "a").as_deref(), Some("2"));
        assert_eq!(value("let a = '*.txt'", "a").as_deref(), Some("*.txt"));
        assert_eq!(
            value("let a = *.txt", "a").as_deref(),
            Some("*.txt"),
            "never matched"
        );
        assert_eq!(value("let a = a?b", "a").as_deref(), Some("a?b"));
        assert_eq!(value("", "a"), None);
        assert_eq!(failure("let a = $b"), (1, 9, Code::UnsetVariable));
        assert_eq!(failure("echo $nothing"), (1, 6, Code::UnsetVariable));
    }

    #[test]
    fn true_false_and_programs_that_are_not_there_set_the_status() {
        assert_eq!(ran("true").status(), 0);
        assert_eq!(ran("false").status(), 1);
        assert_eq!(ran("nothing here").status(), 127);
        assert_eq!(value("false; let s = $?", "s").as_deref(), Some("1"));
        assert_eq!(value("nothing; let s = $?", "s").as_deref(), Some("127"));
        assert_eq!(
            value("let s = $?", "s").as_deref(),
            Some("0"),
            "a new session"
        );
        assert_eq!(
            ran("a | b").status(),
            127,
            "a pipeline ends as its last command"
        );
        assert_eq!(ran("not a | b").status(), 0);
    }

    #[test]
    fn joins_run_the_next_only_as_they_say() {
        let marks = |list: &str| {
            value(
                &format!("let m = no\nfn mark {{ let m = yes }}\n{list}"),
                "m",
            )
        };
        assert_eq!(marks("true and mark").as_deref(), Some("yes"));
        assert_eq!(marks("false and mark").as_deref(), Some("no"));
        assert_eq!(marks("true or mark").as_deref(), Some("no"));
        assert_eq!(marks("false or mark").as_deref(), Some("yes"));
        assert_eq!(
            marks("false and true or mark").as_deref(),
            Some("yes"),
            "from the left"
        );
        assert_eq!(marks("true or false and mark").as_deref(), Some("yes"));
        assert_eq!(marks("true and false and mark").as_deref(), Some("no"));
        assert_eq!(marks("not false and mark").as_deref(), Some("yes"));
        assert_eq!(ran("false and true").status(), 1);
        assert_eq!(ran("false or false").status(), 1);
        assert_eq!(ran("not true").status(), 1);
        assert_eq!(ran("not nothing").status(), 0);
        assert_eq!(
            value("false and true; let s = $?", "s").as_deref(),
            Some("1")
        );
    }

    #[test]
    fn if_runs_the_branch_its_condition_picks() {
        let picked = |text: &str| value(&format!("let r = none\n{text}"), "r");
        assert_eq!(
            picked("if true { let r = a } else { let r = b }").as_deref(),
            Some("a")
        );
        assert_eq!(
            picked("if false { let r = a } else { let r = b }").as_deref(),
            Some("b")
        );
        assert_eq!(picked("if false { let r = a }").as_deref(), Some("none"));
        assert_eq!(
            picked("if false { let r = a } else if true { let r = c } else { let r = d }")
                .as_deref(),
            Some("c")
        );
        assert_eq!(
            picked("if nothing { let r = a } else { let r = $? }").as_deref(),
            Some("127")
        );
        assert_eq!(ran("if false { true }").status(), 0, "no branch ran");
        assert_eq!(ran("if true { false }").status(), 1, "the branch's last");
        assert_eq!(ran("if false { true } else { nothing }").status(), 127);
    }

    #[test]
    fn variables_set_in_a_block_go_with_it() {
        assert_eq!(value("if true { let inner = 1 }", "inner"), None);
        assert_eq!(
            value("let outer = 0\nif true { let outer = 1 }", "outer").as_deref(),
            Some("1")
        );
        assert_eq!(
            value("let a = 0\nif true { if true { let a = 2 } }", "a").as_deref(),
            Some("2"),
            "however deep"
        );
        assert_eq!(
            value(
                "if true { let b = 1\nif true { let b = 2 }\nlet c = $b }",
                "c"
            ),
            None,
            "c goes too"
        );
        assert_eq!(
            value(
                "let c = -\nif true { let b = 1\nif true { let b = 2 }\nlet c = $b }",
                "c"
            )
            .as_deref(),
            Some("2"),
            "the inner block set the b of the outer one"
        );
    }

    #[test]
    fn for_sets_its_variable_for_each_word() {
        assert_eq!(
            value(
                "let all = ''\nfor x in a b c { let all = \"$all$x\" }",
                "all"
            )
            .as_deref(),
            Some("abc")
        );
        assert_eq!(
            value("for x in a b { }", "x"),
            None,
            "the variable goes with the loop"
        );
        assert_eq!(
            value("let x = outer\nfor x in a b { let x = changed }", "x").as_deref(),
            Some("outer"),
            "the loop's own x hides the outer one"
        );
        assert_eq!(
            ran("false\nfor x in { true }").status(),
            0,
            "no words, nothing ran"
        );
        assert_eq!(ran("for x in a { false }").status(), 1);
        assert_eq!(
            value(
                "let w = -\nlet v = \"p q\"\nfor x in $v { let w = $x }",
                "w"
            )
            .as_deref(),
            Some("p q")
        );
        assert_eq!(failure("for x in *.txt { true }"), (1, 10, Code::NoMatch));
    }

    #[test]
    fn break_and_continue_leave_the_innermost_loop() {
        let out = |text: &str| value(&format!("let out = ''\n{text}"), "out");
        assert_eq!(
            out("for x in a b c { let out = \"$out$x\"; break }").as_deref(),
            Some("a")
        );
        assert_eq!(
            out("for x in a b c { continue; let out = \"$out$x\" }").as_deref(),
            Some("")
        );
        assert_eq!(
            out("for x in a b { for y in 1 2 { let out = \"$out$x$y\"; break } }").as_deref(),
            Some("a1b1"),
            "only the inner loop"
        );
        assert_eq!(
            out("for x in a b { if true { continue }; let out = $x }").as_deref(),
            Some(""),
            "from inside an if"
        );
        assert_eq!(
            out("while true { let out = once; break }").as_deref(),
            Some("once")
        );
        assert_eq!(
            out("let n = ''\nwhile true { if true { let out = \"$out.\"; break } }").as_deref(),
            Some(".")
        );
    }

    #[test]
    fn while_runs_as_long_as_its_condition_holds() {
        assert_eq!(
            value("let r = no\nwhile false { let r = yes }", "r").as_deref(),
            Some("no")
        );
        assert_eq!(
            ran("false\nwhile false { true }").status(),
            0,
            "the body never ran"
        );
        assert_eq!(
            ran("while true { false; break }").status(),
            1,
            "the body's last"
        );
        assert_eq!(ran("fn f { while true { return 4 } }\nf").status(), 4);
    }

    #[test]
    fn functions_take_arguments_and_return_statuses() {
        assert_eq!(
            value(
                "let got = -\nfn args { let got = \"$# $1 $2\" }\nargs a b",
                "got"
            )
            .as_deref(),
            Some("2 a b")
        );
        assert_eq!(
            value("let n = $#", "n").as_deref(),
            Some("0"),
            "outside a function"
        );
        assert_eq!(failure("let a = $1"), (1, 9, Code::UnsetVariable));
        assert_eq!(
            failure("fn f { let a = $2 }\nf one"),
            (1, 16, Code::UnsetVariable)
        );
        assert_eq!(ran("fn f { return 3 }\nf").status(), 3);
        assert_eq!(ran("fn f { false; return }\nf").status(), 0);
        assert_eq!(ran("fn f { false }\nf").status(), 1, "the last command's");
        assert_eq!(ran("fn f { }\nfalse\nf").status(), 0);
        assert_eq!(
            value("fn f { return 7 }\nf; let s = $?", "s").as_deref(),
            Some("7")
        );
        assert_eq!(
            value(
                "let after = no\nfn f { for x in a b { return 2 }\nlet after = yes }\nf",
                "after"
            )
            .as_deref(),
            Some("no")
        );
        assert_eq!(
            ran("fn f { return 1 }\nfn f { return 2 }\nf").status(),
            2,
            "made again"
        );
        assert_eq!(
            ran("fn true { return 5 }\ntrue").status(),
            0,
            "true stays true"
        );
        assert_eq!(
            value(
                "let r = -\nfn inner { let r = $1 }\nfn outer { inner \"$1$1\" }\nouter x",
                "r"
            )
            .as_deref(),
            Some("xx"),
            "each call its own arguments"
        );
        assert_eq!(
            value(
                "let r = -\nfn inner { true }\nfn outer { inner y; let r = $1 }\nouter x",
                "r"
            )
            .as_deref(),
            Some("x"),
            "and back to the caller's"
        );
    }

    #[test]
    fn return_takes_a_number_up_to_255() {
        assert_eq!(ran("fn f { return 255 }\nf").status(), 255);
        assert_eq!(ran("fn f { return 0 }\nf").status(), 0);
        assert_eq!(failure("fn f { return 256 }\nf"), (1, 15, Code::BadStatus));
        assert_eq!(failure("fn f { return -1 }\nf"), (1, 15, Code::BadStatus));
        assert_eq!(failure("return abc"), (1, 8, Code::BadStatus));
        let mut session = Session::new();
        let flow = session.run(
            &script("let a = 1\nreturn 9\nlet a = 2").unwrap(),
            &mut Bare,
        );
        assert_eq!(
            flow,
            Ok(Flow::Return),
            "outside a function it ends the script"
        );
        assert_eq!(
            (session.status(), session.variable("a").as_deref()),
            (9, Some("1"))
        );
    }

    #[test]
    fn break_outside_a_loop_ends_the_script() {
        let mut session = Session::new();
        let flow = session.run(&script("let a = 1\nbreak\nlet a = 2").unwrap(), &mut Bare);
        assert_eq!(
            (flow, session.variable("a").as_deref()),
            (Ok(Flow::Break), Some("1"))
        );
        let flow = Session::new().run(&script("continue").unwrap(), &mut Bare);
        assert_eq!(flow, Ok(Flow::Continue));
        assert_eq!(
            Session::new().run(&script("true").unwrap(), &mut Bare),
            Ok(Flow::Next)
        );
    }

    #[test]
    fn functions_call_each_other_only_so_deep() {
        let e = Session::new()
            .run(&script("fn down { down }\ndown").unwrap(), &mut Bare)
            .unwrap_err();
        assert_eq!(
            (e.code, e.at),
            (
                Code::TooDeep,
                Pos {
                    line: 1,
                    column: 11
                }
            )
        );
        let chain = |calls: usize| -> String {
            let mut lines: Vec<String> = (0..calls)
                .map(|i| format!("fn f{i} {{ f{} }}", i + 1))
                .collect();
            lines.push(format!("fn f{calls} {{ let r = deepest }}"));
            format!("let r = -\n{}\nf0\n", lines.join("\n"))
        };
        let too_deep = Session::new().run(&script(&chain(DEPTH)).unwrap(), &mut Bare);
        assert_eq!(
            too_deep.map(|_| ()).unwrap_err().code,
            Code::TooDeep,
            "one call past the depth"
        );
        assert_eq!(
            value(&chain(DEPTH - 1), "r").as_deref(),
            Some("deepest"),
            "{DEPTH} calls fit"
        );
    }

    #[test]
    fn a_session_starts_empty_at_the_root() {
        let session = Session::default();
        assert_eq!((session.status(), session.directory.as_str()), (0, "/"));
        assert_eq!(session.variable("anything"), None);
        assert_eq!(session.variable("0"), None);
        assert_eq!(session.variable("1"), None);
        assert_eq!(session.variable("#").as_deref(), Some("0"));
    }

    #[test]
    fn names_with_digits_are_variables_and_digits_alone_arguments() {
        assert_eq!(value("let a1 = x\nlet b = $a1", "b").as_deref(), Some("x"));
        assert_eq!(
            value("let _2 = y\nlet b = ${_2}3", "b").as_deref(),
            Some("y3")
        );
        assert_eq!(
            value(
                "let r = -\nfn f { let r = $10 }\nf 1 2 3 4 5 6 7 8 9 ten",
                "r"
            )
            .as_deref(),
            Some("ten")
        );
        assert_eq!(
            failure("fn f { let r = $0 }\nf a"),
            (1, 16, Code::UnsetVariable)
        );
    }

    #[test]
    fn every_word_of_a_pipeline_and_its_paths_is_expanded() {
        assert_eq!(failure("echo > $nope"), (1, 8, Code::UnsetVariable));
        assert_eq!(failure("cat < \"$nope\""), (1, 7, Code::UnsetVariable));
        assert_eq!(failure("a | b $nope"), (1, 7, Code::UnsetVariable));
        assert_eq!(failure("a $nope | b"), (1, 3, Code::UnsetVariable));
        assert_eq!(failure("a | b >> $nope"), (1, 10, Code::UnsetVariable));
        assert_eq!(failure("a | b *.txt"), (1, 7, Code::NoMatch));
        assert_eq!(ran("let f = out\necho > $f").status(), 127, "set, it runs");
        assert_eq!(ran("echo > *.txt").status(), 127, "a path is never matched");
    }

    #[test]
    fn not_turns_any_status_into_its_opposite() {
        assert_eq!(ran("fn three { return 3 }\nnot three").status(), 0);
        assert_eq!(ran("fn zero { return 0 }\nnot zero").status(), 1);
        assert_eq!(value("not false; let s = $?", "s").as_deref(), Some("0"));
        assert_eq!(value("not true; let s = $?", "s").as_deref(), Some("1"));
    }
}
