//! The commands a sh script's tokens stand for, as POSIX's shell grammar
//! puts them together.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;

use super::lex::{self, HereDoc, Op, Part, Token, Word};
use crate::parse::is_name;
use crate::{Code, Error, Pos};

/// How a redirection opens what it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// `<`: the file, to read.
    Read,
    /// `>`: the file, made new or emptied, to write.
    Write,
    /// `>|`: as `>`, even where the shell was told to leave files that
    /// exist alone.
    Clobber,
    /// `>>`: the file, to write after what it holds.
    Append,
    /// `<>`: the file, to read and write.
    ReadWrite,
    /// `<&`: a copy of the stream the word names, or none for `-`.
    CopyIn,
    /// `>&`: as `<&`, to write.
    CopyOut,
}

/// What one of a command's streams is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redirection {
    File { stream: u32, how: How, word: Word },
    Here { stream: u32, doc: HereDoc },
}

/// `NAME=value` before a command's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
}

/// A command of words, with the variables it sets first and where its
/// streams go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Simple {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirections: Vec<Redirection>,
    /// Where it starts.
    pub at: Pos,
}

/// `name() body`: a command made of the body, which runs it with the
/// words after the name as its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub body: Compound,
    /// What the body's streams are made each time it runs.
    pub redirections: Vec<Redirection>,
    /// Where its name is.
    pub at: Pos,
}

/// One of a `case`'s patterns and what runs when it matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseItem {
    pub patterns: Vec<Word>,
    pub body: Vec<Item>,
    /// Whether it ends with `;&`, which runs the next item's body too.
    pub falls: bool,
}

/// A command made of lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compound {
    /// `{ list; }`
    Brace(Vec<Item>),
    /// `( list )`, run in a copy of the shell.
    Subshell(Vec<Item>),
    /// `for name in words; do list; done`, or without `in`, for each of
    /// the positional parameters.
    For {
        name: String,
        words: Option<Vec<Word>>,
        body: Vec<Item>,
    },
    /// `case word in pattern) list;; esac`
    Case { word: Word, items: Vec<CaseItem> },
    /// `if list; then list; elif list; then list; else list; fi`
    If {
        branches: Vec<(Vec<Item>, Vec<Item>)>,
        otherwise: Option<Vec<Item>>,
    },
    /// `while list; do list; done`, or with `until`, while the condition
    /// fails.
    While {
        until: bool,
        condition: Vec<Item>,
        body: Vec<Item>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Simple(Simple),
    Compound(Compound, Vec<Redirection>),
    Function(Function),
}

/// Commands joined by `|`, the first one's output the next one's input,
/// and with `!` before them, their status turned round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub not: bool,
    pub commands: Vec<Command>,
}

/// How a pipeline joins the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// `&&`: it runs when the one before ended well.
    And,
    /// `||`: it runs when the one before did not.
    Or,
}

/// Pipelines joined by `&&` and `||`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndOr {
    pub first: Pipeline,
    pub rest: Vec<(Join, Pipeline)>,
}

/// One of a list's and-or lists, and whether `&` runs it in the
/// background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub and_or: AndOr,
    pub background: bool,
}

/// Where a parser takes its tokens from.
pub trait Source {
    /// The next token and where it starts; none at the end. `depth` is how
    /// many compound commands and expansions the token stands inside, for
    /// a source that reads expansions in it.
    ///
    /// # Errors
    ///
    /// What reading the script found wrong.
    fn token(&mut self, depth: usize) -> Result<Option<(Token, Pos)>, Error>;
}

impl<I: Iterator<Item = (Token, Pos)>> Source for I {
    fn token(&mut self, _: usize) -> Result<Option<(Token, Pos)>, Error> {
        Ok(self.next())
    }
}

/// Reads the commands of the `$(` at `at`, which stands inside `depth`
/// compound commands and expansions, up to the `)` that closes it, and
/// takes that `)`.
pub(super) fn substitution(source: impl Source, depth: usize, at: Pos) -> Result<(), Error> {
    let mut parser = Parser::new(source);
    parser.depth = depth;
    parser.list()?;
    match parser.take()? {
        Some((Token::Op(Op::Close), _)) => Ok(()),
        Some((_, here)) => error(here, Code::Unexpected),
        None => error(at, Code::Unclosed),
    }
}

/// The commands of `script`.
///
/// # Errors
///
/// At the first thing in it that is not sh, with its code.
pub fn script(script: &str) -> Result<Vec<Item>, Error> {
    let mut parser = Parser::new(lex::tokens(script)?.into_iter());
    let list = parser.list()?;
    match parser.peek()? {
        None => Ok(list),
        Some(&(_, at)) => error(at, Code::Unexpected),
    }
}

/// Whether `script` does not read because it ends too soon: a quote, a
/// here-document or a compound command left open, or a `|`, `&&` or `||`
/// with its command still to come, which more lines can finish, as a shell
/// reading typed lines asks before it runs them.
#[must_use]
pub fn wants_more(script: &str) -> bool {
    let tokens = match lex::tokens(script) {
        Ok(tokens) => tokens,
        Err(e) => return e.code == Code::Unclosed,
    };
    let ended = Cell::new(false);
    let mut parser = Parser::new(Ending {
        tokens: tokens.into_iter(),
        ended: &ended,
    });
    let read = parser.list().is_ok() && matches!(parser.peek(), Ok(None));
    !read && ended.get()
}

/// Tokens that keep whether the parser took them all and asked for more.
struct Ending<'e, I> {
    tokens: I,
    ended: &'e Cell<bool>,
}

impl<I: Iterator<Item = (Token, Pos)>> Iterator for Ending<'_, I> {
    type Item = (Token, Pos);

    fn next(&mut self) -> Option<(Token, Pos)> {
        let next = self.tokens.next();
        if next.is_none() {
            self.ended.set(true);
        }
        next
    }
}

/// The words that end a list where a command could start.
const ENDS: [&str; 8] = ["then", "else", "elif", "fi", "do", "done", "esac", "}"];

fn error<T>(at: Pos, code: Code) -> Result<T, Error> {
    Err(Error { at, code })
}

/// The word `token` is when it is one written plainly.
fn plain(token: &Token) -> Option<&str> {
    match token {
        Token::Word(word) => word.plain(),
        _ => None,
    }
}

/// The assignment `word` is when it starts with a name and `=`, unquoted.
fn assignment(word: &Word) -> Option<Assignment> {
    let Some(Part::Text {
        text,
        quoted: false,
    }) = word.parts.first()
    else {
        return None;
    };
    let (name, value) = text.split_once('=')?;
    if !is_name(name) {
        return None;
    }
    let mut parts = word.parts.clone();
    if value.is_empty() {
        parts.remove(0);
    } else {
        parts[0] = Part::Text {
            text: value.into(),
            quoted: false,
        };
    }
    Some(Assignment {
        name: name.into(),
        value: Word { parts, at: word.at },
    })
}

/// How the redirection operator `op` opens what it names, and the stream
/// it stands for without a number.
fn how(op: Op) -> Option<(How, u32)> {
    Some(match op {
        Op::Less => (How::Read, 0),
        Op::Great => (How::Write, 1),
        Op::Clobber => (How::Clobber, 1),
        Op::DoubleGreat => (How::Append, 1),
        Op::LessGreat => (How::ReadWrite, 0),
        Op::LessAnd => (How::CopyIn, 0),
        Op::GreatAnd => (How::CopyOut, 1),
        _ => return None,
    })
}

struct Parser<S> {
    source: S,
    /// The next token when it was looked at but not taken.
    peeked: Option<(Token, Pos)>,
    /// Where the last token taken starts, but for line ends, so that what
    /// is missing at the end is reported where it was missed.
    last: Pos,
    /// How many compound commands the one being read stands inside.
    depth: usize,
}

impl<S: Source> Parser<S> {
    fn new(source: S) -> Parser<S> {
        Parser {
            source,
            peeked: None,
            last: Pos { line: 1, column: 1 },
            depth: 0,
        }
    }

    fn peek(&mut self) -> Result<Option<&(Token, Pos)>, Error> {
        if self.peeked.is_none() {
            self.peeked = self.source.token(self.depth)?;
        }
        Ok(self.peeked.as_ref())
    }

    fn take(&mut self) -> Result<Option<(Token, Pos)>, Error> {
        let token = match self.peeked.take() {
            Some(token) => Some(token),
            None => self.source.token(self.depth)?,
        };
        if let Some((token, at)) = &token
            && *token != Token::Newline
        {
            self.last = *at;
        }
        Ok(token)
    }

    /// Where the next token starts, or the last one at the end.
    fn here(&mut self) -> Result<Pos, Error> {
        let last = self.last;
        Ok(self.peek()?.map_or(last, |&(_, at)| at))
    }

    fn fail<T>(&mut self, code: Code) -> Result<T, Error> {
        error(self.here()?, code)
    }

    /// Takes the line ends here.
    fn newlines(&mut self) -> Result<(), Error> {
        while matches!(self.peek()?, Some((Token::Newline, _))) {
            self.take()?;
        }
        Ok(())
    }

    /// Takes `op` when it comes next, and says whether it did.
    fn took(&mut self, op: Op) -> Result<bool, Error> {
        let next = matches!(self.peek()?, Some((Token::Op(o), _)) if *o == op);
        if next {
            self.take()?;
        }
        Ok(next)
    }

    /// Whether what comes next ends a list: the end, a `)`, `;;` or `;&`,
    /// or a word that closes what the list stands in.
    fn ends_list(&mut self) -> Result<bool, Error> {
        Ok(match self.peek()? {
            None => true,
            Some((Token::Op(op), _)) => matches!(op, Op::Close | Op::DoubleSemi | Op::SemiAnd),
            Some((token, _)) => plain(token).is_some_and(|w| ENDS.contains(&w)),
        })
    }

    /// And-or lists, each ended by `;`, `&` or a line's end, up to what
    /// ends the list; maybe none.
    fn list(&mut self) -> Result<Vec<Item>, Error> {
        let mut items = Vec::new();
        loop {
            self.newlines()?;
            if self.ends_list()? {
                return Ok(items);
            }
            let and_or = self.and_or()?;
            let background = match self.peek()? {
                Some((Token::Op(Op::Background), _)) => true,
                Some((Token::Op(Op::Semi) | Token::Newline, _)) => false,
                _ => {
                    items.push(Item {
                        and_or,
                        background: false,
                    });
                    return Ok(items);
                }
            };
            self.take()?;
            items.push(Item { and_or, background });
        }
    }

    fn and_or(&mut self) -> Result<AndOr, Error> {
        let first = self.pipeline()?;
        let mut rest = Vec::new();
        loop {
            let join = if self.took(Op::AndIf)? {
                Join::And
            } else if self.took(Op::OrIf)? {
                Join::Or
            } else {
                return Ok(AndOr { first, rest });
            };
            self.newlines()?;
            rest.push((join, self.pipeline()?));
        }
    }

    fn pipeline(&mut self) -> Result<Pipeline, Error> {
        let not = matches!(self.peek()?, Some((token, _)) if plain(token) == Some("!"));
        if not {
            self.take()?;
        }
        let mut commands = vec![self.command()?];
        while self.took(Op::Pipe)? {
            self.newlines()?;
            commands.push(self.command()?);
        }
        Ok(Pipeline { not, commands })
    }

    fn command(&mut self) -> Result<Command, Error> {
        let reserved = match self.peek()? {
            Some((token, _)) => {
                plain(token).is_some_and(|w| w == "!" || w == "in" || ENDS.contains(&w))
            }
            None => false,
        };
        if reserved {
            return self.fail(Code::Unexpected);
        }
        if let Some(compound) = self.compound()? {
            return Ok(Command::Compound(compound, self.redirections()?));
        }
        let simple = self.simple()?;
        let name = match simple.words.as_slice() {
            [word] if simple.assignments.is_empty() && simple.redirections.is_empty() => {
                word.plain().filter(|w| is_name(w))
            }
            _ => None,
        };
        let Some(name) = name.map(String::from) else {
            return Ok(Command::Simple(simple));
        };
        if !self.took(Op::Open)? {
            return Ok(Command::Simple(simple));
        }
        if !self.took(Op::Close)? {
            return self.fail(Code::ExpectedCloseParen);
        }
        self.newlines()?;
        let Some(body) = self.compound()? else {
            return self.fail(Code::ExpectedBody);
        };
        Ok(Command::Function(Function {
            name,
            body,
            redirections: self.redirections()?,
            at: simple.at,
        }))
    }

    /// The redirection that comes next, taken; none when something else
    /// does.
    fn redirection(&mut self) -> Result<Option<Redirection>, Error> {
        let stream = match self.peek()? {
            Some(&(Token::IoNumber(n), _)) => {
                self.take()?;
                Some(n)
            }
            _ => None,
        };
        let Some(&(Token::Op(op), at)) = self.peek()? else {
            return Ok(None);
        };
        if matches!(op, Op::DoubleLess | Op::DoubleLessDash) {
            self.take()?;
            let Some((Token::HereDoc(doc), _)) = self.peek()? else {
                return error(at, Code::MissingDelimiter);
            };
            let doc = doc.clone();
            self.take()?;
            return Ok(Some(Redirection::Here {
                stream: stream.unwrap_or(0),
                doc,
            }));
        }
        let Some((how, usual)) = how(op) else {
            return Ok(None);
        };
        self.take()?;
        let Some((Token::Word(word), _)) = self.peek()? else {
            return error(at, Code::MissingPath);
        };
        let word = word.clone();
        self.take()?;
        Ok(Some(Redirection::File {
            stream: stream.unwrap_or(usual),
            how,
            word,
        }))
    }

    /// The redirections that come next, taken.
    fn redirections(&mut self) -> Result<Vec<Redirection>, Error> {
        let mut redirections = Vec::new();
        while let Some(redirection) = self.redirection()? {
            redirections.push(redirection);
        }
        Ok(redirections)
    }

    /// Words, assignments before them, and redirections among them.
    fn simple(&mut self) -> Result<Simple, Error> {
        let mut simple = Simple {
            assignments: Vec::new(),
            words: Vec::new(),
            redirections: Vec::new(),
            at: self.here()?,
        };
        loop {
            if let Some(redirection) = self.redirection()? {
                simple.redirections.push(redirection);
                continue;
            }
            let Some((Token::Word(word), _)) = self.peek()? else {
                break;
            };
            if let Some(assignment) = assignment(word).filter(|_| simple.words.is_empty()) {
                simple.assignments.push(assignment);
            } else {
                let word = word.clone();
                simple.words.push(word);
            }
            self.take()?;
        }
        if simple.assignments.is_empty()
            && simple.words.is_empty()
            && simple.redirections.is_empty()
        {
            return self.fail(Code::MissingCommand);
        }
        Ok(simple)
    }

    /// The compound command that starts here, taken; none when what comes
    /// next starts none.
    fn compound(&mut self) -> Result<Option<Compound>, Error> {
        let (kind, at) = match self.peek()? {
            Some(&(Token::Op(Op::Open), at)) => ("(", at),
            Some((token, at)) => match plain(token) {
                Some("{") => ("{", *at),
                Some("if") => ("if", *at),
                Some("while") => ("while", *at),
                Some("until") => ("until", *at),
                Some("for") => ("for", *at),
                Some("case") => ("case", *at),
                _ => return Ok(None),
            },
            None => return Ok(None),
        };
        if self.depth >= lex::DEEPEST {
            return error(at, Code::TooNested);
        }
        self.depth += 1;
        self.take()?;
        let compound = match kind {
            "(" => {
                let list = self.filled()?;
                if !self.took(Op::Close)? {
                    return self.missing(at, Code::ExpectedCloseParen);
                }
                Compound::Subshell(list)
            }
            "{" => Compound::Brace(self.closed(at, "}", Code::ExpectedCloseBrace)?),
            "if" => self.if_clause(at)?,
            "while" | "until" => Compound::While {
                until: kind == "until",
                condition: self.closed(at, "do", Code::ExpectedDo)?,
                body: self.closed(at, "done", Code::ExpectedDone)?,
            },
            "for" => self.for_clause(at)?,
            _ => self.case_clause(at)?,
        };
        self.depth -= 1;
        Ok(Some(compound))
    }

    /// A list that holds at least one command.
    fn filled(&mut self) -> Result<Vec<Item>, Error> {
        let list = self.list()?;
        if list.is_empty() {
            return self.fail(Code::MissingCommand);
        }
        Ok(list)
    }

    /// The error `code` for what is missing here: at what stands here
    /// instead, or at `at`, where what misses it starts, at the end.
    fn missing<T>(&mut self, at: Pos, code: Code) -> Result<T, Error> {
        let at = self.peek()?.map_or(at, |&(_, here)| here);
        error(at, code)
    }

    /// Takes the reserved word `word` when it comes next, and says whether
    /// it did.
    fn took_word(&mut self, word: &str) -> Result<bool, Error> {
        let next = matches!(self.peek()?, Some((token, _)) if plain(token) == Some(word));
        if next {
            self.take()?;
        }
        Ok(next)
    }

    /// A list that holds a command, and `word` after it, which ends what
    /// starts at `at`; `code` when it does not come.
    fn closed(&mut self, at: Pos, word: &str, code: Code) -> Result<Vec<Item>, Error> {
        let list = self.filled()?;
        if !self.took_word(word)? {
            return self.missing(at, code);
        }
        Ok(list)
    }

    fn if_clause(&mut self, at: Pos) -> Result<Compound, Error> {
        let mut branches = Vec::new();
        loop {
            let condition = self.closed(at, "then", Code::ExpectedThen)?;
            branches.push((condition, self.filled()?));
            if self.took_word("fi")? {
                return Ok(Compound::If {
                    branches,
                    otherwise: None,
                });
            }
            if self.took_word("else")? {
                let otherwise = Some(self.closed(at, "fi", Code::ExpectedFi)?);
                return Ok(Compound::If {
                    branches,
                    otherwise,
                });
            }
            if !self.took_word("elif")? {
                return self.missing(at, Code::ExpectedFi);
            }
        }
    }

    fn for_clause(&mut self, at: Pos) -> Result<Compound, Error> {
        let name = match self.peek()? {
            Some((token, _)) => plain(token).filter(|w| is_name(w)).map(String::from),
            None => None,
        };
        let Some(name) = name else {
            return self.missing(at, Code::ExpectedName);
        };
        self.take()?;
        let mut words = None;
        if !self.took(Op::Semi)? {
            self.newlines()?;
            if self.took_word("in")? {
                let mut list = Vec::new();
                while let Some((Token::Word(word), _)) = self.peek()? {
                    list.push(word.clone());
                    self.take()?;
                }
                words = Some(list);
                let ended =
                    self.took(Op::Semi)? || matches!(self.peek()?, Some((Token::Newline, _)));
                if !ended {
                    return self.missing(at, Code::ExpectedDo);
                }
            }
        }
        self.newlines()?;
        if !self.took_word("do")? {
            return self.missing(at, Code::ExpectedDo);
        }
        let body = self.closed(at, "done", Code::ExpectedDone)?;
        Ok(Compound::For { name, words, body })
    }

    fn case_clause(&mut self, at: Pos) -> Result<Compound, Error> {
        let Some((Token::Word(word), _)) = self.peek()? else {
            return self.missing(at, Code::ExpectedCaseIn);
        };
        let word = word.clone();
        self.take()?;
        self.newlines()?;
        if !self.took_word("in")? {
            return self.missing(at, Code::ExpectedCaseIn);
        }
        let mut items = Vec::new();
        loop {
            self.newlines()?;
            if self.took_word("esac")? {
                return Ok(Compound::Case { word, items });
            }
            if self.peek()?.is_none() {
                return error(at, Code::ExpectedEsac);
            }
            let starts = matches!(
                self.peek()?,
                Some((Token::Word(_) | Token::Op(Op::Open), _))
            );
            if !starts && !items.is_empty() {
                return self.missing(at, Code::ExpectedEsac);
            }
            self.took(Op::Open)?;
            let mut patterns = vec![self.pattern()?];
            while self.took(Op::Pipe)? {
                patterns.push(self.pattern()?);
            }
            if !self.took(Op::Close)? {
                return self.missing(at, Code::ExpectedCloseParen);
            }
            let body = self.list()?;
            let falls = self.took(Op::SemiAnd)?;
            let ended = falls || self.took(Op::DoubleSemi)?;
            items.push(CaseItem {
                patterns,
                body,
                falls,
            });
            if !ended {
                if self.took_word("esac")? {
                    return Ok(Compound::Case { word, items });
                }
                return self.missing(at, Code::ExpectedEsac);
            }
        }
    }

    /// One of a case item's patterns: any word.
    fn pattern(&mut self) -> Result<Word, Error> {
        let Some((Token::Word(word), _)) = self.peek()? else {
            return self.fail(Code::ExpectedPattern);
        };
        let word = word.clone();
        self.take()?;
        Ok(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh::lex::tests::written_short;
    use alloc::format;

    fn word(word: &Word) -> String {
        written_short(&word.parts)
    }

    fn redirection(r: &Redirection) -> String {
        match r {
            Redirection::File {
                stream,
                how,
                word: w,
            } => {
                let op = match how {
                    How::Read => "<",
                    How::Write => ">",
                    How::Clobber => ">|",
                    How::Append => ">>",
                    How::ReadWrite => "<>",
                    How::CopyIn => "<&",
                    How::CopyOut => ">&",
                };
                format!("{stream}{op}{}", word(w))
            }
            Redirection::Here { stream, doc } => {
                format!("{stream}<<{}:{}", word(&doc.word), written_short(&doc.body))
            }
        }
    }

    fn compound(c: &Compound) -> String {
        match c {
            Compound::Brace(l) => format!("{{ {}; }}", list(l)),
            Compound::Subshell(l) => format!("( {} )", list(l)),
            Compound::For { name, words, body } => {
                let mut head = vec![format!("for {name}")];
                if let Some(words) = words {
                    head.push("in".into());
                    head.extend(words.iter().map(word));
                }
                format!("{}; do {}; done", head.join(" "), list(body))
            }
            Compound::Case { word: w, items } => {
                let mut out = vec![format!("case {} in", word(w))];
                for i in items {
                    let patterns: Vec<String> = i.patterns.iter().map(word).collect();
                    let end = if i.falls { ";&" } else { ";;" };
                    out.push(format!("{}) {}{end}", patterns.join("|"), list(&i.body)));
                }
                out.push("esac".into());
                out.join(" ")
            }
            Compound::If {
                branches,
                otherwise,
            } => {
                let mut out = Vec::new();
                for (i, (condition, body)) in branches.iter().enumerate() {
                    let key = if i == 0 { "if" } else { "elif" };
                    out.push(format!("{key} {}; then {};", list(condition), list(body)));
                }
                if let Some(otherwise) = otherwise {
                    out.push(format!("else {};", list(otherwise)));
                }
                out.push("fi".into());
                out.join(" ")
            }
            Compound::While {
                until,
                condition,
                body,
            } => {
                let key = if *until { "until" } else { "while" };
                format!("{key} {}; do {}; done", list(condition), list(body))
            }
        }
    }

    fn command(command: &Command) -> String {
        let mut out: Vec<String> = Vec::new();
        let redirections = match command {
            Command::Simple(simple) => {
                out.extend(
                    simple
                        .assignments
                        .iter()
                        .map(|a| format!("{}={}", a.name, word(&a.value))),
                );
                out.extend(simple.words.iter().map(word));
                &simple.redirections
            }
            Command::Compound(c, redirections) => {
                out.push(compound(c));
                redirections
            }
            Command::Function(f) => {
                out.push(format!("{}() {}", f.name, compound(&f.body)));
                &f.redirections
            }
        };
        out.extend(redirections.iter().map(redirection));
        out.join(" ")
    }

    /// A list written short, much as sh would write it back: each
    /// redirection with its stream, a here-document as its word and its
    /// lines, and items joined by `; `.
    fn list(items: &[Item]) -> String {
        items
            .iter()
            .map(|item| {
                let pipeline = |p: &Pipeline| {
                    let commands: Vec<String> = p.commands.iter().map(command).collect();
                    format!("{}{}", if p.not { "! " } else { "" }, commands.join(" | "))
                };
                let mut out = pipeline(&item.and_or.first);
                for (join, p) in &item.and_or.rest {
                    out.push_str(if *join == Join::And { " && " } else { " || " });
                    out.push_str(&pipeline(p));
                }
                if item.background {
                    out.push_str(" &");
                }
                out
            })
            .collect::<Vec<String>>()
            .join("; ")
    }

    /// The commands of `script` written short.
    fn shown(script: &str) -> String {
        list(&super::script(script).unwrap())
    }

    fn refused(script: &str) -> (usize, usize, Code) {
        let e = super::script(script).unwrap_err();
        (e.at.line, e.at.column, e.code)
    }

    #[test]
    fn a_simple_command_is_its_words() {
        assert_eq!(shown("echo hello world"), "echo hello world");
        assert_eq!(shown("  echo   a\t b  "), "echo a b");
        assert_eq!(shown("echo if then fi ! { }"), "echo if then fi ! { }");
        assert_eq!(shown("echo 'a b' \"$c\""), "echo [a b] \"$p{c}");
    }

    #[test]
    fn assignments_come_before_the_first_word() {
        assert_eq!(shown("a=1 b=2 env"), "a=1 b=2 env");
        assert_eq!(shown("a=1"), "a=1");
        assert_eq!(shown("echo a=1 b="), "echo a=1 b=");
        assert_eq!(shown("a=b=c x= y=$z"), "a=b=c x= y=$p{z}");
        assert_eq!(shown("a=\"x y\"$z"), "a=[x y]$p{z}");
        assert_eq!(
            shown("'a'=1 1a=2 a-b=3 _x=4"),
            "[a]=1 1a=2 a-b=3 _x=4",
            "none of them an assignment"
        );
        assert_eq!(shown("_x=4 A1=5"), "_x=4 A1=5");
        let items = super::script("a=1$b").unwrap();
        let Command::Simple(simple) = &items[0].and_or.first.commands[0] else {
            panic!("{items:?}")
        };
        assert_eq!(simple.assignments[0].name, "a");
        assert_eq!(simple.assignments[0].value.at, Pos { line: 1, column: 1 });
        assert_eq!(simple.words, []);
    }

    #[test]
    fn redirections_stand_anywhere_and_name_their_stream() {
        assert_eq!(shown("cat <in >out 2>>err"), "cat 0<in 1>out 2>>err");
        assert_eq!(shown(">out echo hi"), "echo hi 1>out");
        assert_eq!(
            shown("a >|f <>g 3<&0 2>&1 1>&-"),
            "a 1>|f 0<>g 3<&0 2>&1 1>&-"
        );
        assert_eq!(shown("<in"), "0<in", "a redirection alone");
        assert_eq!(shown("a=1 >f b=2"), "a=1 b=2 1>f");
        assert_eq!(shown("echo >f a=1"), "echo a=1 1>f");
    }

    #[test]
    fn a_here_document_is_a_redirection_of_its_lines() {
        assert_eq!(shown("cat <<E\nhi $x\nE\n"), "cat 0<<E:[hi ]\"$p{x}[\n]");
        assert_eq!(shown("cat 3<<-'E'\n\tx\n\tE\n"), "cat 3<<[E]:[x\n]");
        assert_eq!(
            shown("cat <<A; cat <<B\na\nA\nb\nB\necho"),
            "cat 0<<A:[a\n]; cat 0<<B:[b\n]; echo"
        );
        assert_eq!(shown("cat <<A >f\nx\nA\n"), "cat 0<<A:[x\n] 1>f");
    }

    #[test]
    fn pipelines_join_commands_and_not_turns_one_round() {
        assert_eq!(shown("a | b | c"), "a | b | c");
        assert_eq!(shown("! a | b"), "! a | b");
        assert_eq!(shown("a |\n\n b"), "a | b", "a line may end after |");
        assert_eq!(shown("a|b>f"), "a | b 1>f");
    }

    #[test]
    fn and_or_lists_join_pipelines() {
        assert_eq!(shown("a && b || c"), "a && b || c");
        assert_eq!(shown("a &&\n b ||\n\n c"), "a && b || c");
        assert_eq!(shown("! a && ! b"), "! a && ! b");
    }

    #[test]
    fn lists_end_each_item_with_a_separator() {
        assert_eq!(shown("a; b & c\nd"), "a; b &; c; d");
        assert_eq!(shown("a;"), "a");
        assert_eq!(shown("a &"), "a &");
        assert_eq!(shown("a & b &"), "a &; b &");
        assert_eq!(shown("\n\na\n\n"), "a");
        assert_eq!(shown(""), "");
        assert_eq!(shown("# only a comment\n"), "");
        assert_eq!(shown("a && b & c || d"), "a && b &; c || d");
    }

    #[test]
    fn what_is_missing_is_reported_where_it_is_missed() {
        assert_eq!(refused("| a"), (1, 1, Code::MissingCommand));
        assert_eq!(refused("a |"), (1, 3, Code::MissingCommand));
        assert_eq!(refused("a && ;"), (1, 6, Code::MissingCommand));
        assert_eq!(refused("a ; ; b"), (1, 5, Code::MissingCommand));
        assert_eq!(refused(";"), (1, 1, Code::MissingCommand));
        assert_eq!(refused("&"), (1, 1, Code::MissingCommand));
        assert_eq!(refused("!"), (1, 1, Code::MissingCommand));
        assert_eq!(refused("a\n|| b"), (2, 1, Code::MissingCommand));
        assert_eq!(refused("a >"), (1, 3, Code::MissingPath));
        assert_eq!(refused("a > ;"), (1, 3, Code::MissingPath));
        assert_eq!(refused("a 2>"), (1, 4, Code::MissingPath));
        assert_eq!(refused("a >\nb"), (1, 3, Code::MissingPath));
        assert_eq!(refused("cat <<"), (1, 5, Code::MissingDelimiter));
        assert_eq!(refused("cat << ;"), (1, 5, Code::MissingDelimiter));
    }

    #[test]
    fn what_cannot_start_a_command_is_unexpected() {
        for (script, at) in [
            ("fi", (1, 1)),
            ("a; then", (1, 4)),
            ("a )", (1, 3)),
            ("a;;", (1, 2)),
            ("a ;& b", (1, 3)),
            ("a | }", (1, 5)),
            ("! !", (1, 3)),
            ("a && done", (1, 6)),
            ("a\nesac", (2, 1)),
        ] {
            assert_eq!(refused(script), (at.0, at.1, Code::Unexpected), "{script}");
        }
        assert_eq!(refused("echo 'a"), (1, 6, Code::Unclosed), "the lexer's");
    }

    #[test]
    fn each_command_knows_where_it_starts() {
        let items = super::script("a\n  b; c | d").unwrap();
        let at: Vec<(usize, usize)> = items
            .iter()
            .flat_map(|i| &i.and_or.first.commands)
            .map(|c| {
                let Command::Simple(s) = c else {
                    panic!("{c:?}")
                };
                (s.at.line, s.at.column)
            })
            .collect();
        assert_eq!(at, [(1, 1), (2, 3), (2, 6), (2, 10)]);
    }

    #[test]
    fn an_assignment_needs_a_name_and_an_equals_sign_unquoted() {
        let read = |text: &str| assignment(&only(text)).map(|a| (a.name, word(&a.value)));
        assert_eq!(read("a=1"), Some(("a".into(), "1".into())));
        assert_eq!(read("a==1"), Some(("a".into(), "=1".into())));
        assert_eq!(read("a="), Some(("a".into(), String::new())));
        assert_eq!(read("a='x'"), Some(("a".into(), "[x]".into())));
        assert_eq!(read("a=$b$c"), Some(("a".into(), "$p{b}$p{c}".into())));
        for not in [
            "=1", "1a=2", "a-b=3", "$a=1", "'a'=1", "a\\=1", "\"a=1\"", "a",
        ] {
            assert_eq!(read(not), None, "{not}");
        }
        let a = assignment(&only("abc=d$e")).unwrap();
        assert_eq!(a.value.parts.len(), 2);
        assert_eq!(a.value.at, Pos { line: 1, column: 1 });
    }

    /// The one word `text` is.
    fn only(text: &str) -> Word {
        match lex::tokens(text).unwrap().as_slice() {
            [(Token::Word(word), _)] => word.clone(),
            other => panic!("{text:?} is {other:?}"),
        }
    }

    #[test]
    fn comments_and_joined_lines_sit_between_commands() {
        assert_eq!(shown("a # one\nb # two"), "a; b");
        assert_eq!(shown("a | # pipe on\n b"), "a | b");
        assert_eq!(shown("a \\\n| b \\\n&& c"), "a | b && c");
        assert_eq!(shown("a &\nb"), "a &; b");
        assert_eq!(shown("a &&\n# why\nb"), "a && b");
    }

    #[test]
    fn what_is_missing_at_the_end_is_reported_where_it_was_missed() {
        assert_eq!(refused("a |\n\n"), (1, 3, Code::MissingCommand));
        assert_eq!(refused("a &&\n# nothing\n"), (1, 3, Code::MissingCommand));
        assert_eq!(
            refused("\n\n!\n"),
            (3, 2, Code::MissingCommand),
            "after the !"
        );
        assert_eq!(refused("a ) b"), (1, 3, Code::Unexpected));
        assert_eq!(refused("cat <<E"), (1, 5, Code::Unclosed), "the lexer's");
    }

    #[test]
    fn long_pipelines_and_lists_take_no_deeper_stack() {
        let pipeline = format!("{}a", "a | ".repeat(5000));
        let items = super::script(&pipeline).unwrap();
        assert_eq!(items[0].and_or.first.commands.len(), 5001);
        let joined = format!("{}a", "a && b || ".repeat(5000));
        assert_eq!(super::script(&joined).unwrap()[0].and_or.rest.len(), 10_000);
        let list = "a; b &\n".repeat(5000);
        assert_eq!(super::script(&list).unwrap().len(), 10_000);
    }

    #[test]
    fn a_stream_number_stands_for_the_redirection_after_it_alone() {
        assert_eq!(shown("a 2>x >y"), "a 2>x 1>y");
        assert_eq!(shown("a 2 >x"), "a 2 1>x", "a word, not a number");
        assert_eq!(shown("a 0<x <y"), "a 0<x 0<y");
        assert_eq!(shown("a 4<<E <<F\nx\nE\ny\nF\n"), "a 4<<E:[x\n] 0<<F:[y\n]");
    }

    #[test]
    fn braces_and_parentheses_group_lists() {
        assert_eq!(shown("{ a; b; }"), "{ a; b; }");
        assert_eq!(shown("{\n a\n b\n}"), "{ a; b; }");
        assert_eq!(shown("(a; b)"), "( a; b )");
        assert_eq!(shown("( a & )"), "( a & )");
        assert_eq!(shown("{ a; } >f 2>&1"), "{ a; } 1>f 2>&1");
        assert_eq!(shown("(a) | { b; } && c"), "( a ) | { b; } && c");
        assert_eq!(shown("((a))"), "( ( a ) )");
        assert_eq!(shown("{ { a; }; }"), "{ { a; }; }");
        assert_eq!(shown("echo { }"), "echo { }", "words after the first");
    }

    #[test]
    fn if_holds_its_branches_in_order() {
        assert_eq!(shown("if a; then b; fi"), "if a; then b; fi");
        assert_eq!(shown("if a\nthen\n b\nfi"), "if a; then b; fi");
        assert_eq!(
            shown("if a; then b; elif c; then d; elif e; then f; else g; fi"),
            "if a; then b; elif c; then d; elif e; then f; else g; fi"
        );
        assert_eq!(shown("if a; b; then c; d; fi"), "if a; b; then c; d; fi");
        assert_eq!(
            shown("if if a; then b; fi; then c; fi"),
            "if if a; then b; fi; then c; fi"
        );
        assert_eq!(shown("if a; then b; fi >f"), "if a; then b; fi 1>f");
        assert_eq!(
            shown("echo if; if a; then echo then fi; fi"),
            "echo if; if a; then echo then fi; fi"
        );
    }

    #[test]
    fn loops_hold_a_condition_and_a_body() {
        assert_eq!(shown("while a; do b; done"), "while a; do b; done");
        assert_eq!(shown("until a\ndo\n b\ndone"), "until a; do b; done");
        assert_eq!(
            shown("while a; do while b; do c; done; done"),
            "while a; do while b; do c; done; done"
        );
        assert_eq!(
            shown("while read -r l; do echo $l; done <f"),
            "while read -r l; do echo $p{l}; done 0<f"
        );
    }

    #[test]
    fn for_takes_its_words_or_the_parameters() {
        assert_eq!(
            shown("for x in a b c; do echo $x; done"),
            "for x in a b c; do echo $p{x}; done"
        );
        assert_eq!(
            shown("for x in; do a; done"),
            "for x in; do a; done",
            "no words"
        );
        assert_eq!(shown("for x do a; done"), "for x; do a; done");
        assert_eq!(shown("for x; do a; done"), "for x; do a; done");
        assert_eq!(shown("for x\ndo a; done"), "for x; do a; done");
        assert_eq!(shown("for x\nin a\ndo a; done"), "for x in a; do a; done");
        assert_eq!(
            shown("for x in do done in; do a; done"),
            "for x in do done in; do a; done",
            "after in, reserved words are words"
        );
        assert_eq!(
            shown("for x in *.txt \"$@\"\n\ndo a\ndone"),
            "for x in *.txt \"$p{@}; do a; done"
        );
    }

    #[test]
    fn case_holds_patterns_and_what_runs_for_them() {
        assert_eq!(
            shown("case $x in a) b;; c|d) e;; esac"),
            "case $p{x} in a) b;; c|d) e;; esac"
        );
        assert_eq!(
            shown("case x in\n(a)\n b\n ;;\n*) c\nesac"),
            "case x in a) b;; *) c;; esac"
        );
        assert_eq!(
            shown("case x in a) b;& c) d;; esac"),
            "case x in a) b;& c) d;; esac"
        );
        assert_eq!(shown("case x in esac"), "case x in esac");
        assert_eq!(shown("case x\nin\nesac"), "case x in esac");
        assert_eq!(
            shown("case x in a) ;; esac"),
            "case x in a) ;; esac",
            "an empty body"
        );
        assert_eq!(
            shown("case x in (esac) a;; in|do) b;; esac"),
            "case x in esac) a;; in|do) b;; esac",
            "after ( or |, esac is a pattern"
        );
        assert_eq!(
            shown("case a in a) case b in b) c;; esac;; esac"),
            "case a in a) case b in b) c;; esac;; esac"
        );
        assert_eq!(
            shown("case $1 in -h|--help) usage;; *) run \"$@\";; esac >log"),
            "case $p{1} in -h|--help) usage;; *) run \"$p{@};; esac 1>log"
        );
    }

    #[test]
    fn a_function_is_a_name_and_a_compound_command() {
        assert_eq!(shown("f() { a; }"), "f() { a; }");
        assert_eq!(shown("f ( ) { a; }"), "f() { a; }");
        assert_eq!(shown("f()\n\n{ a; }"), "f() { a; }");
        assert_eq!(shown("f() ( a )"), "f() ( a )");
        assert_eq!(shown("f() if a; then b; fi"), "f() if a; then b; fi");
        assert_eq!(shown("f() { a; } >log; f"), "f() { a; } 1>log; f");
        assert_eq!(shown("_f1() { a; }"), "_f1() { a; }");
        let items = super::script("\n  g() { a; }").unwrap();
        let Command::Function(f) = &items[0].and_or.first.commands[0] else {
            panic!("{items:?}")
        };
        assert_eq!((f.name.as_str(), f.at), ("g", Pos { line: 2, column: 3 }));
        for (script, at) in [
            ("echo a (b)", (1, 8)),
            ("in() { a; }", (1, 1)),
            ("a-b() { a; }", (1, 4)),
            ("a=1 f() { a; }", (1, 6)),
            ("'f'() { a; }", (1, 4)),
            ("f >x () { a; }", (1, 6)),
        ] {
            assert_eq!(refused(script), (at.0, at.1, Code::Unexpected), "{script}");
        }
    }

    #[test]
    fn a_compound_command_left_open_names_what_it_misses() {
        for (script, at, code) in [
            ("if a", (1, 1), Code::ExpectedThen),
            ("if a; b; fi", (1, 10), Code::ExpectedThen),
            ("if a; then b", (1, 1), Code::ExpectedFi),
            ("if a; then b; done", (1, 15), Code::ExpectedFi),
            ("if a; then b; else c", (1, 1), Code::ExpectedFi),
            ("if a; then b; else c; done", (1, 23), Code::ExpectedFi),
            ("while a", (1, 1), Code::ExpectedDo),
            ("while a; done", (1, 10), Code::ExpectedDo),
            ("until a; do b", (1, 1), Code::ExpectedDone),
            ("while a; do b; fi", (1, 16), Code::ExpectedDone),
            ("for x in a b", (1, 1), Code::ExpectedDo),
            ("for x in a b; done", (1, 15), Code::ExpectedDo),
            ("for x in a >f; do b; done", (1, 12), Code::ExpectedDo),
            ("for x do b", (1, 1), Code::ExpectedDone),
            ("for", (1, 1), Code::ExpectedName),
            ("for 1x in a; do b; done", (1, 5), Code::ExpectedName),
            ("for 'x' in a; do b; done", (1, 5), Code::ExpectedName),
            ("case", (1, 1), Code::ExpectedCaseIn),
            ("case x", (1, 1), Code::ExpectedCaseIn),
            ("case x a) b;; esac", (1, 8), Code::ExpectedCaseIn),
            ("case ; in esac", (1, 6), Code::ExpectedCaseIn),
            ("case x in a) b;;", (1, 1), Code::ExpectedEsac),
            ("case x in a) b", (1, 1), Code::ExpectedEsac),
            ("case x in a) b; done", (1, 17), Code::ExpectedEsac),
            ("case x in a b) c;; esac", (1, 13), Code::ExpectedCloseParen),
            ("case x in a", (1, 1), Code::ExpectedCloseParen),
            ("case x in ) a;; esac", (1, 11), Code::ExpectedPattern),
            ("case x in a|) b;; esac", (1, 13), Code::ExpectedPattern),
            ("(a", (1, 1), Code::ExpectedCloseParen),
            ("(a; }", (1, 5), Code::ExpectedCloseParen),
            ("{ a", (1, 1), Code::ExpectedCloseBrace),
            ("{ a; )", (1, 6), Code::ExpectedCloseBrace),
            ("f(", (1, 2), Code::ExpectedCloseParen),
            ("f(a)", (1, 3), Code::ExpectedCloseParen),
            ("f() a", (1, 5), Code::ExpectedBody),
            ("echo (a)", (1, 7), Code::ExpectedCloseParen),
            ("f()", (1, 3), Code::ExpectedBody),
        ] {
            assert_eq!(refused(script), (at.0, at.1, code), "{script}");
        }
    }

    #[test]
    fn a_compound_command_needs_a_command_in_each_list() {
        for (script, at) in [
            ("{ }", (1, 3)),
            ("( )", (1, 3)),
            ("if then a; fi", (1, 4)),
            ("if a; then fi", (1, 12)),
            ("if a; then b; else fi", (1, 20)),
            ("while do a; done", (1, 7)),
            ("while a; do done", (1, 13)),
            ("for x in a; do done", (1, 16)),
            ("{\n\n}", (3, 1)),
        ] {
            assert_eq!(
                refused(script),
                (at.0, at.1, Code::MissingCommand),
                "{script}"
            );
        }
    }

    #[test]
    fn compound_commands_stand_inside_each_other_sixteen_deep() {
        let braces = |n: usize| format!("{}a{}", "{ ".repeat(n), "; }".repeat(n));
        assert!(super::script(&braces(16)).is_ok());
        assert_eq!(refused(&braces(17)), (1, 33, Code::TooNested));
        let ifs = |n: usize| format!("{}a{}", "if a; then ".repeat(n), "; fi".repeat(n));
        assert!(super::script(&ifs(16)).is_ok());
        assert_eq!(refused(&ifs(17)), (1, 177, Code::TooNested));
        assert!(
            super::script(&"{ a; }; ".repeat(100)).is_ok(),
            "side by side"
        );
        let mixed = format!("{}a{}", "( while a; do ".repeat(8), "; done )".repeat(8));
        assert!(super::script(&mixed).is_ok());
        assert_eq!(refused(&format!("{{ {mixed}; }}")).2, Code::TooNested);
    }

    #[test]
    fn a_script_of_compound_commands_reads_as_sh_reads_it() {
        let script = r#"#!/bin/sh
set -eu
usage() {
	echo "usage: ${0##*/} [-v] file..." >&2
	exit 2
}
verbose=0
while getopts v opt; do
	case $opt in
	v) verbose=1 ;;
	*) usage ;;
	esac
done
shift $((OPTIND - 1))
[ $# -gt 0 ] || usage
for f; do
	if [ -d "$f" ]; then
		(cd "$f" && ls)
	elif [ "$verbose" = 1 ]; then
		echo "$f"
	fi
done
"#;
        assert_eq!(
            shown(script),
            "set -eu; \
             usage() { echo [usage: ]\"$b{0##*/}[ [-v] file...] 1>&2; exit 2; }; \
             verbose=0; \
             while getopts v opt; do case $p{opt} in v) verbose=1;; *) usage;; esac; done; \
             shift $a{OPTIND - 1}; \
             [ $p{#} -gt 0 ] || usage; \
             for f; do if [ -d \"$p{f} ]; then ( cd \"$p{f} && ls ); \
             elif [ \"$p{verbose} = 1 ]; then echo \"$p{f}; fi; done"
        );
    }

    #[test]
    fn a_here_document_in_a_compound_command_starts_after_its_line() {
        assert_eq!(
            shown("if a; then cat <<E; fi\nx\nE\n"),
            "if a; then cat 0<<E:[x\n]; fi"
        );
        assert_eq!(
            shown("while read l; do\n\tcat <<-END\n\tline: $l\n\tEND\ndone <input\n"),
            "while read l; do cat 0<<END:[line: ]\"$p{l}[\n]; done 0<input"
        );
        assert_eq!(shown("( cat ) <<E\nx\nE\n"), "( cat ) 0<<E:[x\n]");
        assert_eq!(
            shown("f() { cat <<E; }\nbody\nE\nf"),
            "f() { cat 0<<E:[body\n]; }; f"
        );
    }

    #[test]
    fn compound_commands_stand_wherever_a_command_does() {
        assert_eq!(
            shown("{ a; b; } | while read x; do c; done"),
            "{ a; b; } | while read x; do c; done"
        );
        assert_eq!(shown("! { a; }"), "! { a; }");
        assert_eq!(shown("if ! a; then b; fi"), "if ! a; then b; fi");
        assert_eq!(shown("( a ) & { b; } &"), "( a ) &; { b; } &");
        assert_eq!(
            shown("f() case $1 in a) b;; esac"),
            "f() case $p{1} in a) b;; esac"
        );
        assert_eq!(
            shown("if a | b && c; then d; fi"),
            "if a | b && c; then d; fi"
        );
        assert_eq!(shown("while :; do :; done"), "while :; do :; done");
        assert_eq!(shown("a;{ b; }"), "a; { b; }");
        assert_eq!(
            shown("a && if b; then c; fi || d"),
            "a && if b; then c; fi || d"
        );
    }

    #[test]
    fn case_patterns_are_words_of_any_kind() {
        assert_eq!(
            shown("case \"$x\" in \"a b\"|'c'|d\\ e|*.txt|[abc]) f;; esac"),
            "case \"$p{x} in [a b]|[c]|d[ ]e|*.txt|[abc]) f;; esac"
        );
        assert_eq!(shown("case x in a);; esac"), "case x in a) ;; esac");
        assert_eq!(
            shown("case x in a) b; c & d ;; esac"),
            "case x in a) b; c &; d;; esac"
        );
        assert_eq!(
            shown("case x in a) echo esac;; esac"),
            "case x in a) echo esac;; esac",
            "esac as an argument"
        );
        assert_eq!(
            shown("case x in\n\n  a)\n\n    b\n\n  ;;\n\nesac"),
            "case x in a) b;; esac"
        );
    }

    #[test]
    fn a_closing_word_counts_only_where_a_command_could_start() {
        assert_eq!(refused("{ a }"), (1, 1, Code::ExpectedCloseBrace));
        assert_eq!(refused("if a; then b fi"), (1, 1, Code::ExpectedFi));
        assert_eq!(refused("while a do b; done"), (1, 15, Code::ExpectedDo));
        assert_eq!(shown("{ a; }"), "{ a; }");
        assert_eq!(shown("{ echo }; }"), "{ echo }; }");
    }

    #[test]
    fn comments_and_line_ends_sit_anywhere_between_words() {
        assert_eq!(
            shown("if a # x\nthen # y\n b # z\nfi # w"),
            "if a; then b; fi"
        );
        assert_eq!(
            shown("for x in a # b c\ndo\n# none\n y\ndone"),
            "for x in a; do y; done"
        );
        assert_eq!(shown("f() # g\n{ a; }"), "f() { a; }");
    }

    #[test]
    fn commands_inside_compound_ones_know_where_they_start() {
        let items = super::script("if a\nthen\n  b\nfi").unwrap();
        let Command::Compound(Compound::If { branches, .. }, _) =
            &items[0].and_or.first.commands[0]
        else {
            panic!("{items:?}")
        };
        let Command::Simple(b) = &branches[0].1[0].and_or.first.commands[0] else {
            panic!("{branches:?}")
        };
        assert_eq!(b.at, Pos { line: 3, column: 3 });
    }

    #[test]
    fn a_service_script_reads_as_sh_reads_it() {
        let script = r#"#!/bin/sh
# Start or stop the daemon.
PIDFILE=/var/run/d.pid

start() {
	if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
		echo "already running" >&2
		return 1
	fi
	daemon --background --pidfile "$PIDFILE" &
	echo $! >"$PIDFILE"
}

stop() {
	[ -f "$PIDFILE" ] || return 0
	kill "$(cat "$PIDFILE")" && rm -f "$PIDFILE"
}

case "$1" in
start|stop) "$1" ;;
restart)
	stop
	start
	;;
*)
	echo "usage: $0 {start|stop|restart}" >&2
	exit 64
	;;
esac
"#;
        assert_eq!(
            shown(script),
            "PIDFILE=/var/run/d.pid; \
             start() { if [ -f \"$p{PIDFILE} ] && kill -0 \"$c{cat \"$PIDFILE\"} 2>/dev/null; \
             then echo [already running] 1>&2; return 1; fi; \
             daemon --background --pidfile \"$p{PIDFILE} &; echo $p{!} 1>\"$p{PIDFILE}; }; \
             stop() { [ -f \"$p{PIDFILE} ] || return 0; \
             kill \"$c{cat \"$PIDFILE\"} && rm -f \"$p{PIDFILE}; }; \
             case \"$p{1} in start|stop) \"$p{1};; restart) stop; start;; \
             *) echo [usage: ]\"$p{0}[ {start|stop|restart}] 1>&2; exit 64;; esac"
        );
    }

    #[test]
    fn every_command_reads_the_same_in_every_place() {
        let commands = [
            "{ a; }",
            "( a )",
            "if a; then b; fi",
            "if a; then b; elif c; then d; else e; fi",
            "while a; do b; done",
            "until a; do b; done",
            "for x in y; do b; done",
            "for x; do b; done",
            "case x in y) b;; z|w) c;& esac",
            "f() { a; }",
            "a | b",
            "! a && b",
        ];
        let places = [
            ("@", "@"),
            ("{ @; }", "{ @; }"),
            ("( @ )", "( @ )"),
            ("if @; then @; fi", "if @; then @; fi"),
            ("while @; do @; done", "while @; do @; done"),
            ("for i in 1; do @; done", "for i in 1; do @; done"),
            ("case z in z) @;; esac", "case z in z) @;; esac"),
            ("g() { @; }", "g() { @; }"),
            ("x && @ || y", "x && @ || y"),
            ("@ | z", "@ | z"),
            ("@\n\n@", "@; @"),
        ];
        for (place, want) in places {
            for command in commands {
                let script = place.replace('@', command);
                assert_eq!(shown(&script), want.replace('@', command), "{script}");
            }
        }
    }

    #[test]
    fn in_starts_no_command() {
        assert_eq!(refused("in a"), (1, 1, Code::Unexpected));
        assert_eq!(refused("a; in"), (1, 4, Code::Unexpected));
        assert_eq!(shown("echo in"), "echo in");
        assert_eq!(
            shown("case in in in) echo in;; esac"),
            "case in in in) echo in;; esac",
            "but as a word"
        );
        assert_eq!(refused("case x in a) in;; esac"), (1, 14, Code::Unexpected));
    }

    #[test]
    fn a_case_inside_parentheses_closes_its_patterns_first() {
        assert_eq!(
            shown("( case x in a) b;; esac )"),
            "( case x in a) b;; esac )"
        );
        assert_eq!(
            shown("(case x in (a) b;; esac)"),
            "( case x in a) b;; esac )"
        );
        assert_eq!(refused("( case x in a) b;; )"), (1, 20, Code::ExpectedEsac));
        assert_eq!(
            refused("case x in a) b;; ;; esac"),
            (1, 18, Code::ExpectedEsac)
        );
        assert_eq!(refused("case x in ;; esac"), (1, 11, Code::ExpectedPattern));
    }

    #[test]
    fn an_inner_command_left_open_is_reported_before_the_outer() {
        assert_eq!(
            refused("if a; then { b; fi"),
            (1, 17, Code::ExpectedCloseBrace)
        );
        assert_eq!(
            refused("while a; do ( b; done"),
            (1, 18, Code::ExpectedCloseParen)
        );
        assert_eq!(refused("{ if a; then b; }"), (1, 17, Code::ExpectedFi));
        assert_eq!(
            refused("f() { while a; do b; }"),
            (1, 22, Code::ExpectedDone)
        );
    }

    #[test]
    fn background_items_stand_inside_compound_lists_too() {
        assert_eq!(shown("{ a & b; }"), "{ a &; b; }");
        assert_eq!(shown("( a; b & )"), "( a; b & )");
        assert_eq!(shown("if a & then b; fi"), "if a &; then b; fi");
        assert_eq!(shown("while a & do b & done"), "while a &; do b &; done");
        assert_eq!(shown("case x in y) a & ;; esac"), "case x in y) a &;; esac");
    }

    #[test]
    fn a_configure_script_reads_as_sh_reads_it() {
        let script = r#"#!/bin/sh
prefix=/usr/local
for arg; do
	case $arg in
	--prefix=*) prefix=${arg#*=} ;;
	--help)
		cat <<USAGE
usage: ./configure [--prefix=DIR]
USAGE
		exit 0
		;;
	*) echo "unknown option: $arg" >&2; exit 1 ;;
	esac
done
{
	echo "PREFIX = $prefix"
	if command -v cc >/dev/null 2>&1; then
		echo "CC = cc"
	else
		echo "CC = gcc"
	fi
} >config.mk
trap 'rm -f conftest.c' EXIT
until [ -f config.mk ]; do sleep 1; done
"#;
        assert_eq!(
            shown(script),
            "prefix=/usr/local; \
             for arg; do case $p{arg} in --prefix=*) prefix=$b{arg#*=};; \
             --help) cat 0<<USAGE:[usage: ./configure [--prefix=DIR]\n]; exit 0;; \
             *) echo [unknown option: ]\"$p{arg} 1>&2; exit 1;; esac; done; \
             { echo [PREFIX = ]\"$p{prefix}; \
             if command -v cc 1>/dev/null 2>&1; then echo [CC = cc]; else echo [CC = gcc]; fi; \
             } 1>config.mk; \
             trap [rm -f conftest.c] EXIT; \
             until [ -f config.mk ]; do sleep 1; done"
        );
    }

    #[test]
    fn a_command_left_open_over_lines_is_reported_where_it_starts() {
        assert_eq!(refused("if a\nthen\n  b\n"), (1, 1, Code::ExpectedFi));
        assert_eq!(
            refused("x\n\nwhile a\ndo\n  b\n"),
            (3, 1, Code::ExpectedDone)
        );
        assert_eq!(refused("f() {\n  a\n"), (1, 5, Code::ExpectedCloseBrace));
        assert_eq!(
            refused("case $x in\n  a) b ;;\n"),
            (1, 1, Code::ExpectedEsac)
        );
        assert_eq!(refused("for i in 1 2\n"), (1, 1, Code::ExpectedDo));
        assert_eq!(refused("(\n  a\n"), (1, 1, Code::ExpectedCloseParen));
        assert_eq!(
            refused("if a; then\n  b\nfi\nif c; then\n  d\n"),
            (4, 1, Code::ExpectedFi)
        );
        assert_eq!(refused("if a\nthen b\nesac"), (3, 1, Code::ExpectedFi));
    }

    #[test]
    fn what_follows_a_compound_command_is_a_redirection_or_a_separator() {
        assert_eq!(refused("{ a; } }"), (1, 8, Code::Unexpected));
        assert_eq!(refused("(a))"), (1, 4, Code::Unexpected));
        assert_eq!(refused("if a; then b; fi fi"), (1, 18, Code::Unexpected));
        assert_eq!(refused("{ a; } b"), (1, 8, Code::Unexpected));
        assert_eq!(shown("{ a; } 2>e >o"), "{ a; } 2>e 1>o");
        assert_eq!(shown("{ a; }\nb"), "{ a; }; b");
    }

    #[test]
    fn a_script_of_simple_commands_reads_as_sh_reads_it() {
        let script = r#"PATH=/bin:/usr/bin export PATH
ls -l "$dir" | grep -v '^d' | wc -l >count 2>/dev/null &&
	echo counted || echo failed >&2
cat <<END >>log; date
run at $(date)
END
"#;
        assert_eq!(
            shown(script),
            "PATH=/bin:/usr/bin export PATH; \
             ls -l \"$p{dir} | grep -v [^d] | wc -l 1>count 2>/dev/null \
             && echo counted || echo failed 1>&2; \
             cat 0<<END:[run at ]\"$c{date}[\n] 1>>log; date"
        );
    }

    #[test]
    fn a_script_left_open_at_its_end_wants_more() {
        for open in [
            "echo 'a",
            "echo \"a",
            "echo a |\n",
            "true &&",
            "false ||\n",
            "if true; then\n",
            "if true; then echo a\n",
            "if true; then echo a; else\n",
            "while true\n",
            "for i in a b; do\n",
            "case a in\n",
            "case a in a) echo\n",
            "{ echo a\n",
            "( echo a\n",
            "f() {\n",
            "f()\n",
            "cat <<EOF\nline\n",
            "echo $(echo a\n",
            "echo ${a\n",
        ] {
            assert!(wants_more(open), "{open:?}");
        }
    }

    #[test]
    fn a_script_that_reads_or_is_wrong_before_its_end_wants_none() {
        for done in [
            "",
            "echo a\n",
            "if true; then echo a; fi\n",
            "fi\n",
            "echo a; fi; if true; then\n",
            "| echo a\n",
            "echo a )\n",
            ";; echo\n",
            "done\n",
        ] {
            assert!(!wants_more(done), "{done:?}");
        }
    }
}
