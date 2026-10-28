//! A bc program's tokens as its items, a line of statements to run or a
//! function to define, read one at a time as GNU's bc reads them: with
//! POSIX's grammar, and GNU's `else`, `continue`, `halt`, `print`, `&&`,
//! `||`, `!`, comparisons anywhere, and `return` without parentheses.
//! Assignment binds tighter than comparison, and a unary minus tighter
//! than `^`, as in GNU's.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lex::{Op, Rel, Spot, Token};

/// Where a number is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Variable(String),
    Element(String, Box<Expr>),
    Scale,
    Ibase,
    Obase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// The digits and point of a number, read in `ibase` as it runs.
    Number(Vec<u8>),
    Place(Place),
    /// `place = value`, or with an operator, `place op= value`.
    Assign(Place, Option<Op>, Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Negate(Box<Expr>),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Compare(Rel, Box<Expr>, Box<Expr>),
    /// `++` or `--`, before the place or after it.
    Step {
        place: Place,
        up: bool,
        before: bool,
    },
    Call(String, Vec<Argument>),
    Length(Box<Expr>),
    Sqrt(Box<Expr>),
    ScaleOf(Box<Expr>),
}

/// What a function is called with: a value, or an array by its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Argument {
    Value(Expr),
    Array(String),
}

/// What `print` writes: a value, or a string with its escapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Printed {
    Value(Expr),
    String(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Statement {
    /// An expression, and whether its value is written: all but an
    /// assignment outside parentheses are.
    Expr(Expr, bool),
    String(Vec<u8>),
    Print(Vec<Printed>),
    Block(Vec<Statement>),
    If(Expr, Box<Statement>, Option<Box<Statement>>),
    While(Expr, Box<Statement>),
    For(Option<Expr>, Option<Expr>, Option<Expr>, Box<Statement>),
    Break,
    Continue,
    Return(Option<Expr>),
    Halt,
}

/// A parameter or an `auto` variable, an array if it has `[]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub name: String,
    pub array: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub parameters: Vec<Local>,
    pub autos: Vec<Local>,
    pub body: Vec<Statement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// Statements to run as soon as they are read.
    Run(Vec<Statement>),
    Define(String, Function),
}

/// What reading the next item from tokens came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Read {
    /// An item, and how many tokens it took, the newline after it with
    /// them.
    Item(Item, usize),
    /// Lines of nothing, which took that many tokens.
    Nothing(usize),
    /// The tokens end before the item does.
    More,
    /// `quit`, which ends bc as soon as it is read.
    Quit,
    /// What is wrong, on its line, and how many tokens to pass over to
    /// the end of that line.
    Wrong(String, usize, usize),
}

/// Why the tokens make no item.
enum Stop {
    More,
    Quit,
    Wrong(String, usize),
}

type Parsed<T> = Result<T, Stop>;

/// How a token is named when it cannot come where it is.
fn named(token: &Token) -> String {
    match token {
        Token::Number(digits) => format!("the number {}", String::from_utf8_lossy(digits)),
        Token::String(_) => String::from("a string"),
        Token::Name(name) => format!("the name {name}"),
        Token::Newline => String::from("the end of the line"),
        Token::Semicolon => String::from(";"),
        Token::Comma => String::from(","),
        Token::Open => String::from("("),
        Token::Close => String::from(")"),
        Token::OpenBracket => String::from("["),
        Token::CloseBracket => String::from("]"),
        Token::OpenBrace => String::from("{"),
        Token::CloseBrace => String::from("}"),
        Token::Else => String::from("else"),
        Token::Define => String::from("define"),
        Token::Auto => String::from("auto"),
        _ => String::from("an operator"),
    }
}

/// How deep expressions and statements may nest, so that reading and
/// running them fits the stack a program has.
pub const NESTING: usize = 100;

struct Parser<'t> {
    tokens: &'t [Spot],
    at: usize,
    /// How deep the expression or statement being read is.
    depth: usize,
    /// Whether a function's body is being read.
    function: bool,
    /// How many loops the statement being read is inside.
    loops: usize,
}

/// Reads the next item of `tokens`.
#[must_use]
pub fn next_item(tokens: &[Spot]) -> Read {
    let mut parser = Parser {
        tokens,
        at: 0,
        depth: 0,
        function: false,
        loops: 0,
    };
    let blank = tokens
        .iter()
        .take_while(|s| matches!(s.token, Token::Newline | Token::Semicolon))
        .count();
    if blank > 0 {
        return Read::Nothing(blank);
    }
    if tokens.is_empty() {
        return Read::More;
    }
    match parser.item() {
        Ok(item) => Read::Item(item, parser.at),
        Err(Stop::More) => Read::More,
        Err(Stop::Quit) => Read::Quit,
        Err(Stop::Wrong(why, line)) => {
            // On to the end of the line the mistake is on.
            let mut skip = parser.at;
            while skip < tokens.len() && tokens[skip].token != Token::Newline {
                skip += 1;
            }
            Read::Wrong(why, line, (skip + 1).min(tokens.len()))
        }
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|s| &s.token)
    }

    /// The next token, which must be there.
    fn next(&mut self) -> Parsed<&Token> {
        let spot = self.tokens.get(self.at).ok_or(Stop::More)?;
        self.at += 1;
        if spot.token == Token::Quit {
            return Err(Stop::Quit);
        }
        Ok(&spot.token)
    }

    fn line(&self) -> usize {
        self.tokens
            .get(self.at)
            .or(self.tokens.last())
            .map_or(1, |s| s.line)
    }

    fn wrong<T>(&self, why: String) -> Parsed<T> {
        Err(Stop::Wrong(why, self.line()))
    }

    /// What is wrong with the token at hand coming here.
    fn unwanted<T>(&self) -> Parsed<T> {
        match self.peek() {
            None => Err(Stop::More),
            Some(Token::Quit) => Err(Stop::Quit),
            Some(token) => self.wrong(format!("{} cannot come here", named(token))),
        }
    }

    fn expect(&mut self, wanted: &Token) -> Parsed<()> {
        if self.peek() == Some(wanted) {
            self.at += 1;
            return Ok(());
        }
        if self.peek().is_none() {
            return Err(Stop::More);
        }
        let shown = named(wanted);
        match self.peek() {
            Some(Token::Quit) => Err(Stop::Quit),
            Some(token) => self.wrong(format!("{shown} was wanted, not {}", named(token))),
            None => Err(Stop::More),
        }
    }

    /// Reads with `read` one level deeper.
    fn deeper<T>(&mut self, read: impl FnOnce(&mut Self) -> Parsed<T>) -> Parsed<T> {
        if self.depth == NESTING {
            return self.wrong(format!(
                "expressions and statements nest deeper than {NESTING}"
            ));
        }
        self.depth += 1;
        let read = read(self);
        self.depth -= 1;
        read
    }

    fn newlines(&mut self) {
        while self.peek() == Some(&Token::Newline) {
            self.at += 1;
        }
    }

    fn item(&mut self) -> Parsed<Item> {
        if self.peek() == Some(&Token::Define) {
            self.at += 1;
            let (name, function) = self.define()?;
            self.end_of_line()?;
            return Ok(Item::Define(name, function));
        }
        let mut statements = alloc::vec![self.statement()?];
        while self.peek() == Some(&Token::Semicolon) {
            self.at += 1;
            if matches!(self.peek(), Some(Token::Newline | Token::Semicolon)) {
                continue;
            }
            statements.push(self.statement()?);
        }
        self.end_of_line()?;
        Ok(Item::Run(statements))
    }

    fn end_of_line(&mut self) -> Parsed<()> {
        match self.peek() {
            Some(Token::Newline) => {
                self.at += 1;
                Ok(())
            }
            _ => self.unwanted(),
        }
    }

    fn define(&mut self) -> Parsed<(String, Function)> {
        let name = self.name()?;
        self.expect(&Token::Open)?;
        let mut parameters = Vec::new();
        if self.peek() != Some(&Token::Close) {
            parameters.push(self.local()?);
            while self.peek() == Some(&Token::Comma) {
                self.at += 1;
                parameters.push(self.local()?);
            }
        }
        self.expect(&Token::Close)?;
        self.newlines();
        self.expect(&Token::OpenBrace)?;
        self.newlines();
        let mut autos = Vec::new();
        if self.peek() == Some(&Token::Auto) {
            self.at += 1;
            autos.push(self.local()?);
            while self.peek() == Some(&Token::Comma) {
                self.at += 1;
                autos.push(self.local()?);
            }
            match self.peek() {
                Some(Token::Newline | Token::Semicolon) => self.at += 1,
                Some(Token::CloseBrace) => {}
                _ => return self.unwanted(),
            }
        }
        self.function = true;
        let body = self.block_body();
        self.function = false;
        Ok((
            name,
            Function {
                parameters,
                autos,
                body: body?,
            },
        ))
    }

    fn name(&mut self) -> Parsed<String> {
        match self.peek() {
            Some(Token::Name(name)) => {
                let name = name.clone();
                self.at += 1;
                Ok(name)
            }
            _ => self.unwanted(),
        }
    }

    fn local(&mut self) -> Parsed<Local> {
        let name = self.name()?;
        let array = self.peek() == Some(&Token::OpenBracket);
        if array {
            self.at += 1;
            self.expect(&Token::CloseBracket)?;
        }
        Ok(Local { name, array })
    }

    /// The statements up to the `}` that closes a block, which is taken.
    fn block_body(&mut self) -> Parsed<Vec<Statement>> {
        let mut statements = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Newline | Token::Semicolon) => self.at += 1,
                Some(Token::CloseBrace) => {
                    self.at += 1;
                    return Ok(statements);
                }
                None => return Err(Stop::More),
                Some(_) => {
                    statements.push(self.statement()?);
                    // A statement ends at a newline, a ; or the }.
                    if !matches!(
                        self.peek(),
                        Some(Token::Newline | Token::Semicolon | Token::CloseBrace)
                    ) {
                        return self.unwanted();
                    }
                }
            }
        }
    }

    fn statement(&mut self) -> Parsed<Statement> {
        self.deeper(Self::statement_here)
    }

    fn statement_here(&mut self) -> Parsed<Statement> {
        let Some(token) = self.peek().cloned() else {
            return Err(Stop::More);
        };
        Ok(match token {
            Token::OpenBrace => {
                self.at += 1;
                Statement::Block(self.block_body()?)
            }
            Token::If => {
                self.at += 1;
                let condition = self.condition()?;
                let then = Box::new(self.statement()?);
                // else goes on the line the statement ends on.
                let otherwise = if self.peek() == Some(&Token::Else) {
                    self.at += 1;
                    self.newlines();
                    Some(Box::new(self.statement()?))
                } else {
                    None
                };
                Statement::If(condition, then, otherwise)
            }
            Token::While => {
                self.at += 1;
                let condition = self.condition()?;
                Statement::While(condition, Box::new(self.looped()?))
            }
            Token::For => {
                self.at += 1;
                self.expect(&Token::Open)?;
                let first = self.optional(&Token::Semicolon)?;
                self.expect(&Token::Semicolon)?;
                let test = self.optional(&Token::Semicolon)?;
                self.expect(&Token::Semicolon)?;
                let step = self.optional(&Token::Close)?;
                self.expect(&Token::Close)?;
                self.newlines();
                Statement::For(first, test, step, Box::new(self.looped()?))
            }
            Token::Break | Token::Continue => {
                if self.loops == 0 {
                    return self.wrong(format!(
                        "{} outside a loop",
                        if token == Token::Break {
                            "break"
                        } else {
                            "continue"
                        }
                    ));
                }
                self.at += 1;
                if token == Token::Break {
                    Statement::Break
                } else {
                    Statement::Continue
                }
            }
            Token::Return => {
                if !self.function {
                    return self.wrong(String::from("return outside a function"));
                }
                self.at += 1;
                let ends = matches!(
                    self.peek(),
                    None | Some(
                        Token::Newline | Token::Semicolon | Token::CloseBrace | Token::Else
                    )
                );
                Statement::Return(if ends { None } else { Some(self.expr()?) })
            }
            Token::Halt => {
                self.at += 1;
                Statement::Halt
            }
            Token::Quit => return Err(Stop::Quit),
            Token::String(text) => {
                self.at += 1;
                Statement::String(text)
            }
            Token::Print => {
                self.at += 1;
                let mut printed = alloc::vec![self.printed()?];
                while self.peek() == Some(&Token::Comma) {
                    self.at += 1;
                    printed.push(self.printed()?);
                }
                Statement::Print(printed)
            }
            _ => {
                let grouped = token == Token::Open;
                let expr = self.expr()?;
                let shown = grouped || !matches!(expr, Expr::Assign(..));
                Statement::Expr(expr, shown)
            }
        })
    }

    fn printed(&mut self) -> Parsed<Printed> {
        if let Some(Token::String(text)) = self.peek() {
            let text = text.clone();
            self.at += 1;
            return Ok(Printed::String(text));
        }
        Ok(Printed::Value(self.expr()?))
    }

    /// `( expression )`, then the newlines before the statement.
    fn condition(&mut self) -> Parsed<Expr> {
        self.expect(&Token::Open)?;
        let condition = self.expr()?;
        self.expect(&Token::Close)?;
        self.newlines();
        Ok(condition)
    }

    /// The statement a loop runs.
    fn looped(&mut self) -> Parsed<Statement> {
        self.loops += 1;
        let body = self.statement();
        self.loops -= 1;
        body
    }

    /// An expression, or none when `end` comes first.
    fn optional(&mut self, end: &Token) -> Parsed<Option<Expr>> {
        if self.peek() == Some(end) {
            return Ok(None);
        }
        self.expr().map(Some)
    }

    /// An expression, from `||`, which binds least.
    fn expr(&mut self) -> Parsed<Expr> {
        self.deeper(Self::or)
    }

    fn or(&mut self) -> Parsed<Expr> {
        let mut left = self.and()?;
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            left = Expr::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Parsed<Expr> {
        let mut left = self.not()?;
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            left = Expr::And(Box::new(left), Box::new(self.not()?));
        }
        Ok(left)
    }

    fn not(&mut self) -> Parsed<Expr> {
        if self.peek() == Some(&Token::Not) {
            self.at += 1;
            let inner = self.deeper(Self::not)?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.compare()
    }

    fn compare(&mut self) -> Parsed<Expr> {
        let mut left = self.assign()?;
        while let Some(&Token::Rel(rel)) = self.peek() {
            self.at += 1;
            left = Expr::Compare(rel, Box::new(left), Box::new(self.assign()?));
        }
        Ok(left)
    }

    fn assign(&mut self) -> Parsed<Expr> {
        let left = self.sum()?;
        let Some(&Token::Assign(op)) = self.peek() else {
            return Ok(left);
        };
        let Expr::Place(place) = left else {
            return self.wrong(String::from(
                "only a variable, an element or scale, ibase or obase takes a value",
            ));
        };
        self.at += 1;
        Ok(Expr::Assign(place, op, Box::new(self.assign()?)))
    }

    fn sum(&mut self) -> Parsed<Expr> {
        let mut left = self.product()?;
        while let Some(&Token::Op(op @ (Op::Add | Op::Sub))) = self.peek() {
            self.at += 1;
            left = Expr::Binary(op, Box::new(left), Box::new(self.product()?));
        }
        Ok(left)
    }

    fn product(&mut self) -> Parsed<Expr> {
        let mut left = self.power()?;
        while let Some(&Token::Op(op @ (Op::Mul | Op::Div | Op::Rem))) = self.peek() {
            self.at += 1;
            left = Expr::Binary(op, Box::new(left), Box::new(self.power()?));
        }
        Ok(left)
    }

    fn power(&mut self) -> Parsed<Expr> {
        let base = self.unary()?;
        if self.peek() == Some(&Token::Op(Op::Pow)) {
            self.at += 1;
            let exponent = self.deeper(Self::power)?;
            return Ok(Expr::Binary(Op::Pow, Box::new(base), Box::new(exponent)));
        }
        Ok(base)
    }

    fn unary(&mut self) -> Parsed<Expr> {
        if self.peek() == Some(&Token::Op(Op::Sub)) {
            self.at += 1;
            let inner = self.deeper(Self::unary)?;
            return Ok(Expr::Negate(Box::new(inner)));
        }
        self.step()
    }

    fn step(&mut self) -> Parsed<Expr> {
        if let Some(up @ (Token::Increment | Token::Decrement)) = self.peek() {
            let up = *up == Token::Increment;
            self.at += 1;
            let place = self.place()?;
            return Ok(Expr::Step {
                place,
                up,
                before: true,
            });
        }
        let primary = self.primary()?;
        if let Expr::Place(place) = &primary
            && let Some(up @ (Token::Increment | Token::Decrement)) = self.peek()
        {
            let up = *up == Token::Increment;
            self.at += 1;
            return Ok(Expr::Step {
                place: place.clone(),
                up,
                before: false,
            });
        }
        Ok(primary)
    }

    /// A place, as `++` and `--` before it take one.
    fn place(&mut self) -> Parsed<Place> {
        match self.primary()? {
            Expr::Place(place) => Ok(place),
            _ => self.wrong(String::from(
                "++ and -- take a variable, an element or scale, ibase or obase",
            )),
        }
    }

    fn primary(&mut self) -> Parsed<Expr> {
        let token = self.next()?.clone();
        Ok(match token {
            Token::Number(digits) => Expr::Number(digits),
            Token::Open => {
                let inner = self.expr()?;
                self.expect(&Token::Close)?;
                inner
            }
            Token::Name(name) => match self.peek() {
                Some(Token::Open) => {
                    self.at += 1;
                    Expr::Call(name, self.arguments()?)
                }
                Some(Token::OpenBracket) => {
                    self.at += 1;
                    let index = self.expr()?;
                    self.expect(&Token::CloseBracket)?;
                    Expr::Place(Place::Element(name, Box::new(index)))
                }
                _ => Expr::Place(Place::Variable(name)),
            },
            Token::Scale if self.peek() == Some(&Token::Open) => {
                Expr::ScaleOf(Box::new(self.argument()?))
            }
            Token::Scale => Expr::Place(Place::Scale),
            Token::Ibase => Expr::Place(Place::Ibase),
            Token::Obase => Expr::Place(Place::Obase),
            Token::Length => Expr::Length(Box::new(self.argument()?)),
            Token::Sqrt => Expr::Sqrt(Box::new(self.argument()?)),
            _ => {
                self.at -= 1;
                return self.unwanted();
            }
        })
    }

    /// `( expression )`, as the built-in functions take one.
    fn argument(&mut self) -> Parsed<Expr> {
        self.expect(&Token::Open)?;
        let inner = self.expr()?;
        self.expect(&Token::Close)?;
        Ok(inner)
    }

    /// What a call is given, up to the `)` it ends with, which is taken.
    fn arguments(&mut self) -> Parsed<Vec<Argument>> {
        let mut arguments = Vec::new();
        if self.peek() == Some(&Token::Close) {
            self.at += 1;
            return Ok(arguments);
        }
        loop {
            // An array goes by its name and [].
            let array = matches!(
                (
                    self.peek(),
                    self.tokens.get(self.at + 1).map(|s| &s.token),
                    self.tokens.get(self.at + 2).map(|s| &s.token)
                ),
                (
                    Some(Token::Name(_)),
                    Some(Token::OpenBracket),
                    Some(Token::CloseBracket)
                )
            );
            if array {
                let name = self.name()?;
                self.at += 2;
                arguments.push(Argument::Array(name));
            } else {
                arguments.push(Argument::Value(self.expr()?));
            }
            match self.next()? {
                Token::Comma => {}
                Token::Close => return Ok(arguments),
                _ => {
                    self.at -= 1;
                    return self.unwanted();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::tokens;

    fn read(text: &str) -> Read {
        next_item(&tokens(text.as_bytes(), 1).unwrap())
    }

    fn statements(text: &str) -> Vec<Statement> {
        match read(text) {
            Read::Item(Item::Run(statements), _) => statements,
            other => panic!("{text:?}: {other:?}"),
        }
    }

    /// The expression of a line that is one.
    fn expr(text: &str) -> Expr {
        match statements(&format!("{text}\n")).as_slice() {
            [Statement::Expr(expr, _)] => expr.clone(),
            other => panic!("{text:?}: {other:?}"),
        }
    }

    // The tree holds its children in boxes.
    #[allow(clippy::unnecessary_box_returns)]
    fn n(digits: &str) -> Box<Expr> {
        Box::new(Expr::Number(digits.as_bytes().to_vec()))
    }

    fn v(name: &str) -> Place {
        Place::Variable(name.into())
    }

    #[allow(clippy::unnecessary_box_returns)]
    fn var(name: &str) -> Box<Expr> {
        Box::new(Expr::Place(v(name)))
    }

    #[allow(clippy::unnecessary_box_returns)]
    fn bin(op: Op, a: Box<Expr>, b: Box<Expr>) -> Box<Expr> {
        Box::new(Expr::Binary(op, a, b))
    }

    #[test]
    fn operators_bind_as_gnu_bc_s_do() {
        use Op::{Add, Div, Mul, Pow, Rem, Sub};
        assert_eq!(expr("1+2*3"), *bin(Add, n("1"), bin(Mul, n("2"), n("3"))));
        assert_eq!(expr("1*2+3"), *bin(Add, bin(Mul, n("1"), n("2")), n("3")));
        assert_eq!(
            expr("1-2-3"),
            *bin(Sub, bin(Sub, n("1"), n("2")), n("3")),
            "from the left"
        );
        assert_eq!(expr("8/4/2"), *bin(Div, bin(Div, n("8"), n("4")), n("2")));
        assert_eq!(expr("2*3%4"), *bin(Rem, bin(Mul, n("2"), n("3")), n("4")));
        assert_eq!(
            expr("2^3^2"),
            *bin(Pow, n("2"), bin(Pow, n("3"), n("2"))),
            "from the right"
        );
        assert_eq!(
            expr("-2^2"),
            *bin(Pow, Box::new(Expr::Negate(n("2"))), n("2")),
            "a minus before ^"
        );
        assert_eq!(
            expr("2^-1"),
            *bin(Pow, n("2"), Box::new(Expr::Negate(n("1"))))
        );
        assert_eq!(
            expr("-a++"),
            Expr::Negate(Box::new(Expr::Step {
                place: v("a"),
                up: true,
                before: false
            }))
        );
        assert_eq!(expr("(1+2)*3"), *bin(Mul, bin(Add, n("1"), n("2")), n("3")));
    }

    #[test]
    fn assignment_binds_tighter_than_comparison() {
        assert_eq!(
            expr("a = b = 1"),
            Expr::Assign(v("a"), None, Box::new(Expr::Assign(v("b"), None, n("1"))))
        );
        assert_eq!(
            expr("x = 1 == 1"),
            Expr::Compare(
                Rel::Equal,
                Box::new(Expr::Assign(v("x"), None, n("1"))),
                n("1")
            )
        );
        assert_eq!(
            expr("a += 2 * 3"),
            Expr::Assign(v("a"), Some(Op::Add), bin(Op::Mul, n("2"), n("3")))
        );
        assert_eq!(
            expr("1 < 2 < 3"),
            Expr::Compare(
                Rel::Less,
                Box::new(Expr::Compare(Rel::Less, n("1"), n("2"))),
                n("3")
            )
        );
        assert_eq!(
            expr("!0 + 1"),
            Expr::Not(bin(Op::Add, n("0"), n("1"))),
            "! below +"
        );
        assert_eq!(
            expr("1 || 0 && 0"),
            Expr::Or(n("1"), Box::new(Expr::And(n("0"), n("0"))))
        );
        assert_eq!(
            expr("!!1 && 1"),
            Expr::And(Box::new(Expr::Not(Box::new(Expr::Not(n("1"))))), n("1"))
        );
        for (text, rel) in [
            ("1 <= 2", Rel::LessEqual),
            ("1 > 2", Rel::Greater),
            ("1 >= 2", Rel::GreaterEqual),
            ("1 != 2", Rel::NotEqual),
        ] {
            assert_eq!(expr(text), Expr::Compare(rel, n("1"), n("2")), "{text}");
        }
    }

    #[test]
    fn places_steps_and_built_ins_are_known() {
        assert_eq!(
            expr("a[1+2]"),
            Expr::Place(Place::Element("a".into(), bin(Op::Add, n("1"), n("2"))))
        );
        assert_eq!(
            expr("++a"),
            Expr::Step {
                place: v("a"),
                up: true,
                before: true
            }
        );
        assert_eq!(
            expr("a--"),
            Expr::Step {
                place: v("a"),
                up: false,
                before: false
            }
        );
        assert_eq!(
            expr("--a[0]"),
            Expr::Step {
                place: Place::Element("a".into(), n("0")),
                up: false,
                before: true
            }
        );
        assert_eq!(
            expr("scale++"),
            Expr::Step {
                place: Place::Scale,
                up: true,
                before: false
            }
        );
        assert_eq!(expr("ibase"), Expr::Place(Place::Ibase));
        assert_eq!(expr("obase"), Expr::Place(Place::Obase));
        assert_eq!(expr("scale"), Expr::Place(Place::Scale));
        assert_eq!(expr("scale(1.5)"), Expr::ScaleOf(n("1.5")));
        assert_eq!(expr("length(10)"), Expr::Length(n("10")));
        assert_eq!(expr("sqrt(2)"), Expr::Sqrt(n("2")));
        assert_eq!(
            expr("x = obase = 2"),
            Expr::Assign(
                v("x"),
                None,
                Box::new(Expr::Assign(Place::Obase, None, n("2")))
            )
        );
    }

    #[test]
    fn calls_take_values_and_arrays() {
        assert_eq!(expr("f()"), Expr::Call("f".into(), Vec::new()));
        assert_eq!(
            expr("f(1, a[], b)"),
            Expr::Call(
                "f".into(),
                alloc::vec![
                    Argument::Value(*n("1")),
                    Argument::Array("a".into()),
                    Argument::Value(*var("b"))
                ]
            )
        );
        assert_eq!(
            expr("f(a[1])"),
            Expr::Call(
                "f".into(),
                alloc::vec![Argument::Value(Expr::Place(Place::Element(
                    "a".into(),
                    n("1")
                )))]
            ),
            "an element is a value"
        );
        assert_eq!(
            expr("g(f(2))"),
            Expr::Call(
                "g".into(),
                alloc::vec![Argument::Value(Expr::Call(
                    "f".into(),
                    alloc::vec![Argument::Value(*n("2"))]
                ))]
            )
        );
    }

    #[test]
    fn statements_take_their_forms() {
        assert_eq!(
            statements("if (a) 1 else 2\n"),
            [Statement::If(
                *var("a"),
                Box::new(Statement::Expr(*n("1"), true)),
                Some(Box::new(Statement::Expr(*n("2"), true)))
            )]
        );
        assert_eq!(
            statements("if (a)\n\n1\n"),
            [Statement::If(
                *var("a"),
                Box::new(Statement::Expr(*n("1"), true)),
                None
            )],
            "newlines may come before the statement"
        );
        assert_eq!(
            statements("while (a) { break; continue }\n"),
            [Statement::While(
                *var("a"),
                Box::new(Statement::Block(alloc::vec![
                    Statement::Break,
                    Statement::Continue
                ]))
            )]
        );
        assert_eq!(
            statements("for (;;) break\n"),
            [Statement::For(None, None, None, Box::new(Statement::Break))]
        );
        assert_eq!(
            statements("for (i = 0; i < 2; i++) i\n"),
            [Statement::For(
                Some(Expr::Assign(v("i"), None, n("0"))),
                Some(Expr::Compare(Rel::Less, var("i"), n("2"))),
                Some(Expr::Step {
                    place: v("i"),
                    up: true,
                    before: false
                }),
                Box::new(Statement::Expr(*var("i"), true))
            )]
        );
        assert_eq!(
            statements("print \"a\", 1; \"b\"; halt\n"),
            [
                Statement::Print(alloc::vec![
                    Printed::String(b"a".to_vec()),
                    Printed::Value(*n("1"))
                ]),
                Statement::String(b"b".to_vec()),
                Statement::Halt
            ]
        );
        assert_eq!(
            statements("1;;2;\n"),
            [
                Statement::Expr(*n("1"), true),
                Statement::Expr(*n("2"), true)
            ],
            "empty statements are none"
        );
        assert_eq!(
            statements("a = 1; (a = 1)\n"),
            [
                Statement::Expr(Expr::Assign(v("a"), None, n("1")), false),
                Statement::Expr(Expr::Assign(v("a"), None, n("1")), true)
            ],
            "an assignment is written in parentheses"
        );
        assert_eq!(
            statements("{\n1\n\n2\n}\n"),
            [Statement::Block(alloc::vec![
                Statement::Expr(*n("1"), true),
                Statement::Expr(*n("2"), true)
            ])]
        );
    }

    #[test]
    fn functions_are_defined_with_parameters_and_autos() {
        let Read::Item(Item::Define(name, f), taken) =
            read("define f(a, b[]) {\n auto c, d[]\n return (a)\n}\n")
        else {
            panic!("no definition");
        };
        assert_eq!(name, "f");
        let local = |name: &str, array| Local {
            name: name.into(),
            array,
        };
        assert_eq!(f.parameters, [local("a", false), local("b", true)]);
        assert_eq!(f.autos, [local("c", false), local("d", true)]);
        assert_eq!(f.body, [Statement::Return(Some(*var("a")))]);
        assert_eq!(taken, 25, "every token, the last newline with them");
        let Read::Item(Item::Define(_, f), _) = read("define g()\n{ auto x; return }\n") else {
            panic!("no definition");
        };
        assert_eq!(f.autos, [local("x", false)]);
        assert_eq!(f.body, [Statement::Return(None)], "a {{ on the next line");
        let Read::Item(Item::Define(_, f), _) = read("define h() { return x * 2 }\n") else {
            panic!("no definition");
        };
        assert_eq!(
            f.body,
            [Statement::Return(Some(*bin(Op::Mul, var("x"), n("2"))))],
            "return without parentheses, as GNU's takes it"
        );
        let Read::Item(Item::Define(_, f), _) = read("define e() {}\n") else {
            panic!("no definition");
        };
        assert_eq!(f.body, []);
    }

    #[test]
    fn reading_stops_where_the_tokens_do() {
        for unfinished in [
            "1",
            "if (1)",
            "{\n1\n",
            "define f() {",
            "f(1,",
            "while (1)\n",
            "a[",
            "1 +",
        ] {
            assert_eq!(read(unfinished), Read::More, "{unfinished:?}");
        }
        assert_eq!(read(""), Read::More);
        assert_eq!(read("\n\n;\n1\n"), Read::Nothing(4));
        assert!(matches!(read("1;2\n3\n"), Read::Item(Item::Run(_), 4)));
        assert!(matches!(
            read("if (1) {\n1\n} else {\n2\n}\n"),
            Read::Item(Item::Run(_), 16)
        ));
    }

    #[test]
    fn quit_is_read_wherever_it_is() {
        for text in [
            "quit\n",
            "1; quit\n",
            "if (0) quit\n",
            "while (0) { quit }\n",
            "define f() { quit }\n",
            "1 + quit\n",
            "f(quit)\n",
        ] {
            assert_eq!(read(text), Read::Quit, "{text:?}");
        }
    }

    #[test]
    fn mistakes_say_what_and_where_and_pass_over_their_line() {
        let wrong = |text: &str| match read(text) {
            Read::Wrong(why, line, taken) => (why, line, taken),
            other => panic!("{text:?}: {other:?}"),
        };
        assert_eq!(
            wrong("1 2\n3\n"),
            ("the number 2 cannot come here".into(), 1, 3)
        );
        assert_eq!(wrong("break\n"), ("break outside a loop".into(), 1, 2));
        assert_eq!(
            wrong(
                "while (1) break; continue
"
            ),
            ("continue outside a loop".into(), 1, 8),
            "a loop's statement ends where it does"
        );
        assert_eq!(
            wrong("return\n"),
            ("return outside a function".into(), 1, 2)
        );
        assert_eq!(
            wrong("f(1 2)\n"),
            ("the number 2 cannot come here".into(), 1, 6)
        );
        assert_eq!(
            wrong("define 1() {}\n"),
            ("the number 1 cannot come here".into(), 1, 7)
        );
        assert_eq!(wrong("define f( {}\n"), ("{ cannot come here".into(), 1, 6));
        assert_eq!(wrong("(1;\n"), (") was wanted, not ;".into(), 1, 4));
        assert_eq!(
            wrong("1 = 2\n"),
            (
                "only a variable, an element or scale, ibase or obase takes a value".into(),
                1,
                4
            )
        );
        assert_eq!(
            wrong("x++ ++\n"),
            ("an operator cannot come here".into(), 1, 4)
        );
        assert_eq!(wrong("else 1\n"), ("else cannot come here".into(), 1, 3));
        assert_eq!(
            wrong("define f() { auto }\n"),
            ("} cannot come here".into(), 1, 8)
        );
        assert_eq!(
            wrong("{\n1 2\n}\n"),
            ("the number 2 cannot come here".into(), 2, 5),
            "from the line of the mistake to its end"
        );
    }

    #[test]
    fn nesting_is_counted_by_level() {
        let parens = |n: usize| format!("{}1{}\n", "(".repeat(n), ")".repeat(n));
        assert!(matches!(read(&parens(98)), Read::Item(..)));
        assert!(matches!(read(&parens(99)), Read::Wrong(..)));
        let blocks = |n: usize| format!("{}1{}\n", "{".repeat(n), "}".repeat(n));
        assert!(matches!(read(&blocks(98)), Read::Item(..)));
        assert!(matches!(read(&blocks(99)), Read::Wrong(..)));
        assert_eq!(NESTING, 100);
    }
}
