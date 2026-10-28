//! An awk program read into its tree, as POSIX's grammar has it: rules
//! of a pattern and an action, `BEGIN` and `END` actions, and functions.
//! Names become places: a variable of the whole program by its number,
//! a function's parameter by its own. Regular expressions written in the
//! program are read once, here.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use regex::{Regex, Syntax};

use crate::lex::{self, Builtin, Error, Keyword, Placed, Token};

/// The variables awk keeps itself, at these places among the program's.
pub const SPECIAL: [&str; 16] = [
    "NR", "NF", "FNR", "FS", "OFS", "ORS", "RS", "FILENAME", "SUBSEP", "RSTART", "RLENGTH",
    "CONVFMT", "OFMT", "ENVIRON", "ARGC", "ARGV",
];
pub const NR: usize = 0;
pub const NF: usize = 1;
pub const FNR: usize = 2;
pub const FS: usize = 3;
pub const OFS: usize = 4;
pub const ORS: usize = 5;
pub const RS: usize = 6;
pub const FILENAME: usize = 7;
pub const SUBSEP: usize = 8;
pub const RSTART: usize = 9;
pub const RLENGTH: usize = 10;
pub const CONVFMT: usize = 11;
pub const OFMT: usize = 12;
pub const ENVIRON: usize = 13;
pub const ARGC: usize = 14;
pub const ARGV: usize = 15;

/// Where a variable is: among the program's, or among the parameters of
/// the function running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    Global(usize),
    Local(usize),
}

/// What can be assigned to.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Var(Var),
    /// An element of an array, by its subscripts.
    Element(Var, Vec<Expr>),
    Field(Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Less,
    LessEqual,
    Equal,
    NotEqual,
    Greater,
    GreaterEqual,
}

/// Where `getline` reads.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The input the rules read.
    Main,
    File(Box<Expr>),
    /// The output of a command.
    Command(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    String(Vec<u8>),
    /// A regular expression written in the program, by its place among
    /// [`Program::regexes`]; alone it matches `$0`.
    Regex(usize),
    Var(Var),
    Place(Box<Place>),
    /// An assignment, with the operation `op=` does first.
    Assign(Option<Arith>, Box<Place>, Box<Expr>),
    /// `++` or `--`, before the place or after it.
    Step {
        place: Box<Place>,
        by: f64,
        before: bool,
    },
    Negate(Box<Expr>),
    Plus(Box<Expr>),
    Not(Box<Expr>),
    Arith(Arith, Box<Expr>, Box<Expr>),
    Concat(Box<Expr>, Box<Expr>),
    Compare(Relation, Box<Expr>, Box<Expr>),
    /// `~`, or `!~` when not.
    Match {
        not: bool,
        text: Box<Expr>,
        regex: Box<Expr>,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Whether the array has an element with these subscripts.
    In(Vec<Expr>, Var),
    /// A function of the program, by its place among
    /// [`Program::functions`].
    Call(usize, Vec<Expr>),
    Builtin(Builtin, Vec<Expr>),
    Getline {
        from: Source,
        into: Option<Box<Place>>,
    },
}

/// Where `print` and `printf` write.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    Standard,
    /// A file, made anew the first time.
    File(Expr),
    Append(Expr),
    /// The input of a command.
    Pipe(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Print(Vec<Expr>, Output),
    Printf(Vec<Expr>, Output),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    Do(Box<Stmt>, Expr),
    For {
        start: Option<Expr>,
        test: Option<Expr>,
        step: Option<Expr>,
        body: Box<Stmt>,
    },
    /// Each subscript of the array in turn, in the variable.
    ForIn(Var, Var, Box<Stmt>),
    Block(Vec<Stmt>),
    Break,
    Continue,
    Next,
    Nextfile,
    Exit(Option<Expr>),
    Return(Option<Expr>),
    /// An element of an array, or all of them.
    Delete(Var, Option<Vec<Expr>>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    All,
    Expr(Expr),
    /// From a record the first matches to the next the second does.
    Range(Expr, Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub pattern: Pattern,
    /// None writes the record.
    pub action: Option<Vec<Stmt>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: usize,
    pub body: Vec<Stmt>,
}

#[derive(Debug)]
pub struct Program {
    pub begin: Vec<Stmt>,
    pub rules: Vec<Rule>,
    pub end: Vec<Stmt>,
    pub functions: Vec<Function>,
    /// The program's variables by their places, [`SPECIAL`] first.
    pub globals: Vec<String>,
    pub regexes: Vec<Regex>,
}

/// The names the program gives functions, in the order it defines them,
/// found before anything is read so that a call may come first.
fn function_names(tokens: &[Placed]) -> Vec<String> {
    tokens
        .windows(2)
        .filter_map(|pair| match pair {
            [
                (Token::Keyword(Keyword::Function), _),
                (Token::Name(name) | Token::FuncName(name), _),
            ] => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// How deep expressions and statements may nest in each other, and how
/// many operations an expression may join in a row: running a program
/// goes down its tree by recursion, which must fit the stack a program
/// on Meridian has.
pub const NESTING: usize = 100;
pub const CHAIN: usize = 1000;

/// The ranks of the operators that join two operands, the loosest
/// first, as POSIX's grammar orders them.
const OR: u8 = 1;
const AND: u8 = 2;
const IN: u8 = 3;
const MATCH: u8 = 4;
const COMPARE: u8 = 5;
/// `| getline`, which reads a command a concatenation names.
const PIPE: u8 = 6;
const CONCAT: u8 = 7;
const ADD: u8 = 8;
const MUL: u8 = 9;

/// An operator that joins two operands.
#[derive(Clone, Copy)]
enum Infix {
    Or,
    And,
    In,
    Match(bool),
    Compare(Relation),
    Getline,
    Concat,
    Arith(Arith),
}

struct Parser {
    tokens: Vec<Placed>,
    at: usize,
    functions: Vec<String>,
    globals: Vec<String>,
    /// The parameters of the function being read.
    params: Vec<String>,
    regexes: Vec<Regex>,
    /// Whether `>` ends an expression rather than comparing, as it does
    /// among what `print` writes outside parentheses.
    printing: bool,
    /// How deep the expression or statement being read is.
    nesting: usize,
    /// How many loops, and functions, the statement being read is in.
    loops: usize,
    in_function: bool,
}

/// What a token is called in a message.
fn named(token: Option<&Token>) -> String {
    match token {
        None => String::from("the end"),
        Some(Token::Newline) => String::from("the end of a line"),
        Some(Token::Number(_)) => String::from("a number"),
        Some(Token::String(_)) => String::from("a string"),
        Some(Token::Regex(_)) => String::from("a regular expression"),
        Some(Token::Name(name) | Token::FuncName(name)) => name.clone(),
        Some(other) => format!("{other:?}"),
    }
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    fn peek_at(&self, ahead: usize) -> Option<&Token> {
        self.tokens.get(self.at + ahead).map(|(token, _)| token)
    }

    fn line(&self) -> usize {
        self.tokens
            .get(self.at)
            .or_else(|| self.tokens.last())
            .map_or(1, |&(_, line)| line)
    }

    fn error(&self, what: &str) -> Error {
        Error {
            line: self.line(),
            what: String::from(what),
        }
    }

    /// An error for the token at hand, which `wanted` should have been.
    fn wanted(&self, wanted: &str) -> Error {
        self.error(&format!("{wanted} was wanted, not {}", named(self.peek())))
    }

    fn take(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).map(|(token, _)| token.clone());
        self.at += 1;
        token
    }

    fn eat(&mut self, token: &Token) -> bool {
        let found = self.peek() == Some(token);
        if found {
            self.at += 1;
        }
        found
    }

    fn expect(&mut self, token: &Token, wanted: &str) -> Result<(), Error> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.wanted(wanted))
        }
    }

    fn newlines(&mut self) {
        while self.eat(&Token::Newline) {}
    }

    /// Newlines and semicolons, which part statements and rules.
    fn separators(&mut self) {
        while self.eat(&Token::Newline) || self.eat(&Token::Semicolon) {}
    }

    /// The place of the variable `name`: a parameter of the function
    /// being read, or one of the program's.
    fn var(&mut self, name: &str) -> Result<Var, Error> {
        if self.functions.iter().any(|f| f == name) {
            return Err(self.error(&format!("{name} is a function, not a variable")));
        }
        if let Some(at) = self.params.iter().position(|p| p == name) {
            return Ok(Var::Local(at));
        }
        if let Some(at) = self.globals.iter().position(|g| g == name) {
            return Ok(Var::Global(at));
        }
        self.globals.push(String::from(name));
        Ok(Var::Global(self.globals.len() - 1))
    }

    /// The regular expression `pattern`, read once and kept.
    fn regex(&mut self, pattern: &[u8]) -> Result<Expr, Error> {
        let text = String::from_utf8_lossy(pattern);
        let regex = Regex::new(&text, Syntax::Extended, false)
            .map_err(|e| self.error(&format!("/{text}/: {}", e.code.says())))?;
        self.regexes.push(regex);
        Ok(Expr::Regex(self.regexes.len() - 1))
    }

    fn program(&mut self) -> Result<Program, Error> {
        let mut begin = Vec::new();
        let mut rules = Vec::new();
        let mut end = Vec::new();
        let mut functions: Vec<Option<Function>> = self.functions.iter().map(|_| None).collect();
        self.separators();
        while self.peek().is_some() {
            match self.peek() {
                Some(Token::Keyword(Keyword::Begin)) => {
                    self.at += 1;
                    self.newlines();
                    begin.extend(self.action()?);
                }
                Some(Token::Keyword(Keyword::End)) => {
                    self.at += 1;
                    self.newlines();
                    end.extend(self.action()?);
                }
                Some(Token::Keyword(Keyword::Function)) => {
                    let function = self.function()?;
                    let at = self.functions.iter().position(|f| *f == function.name);
                    let slot = at.and_then(|at| functions.get_mut(at));
                    match slot {
                        Some(slot @ None) => *slot = Some(function),
                        _ => {
                            return Err(self.error(&format!(
                                "the function {} is defined twice",
                                function.name
                            )));
                        }
                    }
                }
                _ => rules.push(self.rule()?),
            }
            let braced = matches!(self.tokens.get(self.at - 1), Some((Token::CloseBrace, _)));
            if !braced && !matches!(self.peek(), None | Some(Token::Newline | Token::Semicolon)) {
                return Err(self.wanted("a new line or ;"));
            }
            self.separators();
        }
        Ok(Program {
            begin,
            rules,
            end,
            functions: functions.into_iter().flatten().collect(),
            globals: core::mem::take(&mut self.globals),
            regexes: core::mem::take(&mut self.regexes),
        })
    }

    fn rule(&mut self) -> Result<Rule, Error> {
        if self.peek() == Some(&Token::OpenBrace) {
            return Ok(Rule {
                pattern: Pattern::All,
                action: Some(self.action()?),
            });
        }
        let first = self.expression()?;
        let pattern = if self.eat(&Token::Comma) {
            self.newlines();
            Pattern::Range(first, self.expression()?)
        } else {
            Pattern::Expr(first)
        };
        let action = if self.peek() == Some(&Token::OpenBrace) {
            Some(self.action()?)
        } else {
            None
        };
        Ok(Rule { pattern, action })
    }

    fn function(&mut self) -> Result<Function, Error> {
        self.at += 1;
        let name = self.name("the function's name")?;
        self.expect(&Token::OpenParen, "(")?;
        let mut params = Vec::new();
        if !self.eat(&Token::CloseParen) {
            loop {
                self.newlines();
                let param = self.name("a parameter")?;
                if param == name || params.contains(&param) || SPECIAL.contains(&param.as_str()) {
                    self.at -= 1;
                    return Err(self.error(&format!("{param} cannot be a parameter of {name}")));
                }
                params.push(param);
                self.newlines();
                if self.eat(&Token::CloseParen) {
                    break;
                }
                self.expect(&Token::Comma, ", or )")?;
            }
        }
        self.newlines();
        self.params = params;
        self.in_function = true;
        let body = self.action();
        self.in_function = false;
        let params = core::mem::take(&mut self.params).len();
        Ok(Function {
            name,
            params,
            body: body?,
        })
    }

    /// `{` statements `}`.
    fn action(&mut self) -> Result<Vec<Stmt>, Error> {
        self.expect(&Token::OpenBrace, "{")?;
        let mut statements = Vec::new();
        loop {
            self.separators();
            if self.eat(&Token::CloseBrace) {
                return Ok(statements);
            }
            if self.peek().is_none() {
                return Err(self.wanted("}"));
            }
            statements.push(self.statement()?);
        }
    }

    /// Whether the token at hand ends a simple statement: `;`, a newline,
    /// `}` or the end, of which the first two are taken.
    fn ended(&mut self) -> bool {
        match self.peek() {
            Some(Token::Semicolon | Token::Newline) => {
                self.at += 1;
                true
            }
            None | Some(Token::CloseBrace) => true,
            _ => false,
        }
    }

    /// A simple statement, ended.
    fn simple(&mut self, statement: Stmt) -> Result<Stmt, Error> {
        if self.ended() {
            Ok(statement)
        } else {
            Err(self.wanted("the end of the statement"))
        }
    }

    /// A statement as the body of `if`, `else` or a loop: one after any
    /// newlines, or `;` alone for none.
    fn body(&mut self) -> Result<Stmt, Error> {
        self.newlines();
        if self.eat(&Token::Semicolon) {
            return Ok(Stmt::Block(Vec::new()));
        }
        self.statement()
    }

    fn looped(&mut self) -> Result<Stmt, Error> {
        self.loops += 1;
        let body = self.body();
        self.loops -= 1;
        body
    }

    fn condition(&mut self) -> Result<Expr, Error> {
        self.expect(&Token::OpenParen, "(")?;
        let condition = self.expression()?;
        self.expect(&Token::CloseParen, ")")?;
        Ok(condition)
    }

    fn statement(&mut self) -> Result<Stmt, Error> {
        self.nested(Self::statement_here)
    }

    fn statement_here(&mut self) -> Result<Stmt, Error> {
        let Some(Token::Keyword(keyword)) = self.peek() else {
            if self.peek() == Some(&Token::OpenBrace) {
                return Ok(Stmt::Block(self.action()?));
            }
            if self.eat(&Token::Semicolon) {
                return Ok(Stmt::Block(Vec::new()));
            }
            let expr = self.expression()?;
            return self.simple(Stmt::Expr(expr));
        };
        let keyword = *keyword;
        if keyword == Keyword::Getline {
            let expr = self.expression()?;
            return self.simple(Stmt::Expr(expr));
        }
        self.at += 1;
        match keyword {
            Keyword::If => {
                let condition = self.condition()?;
                let then = self.body()?;
                let before = self.at;
                self.separators();
                if self.eat(&Token::Keyword(Keyword::Else)) {
                    let otherwise = self.body()?;
                    return Ok(Stmt::If(
                        condition,
                        Box::new(then),
                        Some(Box::new(otherwise)),
                    ));
                }
                self.at = before;
                Ok(Stmt::If(condition, Box::new(then), None))
            }
            Keyword::While => {
                let condition = self.condition()?;
                Ok(Stmt::While(condition, Box::new(self.looped()?)))
            }
            Keyword::Do => {
                let body = self.looped()?;
                self.separators();
                self.expect(&Token::Keyword(Keyword::While), "while")?;
                let condition = self.condition()?;
                self.simple(Stmt::Do(Box::new(body), condition))
            }
            Keyword::For => self.for_loop(),
            Keyword::Break | Keyword::Continue => {
                if self.loops == 0 {
                    self.at -= 1;
                    return Err(self.error("break and continue go only in a loop"));
                }
                self.simple(if keyword == Keyword::Break {
                    Stmt::Break
                } else {
                    Stmt::Continue
                })
            }
            Keyword::Next | Keyword::Nextfile => self.simple(if keyword == Keyword::Next {
                Stmt::Next
            } else {
                Stmt::Nextfile
            }),
            Keyword::Exit => {
                let code = if self.peek_ends() {
                    None
                } else {
                    Some(self.expression()?)
                };
                self.simple(Stmt::Exit(code))
            }
            Keyword::Return => {
                if !self.in_function {
                    self.at -= 1;
                    return Err(self.error("return goes only in a function"));
                }
                let value = if self.peek_ends() {
                    None
                } else {
                    Some(self.expression()?)
                };
                self.simple(Stmt::Return(value))
            }
            Keyword::Delete => {
                let array = self.array()?;
                let subscripts = if self.eat(&Token::OpenBracket) {
                    Some(self.subscripts()?)
                } else {
                    None
                };
                self.simple(Stmt::Delete(array, subscripts))
            }
            Keyword::Print | Keyword::Printf => self.print(keyword == Keyword::Printf),
            _ => {
                self.at -= 1;
                Err(self.wanted("a statement"))
            }
        }
    }

    /// Whether the token at hand ends a statement, without taking it.
    fn peek_ends(&self) -> bool {
        matches!(
            self.peek(),
            None | Some(Token::Semicolon | Token::Newline | Token::CloseBrace)
        )
    }

    fn for_loop(&mut self) -> Result<Stmt, Error> {
        self.expect(&Token::OpenParen, "(")?;
        if let (
            Some(Token::Name(name)),
            Some(Token::Keyword(Keyword::In)),
            Some(Token::Name(_)),
            Some(Token::CloseParen),
        ) = (
            self.peek(),
            self.peek_at(1),
            self.peek_at(2),
            self.peek_at(3),
        ) {
            let name = name.clone();
            let var = self.var(&name)?;
            self.at += 2;
            let array = self.array()?;
            self.at += 1;
            return Ok(Stmt::ForIn(var, array, Box::new(self.looped()?)));
        }
        let start = self.optional(&Token::Semicolon)?;
        self.expect(&Token::Semicolon, ";")?;
        self.newlines();
        let test = self.optional(&Token::Semicolon)?;
        self.expect(&Token::Semicolon, ";")?;
        self.newlines();
        let step = self.optional(&Token::CloseParen)?;
        self.expect(&Token::CloseParen, ")")?;
        let body = Box::new(self.looped()?);
        Ok(Stmt::For {
            start,
            test,
            step,
            body,
        })
    }

    /// An expression, or none when `end` comes first.
    fn optional(&mut self, end: &Token) -> Result<Option<Expr>, Error> {
        if self.peek() == Some(end) {
            Ok(None)
        } else {
            self.expression().map(Some)
        }
    }

    /// The array named at hand.
    fn array(&mut self) -> Result<Var, Error> {
        let name = self.name("the name of an array")?;
        self.var(&name)
    }

    /// The name at hand, taken.
    fn name(&mut self, wanted: &str) -> Result<String, Error> {
        if let Some(Token::Name(name) | Token::FuncName(name)) = self.peek() {
            let name = name.clone();
            self.at += 1;
            return Ok(name);
        }
        Err(self.wanted(wanted))
    }

    /// Subscripts after `[`, and the `]` that ends them.
    fn subscripts(&mut self) -> Result<Vec<Expr>, Error> {
        let printing = core::mem::replace(&mut self.printing, false);
        let list = self.list();
        self.printing = printing;
        let list = list?;
        self.expect(&Token::CloseBracket, "]")?;
        Ok(list)
    }

    /// Expressions parted by commas, a newline allowed after each comma.
    fn list(&mut self) -> Result<Vec<Expr>, Error> {
        let mut list = alloc::vec![self.expression()?];
        while self.eat(&Token::Comma) {
            self.newlines();
            list.push(self.expression()?);
        }
        Ok(list)
    }

    fn print(&mut self, formatted: bool) -> Result<Stmt, Error> {
        let mut args = Vec::new();
        if !self.peek_ends()
            && !matches!(
                self.peek(),
                Some(Token::Greater | Token::Append | Token::Pipe)
            )
        {
            args = self.print_args()?;
        }
        let output = match self.peek() {
            Some(Token::Greater) => Some(Output::File as fn(Expr) -> Output),
            Some(Token::Append) => Some(Output::Append as fn(Expr) -> Output),
            Some(Token::Pipe) => Some(Output::Pipe as fn(Expr) -> Output),
            _ => None,
        };
        let output = match output {
            Some(output) => {
                self.at += 1;
                output(self.binary(CONCAT)?)
            }
            None => Output::Standard,
        };
        if formatted && args.is_empty() {
            return Err(self.error("printf wants a format"));
        }
        self.simple(if formatted {
            Stmt::Printf(args, output)
        } else {
            Stmt::Print(args, output)
        })
    }

    /// What `print` writes: a list in parentheses, when the statement
    /// ends or its output is named after it, or else a list in which `>`
    /// names the output.
    fn print_args(&mut self) -> Result<Vec<Expr>, Error> {
        if self.peek() == Some(&Token::OpenParen) {
            let before = self.at;
            self.at += 1;
            if let Ok(list) = self.list()
                && self.eat(&Token::CloseParen)
                && (self.peek_ends()
                    || matches!(
                        self.peek(),
                        Some(Token::Greater | Token::Append | Token::Pipe)
                    ))
            {
                return Ok(list);
            }
            self.at = before;
        }
        self.printing = true;
        let list = self.list();
        self.printing = false;
        list
    }

    /// A whole expression, assignments and all.
    fn expression(&mut self) -> Result<Expr, Error> {
        self.nested(Self::assignment)
    }

    /// Reads with `read` one level deeper among expressions and
    /// statements.
    fn nested<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T, Error>) -> Result<T, Error> {
        if self.nesting >= NESTING {
            return Err(self.error(&format!(
                "expressions and statements nested more than {NESTING} deep"
            )));
        }
        self.nesting += 1;
        let read = read(self);
        self.nesting -= 1;
        read
    }

    /// An expression, which may assign.
    fn assignment(&mut self) -> Result<Expr, Error> {
        let printing = self.printing;
        let expr = self.ternary()?;
        let op = match self.peek() {
            Some(Token::Assign) => None,
            Some(Token::AddAssign) => Some(Arith::Add),
            Some(Token::SubAssign) => Some(Arith::Sub),
            Some(Token::MulAssign) => Some(Arith::Mul),
            Some(Token::DivAssign) => Some(Arith::Div),
            Some(Token::ModAssign) => Some(Arith::Mod),
            Some(Token::PowAssign) => Some(Arith::Pow),
            _ => return Ok(expr),
        };
        let Some(place) = place(&expr) else {
            return Err(self.error("only a variable, an element or a field can be assigned"));
        };
        self.at += 1;
        self.newlines();
        self.printing = printing;
        let value = self.expression()?;
        Ok(Expr::Assign(op, Box::new(place), Box::new(value)))
    }

    fn ternary(&mut self) -> Result<Expr, Error> {
        let test = self.binary(OR)?;
        if !self.eat(&Token::Question) {
            return Ok(test);
        }
        self.newlines();
        let yes = self.expression()?;
        self.newlines();
        self.expect(&Token::Colon, ":")?;
        self.newlines();
        let no = self.expression()?;
        Ok(Expr::Cond(Box::new(test), Box::new(yes), Box::new(no)))
    }

    /// The operator at hand that joins two operands, and its rank, when
    /// one does: a concatenation where an operand begins with no sign.
    fn infix(&self) -> Option<(u8, Infix)> {
        Some(match self.peek()? {
            Token::Or => (OR, Infix::Or),
            Token::And => (AND, Infix::And),
            Token::Keyword(Keyword::In) => (IN, Infix::In),
            Token::Tilde => (MATCH, Infix::Match(false)),
            Token::NoMatch => (MATCH, Infix::Match(true)),
            Token::Less => (COMPARE, Infix::Compare(Relation::Less)),
            Token::LessEqual => (COMPARE, Infix::Compare(Relation::LessEqual)),
            Token::Equal => (COMPARE, Infix::Compare(Relation::Equal)),
            Token::NotEqual => (COMPARE, Infix::Compare(Relation::NotEqual)),
            Token::Greater if !self.printing => (COMPARE, Infix::Compare(Relation::Greater)),
            Token::GreaterEqual => (COMPARE, Infix::Compare(Relation::GreaterEqual)),
            Token::Pipe if self.peek_at(1) == Some(&Token::Keyword(Keyword::Getline)) => {
                (PIPE, Infix::Getline)
            }
            Token::Plus => (ADD, Infix::Arith(Arith::Add)),
            Token::Minus => (ADD, Infix::Arith(Arith::Sub)),
            Token::Star => (MUL, Infix::Arith(Arith::Mul)),
            Token::Slash => (MUL, Infix::Arith(Arith::Div)),
            Token::Percent => (MUL, Infix::Arith(Arith::Mod)),
            _ if self.starts_operand() => (CONCAT, Infix::Concat),
            _ => return None,
        })
    }

    /// Operands joined by operators of the rank `least` and above, those
    /// of each rank from the left, but comparisons, of which one only
    /// joins two operands.
    fn binary(&mut self, least: u8) -> Result<Expr, Error> {
        let mut left = self.unary()?;
        let mut compared = false;
        let mut joined = 0;
        while let Some((rank, infix)) = self.infix() {
            if rank < least || (rank == COMPARE && compared) {
                break;
            }
            joined += 1;
            if joined > CHAIN {
                return Err(self.error(&format!("more than {CHAIN} operations in a row")));
            }
            if !matches!(infix, Infix::Concat) {
                self.at += 1;
            }
            let pair = |left, right| (Box::new(left), Box::new(right));
            left = match infix {
                Infix::Or | Infix::And => {
                    self.newlines();
                    let (a, b) = pair(left, self.binary(rank + 1)?);
                    if matches!(infix, Infix::Or) {
                        Expr::Or(a, b)
                    } else {
                        Expr::And(a, b)
                    }
                }
                Infix::In => Expr::In(alloc::vec![left], self.array()?),
                Infix::Match(not) => {
                    let (text, regex) = pair(left, self.binary(rank + 1)?);
                    Expr::Match { not, text, regex }
                }
                Infix::Compare(relation) => {
                    compared = true;
                    let (a, b) = pair(left, self.binary(rank + 1)?);
                    Expr::Compare(relation, a, b)
                }
                Infix::Getline => {
                    self.at += 1;
                    let into = self.getline_place()?;
                    Expr::Getline {
                        from: Source::Command(Box::new(left)),
                        into,
                    }
                }
                Infix::Concat => {
                    let (a, b) = pair(left, self.binary(rank + 1)?);
                    Expr::Concat(a, b)
                }
                Infix::Arith(op) => {
                    let (a, b) = pair(left, self.binary(rank + 1)?);
                    Expr::Arith(op, a, b)
                }
            };
        }
        Ok(left)
    }

    /// Whether the token at hand begins an operand that may follow
    /// another to be joined to it: anything but a sign, which would
    /// add or take away instead.
    fn starts_operand(&self) -> bool {
        matches!(
            self.peek(),
            Some(
                Token::Number(_)
                    | Token::String(_)
                    | Token::Regex(_)
                    | Token::Name(_)
                    | Token::FuncName(_)
                    | Token::Builtin(_)
                    | Token::Dollar
                    | Token::Not
                    | Token::OpenParen
                    | Token::Incr
                    | Token::Decr,
            )
        )
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        let wrap: fn(Box<Expr>) -> Expr = match self.peek() {
            Some(Token::Not) => Expr::Not,
            Some(Token::Minus) => Expr::Negate,
            Some(Token::Plus) => Expr::Plus,
            _ => return self.power(),
        };
        self.at += 1;
        Ok(wrap(Box::new(self.nested(Self::unary)?)))
    }

    /// `^`, which goes from the right and binds tighter than a sign on
    /// its left, though one may come after it.
    fn power(&mut self) -> Result<Expr, Error> {
        let base = self.postfix()?;
        if !self.eat(&Token::Caret) {
            return Ok(base);
        }
        let exponent = self.nested(Self::unary)?;
        Ok(Expr::Arith(Arith::Pow, Box::new(base), Box::new(exponent)))
    }

    fn postfix(&mut self) -> Result<Expr, Error> {
        let expr = self.primary()?;
        let by = match self.peek() {
            Some(Token::Incr) => 1.0,
            Some(Token::Decr) => -1.0,
            _ => return Ok(expr),
        };
        let Some(place) = place(&expr) else {
            return Ok(expr);
        };
        self.at += 1;
        Ok(Expr::Step {
            place: Box::new(place),
            by,
            before: false,
        })
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let Some(token) = self.take() else {
            return Err(self.wanted("an expression"));
        };
        match token {
            Token::Number(n) => Ok(Expr::Number(n)),
            Token::String(bytes) => Ok(Expr::String(bytes)),
            Token::Regex(pattern) => {
                // An expression that cannot be read is told on its line.
                self.at -= 1;
                let regex = self.regex(&pattern)?;
                self.at += 1;
                Ok(regex)
            }
            Token::OpenParen => {
                let printing = core::mem::replace(&mut self.printing, false);
                let list = self.list();
                self.printing = printing;
                let list = list?;
                self.expect(&Token::CloseParen, ")")?;
                if list.len() == 1 {
                    return Ok(list.into_iter().next().unwrap_or(Expr::Number(0.0)));
                }
                if !self.eat(&Token::Keyword(Keyword::In)) {
                    return Err(self.wanted("in after a list in parentheses"));
                }
                let array = self.array()?;
                Ok(Expr::In(list, array))
            }
            Token::Dollar => {
                let index = if matches!(self.peek(), Some(Token::Minus | Token::Plus | Token::Not))
                {
                    self.nested(Self::unary)?
                } else {
                    self.nested(Self::primary)?
                };
                Ok(Expr::Place(Box::new(Place::Field(Box::new(index)))))
            }
            Token::Incr | Token::Decr => {
                let by = if token == Token::Incr { 1.0 } else { -1.0 };
                let operand = self.nested(Self::primary)?;
                let Some(place) = place(&operand) else {
                    return Err(
                        self.error("++ and -- go only with a variable, an element or a field")
                    );
                };
                Ok(Expr::Step {
                    place: Box::new(place),
                    by,
                    before: true,
                })
            }
            Token::Name(name) => {
                let var = self.var(&name)?;
                if self.eat(&Token::OpenBracket) {
                    let subscripts = self.subscripts()?;
                    return Ok(Expr::Place(Box::new(Place::Element(var, subscripts))));
                }
                Ok(Expr::Var(var))
            }
            Token::FuncName(name) => {
                let Some(function) = self.functions.iter().position(|f| *f == name) else {
                    self.at -= 1;
                    return Err(self.error(&format!("no function is called {name}")));
                };
                self.at += 1;
                let args = self.args()?;
                Ok(Expr::Call(function, args))
            }
            Token::Builtin(builtin) => self.builtin(builtin),
            Token::Keyword(Keyword::Getline) => {
                let into = self.getline_place()?;
                let from = if self.eat(&Token::Less) {
                    Source::File(Box::new(self.postfix()?))
                } else {
                    Source::Main
                };
                Ok(Expr::Getline { from, into })
            }
            _ => {
                self.at -= 1;
                Err(self.wanted("an expression"))
            }
        }
    }

    /// The place `getline` reads into, when one follows it.
    fn getline_place(&mut self) -> Result<Option<Box<Place>>, Error> {
        if !matches!(self.peek(), Some(Token::Name(_) | Token::Dollar)) {
            return Ok(None);
        }
        let expr = self.primary()?;
        Ok(place(&expr).map(Box::new))
    }

    /// The arguments of a call, after its `(`, and the `)` that ends them.
    fn args(&mut self) -> Result<Vec<Expr>, Error> {
        self.newlines();
        if self.eat(&Token::CloseParen) {
            return Ok(Vec::new());
        }
        let printing = core::mem::replace(&mut self.printing, false);
        let list = self.list();
        self.printing = printing;
        let list = list?;
        self.newlines();
        self.expect(&Token::CloseParen, ")")?;
        Ok(list)
    }

    fn builtin(&mut self, builtin: Builtin) -> Result<Expr, Error> {
        if builtin == Builtin::Length && self.peek() != Some(&Token::OpenParen) {
            return Ok(Expr::Builtin(builtin, Vec::new()));
        }
        self.expect(&Token::OpenParen, "(")?;
        let args = self.args()?;
        let (least, most) = match builtin {
            Builtin::Length | Builtin::Srand | Builtin::Fflush => (0, 1),
            Builtin::Rand => (0, 0),
            Builtin::Atan2 | Builtin::Index | Builtin::Match => (2, 2),
            Builtin::Split | Builtin::Sub | Builtin::Gsub | Builtin::Substr => (2, 3),
            Builtin::Sprintf => (1, usize::MAX),
            _ => (1, 1),
        };
        if args.len() < least || args.len() > most {
            return Err(self.error(&format!(
                "{} takes {}",
                builtin_name(builtin),
                match (least, most) {
                    (0, 0) => String::from("no arguments"),
                    (l, m) if l == m => format!("{l} arguments"),
                    (l, usize::MAX) => format!("{l} arguments or more"),
                    (l, m) => format!("from {l} to {m} arguments"),
                }
            )));
        }
        if builtin == Builtin::Split && !matches!(args[1], Expr::Var(_)) {
            return Err(self.error("split puts what it splits into an array"));
        }
        if matches!(builtin, Builtin::Sub | Builtin::Gsub)
            && args.get(2).is_some_and(|a| place(a).is_none())
        {
            return Err(self.error("sub and gsub change a variable, an element or a field"));
        }
        Ok(Expr::Builtin(builtin, args))
    }
}

/// What a built-in function is called.
#[must_use]
pub fn builtin_name(builtin: Builtin) -> &'static str {
    match builtin {
        Builtin::Atan2 => "atan2",
        Builtin::Close => "close",
        Builtin::Cos => "cos",
        Builtin::Exp => "exp",
        Builtin::Fflush => "fflush",
        Builtin::Gsub => "gsub",
        Builtin::Index => "index",
        Builtin::Int => "int",
        Builtin::Length => "length",
        Builtin::Log => "log",
        Builtin::Match => "match",
        Builtin::Rand => "rand",
        Builtin::Sin => "sin",
        Builtin::Split => "split",
        Builtin::Sprintf => "sprintf",
        Builtin::Sqrt => "sqrt",
        Builtin::Srand => "srand",
        Builtin::Sub => "sub",
        Builtin::Substr => "substr",
        Builtin::System => "system",
        Builtin::Tolower => "tolower",
        Builtin::Toupper => "toupper",
    }
}

/// The place `expr` names, when it names one.
fn place(expr: &Expr) -> Option<Place> {
    match expr {
        Expr::Var(var) => Some(Place::Var(*var)),
        Expr::Place(place) => Some((**place).clone()),
        _ => None,
    }
}

/// The awk program `text`.
///
/// # Errors
///
/// Where it breaks awk's grammar, and for a regular expression in it
/// that cannot be read.
pub fn parse(text: &str) -> Result<Program, Error> {
    let tokens = lex::tokens(text)?;
    let functions = function_names(&tokens);
    let mut parser = Parser {
        tokens,
        at: 0,
        functions,
        globals: SPECIAL.iter().map(|&name| String::from(name)).collect(),
        params: Vec::new(),
        regexes: Vec::new(),
        printing: false,
        nesting: 0,
        loops: 0,
        in_function: false,
    };
    parser.program()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// `expr` written out with its tree's shape, each operation in
    /// parentheses before its operands.
    fn shown(expr: &Expr, program: &Program) -> String {
        let all = |exprs: &[Expr]| {
            exprs
                .iter()
                .map(|e| shown(e, program))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let var = |var: &Var| match var {
            Var::Global(at) => program.globals[*at].clone(),
            Var::Local(at) => format!("@{at}"),
        };
        let place = |place: &Place| match place {
            Place::Var(v) => var(v),
            Place::Element(v, subscripts) => format!("{}[{}]", var(v), all(subscripts)),
            Place::Field(index) => format!("($ {})", shown(index, program)),
        };
        let arith = |op: &Arith| match op {
            Arith::Add => "+",
            Arith::Sub => "-",
            Arith::Mul => "*",
            Arith::Div => "/",
            Arith::Mod => "%",
            Arith::Pow => "^",
        };
        match expr {
            Expr::Number(n) => n.to_string(),
            Expr::String(bytes) => format!("{:?}", String::from_utf8_lossy(bytes)),
            Expr::Regex(at) => format!("/{at}/"),
            Expr::Var(v) => var(v),
            Expr::Place(p) => place(p),
            Expr::Assign(op, p, value) => format!(
                "({}= {} {})",
                op.as_ref().map_or("", arith),
                place(p),
                shown(value, program)
            ),
            Expr::Step {
                place: p,
                by,
                before,
            } => format!(
                "({}{} {})",
                if *before { "pre" } else { "post" },
                if *by > 0.0 { "++" } else { "--" },
                place(p)
            ),
            Expr::Negate(e) => format!("(neg {})", shown(e, program)),
            Expr::Plus(e) => format!("(plus {})", shown(e, program)),
            Expr::Not(e) => format!("(! {})", shown(e, program)),
            Expr::Arith(op, a, b) => format!(
                "({} {} {})",
                arith(op),
                shown(a, program),
                shown(b, program)
            ),
            Expr::Concat(a, b) => format!("(cat {} {})", shown(a, program), shown(b, program)),
            Expr::Compare(relation, a, b) => {
                let op = match relation {
                    Relation::Less => "<",
                    Relation::LessEqual => "<=",
                    Relation::Equal => "==",
                    Relation::NotEqual => "!=",
                    Relation::Greater => ">",
                    Relation::GreaterEqual => ">=",
                };
                format!("({op} {} {})", shown(a, program), shown(b, program))
            }
            Expr::Match { not, text, regex } => format!(
                "({} {} {})",
                if *not { "!~" } else { "~" },
                shown(text, program),
                shown(regex, program)
            ),
            Expr::And(a, b) => format!("(&& {} {})", shown(a, program), shown(b, program)),
            Expr::Or(a, b) => format!("(|| {} {})", shown(a, program), shown(b, program)),
            Expr::Cond(a, b, c) => format!(
                "(? {} {} {})",
                shown(a, program),
                shown(b, program),
                shown(c, program)
            ),
            Expr::In(subscripts, array) => format!("(in {} {})", all(subscripts), var(array)),
            Expr::Call(at, args) => format!("({} {})", program.functions[*at].name, all(args)),
            Expr::Builtin(builtin, args) => format!("({} {})", builtin_name(*builtin), all(args)),
            Expr::Getline { from, into } => {
                let into = into
                    .as_ref()
                    .map_or(String::new(), |p| format!(" {}", place(p)));
                match from {
                    Source::Main => format!("(getline{into})"),
                    Source::File(file) => format!("(getline{into} < {})", shown(file, program)),
                    Source::Command(command) => {
                        format!("({} | getline{into})", shown(command, program))
                    }
                }
            }
        }
    }

    /// The statement `text`, alone in a `BEGIN` action, as `shown` writes
    /// its expression.
    fn stated(text: &str) -> (Stmt, Program) {
        let program = parse(&format!("function call(x) {{}}\nBEGIN {{ {text}\n}}")).unwrap();
        assert_eq!(program.begin.len(), 1, "{text}");
        (program.begin[0].clone(), program)
    }

    fn expression(text: &str) -> String {
        let (statement, program) = stated(text);
        let Stmt::Expr(expr) = statement else {
            panic!("{text} is no expression: {statement:?}");
        };
        shown(&expr, &program)
    }

    fn wrong(text: &str) -> Error {
        parse(text).unwrap_err()
    }

    #[test]
    fn operators_bind_as_posix_ranks_them() {
        for (text, tree) in [
            ("a = b + c * 2", "(= a (+ b (* c 2)))"),
            ("a - b - c", "(- (- a b) c)"),
            ("a / b % c", "(% (/ a b) c)"),
            ("-2^2", "(neg (^ 2 2))"),
            ("2^-1", "(^ 2 (neg 1))"),
            ("2^3^2", "(^ 2 (^ 3 2))"),
            ("!a + +b", "(+ (! a) (plus b))"),
            ("a < b ~ c", "(~ (< a b) c)"),
            ("a !~ b", "(!~ a b)"),
            ("x ~ /r/", "(~ x /0/)"),
            ("a && b || c && d", "(|| (&& a b) (&& c d))"),
            ("k in a && 1", "(&& (in k a) 1)"),
            ("(i, j) in a", "(in i j a)"),
            ("a ? b : c ? d : e", "(? a b (? c d e))"),
            ("a ? b = 1 : c", "(? a (= b 1) c)"),
            ("a += b -= 1", "(+= a (-= b 1))"),
            ("a ^= 2", "(^= a 2)"),
            ("a *= a /= a %= 1", "(*= a (/= a (%= a 1)))"),
            ("a <= b", "(<= a b)"),
            ("a >= b", "(>= a b)"),
            ("a == b", "(== a b)"),
            ("a != b", "(!= a b)"),
            ("a > b", "(> a b)"),
            ("(a)", "a"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
    }

    #[test]
    fn concatenation_binds_below_signs_and_takes_no_sign() {
        for (text, tree) in [
            ("a b c", "(cat (cat a b) c)"),
            ("1 \" \" -1", "(cat 1 (- \" \" 1))"),
            ("a + b c", "(cat (+ a b) c)"),
            ("a (b)", "(cat a b)"),
            ("$1 $2", "(cat ($ 1) ($ 2))"),
            ("a !b", "(cat a (! b))"),
            ("a < b c", "(< a (cat b c))"),
            ("x = \"a\" length", "(= x (cat \"a\" (length )))"),
            ("a ++b", "(cat (post++ a) b)"),
            ("a++ b", "(cat (post++ a) b)"),
            ("a /r/ 2", "(/ (/ a r) 2)"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
    }

    #[test]
    fn fields_steps_and_elements_are_places() {
        for (text, tree) in [
            ("$NF-1", "(- ($ NF) 1)"),
            ("$(i+1) = 2", "(= ($ (+ i 1)) 2)"),
            ("$i++", "(post++ ($ i))"),
            ("$++i", "($ (pre++ i))"),
            ("$-1", "($ (neg 1))"),
            ("$$0", "($ ($ 0))"),
            ("$x^2", "(^ ($ x) 2)"),
            ("x-- - --y", "(- (post-- x) (pre-- y))"),
            ("a[1, \"b\"] = c[d]", "(= a[1 \"b\"] c[d])"),
            ("++a[i]", "(pre++ a[i])"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
        assert_eq!(
            wrong("BEGIN { x++ ++ }").what,
            "an expression was wanted, not CloseBrace"
        );
        assert_eq!(
            wrong("BEGIN { 1 = 2 }").what,
            "only a variable, an element or a field can be assigned"
        );
        assert_eq!(
            wrong("BEGIN { ++1 }").what,
            "++ and -- go only with a variable, an element or a field"
        );
        assert_eq!(
            wrong("BEGIN { 1++ }").what,
            "an expression was wanted, not CloseBrace",
            "a step after no place is none"
        );
    }

    #[test]
    fn getline_reads_from_the_input_a_file_or_a_command() {
        for (text, tree) in [
            ("getline", "(getline)"),
            ("getline x", "(getline x)"),
            ("getline < f", "(getline < f)"),
            (
                "getline $2 < \"f\" \"g\"",
                "(cat (getline ($ 2) < \"f\") \"g\")",
            ),
            ("\"cmd\" | getline", "(\"cmd\" | getline)"),
            (
                "\"c\" x | getline y > 0",
                "(> ((cat \"c\" x) | getline y) 0)",
            ),
            ("(getline line < f) > 0", "(> (getline line < f) 0)"),
            ("getline a[1]", "(getline a[1])"),
            ("getline < f > 0", "(> (getline < f) 0)"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
    }

    #[test]
    fn calls_and_builtins_take_their_arguments() {
        let program = parse("function f(a, b) { return a + b }\nBEGIN { f(1, 2); g(); length; length($1); split(s, arr, \",\") }\nfunction g() {}").unwrap();
        let shown_all: Vec<String> = program
            .begin
            .iter()
            .map(|s| match s {
                Stmt::Expr(e) => shown(e, &program),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            shown_all,
            [
                "(f 1 2)",
                "(g )",
                "(length )",
                "(length ($ 1))",
                "(split s arr \",\")"
            ]
        );
        assert_eq!(
            program.functions[0].body,
            [Stmt::Return(Some(Expr::Arith(
                Arith::Add,
                Box::new(Expr::Var(Var::Local(0))),
                Box::new(Expr::Var(Var::Local(1)))
            )))],
            "parameters are the function's own"
        );
        assert_eq!(program.functions[1].params, 0);
        for (text, what) in [
            ("BEGIN { h() }", "no function is called h"),
            (
                "BEGIN { substr(\"a\") }",
                "substr takes from 2 to 3 arguments",
            ),
            ("BEGIN { rand(1) }", "rand takes no arguments"),
            ("BEGIN { atan2(1) }", "atan2 takes 2 arguments"),
            ("BEGIN { sprintf() }", "sprintf takes 1 arguments or more"),
            ("BEGIN { sin(1, 2) }", "sin takes 1 arguments"),
            (
                "BEGIN { split(s, a[1]) }",
                "split puts what it splits into an array",
            ),
            (
                "BEGIN { sub(/a/, \"b\", 1) }",
                "sub and gsub change a variable, an element or a field",
            ),
            ("BEGIN { sin }", "( was wanted, not CloseBrace"),
            ("function f(f) {}", "f cannot be a parameter of f"),
            ("function f(a, a) {}", "a cannot be a parameter of f"),
            ("function f(NR) {}", "NR cannot be a parameter of f"),
            (
                "function f() {}\nfunction f() {}",
                "the function f is defined twice",
            ),
            (
                "function f() {}\nBEGIN { f = 1 }",
                "f is a function, not a variable",
            ),
            (
                "function (a) {}",
                "the function's name was wanted, not OpenParen",
            ),
            ("function f(1) {}", "a parameter was wanted, not a number"),
            ("function f(a b) {}", ", or ) was wanted, not b"),
        ] {
            assert_eq!(wrong(text).what, what, "{text}");
        }
        assert_eq!(expression("index(\"ab\",\n \"b\")"), "(index \"ab\" \"b\")");
        assert_eq!(expression("srand()"), "(srand )");
        assert_eq!(expression("sprintf(\"%d\", 1, 2)"), "(sprintf \"%d\" 1 2)");
    }

    #[test]
    fn print_writes_a_list_and_names_its_output() {
        let print = |text: &str| {
            let (statement, program) = stated(text);
            let (args, output, formatted) = match statement {
                Stmt::Print(args, output) => (args, output, false),
                Stmt::Printf(args, output) => (args, output, true),
                other => panic!("{text}: {other:?}"),
            };
            let output = match &output {
                Output::Standard => String::new(),
                Output::File(e) => format!(" > {}", shown(e, &program)),
                Output::Append(e) => format!(" >> {}", shown(e, &program)),
                Output::Pipe(e) => format!(" | {}", shown(e, &program)),
            };
            let args: Vec<String> = args.iter().map(|a| shown(a, &program)).collect();
            format!(
                "{}[{}]{output}",
                if formatted { "printf" } else { "print" },
                args.join(", ")
            )
        };
        for (text, written) in [
            ("print", "print[]"),
            ("print a, b", "print[a, b]"),
            ("print a b", "print[(cat a b)]"),
            ("print a, b > \"f\"", "print[a, b] > \"f\""),
            ("print (a, b) > \"f\"", "print[a, b] > \"f\""),
            ("print (a, b)", "print[a, b]"),
            ("print (a)(b)", "print[(cat a b)]"),
            ("print (a > b)", "print[(> a b)]"),
            (
                "print a > \"f\" \".txt\"",
                "print[a] > (cat \"f\" \".txt\")",
            ),
            ("print >> f", "print[] >> f"),
            ("print \"x\" | \"sort\"", "print[\"x\"] | \"sort\""),
            ("print a[b > c]", "print[a[(> b c)]]"),
            ("print call(a > b)", "print[(call (> a b))]"),
            ("print (1, 2) in a", "print[(in 1 2 a)]"),
            ("printf \"%s\", a", "printf[\"%s\", a]"),
            (
                "printf(\"%s\\n\", a) > \"f\"",
                "printf[\"%s\\n\", a] > \"f\"",
            ),
            ("print a,\n b", "print[a, b]"),
        ] {
            assert_eq!(print(text), written, "{text}");
        }
        assert_eq!(wrong("BEGIN { printf }").what, "printf wants a format");
        assert_eq!(
            wrong("BEGIN { print > \"a\" > \"b\" }").what,
            "the end of the statement was wanted, not Greater"
        );
        assert_eq!(
            wrong("BEGIN { (1, 2) }").what,
            "in after a list in parentheses was wanted, not CloseBrace"
        );
    }

    #[test]
    fn statements_take_their_bodies_across_lines() {
        let (statement, _) = stated("if (a)\n b = 1\nelse\n c = 2");
        let Stmt::If(_, then, Some(otherwise)) = statement else {
            panic!("{statement:?}");
        };
        assert!(matches!(*then, Stmt::Expr(Expr::Assign(..))));
        assert!(matches!(*otherwise, Stmt::Expr(Expr::Assign(..))));
        let (statement, _) = stated("if (a) b; else c");
        assert!(
            matches!(statement, Stmt::If(_, _, Some(_))),
            "{statement:?}"
        );
        let program = parse("BEGIN { if (a) b\n c }").unwrap();
        assert!(matches!(program.begin[0], Stmt::If(_, _, None)));
        assert_eq!(
            program.begin.len(),
            2,
            "with no else the next statement is its own"
        );
        let (statement, _) = stated("while (a) ;");
        assert_eq!(
            statement,
            Stmt::While(
                Expr::Var(Var::Global(16)),
                Box::new(Stmt::Block(Vec::new()))
            )
        );
        let (statement, _) = stated("do { a++ } while (a < 3)");
        assert!(matches!(statement, Stmt::Do(..)));
        let (statement, _) = stated("do a++; while (a < 3)");
        assert!(matches!(statement, Stmt::Do(..)));
        let (statement, _) = stated("for (;;) break");
        assert_eq!(
            statement,
            Stmt::For {
                start: None,
                test: None,
                step: None,
                body: Box::new(Stmt::Break)
            }
        );
        let (statement, program) = stated("for (i = 0; i < 3;\n i++) { continue }");
        let Stmt::For {
            start: Some(start),
            test: Some(test),
            step: Some(step),
            body,
        } = statement
        else {
            panic!("a for loop");
        };
        assert_eq!(
            [
                shown(&start, &program),
                shown(&test, &program),
                shown(&step, &program)
            ],
            ["(= i 0)", "(< i 3)", "(post++ i)"]
        );
        assert_eq!(*body, Stmt::Block(alloc::vec![Stmt::Continue]));
        let (statement, _) = stated("for (k in a) delete a[k]");
        let Stmt::ForIn(k, a, body) = statement else {
            panic!("a for-in loop");
        };
        assert_eq!(*body, Stmt::Delete(a, Some(alloc::vec![Expr::Var(k)])));
        let (statement, _) = stated("delete a");
        assert!(matches!(statement, Stmt::Delete(_, None)));
        for (text, wanted) in [
            ("exit", Stmt::Exit(None)),
            ("exit 2", Stmt::Exit(Some(Expr::Number(2.0)))),
            ("next", Stmt::Next),
            ("nextfile", Stmt::Nextfile),
            ("{}", Stmt::Block(Vec::new())),
            (
                "getline",
                Stmt::Expr(Expr::Getline {
                    from: Source::Main,
                    into: None,
                }),
            ),
        ] {
            assert_eq!(stated(text).0, wanted, "{text}");
        }
    }

    #[test]
    fn statements_that_end_or_leave_and_what_is_wrong_with_them() {
        let program = parse("function f() { return }\nfunction g() { return 1; }").unwrap();
        assert_eq!(program.functions[0].body, [Stmt::Return(None)]);
        assert_eq!(
            program.functions[1].body,
            [Stmt::Return(Some(Expr::Number(1.0)))]
        );
        for (text, what) in [
            ("BEGIN { break }", "break and continue go only in a loop"),
            (
                "BEGIN { while (1) {} continue }",
                "break and continue go only in a loop",
            ),
            ("BEGIN { return }", "return goes only in a function"),
            (
                "BEGIN { a b = 1 }",
                "only a variable, an element or a field can be assigned",
            ),
            (
                "BEGIN { a = 1 b = 2 }",
                "only a variable, an element or a field can be assigned",
            ),
            (
                "BEGIN { x = 1 ) }",
                "the end of the statement was wanted, not CloseParen",
            ),
            (
                "BEGIN { do x while (1) }",
                "the end of the statement was wanted, not Keyword(While)",
            ),
            (
                "BEGIN { delete 1 }",
                "the name of an array was wanted, not a number",
            ),
            ("BEGIN { if a }", "( was wanted, not a"),
            ("BEGIN { if (a b }", ") was wanted, not CloseBrace"),
            (
                "BEGIN { else }",
                "a statement was wanted, not Keyword(Else)",
            ),
            ("BEGIN { for (a; b) c }", "; was wanted, not CloseParen"),
            ("BEGIN { for (a; b; c d }", ") was wanted, not CloseBrace"),
            ("BEGIN { a[1 }", "] was wanted, not CloseBrace"),
            ("BEGIN { f(1 }", "no function is called f"),
            ("BEGIN { x = }", "an expression was wanted, not CloseBrace"),
            ("BEGIN { x = (1 }", ") was wanted, not CloseBrace"),
            ("BEGIN { a ? b }", ": was wanted, not CloseBrace"),
        ] {
            assert_eq!(wrong(text).what, what, "{text}");
        }
    }

    #[test]
    fn a_program_is_rules_actions_and_functions() {
        let program = parse("BEGIN { a = 1 }\n/x/\nNR == 1, NR == 3 { print }\n{ b }; END { c }\nBEGIN { d }\nNR\n{ e }").unwrap();
        assert_eq!(
            program.begin.len(),
            2,
            "each BEGIN adds to the actions before"
        );
        assert_eq!(program.end.len(), 1);
        let shapes: Vec<(String, bool)> = program
            .rules
            .iter()
            .map(|rule| {
                let pattern = match &rule.pattern {
                    Pattern::All => String::from("all"),
                    Pattern::Expr(e) => shown(e, &program),
                    Pattern::Range(a, b) => {
                        format!("{}, {}", shown(a, &program), shown(b, &program))
                    }
                };
                (pattern, rule.action.is_some())
            })
            .collect();
        assert_eq!(
            shapes,
            [
                ("/0/".to_string(), false),
                ("(== NR 1), (== NR 3)".to_string(), true),
                ("all".to_string(), true),
                ("NR".to_string(), false),
                ("all".to_string(), true),
            ],
            "a {{ on the next line is an action of its own"
        );
        assert_eq!(&program.globals[..SPECIAL.len()], SPECIAL.map(String::from));
        assert_eq!(program.globals[NR], "NR");
        assert_eq!(program.globals[ARGV], "ARGV");
        assert_eq!(&program.globals[SPECIAL.len()..], ["a", "b", "c", "d", "e"]);
        assert_eq!(program.regexes.len(), 1);
        let program = parse("\n\n;BEGIN {\n}\n\n").unwrap();
        assert!(program.begin.is_empty() && program.rules.is_empty());
        let program = parse("BEGIN{x}{y}END{z}").unwrap();
        assert_eq!(
            (program.begin.len(), program.rules.len(), program.end.len()),
            (1, 1, 1)
        );
        assert_eq!(parse("").unwrap().rules.len(), 0);
        let program = parse("NR==1,\n NR==2").unwrap();
        assert!(matches!(program.rules[0].pattern, Pattern::Range(..)));
        let program = parse("BEGIN\n{ x }").unwrap();
        assert_eq!(program.begin.len(), 1);
        let program = parse("a && \n b || \n c { x = 1; y = 2 ;; }").unwrap();
        assert_eq!(program.rules[0].action.as_ref().map(Vec::len), Some(2));
        let program = parse("function f(a,\n b)\n{ }\nBEGIN { f(1,\n 2) }").unwrap();
        assert_eq!(program.functions[0].params, 2);
    }

    #[test]
    fn expressions_and_statements_nest_so_deep_and_chain_so_long() {
        let parens = |n: usize| format!("BEGIN {{ x = {}1{} }}", "(".repeat(n), ")".repeat(n));
        assert!(parse(&parens(97)).is_ok());
        assert_eq!(
            wrong(&parens(98)).what,
            "expressions and statements nested more than 100 deep"
        );
        let chain = |n: usize| format!("BEGIN {{ x = 1{} }}", "+1".repeat(n));
        assert!(parse(&chain(CHAIN)).is_ok());
        assert_eq!(
            wrong(&chain(CHAIN + 1)).what,
            "more than 1000 operations in a row"
        );
        let concat = format!("BEGIN {{ x = a{} }}", " a".repeat(CHAIN + 1));
        assert_eq!(wrong(&concat).what, "more than 1000 operations in a row");
        let ifs = |n: usize| format!("BEGIN {{ {} x }}", "if (1) ".repeat(n));
        assert!(
            parse(&ifs(98)).is_ok(),
            "an if's test is no deeper than its body"
        );
        assert!(parse(&ifs(99)).is_err());
        for prefix in ["- ", "!", "$", "++"] {
            let deep = format!("BEGIN {{ x = {}y }}", prefix.repeat(120));
            assert_eq!(
                wrong(&deep).what,
                "expressions and statements nested more than 100 deep",
                "{prefix}"
            );
        }
        let powers = format!("BEGIN {{ x = 2{} }}", "^2".repeat(120));
        assert_eq!(
            wrong(&powers).what,
            "expressions and statements nested more than 100 deep"
        );
        let blocks = format!("BEGIN {}x{}", "{".repeat(120), "}".repeat(120));
        assert_eq!(
            wrong(&blocks).what,
            "expressions and statements nested more than 100 deep"
        );
    }

    #[test]
    fn what_cannot_be_read_says_where() {
        assert_eq!(
            wrong("BEGIN {\n x ~ /a(/ }"),
            Error {
                line: 2,
                what: "/a(/: a parenthesis without its other half".into()
            }
        );
        assert_eq!(
            wrong("BEGIN { x }\n{ y }\nz z {"),
            Error {
                line: 3,
                what: "} was wanted, not the end".into()
            }
        );
        assert_eq!(wrong("BEGIN { x }\n(").line, 2);
        assert_eq!(
            wrong("{ x } )").what,
            "an expression was wanted, not CloseParen"
        );
        assert_eq!(wrong("BEGIN").what, "{ was wanted, not the end");
        assert_eq!(wrong("BEGIN {").what, "} was wanted, not the end");
        assert_eq!(wrong("BEGIN { \"a").what, "a string goes on past its line");
        assert_eq!(
            wrong("BEGIN { x = 1 } BEGIN").what,
            "{ was wanted, not the end"
        );
        assert_eq!(
            wrong("{ x }} ").what,
            "an expression was wanted, not CloseBrace"
        );
        assert_eq!(
            wrong("1 2 3 ,").what,
            "an expression was wanted, not the end"
        );
    }

    #[test]
    fn regular_expressions_stand_where_a_value_or_a_match_goes() {
        for (text, tree) in [
            ("x = /a/", "(= x /0/)"),
            ("x = !/a/", "(= x (! /0/))"),
            ("x = /a/ && /b/", "(= x (&& /0/ /1/))"),
            (
                "x = $1 ~ /a/ || $2 !~ /b/",
                "(= x (|| (~ ($ 1) /0/) (!~ ($ 2) /1/)))",
            ),
            ("split(s, a, /,/)", "(split s a /0/)"),
            ("sub(/a/, \"b\")", "(sub /0/ \"b\")"),
            ("x = (/a/)", "(= x /0/)"),
            ("x = y ~ \"a\" \"b\"", "(= x (~ y (cat \"a\" \"b\")))"),
            ("x = /a/ ? 1 : 2", "(= x (? /0/ 1 2))"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
        let program = parse("/a/ { x } $0 ~ /b/").unwrap();
        assert_eq!(program.regexes.len(), 2, "each written once is read once");
        assert_eq!(
            wrong("BEGIN { x = /a{300}/ }").what,
            "/a{300}/: an interval that is no count from 0 to 255"
        );
        assert_eq!(
            wrong("/(/").what,
            "/(/: a parenthesis without its other half"
        );
    }

    #[test]
    fn assignments_go_from_the_right_and_take_places_only() {
        for (text, tree) in [
            ("$1 = $2 = 3", "(= ($ 1) (= ($ 2) 3))"),
            ("a[i] = b[j] += 1", "(= a[i] (+= b[j] 1))"),
            ("x = y++ + ++z", "(= x (+ (post++ y) (pre++ z)))"),
            ("x = y == z", "(= x (== y z))"),
            ("x = y ? z = 1 : 2", "(= x (? y (= z 1) 2))"),
            ("NF = 3", "(= NF 3)"),
            ("x = -y", "(= x (neg y))"),
            ("x =\n 1", "(= x 1)"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
        for text in [
            "BEGIN { x + y = 1 }",
            "BEGIN { length() = 1 }",
            "BEGIN { \"s\" = 1 }",
            "BEGIN { -x = 1 }",
        ] {
            assert_eq!(
                wrong(text).what,
                "only a variable, an element or a field can be assigned",
                "{text}"
            );
        }
        assert_eq!(
            expression("(x) += 1"),
            "(+= x 1)",
            "a variable in parentheses is still the variable"
        );
    }

    #[test]
    fn membership_binds_below_matching_and_above_and() {
        for (text, tree) in [
            ("x = k in a", "(= x (in k a))"),
            ("x = k in a in b", "(= x (in (in k a) b))"),
            ("x = k ~ r in a", "(= x (in (~ k r) a))"),
            ("x = k in a && j in b", "(= x (&& (in k a) (in j b)))"),
            ("x = (i, j) in a || 0", "(= x (|| (in i j a) 0))"),
            ("x = !(k in a)", "(= x (! (in k a)))"),
            ("x = k \"s\" in a", "(= x (in (cat k \"s\") a))"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
        assert_eq!(
            wrong("BEGIN { x = k in 1 }").what,
            "the name of an array was wanted, not a number"
        );
        assert_eq!(
            wrong("BEGIN { x = (i, j) }").what,
            "in after a list in parentheses was wanted, not CloseBrace"
        );
        assert_eq!(
            wrong("BEGIN { x = (i, j) in }").what,
            "the name of an array was wanted, not CloseBrace"
        );
    }

    #[test]
    fn comparisons_join_two_operands_only() {
        assert_eq!(expression("x = a < b"), "(= x (< a b))");
        assert_eq!(expression("x = a < b ~ c"), "(= x (~ (< a b) c))");
        assert_eq!(expression("x = (a < b) < c"), "(= x (< (< a b) c))");
        assert_eq!(
            wrong("BEGIN { x = a < b < c }").what,
            "the end of the statement was wanted, not Less"
        );
        assert_eq!(expression("x = a < b c"), "(= x (< a (cat b c)))");
        assert_eq!(expression("x = a + b < c * d"), "(= x (< (+ a b) (* c d)))");
        assert_eq!(
            expression("x = a < b && c >= d"),
            "(= x (&& (< a b) (>= c d)))"
        );
    }

    #[test]
    fn statements_part_at_newlines_and_semicolons_but_go_on_after_operators() {
        let count = |text: &str| parse(text).unwrap().begin.len();
        assert_eq!(count("BEGIN { a; b\n c;; d }"), 4);
        assert_eq!(
            wrong("BEGIN { a = 1 +\n 2 }").what,
            "an expression was wanted, not the end of a line",
            "a line goes on only after the operators POSIX names"
        );
        assert_eq!(
            count("BEGIN { a = 1 \\\n + 2 }"),
            1,
            "a backslash goes on too"
        );
        assert_eq!(count("BEGIN { x = a &&\n\n b || \n c }"), 1);
        assert_eq!(count("BEGIN { f(1,\n 2)\n }\nfunction f(a, b) {}"), 1);
        assert_eq!(count("BEGIN { print 1,\n 2 }"), 1);
        assert_eq!(count("BEGIN { x = a ?\n b :\n c }"), 1);
        assert_eq!(count("BEGIN { # a comment\n x # another\n }"), 1);
        assert_eq!(count("BEGIN {\n}"), 0);
        assert_eq!(count("BEGIN { if (a) { b } else { c } d }"), 2);
        assert_eq!(count("BEGIN { while (a)\n\n b }"), 1);
        assert_eq!(count("BEGIN { for (;;)\n b }"), 1);
        assert_eq!(count("BEGIN { do\n b\n while (a) }"), 1);
        assert_eq!(count("BEGIN { a } ; BEGIN { b }"), 2);
    }

    #[test]
    fn keywords_name_nothing_else() {
        for text in [
            "BEGIN { if = 1 }",
            "BEGIN { x = while }",
            "BEGIN { print = 1 }",
            "BEGIN { x = BEGIN }",
            "BEGIN { getline = 1 }",
            "BEGIN { function = 2 }",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert_eq!(
            wrong("func f() {}").what,
            "no function is called f",
            "only function makes a function"
        );
        assert_eq!(
            wrong("BEGIN { length = 1 }").what,
            "only a variable, an element or a field can be assigned"
        );
        assert_eq!(
            wrong("BEGIN { substr = 1 }").what,
            "( was wanted, not Assign"
        );
    }

    #[test]
    fn a_function_s_parameters_are_its_own_and_its_name_no_variable() {
        let program = parse("function f(a, b) { c = a; d[b] = 1 }\nBEGIN { a = 2; f(a) }").unwrap();
        let body = &program.functions[0].body;
        assert_eq!(
            body[0],
            Stmt::Expr(Expr::Assign(
                None,
                Box::new(Place::Var(Var::Global(16))),
                Box::new(Expr::Var(Var::Local(0)))
            )),
            "c is the program's, a the function's"
        );
        assert_eq!(
            body[1],
            Stmt::Expr(Expr::Assign(
                None,
                Box::new(Place::Element(
                    Var::Global(17),
                    alloc::vec![Expr::Var(Var::Local(1))]
                )),
                Box::new(Expr::Number(1.0))
            ))
        );
        assert_eq!(program.globals[18], "a", "the a outside is another");
        assert_eq!(
            wrong("function f(a) { f[1] = a }").what,
            "f is a function, not a variable"
        );
        assert_eq!(
            wrong("function f(a) {} BEGIN { delete f }").what,
            "f is a function, not a variable"
        );
        assert_eq!(
            wrong("function f(a) {} BEGIN { for (f in x) ; }").what,
            "f is a function, not a variable"
        );
        assert_eq!(
            wrong("function f() { return }\nfunction g( { }").what,
            "a parameter was wanted, not OpenBrace"
        );
        assert_eq!(
            wrong("function f() return").what,
            "{ was wanted, not Keyword(Return)"
        );
    }

    #[test]
    fn getline_forms_bind_as_posix_grammar_has_them() {
        for (text, tree) in [
            ("x = getline", "(= x (getline))"),
            ("x = getline + 1", "(= x (+ (getline) 1))"),
            (
                "x = getline y < \"f\" \"g\"",
                "(= x (cat (getline y < \"f\") \"g\"))",
            ),
            ("x = getline < $1", "(= x (getline < ($ 1)))"),
            ("x = getline a[i] < f", "(= x (getline a[i] < f))"),
            (
                "x = \"a\" | getline y z",
                "(= x (cat (\"a\" | getline y) z))",
            ),
            (
                "x = \"a\" | getline | getline",
                "(= x ((\"a\" | getline) | getline))",
            ),
            ("while ((\"cmd\" | getline l) > 0) n++", ""),
        ] {
            if tree.is_empty() {
                assert!(parse(&format!("BEGIN {{ {text} }}")).is_ok(), "{text}");
                continue;
            }
            assert_eq!(expression(text), tree, "{text}");
        }
        assert_eq!(
            wrong("BEGIN { getline < }").what,
            "an expression was wanted, not CloseBrace"
        );
    }

    #[test]
    fn delete_exit_and_return_take_what_follows() {
        let (statement, program) = stated("delete a[1, 2]");
        let Stmt::Delete(_, Some(subscripts)) = statement else {
            panic!("a delete");
        };
        assert_eq!(
            subscripts
                .iter()
                .map(|s| shown(s, &program))
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(
            stated("exit 1 + 2").0,
            Stmt::Exit(Some(Expr::Arith(
                Arith::Add,
                Box::new(Expr::Number(1.0)),
                Box::new(Expr::Number(2.0))
            )))
        );
        let program = parse("function f() { if (1) return 1; else { return } }").unwrap();
        assert!(
            matches!(&program.functions[0].body[0], Stmt::If(_, then, Some(_)) if matches!(**then, Stmt::Return(Some(_))))
        );
        assert_eq!(
            wrong("BEGIN { delete a[] }").what,
            "an expression was wanted, not CloseBracket"
        );
        assert_eq!(
            wrong("BEGIN { exit 1 2 ) }").what,
            "the end of the statement was wanted, not CloseParen"
        );
        assert_eq!(
            wrong("BEGIN { next 1 }").what,
            "the end of the statement was wanted, not a number"
        );
        assert_eq!(
            wrong("BEGIN { while (1) break 1 }").what,
            "the end of the statement was wanted, not a number"
        );
    }

    #[test]
    fn messages_name_the_token_found() {
        for (text, what) in [
            (
                "function /a/ () {}",
                "the function's name was wanted, not a regular expression",
            ),
            (
                "function \"f\"() {}",
                "the function's name was wanted, not a string",
            ),
            (
                "function 1() {}",
                "the function's name was wanted, not a number",
            ),
            (
                "function\nf() {}",
                "the function's name was wanted, not the end of a line",
            ),
            ("function", "the function's name was wanted, not the end"),
            (
                "BEGIN { delete x y }",
                "the end of the statement was wanted, not y",
            ),
            (
                "BEGIN { delete (x) }",
                "the name of an array was wanted, not OpenParen",
            ),
            (
                "BEGIN { x = 1 } }",
                "an expression was wanted, not CloseBrace",
            ),
        ] {
            assert_eq!(wrong(text).what, what, "{text}");
        }
    }

    #[test]
    fn errors_say_the_line_of_the_token_found() {
        assert_eq!(
            wrong("BEGIN {\n\n x = \n\n}").line,
            5,
            "a line may go on after an assignment's ="
        );
        assert_eq!(wrong("\n\nBEGIN").line, 3);
        assert_eq!(wrong("BEGIN { x }\nEND {\n y = (1\n}").line, 3);
        assert_eq!(wrong("{\n}\n}").line, 3);
        assert!(parse("").is_ok(), "no program is an empty one");
    }

    #[test]
    fn a_rule_s_action_and_pattern_take_their_whole_tree() {
        let program = parse("$1 > 0 { n++; s += $1 }\nEND { print s / n }").unwrap();
        let rule = &program.rules[0];
        let Pattern::Expr(pattern) = &rule.pattern else {
            panic!("a pattern of one expression");
        };
        assert_eq!(shown(pattern, &program), "(> ($ 1) 0)");
        let action = rule.action.as_ref().unwrap();
        let shown_action: Vec<String> = action
            .iter()
            .map(|s| match s {
                Stmt::Expr(e) => shown(e, &program),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(shown_action, ["(post++ n)", "(+= s ($ 1))"]);
        let Stmt::Print(args, Output::Standard) = &program.end[0] else {
            panic!("a print");
        };
        assert_eq!(shown(&args[0], &program), "(/ s n)");
    }

    #[test]
    fn a_print_in_a_pattern_s_action_takes_its_redirection() {
        let program = parse("/x/ { print $1, $2 > \"out\" }").unwrap();
        let Some(action) = &program.rules[0].action else {
            panic!("an action");
        };
        let Stmt::Print(args, Output::File(target)) = &action[0] else {
            panic!("a print to a file");
        };
        assert_eq!(args.len(), 2);
        assert_eq!(shown(target, &program), "\"out\"");
        let program = parse("{ print | \"sort -r\" }").unwrap();
        let Some(action) = &program.rules[0].action else {
            panic!("an action");
        };
        assert!(matches!(&action[0], Stmt::Print(args, Output::Pipe(_)) if args.is_empty()));
        let program = parse("{ printf \"%s\", $0 >> \"log\" }").unwrap();
        let Some(action) = &program.rules[0].action else {
            panic!("an action");
        };
        assert!(matches!(&action[0], Stmt::Printf(args, Output::Append(_)) if args.len() == 2));
    }

    #[test]
    fn functions_keep_their_order_and_their_parameter_counts() {
        let program = parse(
            "function b(x) { return x }\nfunction a(x, y, z) { return b(x) }\nBEGIN { print a(1) }",
        )
        .unwrap();
        let names: Vec<(&str, usize)> = program
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.params))
            .collect();
        assert_eq!(names, [("b", 1), ("a", 3)]);
        let Stmt::Return(Some(Expr::Call(called, args))) = &program.functions[1].body[0] else {
            panic!("a return of a call");
        };
        assert_eq!(
            (*called, args.len()),
            (0, 1),
            "a call names the function by its place"
        );
        assert_eq!(
            program.globals.len(),
            SPECIAL.len(),
            "parameters are no globals"
        );
    }

    /// Every rule's pattern and action of `text`, as `shown` writes them.
    fn rules(text: &str) -> Vec<(String, Vec<String>)> {
        let program = parse(text).unwrap();
        program
            .rules
            .iter()
            .map(|rule| {
                let pattern = match &rule.pattern {
                    Pattern::All => String::from("all"),
                    Pattern::Expr(e) => shown(e, &program),
                    Pattern::Range(a, b) => {
                        format!("{}, {}", shown(a, &program), shown(b, &program))
                    }
                };
                let action = rule
                    .action
                    .iter()
                    .flatten()
                    .map(|s| match s {
                        Stmt::Expr(e) => shown(e, &program),
                        other => format!("{other:?}"),
                    })
                    .collect();
                (pattern, action)
            })
            .collect()
    }

    #[test]
    fn patterns_are_any_expression() {
        let found = rules(
            "NR > 1 && $3 ~ /x/ { a }\n!/y/ { b }\n$1 in seen { c }\n(NR % 2) { d }\nlength > 3 { e }",
        );
        let patterns: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            patterns,
            [
                "(&& (> NR 1) (~ ($ 3) /0/))",
                "(! /1/)",
                "(in ($ 1) seen)",
                "(% NR 2)",
                "(> (length ) 3)",
            ]
        );
        assert!(found.iter().all(|(_, action)| action.len() == 1));
    }

    #[test]
    fn ranges_take_any_two_expressions() {
        let found = rules("/a/, /b/\nNR == 2, $1 == \"end\" { x }\n$1, $2");
        let patterns: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            patterns,
            ["/0/, /1/", "(== NR 2), (== ($ 1) \"end\")", "($ 1), ($ 2)"]
        );
        assert_eq!(
            found[0].1.len(),
            0,
            "a range without an action writes its records"
        );
        assert_eq!(
            wrong("/a/, { x }").what,
            "an expression was wanted, not OpenBrace"
        );
    }

    #[test]
    fn a_rule_without_an_action_and_one_without_a_pattern() {
        let program = parse("NR == 1\n{ x }\nEND { }").unwrap();
        assert_eq!(program.rules.len(), 2);
        assert_eq!(program.rules[0].action, None, "it writes the record");
        assert_eq!(program.rules[1].pattern, Pattern::All);
        assert_eq!(program.rules[1].action.as_ref().map(Vec::len), Some(1));
        assert_eq!(program.end, Vec::new());
        assert_eq!(
            wrong("NR == 1 NR == 2").what,
            "a new line or ; was wanted, not Equal"
        );
    }

    #[test]
    fn print_s_list_takes_any_expression_but_bare_greater() {
        for (text, count) in [
            ("print a, b, c", 3),
            ("print (a), (b)", 2),
            ("print a (b), c", 2),
            ("print a < b, c", 2),
            ("print a ~ b, c", 2),
            ("print a ? b : c", 1),
            ("print a = 1, b = 2", 2),
            ("print length", 1),
            ("print getline", 1),
            ("print $1 $2", 1),
        ] {
            let (statement, _) = stated(text);
            let Stmt::Print(args, Output::Standard) = &statement else {
                panic!("{text}: {statement:?}");
            };
            assert_eq!(args.len(), count, "{text}");
        }
        assert_eq!(
            wrong("BEGIN { print (a, b), c }").what,
            "in after a list in parentheses was wanted, not Comma",
            "a list in parentheses is print's whole list or goes before in"
        );
    }

    #[test]
    fn an_if_s_else_binds_to_the_nearest_if() {
        let (statement, _) = stated("if (a) if (b) x; else y");
        let Stmt::If(_, inner, None) = statement else {
            panic!("the outer if has no else: {statement:?}");
        };
        assert!(matches!(*inner, Stmt::If(_, _, Some(_))), "{inner:?}");
        let (statement, _) = stated("if (a) { if (b) x } else y");
        assert!(matches!(statement, Stmt::If(_, _, Some(_))));
    }

    #[test]
    fn loops_mark_break_and_continue_as_theirs() {
        assert!(parse("BEGIN { while (1) { if (x) break; continue } }").is_ok());
        assert!(parse("BEGIN { for (;;) for (k in a) break }").is_ok());
        assert!(parse("BEGIN { do { continue } while (0) }").is_ok());
        assert_eq!(
            wrong("function f() { break }").what,
            "break and continue go only in a loop",
            "a function's body is no loop"
        );
        assert_eq!(
            wrong("BEGIN { while (1) {} if (x) break }").what,
            "break and continue go only in a loop",
            "the loop has ended"
        );
        assert!(parse("BEGIN { while (1) {} } END { for (;;) break }").is_ok());
    }

    #[test]
    fn builtins_take_their_arguments_in_their_places() {
        for (text, tree) in [
            ("substr(s, 1)", "(substr s 1)"),
            ("substr(s, 1, 2)", "(substr s 1 2)"),
            ("split(s, a)", "(split s a)"),
            ("match(s, /r/)", "(match s /0/)"),
            ("gsub(/a/, \"b\", $2)", "(gsub /0/ \"b\" ($ 2))"),
            ("sub(\"a\", \"b\", x[1])", "(sub \"a\" \"b\" x[1])"),
            ("close(\"f\")", "(close \"f\")"),
            ("system(\"ls\")", "(system \"ls\")"),
            ("fflush()", "(fflush )"),
            ("fflush(\"f\")", "(fflush \"f\")"),
            ("tolower(toupper(x))", "(tolower (toupper x))"),
            ("int(-x)", "(int (neg x))"),
            ("rand()", "(rand )"),
            ("length(a b)", "(length (cat a b))"),
            ("sqrt (4)", "(sqrt 4)"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
        assert_eq!(
            wrong("BEGIN { split(s) }").what,
            "split takes from 2 to 3 arguments"
        );
        assert_eq!(
            wrong("BEGIN { length(1, 2) }").what,
            "length takes from 0 to 1 arguments"
        );
        assert_eq!(wrong("BEGIN { close() }").what, "close takes 1 arguments");
        assert_eq!(wrong("BEGIN { match(s) }").what, "match takes 2 arguments");
        assert_eq!(
            wrong("BEGIN { gsub(/a/) }").what,
            "gsub takes from 2 to 3 arguments"
        );
    }

    #[test]
    fn prefixes_chain_and_steps_take_fields() {
        for (text, tree) in [
            ("x = !!y", "(= x (! (! y)))"),
            ("x = - -y", "(= x (neg (neg y)))"),
            ("x = -!y", "(= x (neg (! y)))"),
            ("x = !-y", "(= x (! (neg y)))"),
            ("x = + - + y", "(= x (plus (neg (plus y))))"),
            ("++$i", "(pre++ ($ i))"),
            ("$i--", "(post-- ($ i))"),
            ("x = $i++ + 1", "(= x (+ (post++ ($ i)) 1))"),
            ("x = -$1^2", "(= x (neg (^ ($ 1) 2)))"),
            ("x = 2^-$1", "(= x (^ 2 (neg ($ 1))))"),
            ("x = !$1", "(= x (! ($ 1)))"),
            ("x = $NF--", "(= x (post-- ($ NF)))"),
            ("x = a[i]++", "(= x (post++ a[i]))"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
    }

    #[test]
    fn functions_cannot_be_defined_inside_anything() {
        assert_eq!(
            wrong("function f() { function g() {} }").what,
            "a statement was wanted, not Keyword(Function)"
        );
        assert_eq!(
            wrong("BEGIN { function g() {} }").what,
            "a statement was wanted, not Keyword(Function)"
        );
        let program = parse("function f() {}\n{ x }\nfunction g() {}\nEND { y }").unwrap();
        assert_eq!(
            (
                program.functions.len(),
                program.rules.len(),
                program.end.len()
            ),
            (2, 1, 1),
            "they may come between rules"
        );
    }

    #[test]
    fn begin_and_end_keep_their_order_among_themselves() {
        let program = parse("END { a }\nBEGIN { b }\nEND { c }\nBEGIN { d }").unwrap();
        let shown_begin: Vec<String> = program
            .begin
            .iter()
            .map(|s| match s {
                Stmt::Expr(e) => shown(e, &program),
                other => format!("{other:?}"),
            })
            .collect();
        let shown_end: Vec<String> = program
            .end
            .iter()
            .map(|s| match s {
                Stmt::Expr(e) => shown(e, &program),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(shown_begin, ["b", "d"]);
        assert_eq!(shown_end, ["a", "c"]);
    }

    #[test]
    fn print_s_output_is_a_concatenation_and_no_more() {
        let target = |text: &str| {
            let (statement, program) = stated(text);
            match statement {
                Stmt::Print(_, Output::File(e) | Output::Append(e) | Output::Pipe(e))
                | Stmt::Printf(_, Output::File(e) | Output::Append(e) | Output::Pipe(e)) => {
                    shown(&e, &program)
                }
                other => panic!("{text}: {other:?}"),
            }
        };
        assert_eq!(target("print > $1 \".txt\""), "(cat ($ 1) \".txt\")");
        assert_eq!(
            target("print >> dir \"/\" name"),
            "(cat (cat dir \"/\") name)"
        );
        assert_eq!(target("print | \"sort -k\" n"), "(cat \"sort -k\" n)");
        assert_eq!(target("printf(\"%d\\n\", x) | \"cat\""), "\"cat\"");
        assert_eq!(target("print > (a ? b : c)"), "(? a b c)");
        assert_eq!(target("print > f[1]"), "f[1]");
        assert_eq!(
            wrong("BEGIN { print > a ? b : c }").what,
            "the end of the statement was wanted, not Question",
            "a condition goes in parentheses"
        );
    }

    #[test]
    fn statements_may_be_empty_where_one_is_wanted() {
        for text in [
            "BEGIN { if (x) ; else ; }",
            "BEGIN { while (x) ; }",
            "BEGIN { for (;;) ; }",
            "BEGIN { for (k in a) ; }",
            "BEGIN { ;;; }",
            "BEGIN { {} {} }",
            "BEGIN { if (x) {} else {} }",
        ] {
            assert!(parse(text).is_ok(), "{text}");
        }
        let (statement, _) = stated("for (k in a) ;");
        assert!(matches!(statement, Stmt::ForIn(_, _, body) if *body == Stmt::Block(Vec::new())));
        let (statement, _) = stated("if (x) ; else y");
        let Stmt::If(_, then, Some(_)) = statement else {
            panic!("an if with an else");
        };
        assert_eq!(*then, Stmt::Block(Vec::new()));
    }

    #[test]
    fn getline_stands_as_a_statement_in_its_forms() {
        for (text, tree) in [
            ("getline", "(getline)"),
            ("getline x", "(getline x)"),
            ("getline < \"f\"", "(getline < \"f\")"),
            ("getline x < f", "(getline x < f)"),
            ("getline $0", "(getline ($ 0))"),
            ("\"cmd\" | getline", "(\"cmd\" | getline)"),
        ] {
            let (statement, program) = stated(text);
            let Stmt::Expr(expr) = statement else {
                panic!("{text}");
            };
            assert_eq!(shown(&expr, &program), tree, "{text}");
        }
    }

    #[test]
    fn each_mistake_is_told_at_its_line() {
        for (text, line, what) in [
            (
                "BEGIN {\n x = 1\n y = (2\n}",
                3,
                ") was wanted, not the end of a line",
            ),
            (
                "{ print }\n\n{ print $ }",
                3,
                "an expression was wanted, not CloseBrace",
            ),
            ("function f(a,\n a) {}", 2, "a cannot be a parameter of f"),
            (
                "BEGIN { x = 1 }\nEND { break }",
                2,
                "break and continue go only in a loop",
            ),
            ("\n\n{ return }", 3, "return goes only in a function"),
            (
                "BEGIN {\n if (1)\n else x\n}",
                3,
                "a statement was wanted, not Keyword(Else)",
            ),
            (
                "BEGIN {\n getline x y z <\n}",
                2,
                "an expression was wanted, not the end of a line",
            ),
            ("BEGIN { a[1\n] }", 1, "] was wanted, not the end of a line"),
            ("BEGIN {\n\n printf\n}", 3, "printf wants a format"),
            (
                "BEGIN {\n x = substr(1)\n}",
                2,
                "substr takes from 2 to 3 arguments",
            ),
            (
                "BEGIN {\n x ~ /[/\n}",
                2,
                "a regular expression goes on past its line",
            ),
            (
                "BEGIN { x }\nfunction f() {}\nfunction f() {}",
                3,
                "the function f is defined twice",
            ),
        ] {
            let found = wrong(text);
            assert_eq!((found.line, found.what.as_str()), (line, what), "{text:?}");
        }
    }

    #[test]
    fn arguments_are_whole_expressions_of_their_own() {
        for (text, tree) in [
            ("call(a > b)", "(call (> a b))"),
            ("call(a ? b : c)", "(call (? a b c))"),
            ("call(k in a)", "(call (in k a))"),
            ("call(x = 1)", "(call (= x 1))"),
            ("call((a, b) in c)", "(call (in a b c))"),
            ("call(a b)", "(call (cat a b))"),
            ("call(-a)", "(call (neg a))"),
            ("call(\"cmd\" | getline)", "(call (\"cmd\" | getline))"),
        ] {
            assert_eq!(expression(text), tree, "{text}");
        }
        assert_eq!(
            wrong("function f(x) {} BEGIN { f(1,) }").what,
            "an expression was wanted, not CloseParen"
        );
    }
}
