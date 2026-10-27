//! The commands, lists and blocks a script's words and operators stand
//! for, sections 3 to 6 of the specification.

use alloc::string::String;
use alloc::vec::Vec;

use crate::lex::{self, Token, Word};
use crate::{Code, Error, Pos};

/// Where a redirection sends what a command writes, or takes what it
/// reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirect {
    /// `>`: into the file, made new or emptied.
    Out,
    /// `>>`: after what the file holds.
    Append,
    /// `<`: from the file.
    In,
    /// `2>`: what goes wrong into the file, made new or emptied.
    ErrorOut,
    /// `2>>`: what goes wrong after what the file holds.
    ErrorAppend,
    /// `2>&1`: what goes wrong where the output goes when it comes.
    ErrorToOutput,
}

/// A command: the words that name it and its arguments, and its
/// redirections, each with the word for its path but `2>&1`, which has
/// none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<Word>,
    pub redirections: Vec<(Redirect, Option<Word>)>,
}

/// Commands joined with `|`, and whether `not` stands before them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub not: bool,
    pub commands: Vec<Command>,
}

/// How a pipeline is joined to the ones before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    And,
    Or,
}

/// Pipelines joined with `and` and `or`, from the left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub first: Pipeline,
    pub rest: Vec<(Join, Pipeline)>,
}

/// One statement of a script or a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Statement {
    List(List),
    Let {
        name: String,
        value: Word,
    },
    /// `if`, its block, and what `else` runs, nothing without one; an
    /// `else if` is an `if` alone in it.
    If {
        condition: List,
        then: Vec<Statement>,
        otherwise: Vec<Statement>,
    },
    For {
        name: String,
        words: Vec<Word>,
        body: Vec<Statement>,
    },
    While {
        condition: List,
        body: Vec<Statement>,
    },
    Fn {
        name: String,
        body: Vec<Statement>,
    },
    Break,
    Continue,
    Return(Option<Word>),
    /// `exit`, and the status it is given.
    Exit(Option<Word>),
}

/// The statements of `script`.
///
/// # Errors
///
/// The script's first error, as [`lex::tokens`] or the rules of the
/// specification's sections 3 to 6 find it.
pub fn script(script: &str) -> Result<Vec<Statement>, Error> {
    let tokens = lex::tokens(script)?;
    let mut parser = Parser {
        tokens,
        next: 0,
        end: end_of(script),
    };
    parser.statements(None)
}

/// Where a script ends: right after its last character.
fn end_of(script: &str) -> Pos {
    let line = 1 + script.matches('\n').count();
    let last = script.rsplit('\n').next().unwrap_or("");
    Pos {
        line,
        column: 1 + last.chars().count(),
    }
}

/// Whether `name` is one a variable, a loop or a function may have.
pub(crate) fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

struct Parser {
    tokens: Vec<(Token, Pos)>,
    next: usize,
    end: Pos,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.next).map(|(t, _)| t)
    }

    /// Where the next token starts, or the script's end.
    fn here(&self) -> Pos {
        self.tokens.get(self.next).map_or(self.end, |&(_, at)| at)
    }

    /// Whether the next token is the plain word `word`.
    fn at_word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w.plain() == Some(word))
    }

    fn fail<T>(&self, code: Code) -> Result<T, Error> {
        Err(Error {
            at: self.here(),
            code,
        })
    }

    /// Statements up to the end of the script, or, inside the block of the
    /// keyword at `block`, up to its `}`, which is taken.
    fn statements(&mut self, block: Option<Pos>) -> Result<Vec<Statement>, Error> {
        let mut statements = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Newline | Token::Semicolon) => self.next += 1,
                Some(Token::Close) if block.is_some() => {
                    self.next += 1;
                    return Ok(statements);
                }
                Some(Token::Close) => return self.fail(Code::UnexpectedClose),
                None => {
                    return match block {
                        Some(at) => Err(Error {
                            at,
                            code: Code::UnclosedBlock,
                        }),
                        None => Ok(statements),
                    };
                }
                Some(_) => {
                    statements.push(self.statement()?);
                    match self.peek() {
                        None | Some(Token::Newline | Token::Semicolon | Token::Close) => {}
                        Some(Token::Open) => return self.fail(Code::UnexpectedOpen),
                        Some(_) => return self.fail(Code::ExpectedEnd),
                    }
                }
            }
        }
    }

    fn statement(&mut self) -> Result<Statement, Error> {
        let at = self.here();
        let keyword = match self.peek() {
            Some(Token::Word(w)) => w.plain().map(String::from),
            _ => None,
        };
        let statement = match keyword.as_deref() {
            Some("let") => {
                self.next += 1;
                let name = self.name()?;
                if !self.at_word("=") {
                    return self.fail(Code::ExpectedEquals);
                }
                self.next += 1;
                let Some(Token::Word(value)) = self.peek().cloned() else {
                    return self.fail(Code::ExpectedValue);
                };
                self.next += 1;
                Statement::Let { name, value }
            }
            Some("if") => self.conditional(at)?,
            Some("else") => return self.fail(Code::UnexpectedElse),
            Some("for") => {
                self.next += 1;
                let name = self.name()?;
                if !self.at_word("in") {
                    return self.fail(Code::ExpectedIn);
                }
                self.next += 1;
                let mut words = Vec::new();
                while let Some(Token::Word(w)) = self.peek() {
                    words.push(w.clone());
                    self.next += 1;
                }
                let body = self.block(at)?;
                Statement::For { name, words, body }
            }
            Some("while") => {
                self.next += 1;
                let condition = self.list()?;
                let body = self.block(at)?;
                Statement::While { condition, body }
            }
            Some("fn") => {
                self.next += 1;
                let name = self.name()?;
                let body = self.block(at)?;
                Statement::Fn { name, body }
            }
            Some("break") => {
                self.next += 1;
                Statement::Break
            }
            Some("continue") => {
                self.next += 1;
                Statement::Continue
            }
            Some(keyword @ ("return" | "exit")) => {
                let made = if keyword == "exit" {
                    Statement::Exit
                } else {
                    Statement::Return
                };
                self.next += 1;
                let Some(Token::Word(w)) = self.peek().cloned() else {
                    return Ok(made(None));
                };
                self.next += 1;
                made(Some(w))
            }
            _ => Statement::List(self.list()?),
        };
        Ok(statement)
    }

    /// The `if` at `at`, with its block and any `else`.
    fn conditional(&mut self, at: Pos) -> Result<Statement, Error> {
        self.next += 1;
        let condition = self.list()?;
        let then = self.block(at)?;
        let mut otherwise = Vec::new();
        if self.at_word("else") {
            let at = self.here();
            self.next += 1;
            if self.at_word("if") {
                let at = self.here();
                otherwise.push(self.conditional(at)?);
            } else {
                otherwise = self.block(at)?;
            }
        }
        Ok(Statement::If {
            condition,
            then,
            otherwise,
        })
    }

    /// The name a `let`, `for` or `fn` makes.
    fn name(&mut self) -> Result<String, Error> {
        let name = match self.peek() {
            Some(Token::Word(w)) => w.plain().filter(|n| is_name(n)).map(String::from),
            _ => None,
        };
        let Some(name) = name else {
            return self.fail(Code::ExpectedName);
        };
        self.next += 1;
        Ok(name)
    }

    /// The block of the keyword at `at`: a `{` on its line, the statements
    /// and their `}`.
    fn block(&mut self, at: Pos) -> Result<Vec<Statement>, Error> {
        if self.peek() != Some(&Token::Open) {
            return self.fail(Code::ExpectedBrace);
        }
        self.next += 1;
        self.statements(Some(at))
    }

    /// Pipelines joined with `and` and `or`.
    fn list(&mut self) -> Result<List, Error> {
        let first = self.pipeline()?;
        let mut rest = Vec::new();
        loop {
            let join = if self.at_word("and") || self.peek() == Some(&Token::And) {
                Join::And
            } else if self.at_word("or") || self.peek() == Some(&Token::Or) {
                Join::Or
            } else {
                return Ok(List { first, rest });
            };
            self.next += 1;
            rest.push((join, self.pipeline()?));
        }
    }

    /// Commands joined with `|`, after a `not` or none.
    fn pipeline(&mut self) -> Result<Pipeline, Error> {
        let not = self.at_word("not");
        if not {
            self.next += 1;
        }
        let mut commands = Vec::from([self.command()?]);
        while self.peek() == Some(&Token::Pipe) {
            self.next += 1;
            commands.push(self.command()?);
        }
        Ok(Pipeline { not, commands })
    }

    /// The words and redirections up to the next operator that ends a
    /// command, or to an `and` or `or` after its first word.
    fn command(&mut self) -> Result<Command, Error> {
        let at = self.here();
        let mut command = Command {
            words: Vec::new(),
            redirections: Vec::new(),
        };
        loop {
            let how = match self.peek() {
                Some(Token::Word(w)) => {
                    if matches!(w.plain(), Some("and" | "or")) && !command.words.is_empty() {
                        break;
                    }
                    command.words.push(w.clone());
                    self.next += 1;
                    continue;
                }
                Some(Token::Out) => Redirect::Out,
                Some(Token::Append) => Redirect::Append,
                Some(Token::In) => Redirect::In,
                Some(Token::ErrorOut) => Redirect::ErrorOut,
                Some(Token::ErrorAppend) => Redirect::ErrorAppend,
                Some(Token::ErrorToOutput) => {
                    self.next += 1;
                    command.redirections.push((Redirect::ErrorToOutput, None));
                    continue;
                }
                _ => break,
            };
            self.next += 1;
            let Some(Token::Word(path)) = self.peek().cloned() else {
                return self.fail(Code::MissingPath);
            };
            self.next += 1;
            command.redirections.push((how, Some(path)));
        }
        if command.words.is_empty() {
            let at = if command.redirections.is_empty() {
                self.here()
            } else {
                at
            };
            return Err(Error {
                at,
                code: Code::MissingCommand,
            });
        }
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::*;
    use crate::lex::Part;

    fn word(w: &Word) -> String {
        w.parts
            .iter()
            .map(|p| match p {
                Part::Text { text, plain: true } => text.clone(),
                Part::Text { text, plain: false } => format!("[{text}]"),
                Part::Variable(name) => format!("${{{name}}}"),
            })
            .collect()
    }

    fn command(c: &Command) -> String {
        let mut parts: Vec<String> = c.words.iter().map(word).collect();
        for (how, path) in &c.redirections {
            let sign = match how {
                Redirect::Out => ">",
                Redirect::Append => ">>",
                Redirect::In => "<",
                Redirect::ErrorOut => "2>",
                Redirect::ErrorAppend => "2>>",
                Redirect::ErrorToOutput => "2>&1",
            };
            parts.push(format!(
                "{sign}{}",
                path.as_ref().map(word).unwrap_or_default()
            ));
        }
        parts.join(" ")
    }

    fn pipeline(p: &Pipeline) -> String {
        let commands: Vec<String> = p.commands.iter().map(command).collect();
        let not = if p.not { "not " } else { "" };
        format!("{not}{}", commands.join(" | "))
    }

    fn list(l: &List) -> String {
        let mut text = pipeline(&l.first);
        for (join, p) in &l.rest {
            let join = match join {
                Join::And => "and",
                Join::Or => "or",
            };
            text = format!("{text} {join} {}", pipeline(p));
        }
        text
    }

    fn block(statements: &[Statement]) -> String {
        let all: Vec<String> = statements.iter().map(statement).collect();
        format!("{{{}}}", all.join("; "))
    }

    fn statement(s: &Statement) -> String {
        match s {
            Statement::List(l) => list(l),
            Statement::Let { name, value } => format!("let {name}={}", word(value)),
            Statement::If {
                condition,
                then,
                otherwise,
            } => format!(
                "if({}){}else{}",
                list(condition),
                block(then),
                block(otherwise)
            ),
            Statement::For { name, words, body } => {
                let words: Vec<String> = words.iter().map(word).collect();
                format!("for {name} in({}){}", words.join(" "), block(body))
            }
            Statement::While { condition, body } => {
                format!("while({}){}", list(condition), block(body))
            }
            Statement::Fn { name, body } => format!("fn {name}{}", block(body)),
            Statement::Break => "break".into(),
            Statement::Continue => "continue".into(),
            Statement::Return(None) => "return".into(),
            Statement::Return(Some(w)) => format!("return {}", word(w)),
            Statement::Exit(None) => "exit".into(),
            Statement::Exit(Some(w)) => format!("exit {}", word(w)),
        }
    }

    /// The statements of `text` written short, one string each.
    fn read(text: &str) -> Vec<String> {
        script(text).unwrap().iter().map(statement).collect()
    }

    fn error(text: &str) -> (usize, usize, Code) {
        let e = script(text).unwrap_err();
        (e.at.line, e.at.column, e.code)
    }

    #[test]
    fn commands_are_their_words_and_redirections() {
        assert_eq!(read("ls -l /System"), ["ls -l /System"]);
        assert_eq!(read("echo a > f b >> g < h"), ["echo a b >f >>g <h"]);
        assert_eq!(read("echo a>f"), ["echo a >f"]);
        assert_eq!(read("cat < in > out"), ["cat <in >out"]);
        assert_eq!(read(""), Vec::<String>::new());
        assert_eq!(read("\n\n;;\n"), Vec::<String>::new());
    }

    #[test]
    fn pipes_join_commands_and_words_join_pipelines() {
        assert_eq!(read("ls | grep a | wc"), ["ls | grep a | wc"]);
        assert_eq!(read("a and b or not c | d"), ["a and b or not c | d"]);
        let Statement::List(l) = &script("not a x or b and c").unwrap()[0] else {
            panic!("a list");
        };
        assert!(l.first.not);
        assert_eq!(command(&l.first.commands[0]), "a x");
        let joins: Vec<Join> = l.rest.iter().map(|(j, _)| *j).collect();
        assert_eq!(joins, [Join::Or, Join::And], "from the left, as written");
        assert!(!l.rest[0].1.not && !l.rest[1].1.not);
        assert_eq!(read("and x"), ["and x"], "not after a command, a word");
        assert_eq!(read("not and"), ["not and"]);
        assert_eq!(read("echo 'and' \"or\" not"), ["echo [and] [or] not"]);
        assert_eq!(read("echo x not y"), ["echo x not y"]);
    }

    #[test]
    fn statements_end_at_semicolons_and_lines() {
        assert_eq!(read("a; b\n\nc;"), ["a", "b", "c"]);
        assert_eq!(read("a # x\nb \\\n c"), ["a", "b c"]);
        assert_eq!(error("let x = a b"), (1, 11, Code::ExpectedEnd));
        assert_eq!(error("if a { b } c"), (1, 12, Code::ExpectedEnd));
        assert_eq!(error("return 1 2"), (1, 10, Code::ExpectedEnd));
        assert_eq!(error("break now"), (1, 7, Code::ExpectedEnd));
    }

    #[test]
    fn let_names_one_word() {
        assert_eq!(read("let x = 5"), ["let x=5"]);
        assert_eq!(read("let _a1 = \"$b c\""), ["let _a1=[]${b}[ c]"]);
        assert_eq!(error("let"), (1, 4, Code::ExpectedName));
        assert_eq!(error("let 1x = 2"), (1, 5, Code::ExpectedName));
        assert_eq!(error("let 'x' = 2"), (1, 5, Code::ExpectedName));
        assert_eq!(error("let a-b = 2"), (1, 5, Code::ExpectedName));
        assert_eq!(error("let x 5"), (1, 7, Code::ExpectedEquals));
        assert_eq!(error("let x '=' 5"), (1, 7, Code::ExpectedEquals));
        assert_eq!(error("let x\n= 5"), (1, 6, Code::ExpectedEquals));
        assert_eq!(error("let x ="), (1, 8, Code::ExpectedValue));
        assert_eq!(error("let x = | y"), (1, 9, Code::ExpectedValue));
    }

    #[test]
    fn if_takes_a_block_and_else_another() {
        assert_eq!(read("if a { b }"), ["if(a){b}else{}"]);
        assert_eq!(
            read("if a and b {\n  c\n  d\n} else {\n  e\n}"),
            ["if(a and b){c; d}else{e}"]
        );
        assert_eq!(
            read("if a { b } else if c { d } else { e }"),
            ["if(a){b}else{if(c){d}else{e}}"]
        );
        assert_eq!(read("if a {}"), ["if(a){}else{}"]);
        assert_eq!(read("if a { if b { c } }"), ["if(a){if(b){c}else{}}else{}"]);
        assert_eq!(
            error("if a { b }; else { c }"),
            (1, 13, Code::UnexpectedElse)
        );
    }

    #[test]
    fn blocks_open_on_their_line_and_close() {
        assert_eq!(error("if a\n{ b }"), (1, 5, Code::ExpectedBrace));
        assert_eq!(error("if a { b"), (1, 1, Code::UnclosedBlock));
        assert_eq!(error("x\n  while a {\n b\n"), (2, 3, Code::UnclosedBlock));
        assert_eq!(error("if a { b } else {"), (1, 12, Code::UnclosedBlock));
        assert_eq!(error("if a { b } else"), (1, 16, Code::ExpectedBrace));
        assert_eq!(error("else { x }"), (1, 1, Code::UnexpectedElse));
        assert_eq!(
            error("if a { b }\nelse { c }"),
            (2, 1, Code::UnexpectedElse)
        );
        assert_eq!(error("}"), (1, 1, Code::UnexpectedClose));
        assert_eq!(error("a\n b }"), (2, 4, Code::UnexpectedClose));
        assert_eq!(error("if { b }"), (1, 4, Code::MissingCommand));
        assert_eq!(error("echo {"), (1, 6, Code::UnexpectedOpen));
        assert_eq!(error("'if' a { b }"), (1, 8, Code::UnexpectedOpen));
    }

    #[test]
    fn for_runs_over_words() {
        assert_eq!(
            read("for f in *.txt a \"b c\" { echo $f }"),
            ["for f in(*.txt a [b c]){echo ${f}}"]
        );
        assert_eq!(read("for x in { y }"), ["for x in(){y}"]);
        assert_eq!(error("for { }"), (1, 5, Code::ExpectedName));
        assert_eq!(error("for $x in a { }"), (1, 5, Code::ExpectedName));
        assert_eq!(error("for x { }"), (1, 7, Code::ExpectedIn));
        assert_eq!(error("for x 'in' a { }"), (1, 7, Code::ExpectedIn));
        assert_eq!(error("for x in a | b { }"), (1, 12, Code::ExpectedBrace));
    }

    #[test]
    fn while_and_fn_take_blocks_too() {
        assert_eq!(
            read("while not ls /x { sleep 1 }"),
            ["while(not ls /x){sleep 1}"]
        );
        assert_eq!(
            read("fn greet {\n echo hi $1\n}"),
            ["fn greet{echo hi ${1}}"]
        );
        assert_eq!(error("fn { }"), (1, 4, Code::ExpectedName));
        assert_eq!(error("fn 9 { }"), (1, 4, Code::ExpectedName));
        assert_eq!(error("fn f"), (1, 5, Code::ExpectedBrace));
        assert_eq!(error("while a"), (1, 8, Code::ExpectedBrace));
    }

    #[test]
    fn loops_and_functions_end_early() {
        assert_eq!(
            read("for x in a { break; continue }"),
            ["for x in(a){break; continue}"]
        );
        assert_eq!(
            read("fn f { return; return 3 }"),
            ["fn f{return; return 3}"]
        );
        assert_eq!(error("return | x"), (1, 8, Code::ExpectedEnd));
    }

    #[test]
    fn a_command_is_where_one_has_to_be() {
        assert_eq!(error("| a"), (1, 1, Code::MissingCommand));
        assert_eq!(error("a |"), (1, 4, Code::MissingCommand));
        assert_eq!(error("a\nb |"), (2, 4, Code::MissingCommand));
        assert_eq!(error("a | | b"), (1, 5, Code::MissingCommand));
        assert_eq!(error("a |\nb"), (1, 4, Code::MissingCommand));
        assert_eq!(error("a and"), (1, 6, Code::MissingCommand));
        assert_eq!(error("echo and"), (1, 9, Code::MissingCommand));
        assert_eq!(error("a or ; b"), (1, 6, Code::MissingCommand));
        assert_eq!(error("not"), (1, 4, Code::MissingCommand));
        assert_eq!(error("> f"), (1, 1, Code::MissingCommand));
        assert_eq!(error("a | > f"), (1, 5, Code::MissingCommand));
    }

    #[test]
    fn a_redirection_needs_its_path() {
        assert_eq!(error("echo >"), (1, 7, Code::MissingPath));
        assert_eq!(error("echo > | x"), (1, 8, Code::MissingPath));
        assert_eq!(error("echo >> ; x"), (1, 9, Code::MissingPath));
        assert_eq!(error("cat <\n"), (1, 6, Code::MissingPath));
        assert_eq!(error("echo > >"), (1, 8, Code::MissingPath));
    }

    #[test]
    fn errors_from_reading_words_come_through() {
        assert_eq!(error("echo 'a"), (1, 6, Code::UnterminatedQuote));
        assert_eq!(error("a\rb"), (1, 2, Code::LoneCarriageReturn));
    }

    #[test]
    fn names_are_letters_digits_and_underscores() {
        for good in ["a", "_", "A_1", "_9z"] {
            assert!(is_name(good), "{good}");
        }
        for bad in ["", "1a", "a-b", "a b", "\u{e9}", "a.b"] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_script_ends_after_its_last_character() {
        assert_eq!(end_of(""), Pos { line: 1, column: 1 });
        assert_eq!(end_of("ab"), Pos { line: 1, column: 3 });
        assert_eq!(end_of("ab\n"), Pos { line: 2, column: 1 });
        assert_eq!(end_of("a\n\u{e9}t\u{e9}"), Pos { line: 2, column: 4 });
    }

    /// Scripts and their statements, written short: the start of the
    /// conformance suite for sections 3 to 6.
    const READ_AS: &[(&str, &[&str])] = &[
        ("a", &["a"]),
        ("a b", &["a b"]),
        ("a\nb", &["a", "b"]),
        ("a;b;c", &["a", "b", "c"]),
        ("a ;; b", &["a", "b"]),
        ("a | b", &["a | b"]),
        ("a|b|c|d", &["a | b | c | d"]),
        ("a > f", &["a >f"]),
        ("a >> f", &["a >>f"]),
        ("a < f", &["a <f"]),
        ("a < i > o", &["a <i >o"]),
        ("> f a", &["a >f"]),
        ("a > f | b < g", &["a >f | b <g"]),
        ("a > 'my file'", &["a >[my file]"]),
        ("a > $f", &["a >${f}"]),
        ("a and b", &["a and b"]),
        ("a or b", &["a or b"]),
        ("not a", &["not a"]),
        ("not a | b", &["not a | b"]),
        ("a and not b", &["a and not b"]),
        ("a | b and c | d", &["a | b and c | d"]),
        ("a\nand", &["a", "and"]),
        ("a; or b", &["a", "or b"]),
        ("a and and b", &["a and and b"]),
        ("not not", &["not not"]),
        ("a && b || c", &["a and b or c"]),
        ("a&&not b", &["a and not b"]),
        ("a || b | c", &["a or b | c"]),
        ("if a && b { c }", &["if(a and b){c}else{}"]),
        ("echo a&b", &["echo a&b"]),
        ("let a = b", &["let a=b"]),
        ("let a = 'b c'", &["let a=[b c]"]),
        ("let a = $b", &["let a=${b}"]),
        ("let a = b; c", &["let a=b", "c"]),
        ("let let = 1", &["let let=1"]),
        ("echo let", &["echo let"]),
        ("if a { b }", &["if(a){b}else{}"]),
        ("if a {\n}", &["if(a){}else{}"]),
        ("if a { b; c }", &["if(a){b; c}else{}"]),
        ("if a { b\nc }", &["if(a){b; c}else{}"]),
        ("if a | b { c }", &["if(a | b){c}else{}"]),
        ("if not a { b }", &["if(not a){b}else{}"]),
        ("if a > f { b }", &["if(a >f){b}else{}"]),
        ("a 2> e > o", &["a 2>e >o"]),
        ("a 2>>e x", &["a x 2>>e"]),
        ("a2> e", &["a2 >e"]),
        ("a > o 2>&1", &["a >o 2>&1"]),
        ("a 2>&1 | b", &["a 2>&1 | b"]),
        ("if a { b } else { c }", &["if(a){b}else{c}"]),
        ("if a {\n b\n} else {\n c\n}", &["if(a){b}else{c}"]),
        ("if a { b }; c", &["if(a){b}else{}", "c"]),
        ("if a { b }\nc", &["if(a){b}else{}", "c"]),
        (
            "if a { b } else if c { d } else if e { f }",
            &["if(a){b}else{if(c){d}else{if(e){f}else{}}}"],
        ),
        ("for x in a { b }", &["for x in(a){b}"]),
        ("for x in a b c { b }", &["for x in(a b c){b}"]),
        (
            "for x in $list { echo $x }",
            &["for x in(${list}){echo ${x}}"],
        ),
        ("for in in in { }", &["for in in(in){}"]),
        ("while a { b }", &["while(a){b}"]),
        ("while a and b { c; d }", &["while(a and b){c; d}"]),
        ("fn f { a }", &["fn f{a}"]),
        ("fn f {\n a\n b\n}", &["fn f{a; b}"]),
        (
            "fn f { if a { return 1 } }",
            &["fn f{if(a){return 1}else{}}"],
        ),
        ("break", &["break"]),
        ("continue", &["continue"]),
        ("return", &["return"]),
        ("return $?", &["return ${?}"]),
        ("return; a", &["return", "a"]),
        ("exit", &["exit"]),
        ("exit 3", &["exit 3"]),
        ("exit\na", &["exit", "a"]),
        ("echo exit", &["echo exit"]),
        ("'exit' 3", &["[exit] 3"]),
        ("echo break return fn", &["echo break return fn"]),
        ("'break'", &["[break]"]),
        ("a # if { x", &["a"]),
        ("if a { # c\n b }", &["if(a){b}else{}"]),
        ("  \t a  ", &["a"]),
    ];

    #[test]
    fn the_suite_reads_as_written() {
        for (text, want) in READ_AS {
            assert_eq!(read(text), *want, "{text:?}");
        }
    }

    /// Scripts that do not read, with the line, column and code of their
    /// first error.
    const REFUSED: &[(&str, usize, usize, Code)] = &[
        ("|", 1, 1, Code::MissingCommand),
        ("a ||", 1, 5, Code::MissingCommand),
        ("&& a", 1, 1, Code::MissingCommand),
        ("a && ;", 1, 6, Code::MissingCommand),
        ("a || || b", 1, 6, Code::MissingCommand),
        ("a | ;", 1, 5, Code::MissingCommand),
        ("not | a", 1, 5, Code::MissingCommand),
        ("a or\nb", 1, 5, Code::MissingCommand),
        ("<", 1, 2, Code::MissingPath),
        ("a 2>", 1, 5, Code::MissingPath),
        ("a 2>> ;", 1, 7, Code::MissingPath),
        ("a <", 1, 4, Code::MissingPath),
        ("a > {", 1, 5, Code::MissingPath),
        ("a > }", 1, 5, Code::MissingPath),
        ("a >\nf", 1, 4, Code::MissingPath),
        ("let = 1", 1, 5, Code::ExpectedName),
        ("let $x = 1", 1, 5, Code::ExpectedName),
        ("let x;", 1, 6, Code::ExpectedEquals),
        ("let x == 1", 1, 7, Code::ExpectedEquals),
        ("let x =\n1", 1, 8, Code::ExpectedValue),
        ("let x = {", 1, 9, Code::ExpectedValue),
        ("if a b", 1, 7, Code::ExpectedBrace),
        ("if a ; { b }", 1, 6, Code::ExpectedBrace),
        ("if a { b } else c", 1, 17, Code::ExpectedBrace),
        ("if a { b } else if { c }", 1, 20, Code::MissingCommand),
        ("for x in a\n{ }", 1, 11, Code::ExpectedBrace),
        ("for", 1, 4, Code::ExpectedName),
        ("for x", 1, 6, Code::ExpectedIn),
        ("for x in", 1, 9, Code::ExpectedBrace),
        ("fn", 1, 3, Code::ExpectedName),
        ("fn f x { }", 1, 6, Code::ExpectedBrace),
        ("while { }", 1, 7, Code::MissingCommand),
        ("if a {", 1, 1, Code::UnclosedBlock),
        ("fn f {\n a", 1, 1, Code::UnclosedBlock),
        ("if a { if b { c }", 1, 1, Code::UnclosedBlock),
        ("if a { b }\n  if c {", 2, 3, Code::UnclosedBlock),
        ("for x in a { b", 1, 1, Code::UnclosedBlock),
        ("a }", 1, 3, Code::UnexpectedClose),
        ("if a { b } }", 1, 12, Code::UnexpectedClose),
        ("a {", 1, 3, Code::UnexpectedOpen),
        ("a b {", 1, 5, Code::UnexpectedOpen),
        ("let x = 1 {", 1, 11, Code::UnexpectedOpen),
        ("else", 1, 1, Code::UnexpectedElse),
        ("if a { else }", 1, 8, Code::UnexpectedElse),
        ("a\nelse", 2, 1, Code::UnexpectedElse),
        ("break 2", 1, 7, Code::ExpectedEnd),
        ("continue x", 1, 10, Code::ExpectedEnd),
        ("fn f { } g", 1, 10, Code::ExpectedEnd),
        ("for x in a { } b", 1, 16, Code::ExpectedEnd),
        ("let x = 1 2", 1, 11, Code::ExpectedEnd),
        ("let x = 1 | y", 1, 11, Code::ExpectedEnd),
        ("return a b", 1, 10, Code::ExpectedEnd),
        ("exit 1 2", 1, 8, Code::ExpectedEnd),
        ("exit | a", 1, 6, Code::ExpectedEnd),
    ];

    #[test]
    fn the_suite_refuses_what_it_should_where_it_should() {
        let wrong: Vec<String> = REFUSED
            .iter()
            .filter_map(|&(text, line, column, code)| {
                let got = script(text).err().map(|e| (e.at.line, e.at.column, e.code));
                (got != Some((line, column, code))).then(|| format!("{text:?}: {got:?}"))
            })
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn every_part_of_a_statement_is_kept() {
        let statements = script("for x in a 'b' { cat < i > o | sort >> s }").unwrap();
        let [Statement::For { name, words, body }] = statements.as_slice() else {
            panic!("one for");
        };
        assert_eq!(name, "x");
        assert_eq!(words.iter().map(word).collect::<Vec<_>>(), ["a", "[b]"]);
        assert_eq!((words[0].at.column, words[1].at.column), (10, 12));
        let [Statement::List(l)] = body.as_slice() else {
            panic!("one list in it");
        };
        assert!(l.rest.is_empty() && !l.first.not);
        let [cat, sort] = l.first.commands.as_slice() else {
            panic!("two commands");
        };
        assert_eq!(cat.words.len(), 1);
        assert_eq!(
            cat.redirections
                .iter()
                .map(|(how, path)| (*how, path.as_ref().map(word)))
                .collect::<Vec<_>>(),
            [
                (Redirect::In, Some("i".into())),
                (Redirect::Out, Some("o".into()))
            ]
        );
        assert_eq!(sort.redirections[0].0, Redirect::Append);
        let lets = script("let n = \"v\"").unwrap();
        let [Statement::Let { name, value }] = lets.as_slice() else {
            panic!("one let");
        };
        assert_eq!((name.as_str(), word(value)), ("n", "[v]".into()));
    }

    #[test]
    fn cases_read_the_same_behind_blank_lines_and_inside_a_block() {
        for (text, want) in READ_AS {
            assert_eq!(read(&format!("\n\n{text}")), *want, "{text:?}");
            let inside = format!("if c {{\n{text}\n}}");
            assert_eq!(
                read(&inside),
                [format!("if(c){{{}}}else{{}}", want.join("; "))],
                "{text:?}"
            );
            let looped = format!("for x in a {{ {text}\n}}");
            assert_eq!(
                read(&looped),
                [format!("for x in(a){{{}}}", want.join("; "))],
                "{text:?}"
            );
        }
    }

    #[test]
    fn refusals_move_with_their_scripts() {
        for &(text, line, column, code) in REFUSED {
            assert_eq!(
                error(&format!("a\n\n{text}")),
                (line + 2, column, code),
                "{text:?}"
            );
            assert_eq!(
                error(&format!("a; {text}")),
                (line, column + if line == 1 { 3 } else { 0 }, code),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_example_the_specification_starts_with_reads() {
        let spec = include_str!("../../../docs/formats/shell.md");
        let example = spec.split("```\n").nth(1).unwrap();
        assert_eq!(
            read(example),
            [
                "let folder=/System/Notes",
                "md ${folder}",
                "for file in(*.txt){cp ${file} ${folder}; echo [copied ]${file}}",
                "if(cat ${folder}/todo.txt >/System/last.txt and not ls /System/Old){echo done}else{echo [something went wrong]}",
            ]
        );
    }

    #[test]
    fn scripts_people_write_read_as_they_mean() {
        let backup = "fn backup {
    for f in $1/*.meri {
        cp $f /System/Backup || echo \"could not copy $f\"
    }
}
backup /System
while not ls /System/ready {
    sleep 1
}
if ls /System/a && ls /System/b { echo both } else if ls /System/a { echo a } else { echo neither }
";
        assert_eq!(
            read(backup),
            [
                "fn backup{for f in(${1}/*.meri){cp ${f} /System/Backup or echo [could not copy ]${f}}}",
                "backup /System",
                "while(not ls /System/ready){sleep 1}",
                "if(ls /System/a and ls /System/b){echo both}else{if(ls /System/a){echo a}else{echo neither}}",
            ]
        );
        let counted = "let n = 0\nfor x in a b c {\n  if test $x = b { continue }\n  echo $x > /System/out.txt\n}\nreturn $n";
        assert_eq!(
            read(counted),
            [
                "let n=0",
                "for x in(a b c){if(test ${x} = b){continue}else{}; echo ${x} >/System/out.txt}",
                "return ${n}",
            ]
        );
    }

    #[test]
    fn positions_stay_with_words_however_deep() {
        let statements = script("if a {\n  for x in y {\n    echo  $x\n  }\n}").unwrap();
        let [
            Statement::If {
                condition, then, ..
            },
        ] = statements.as_slice()
        else {
            panic!("one if");
        };
        assert_eq!(
            condition.first.commands[0].words[0].at,
            Pos { line: 1, column: 4 }
        );
        let [Statement::For { words, body, .. }] = then.as_slice() else {
            panic!("a for in it");
        };
        assert_eq!(
            words[0].at,
            Pos {
                line: 2,
                column: 12
            }
        );
        let [Statement::List(echo)] = body.as_slice() else {
            panic!("an echo in that");
        };
        let at: Vec<Pos> = echo.first.commands[0].words.iter().map(|w| w.at).collect();
        assert_eq!(
            at,
            [
                Pos { line: 3, column: 5 },
                Pos {
                    line: 3,
                    column: 11
                }
            ]
        );
    }

    #[test]
    fn operators_need_no_spaces_around_them() {
        let cases = [
            ("a&&b||c|d>e", "a and b or c | d >e"),
            ("a>>f;b<g", "a >>f"),
            ("a|b&&c", "a | b and c"),
            ("x>'y z'", "x >[y z]"),
            ("if a&&b{c}", "if(a and b){c}else{}"),
            ("for i in a b{c}", "for i in(a b){c}"),
            ("while a||b{c}", "while(a or b){c}"),
            ("fn f{a;b}", "fn f{a; b}"),
            ("if a{b}else{c}", "if(a){b}else{c}"),
            ("a&& \\\n b", "a and b"),
        ];
        for (text, first) in cases {
            assert_eq!(read(text)[0], first, "{text:?}");
        }
        assert_eq!(read("a>>f;b<g"), ["a >>f", "b <g"]);
        assert_eq!(
            error("a&&||b"),
            (1, 4, Code::MissingCommand),
            "two joins in a row leave out a command"
        );
        assert_eq!(error("a{b}"), (1, 2, Code::UnexpectedOpen));
    }
}
