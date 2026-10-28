//! find: walks the tree of files under each path it is given and does
//! what its expression says with each file, as POSIX's find does: tests
//! of a file's name, path, type, size and links, `-prune` and `-depth`,
//! `-print` and `-print0`, and `-exec` and `-ok` to run a program on it,
//! joined by `!`, `-a`, `-o` and parentheses. Meridian's files have no
//! owners, modes or times, so `-user`, `-group`, `-nouser`, `-nogroup`,
//! `-perm`, `-atime`, `-ctime`, `-mtime` and `-newer` are refused, as are
//! `-xdev` and `-mount`, since volumes cannot be told apart yet; no file
//! has a second link, and with no symbolic links `-H` and `-L` change
//! nothing. It ends with 0, or 1 when a file could not be read or a
//! program run with `-exec ... {} +` failed.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::child::{self, Ran};
use program::{Ended, Error, Handle, text};
use shell::sh::pattern::Pattern;

program::main!(find);

/// How many bytes the paths of one run of `-exec ... {} +` may take.
const BATCH: usize = 16384;

/// How a number in an expression compares: more than it with `+`, less
/// with `-`, else just it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compare {
    More,
    Less,
    Exactly,
}

impl Compare {
    fn holds(self, value: u64, n: u64) -> bool {
        match self {
            Compare::More => value > n,
            Compare::Less => value < n,
            Compare::Exactly => value == n,
        }
    }
}

/// A test or an action of an expression.
#[derive(Debug, PartialEq, Eq)]
enum Primary {
    /// The last part of the path matches, with its letters folded when
    /// the flag says so.
    Name(Pattern, bool),
    Path(Pattern),
    /// The letter of a type, a directory with `d` and a file with `f`;
    /// no file is of any other.
    Type(char),
    /// The size in blocks of 512 bytes, or in bytes when the flag says.
    Size(Compare, u64, bool),
    Links(Compare, u64),
    Prune,
    Print,
    Print0,
    /// A program to run on each file, which the file's path replaces `{}`
    /// in the words of; with `all`, on many files at once, the run of
    /// them with the number given.
    Exec {
        words: Vec<String>,
        all: Option<usize>,
    },
    /// A program to run on a file when the one at the console says so.
    Ok(Vec<String>),
}

/// An expression: a primary, or expressions joined.
#[derive(Debug, PartialEq, Eq)]
enum Expression {
    Primary(Primary),
    Not(Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
    /// What is always true, as `-depth`, `-H` and `-L` are.
    True,
}

/// What find's words ask for: the paths to start at, the expression, and
/// whether a directory's files come before it.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    paths: Vec<&'w str>,
    expression: Expression,
    depth: bool,
    /// How many runs of `-exec ... {} +` there are.
    batches: usize,
}

/// The words of an expression, read one primary at a time.
struct Parser<'w> {
    words: &'w [String],
    at: usize,
    depth: bool,
    /// Whether an action other than `-print0` came, without which
    /// `-print` is done for every file the expression holds for.
    acts: bool,
    batches: usize,
}

/// A pattern as find's words write one, a backslash making the character
/// after it stand for itself.
fn pattern(written: &str) -> Pattern {
    let mut chars = Vec::new();
    let mut letters = written.chars();
    while let Some(c) = letters.next() {
        // A backslash at the very end has nothing to stand for, so it
        // stands for itself.
        if c == '\\'
            && let Some(next) = letters.next()
        {
            chars.push((next, true));
        } else {
            chars.push((c, false));
        }
    }
    Pattern::new(&chars)
}

/// A number with how it compares, as `-size` and `-links` take one.
fn number(written: &str) -> Option<(Compare, u64)> {
    let (compare, digits) = match written.as_bytes().first()? {
        b'+' => (Compare::More, &written[1..]),
        b'-' => (Compare::Less, &written[1..]),
        _ => (Compare::Exactly, written),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((compare, digits.parse().ok()?))
}

impl<'w> Parser<'w> {
    fn peek(&self) -> Option<&'w str> {
        self.words.get(self.at).map(String::as_str)
    }

    /// The word after `primary`, which wants `what`.
    fn value(&mut self, primary: &str, what: &str) -> Result<&'w str, String> {
        let value = self
            .peek()
            .ok_or_else(|| format!("find: {primary} wants {what}\n"))?;
        self.at += 1;
        Ok(value)
    }

    fn or(&mut self) -> Result<Expression, String> {
        let mut left = self.and()?;
        while self.peek() == Some("-o") {
            self.at += 1;
            left = Expression::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expression, String> {
        let mut left = self.not()?;
        loop {
            match self.peek() {
                None | Some("-o" | ")") => return Ok(left),
                Some("-a") => self.at += 1,
                Some(_) => {}
            }
            left = Expression::And(Box::new(left), Box::new(self.not()?));
        }
    }

    fn not(&mut self) -> Result<Expression, String> {
        if self.peek() == Some("!") {
            self.at += 1;
            return Ok(Expression::Not(Box::new(self.not()?)));
        }
        if self.peek() == Some("(") {
            self.at += 1;
            let inside = self.or()?;
            if self.peek() != Some(")") {
                return Err(String::from("find: a ( without its )\n"));
            }
            self.at += 1;
            return Ok(inside);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expression, String> {
        let word = self
            .peek()
            .ok_or_else(|| String::from("find: an expression ends too soon\n"))?;
        self.at += 1;
        let primary = match word {
            "-name" | "-iname" => {
                let folded = word == "-iname";
                let written = self.value(word, "a pattern")?;
                let written = if folded {
                    written.to_lowercase()
                } else {
                    written.into()
                };
                Primary::Name(pattern(&written), folded)
            }
            "-path" => Primary::Path(pattern(self.value(word, "a pattern")?)),
            "-type" => {
                let letter = self.value(word, "a type")?;
                match letter {
                    "b" | "c" | "d" | "f" | "l" | "p" | "s" => {
                        Primary::Type(letter.chars().next().unwrap_or('f'))
                    }
                    _ => return Err(String::from("find: -type wants b, c, d, f, l, p or s\n")),
                }
            }
            "-size" => {
                let written = self.value(word, "a size")?;
                let (digits, bytes) = match written.strip_suffix('c') {
                    Some(digits) => (digits, true),
                    None => (written, false),
                };
                let (compare, n) = number(digits)
                    .ok_or_else(|| format!("find: -size wants a size, not {written}\n"))?;
                Primary::Size(compare, n, bytes)
            }
            "-links" => {
                let written = self.value(word, "a number")?;
                let (compare, n) = number(written)
                    .ok_or_else(|| format!("find: -links wants a number, not {written}\n"))?;
                Primary::Links(compare, n)
            }
            "-prune" => Primary::Prune,
            "-print" => {
                self.acts = true;
                Primary::Print
            }
            "-print0" => Primary::Print0,
            "-depth" => {
                self.depth = true;
                return Ok(Expression::True);
            }
            "-exec" | "-ok" => {
                self.acts = true;
                return Ok(Expression::Primary(self.command(word)?));
            }
            "-user" | "-group" | "-nouser" | "-nogroup" => {
                return Err(format!("find: {word}: Meridian's files have no owners\n"));
            }
            "-perm" => {
                return Err(String::from(
                    "find: -perm: Meridian's files have no modes\n",
                ));
            }
            "-atime" | "-ctime" | "-mtime" | "-newer" => {
                return Err(format!("find: {word}: Meridian's files keep no times\n"));
            }
            "-xdev" | "-mount" => {
                return Err(format!("find: {word}: volumes cannot be told apart yet\n"));
            }
            _ => return Err(format!("find: {word} is no primary\n")),
        };
        Ok(Expression::Primary(primary))
    }

    /// The program and words of `-exec` or `-ok`, after the word, up to
    /// the `;` that ends them, or for `-exec` the `+` right after `{}`.
    fn command(&mut self, word: &str) -> Result<Primary, String> {
        let start = self.at;
        while let Some(next) = self.peek() {
            self.at += 1;
            let words: Vec<String> = self.words[start..self.at - 1].to_vec();
            let together =
                next == "+" && word == "-exec" && words.last().is_some_and(|w| w == "{}");
            if next == ";" || together {
                if words.is_empty() {
                    return Err(format!("find: {word} wants a program\n"));
                }
                if word == "-ok" {
                    return Ok(Primary::Ok(words));
                }
                let all = together.then(|| {
                    self.batches += 1;
                    self.batches - 1
                });
                return Ok(Primary::Exec { words, all });
            }
        }
        Err(format!("find: {word} wants a ; at its end\n"))
    }
}

/// What find's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        match word.as_str() {
            "-H" | "-L" => words = rest,
            "--" => {
                words = rest;
                break;
            }
            _ => break,
        }
    }
    let expression_at = words
        .iter()
        .position(|w| w.starts_with('-') || w == "!" || w == "(")
        .unwrap_or(words.len());
    if expression_at == 0 {
        return Err(String::from("find: finds from a path at least\n"));
    }
    let mut parser = Parser {
        words: &words[expression_at..],
        at: 0,
        depth: false,
        acts: false,
        batches: 0,
    };
    let mut expression = if parser.words.is_empty() {
        Expression::True
    } else {
        parser.or()?
    };
    if let Some(word) = parser.peek() {
        return Err(format!("find: {word} is out of place\n"));
    }
    if !parser.acts {
        let print = Expression::Primary(Primary::Print);
        expression = Expression::And(Box::new(expression), Box::new(print));
    }
    Ok(Asked {
        paths: words[..expression_at].iter().map(String::as_str).collect(),
        expression,
        depth: parser.depth,
        batches: parser.batches,
    })
}

/// What find needs of a tree of files, of its output and of the one at
/// its console, and how it runs a program.
trait Tree {
    /// Whether the file at `path` is a directory, and its size.
    fn info(&mut self, path: &str) -> Result<(bool, u64), Error>;

    /// The names in the directory at `path`.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// Writes `bytes` on the standard output.
    fn out(&mut self, bytes: &[u8]);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);

    /// The answer typed to `question`, none when no one can answer.
    fn ask(&mut self, question: &str) -> Option<String>;

    /// Runs the program `words` name, after what was written went out.
    fn run(&mut self, words: &[&str]) -> Ran;
}

/// A file found: its path, the last part of it, and what it is.
struct Found<'p> {
    path: &'p str,
    name: &'p str,
    directory: bool,
    size: u64,
}

/// The last part of `path`, as find's -name matches it.
fn last_part(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "" } else { "/" };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// `name` in the directory at `directory`.
fn joined(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// The words of `words` with each `{}` in them the path `path`.
fn replaced(words: &[String], path: &str) -> Vec<String> {
    words.iter().map(|word| word.replace("{}", path)).collect()
}

/// A walk of the trees find was given.
struct Walk<'a, T> {
    tree: &'a mut T,
    asked: &'a Asked<'a>,
    /// Whether the file in hand was pruned.
    pruned: bool,
    /// For each run of `-exec ... {} +`, the paths gathered and their
    /// bytes.
    gathered: Vec<(Vec<String>, usize)>,
    failed: bool,
}

impl<T: Tree> Walk<'_, T> {
    /// What the program `words` name did, saying what went wrong when it
    /// did not run, and whether it ended with 0.
    fn ran(&mut self, words: &[String]) -> bool {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        match self.tree.run(&words) {
            Ran::Ended(ended) => ended == Ended::Exited(0),
            Ran::NotThere => {
                self.tree
                    .say(&format!("find: {}: no such program\n", words[0]));
                self.failed = true;
                false
            }
            Ran::Unstarted(e) => {
                self.tree.say(&text::trouble("find", words[0], e));
                self.failed = true;
                false
            }
        }
    }

    /// Runs the program of `-exec ... {} +` number `batch` on the paths
    /// gathered for it, if any.
    fn run_batch(&mut self, words: &[String], batch: usize) {
        let (paths, _) = core::mem::take(&mut self.gathered[batch]);
        if paths.is_empty() {
            return;
        }
        let mut all = words[..words.len() - 1].to_vec();
        all.extend(paths);
        if !self.ran(&all) {
            self.failed = true;
        }
    }

    fn holds(&mut self, expression: &Expression, file: &Found) -> bool {
        match expression {
            Expression::True => true,
            Expression::Not(inside) => !self.holds(inside, file),
            Expression::And(left, right) => self.holds(left, file) && self.holds(right, file),
            Expression::Or(left, right) => self.holds(left, file) || self.holds(right, file),
            Expression::Primary(primary) => self.primary(primary, file),
        }
    }

    fn primary(&mut self, primary: &Primary, file: &Found) -> bool {
        match primary {
            Primary::Name(pattern, false) => pattern.matches(file.name),
            Primary::Name(pattern, true) => pattern.matches(&file.name.to_lowercase()),
            Primary::Path(pattern) => pattern.matches(file.path),
            Primary::Type(letter) => *letter == if file.directory { 'd' } else { 'f' },
            Primary::Size(compare, n, bytes) => {
                let size = if *bytes {
                    file.size
                } else {
                    file.size.div_ceil(512)
                };
                compare.holds(size, *n)
            }
            Primary::Links(compare, n) => compare.holds(1, *n),
            Primary::Prune => {
                self.pruned = true;
                true
            }
            Primary::Print => {
                self.tree.out(file.path.as_bytes());
                self.tree.out(b"\n");
                true
            }
            Primary::Print0 => {
                self.tree.out(file.path.as_bytes());
                self.tree.out(b"\0");
                true
            }
            Primary::Exec { words, all: None } => self.ran(&replaced(words, file.path)),
            Primary::Exec {
                words,
                all: Some(batch),
            } => {
                let (paths, bytes) = &mut self.gathered[*batch];
                paths.push(String::from(file.path));
                *bytes += file.path.len() + 1;
                if *bytes >= BATCH {
                    self.run_batch(words, *batch);
                }
                true
            }
            Primary::Ok(words) => {
                let words = replaced(words, file.path);
                let question = format!("< {} > ? ", words.join(" "));
                let yes = self
                    .tree
                    .ask(&question)
                    .is_some_and(|answer| answer.starts_with(['y', 'Y']));
                yes && self.ran(&words)
            }
        }
    }

    /// Walks the tree from `path`, a directory's files before or after
    /// it as `-depth` says.
    fn visit(&mut self, path: &str) {
        let (directory, size) = match self.tree.info(path) {
            Ok(info) => info,
            Err(e) => {
                self.tree.say(&text::trouble("find", path, e));
                self.failed = true;
                return;
            }
        };
        let file = Found {
            path,
            name: last_part(path),
            directory,
            size,
        };
        if !self.asked.depth {
            self.holds(&self.asked.expression, &file);
        }
        // Whether this file was pruned, which the next is not yet.
        let pruned = core::mem::take(&mut self.pruned);
        if directory && (self.asked.depth || !pruned) {
            match self.tree.names(path) {
                Ok(mut names) => {
                    names.sort_unstable();
                    for name in names {
                        self.visit(&joined(path, &name));
                    }
                }
                Err(e) => {
                    self.tree.say(&text::trouble("find", path, e));
                    self.failed = true;
                }
            }
        }
        if self.asked.depth {
            self.holds(&self.asked.expression, &file);
        }
    }

    /// Runs every `-exec ... {} +` in `expression` on what it still
    /// holds.
    fn finish(&mut self, expression: &Expression) {
        match expression {
            Expression::Not(inside) => self.finish(inside),
            Expression::And(left, right) | Expression::Or(left, right) => {
                self.finish(left);
                self.finish(right);
            }
            Expression::Primary(Primary::Exec {
                words,
                all: Some(batch),
            }) => self.run_batch(words, *batch),
            _ => {}
        }
    }
}

/// Walks the trees `asked` names, and answers the status find ends with.
fn find_in(tree: &mut impl Tree, asked: &Asked) -> u64 {
    let mut walk = Walk {
        tree,
        asked,
        pruned: false,
        gathered: (0..asked.batches).map(|_| (Vec::new(), 0)).collect(),
        failed: false,
    };
    for path in &asked.paths {
        walk.visit(path);
    }
    walk.finish(&asked.expression);
    u64::from(walk.failed)
}

/// Meridian's files, the program's own output, and the one at its
/// console.
struct System {
    out: text::Out<text::Write>,
}

impl Tree for System {
    fn info(&mut self, path: &str) -> Result<(bool, u64), Error> {
        let file = program::open(path, program::abi::file::READ)?;
        let info = program::info(file);
        let _ = program::close(file);
        let info = info?;
        Ok((info.directory, info.size))
    }

    fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
        let directory = program::open(path, program::abi::file::READ)?;
        let mut names = Vec::new();
        let mut room = [0u8; 1024];
        let mut index = 0;
        let listed = loop {
            match program::list(directory, index, &mut room) {
                Ok((len, _)) => names.push(String::from_utf8_lossy(&room[..len]).into_owned()),
                Err(Error::NotFound) => break Ok(names),
                Err(e) => break Err(e),
            }
            index += 1;
        };
        let _ = program::close(directory);
        listed
    }

    fn out(&mut self, bytes: &[u8]) {
        self.out.put(bytes);
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }

    fn ask(&mut self, question: &str) -> Option<String> {
        self.out.flush();
        let _ = program::write_error(question);
        program::typed_line()
    }

    fn run(&mut self, words: &[&str]) -> Ran {
        self.out.flush();
        child::run(words[0], &words[1..])
    }
}

#[cfg_attr(test, allow(dead_code))]
fn find(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut system = System {
        out: text::standard_output(),
    };
    let status = find_in(&mut system, &asked);
    if system.out.finish() { status } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use shell::commands::{self, Allowed, File, Kind, Streams};

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    fn print() -> Expression {
        Expression::Primary(Primary::Print)
    }

    fn name(written: &str) -> Expression {
        Expression::Primary(Primary::Name(pattern(written), false))
    }

    fn and(a: Expression, b: Expression) -> Expression {
        Expression::And(Box::new(a), Box::new(b))
    }

    fn or(a: Expression, b: Expression) -> Expression {
        Expression::Or(Box::new(a), Box::new(b))
    }

    fn expression_of(text: &str) -> Result<Expression, String> {
        let given = words(text);
        asked(&given).map(|asked| asked.expression)
    }

    #[test]
    fn paths_come_first_and_then_the_expression() {
        let given = words("-H -L a b/ -name x");
        let taken = asked(&given).unwrap();
        assert_eq!(taken.paths, ["a", "b/"]);
        assert_eq!(
            taken.expression,
            and(name("x"), print()),
            "-print for what holds"
        );
        assert!(!taken.depth);
        let given = words("a ! -name x");
        assert_eq!(asked(&given).unwrap().paths, ["a"]);
        let given = words("a ( -name x )");
        assert_eq!(asked(&given).unwrap().paths, ["a"]);
        let given = words("a");
        assert_eq!(
            asked(&given).unwrap().expression,
            and(Expression::True, print())
        );
        let given = words("a -depth");
        let taken = asked(&given).unwrap();
        assert!(taken.depth);
        assert_eq!(taken.expression, and(Expression::True, print()));
        let given = words("-- a");
        assert_eq!(asked(&given).unwrap().paths, ["a"]);
    }

    #[test]
    fn and_binds_tighter_than_or_and_not_tighter_than_both() {
        assert_eq!(
            expression_of("a -name x -o -name y -name z -print"),
            Ok(or(name("x"), and(and(name("y"), name("z")), print())))
        );
        assert_eq!(
            expression_of("a -name x -o -name y -name z -print"),
            expression_of("a -name x -o ( -name y -a -name z ) -a -print"),
            "-a or nothing, as you like"
        );
        assert_eq!(
            expression_of("a ! -name x -print"),
            Ok(and(Expression::Not(Box::new(name("x"))), print()))
        );
        assert_eq!(
            expression_of("a ! ! -name x"),
            Ok(and(
                Expression::Not(Box::new(Expression::Not(Box::new(name("x"))))),
                print()
            ))
        );
        assert_eq!(
            expression_of("a ( -name x -o -name y ) -print"),
            Ok(and(or(name("x"), name("y")), print()))
        );
    }

    #[test]
    fn only_an_action_other_than_print0_keeps_print_away() {
        let printing = |text: &str| {
            let given = words(text);
            matches!(asked(&given).unwrap().expression, Expression::And(_, ref p) if **p == print())
        };
        assert!(printing("a -name x"));
        assert!(printing("a -print0"), "POSIX adds -print to -print0 alone");
        assert!(!printing("a -print"));
        assert!(!printing("a -exec true ;"));
        assert!(!printing("a -ok true ;"));
        assert!(!printing("a -exec echo {} +"));
    }

    #[test]
    fn exec_takes_words_up_to_a_semicolon_or_a_plus_after_braces() {
        let given = words("a -exec echo [{}] + ; -exec echo {} + -exec ls {} +");
        let taken = asked(&given).unwrap();
        assert_eq!(taken.batches, 2);
        let exec = |text: &str, all| {
            Expression::Primary(Primary::Exec {
                words: words(text),
                all,
            })
        };
        assert_eq!(
            taken.expression,
            and(
                and(exec("echo [{}] +", None), exec("echo {}", Some(0))),
                exec("ls {}", Some(1))
            ),
            "a + after another word is a word"
        );
        assert_eq!(
            expression_of("a -ok rm {} ;"),
            Ok(Expression::Primary(Primary::Ok(words("rm {}"))))
        );
    }

    #[test]
    fn words_it_cannot_take_are_refused() {
        let refused = |text: &str| expression_of(text).err();
        assert_eq!(
            refused(""),
            Some("find: finds from a path at least\n".into())
        );
        assert_eq!(
            refused("-name x"),
            Some("find: finds from a path at least\n".into())
        );
        assert_eq!(refused("a -foo"), Some("find: -foo is no primary\n".into()));
        assert_eq!(
            refused("a -name"),
            Some("find: -name wants a pattern\n".into())
        );
        assert_eq!(
            refused("a -path"),
            Some("find: -path wants a pattern\n".into())
        );
        assert_eq!(
            refused("a -type"),
            Some("find: -type wants a type\n".into())
        );
        assert_eq!(
            refused("a -type x"),
            Some("find: -type wants b, c, d, f, l, p or s\n".into())
        );
        assert_eq!(
            refused("a -type dd"),
            Some("find: -type wants b, c, d, f, l, p or s\n".into())
        );
        assert_eq!(
            refused("a -size"),
            Some("find: -size wants a size\n".into())
        );
        assert_eq!(
            refused("a -size 1k"),
            Some("find: -size wants a size, not 1k\n".into())
        );
        assert_eq!(
            refused("a -size +c"),
            Some("find: -size wants a size, not +c\n".into())
        );
        assert_eq!(
            refused("a -links x"),
            Some("find: -links wants a number, not x\n".into())
        );
        assert_eq!(
            refused("a -links"),
            Some("find: -links wants a number\n".into())
        );
        assert_eq!(
            refused("a ( -name x"),
            Some("find: a ( without its )\n".into())
        );
        assert_eq!(
            refused("a -name x )"),
            Some("find: ) is out of place\n".into())
        );
        assert_eq!(
            refused("a !"),
            Some("find: an expression ends too soon\n".into())
        );
        assert_eq!(
            refused("a -name x -o"),
            Some("find: an expression ends too soon\n".into())
        );
        assert_eq!(
            refused("a -exec echo"),
            Some("find: -exec wants a ; at its end\n".into())
        );
        assert_eq!(
            refused("a -exec ;"),
            Some("find: -exec wants a program\n".into())
        );
        assert_eq!(
            refused("a -ok echo {} +"),
            Some("find: -ok wants a ; at its end\n".into())
        );
    }

    #[test]
    fn what_meridian_s_files_cannot_answer_is_refused() {
        let refused = |text: &str| expression_of(text).err();
        for word in ["-user x", "-group x", "-nouser", "-nogroup"] {
            let primary = word.split(' ').next().unwrap();
            assert_eq!(
                refused(&format!("a {word}")),
                Some(format!(
                    "find: {primary}: Meridian's files have no owners\n"
                ))
            );
        }
        assert_eq!(
            refused("a -perm 644"),
            Some("find: -perm: Meridian's files have no modes\n".into())
        );
        for word in ["-atime", "-ctime", "-mtime", "-newer"] {
            assert_eq!(
                refused(&format!("a {word} 1")),
                Some(format!("find: {word}: Meridian's files keep no times\n"))
            );
        }
        for word in ["-xdev", "-mount"] {
            assert_eq!(
                refused(&format!("a {word}")),
                Some(format!("find: {word}: volumes cannot be told apart yet\n"))
            );
        }
    }

    #[test]
    fn patterns_numbers_and_parts_of_paths() {
        assert!(pattern("a*").matches("abc"));
        assert!(!pattern("a\\*").matches("abc"));
        assert!(pattern("a\\*").matches("a*"));
        assert!(
            pattern("*.rs").matches(".x.rs"),
            "a leading dot is no special in find"
        );
        assert!(
            pattern("x\\").matches("x\\"),
            "a backslash at the end stands for itself"
        );
        assert!(
            pattern("*/c*").matches("t/b/c/e"),
            "a star takes slashes in -path"
        );
        assert_eq!(number("+3"), Some((Compare::More, 3)));
        assert_eq!(number("-3"), Some((Compare::Less, 3)));
        assert_eq!(number("3"), Some((Compare::Exactly, 3)));
        assert_eq!(number("+"), None);
        assert_eq!(number(""), None);
        assert_eq!(number("3x"), None);
        assert_eq!(number("++3"), None);
        assert!(Compare::More.holds(4, 3) && !Compare::More.holds(3, 3));
        assert!(Compare::Less.holds(2, 3) && !Compare::Less.holds(3, 3));
        assert!(Compare::Exactly.holds(3, 3) && !Compare::Exactly.holds(4, 3));
        assert_eq!(last_part("a/b/c"), "c");
        assert_eq!(last_part("a/b/"), "b");
        assert_eq!(last_part("/"), "/");
        assert_eq!(last_part("//"), "/");
        assert_eq!(last_part("x"), "x");
        assert_eq!(last_part(""), "");
        assert_eq!(joined("a", "b"), "a/b");
        assert_eq!(joined("a/", "b"), "a/b");
        assert_eq!(
            replaced(&words("echo [{}] {}{}"), "p"),
            words("echo [p] pp")
        );
    }

    /// A tree of files among the host's temporary files, gone once
    /// dropped; what find wrote and said, and what it asked and was
    /// answered. It lists names the other way round from their order, so
    /// nothing leans on how a disk keeps them, and runs the shell's
    /// shared commands, which Meridian's programs of those names run.
    struct Host {
        root: std::path::PathBuf,
        written: Vec<u8>,
        said: String,
        answers: Vec<String>,
        asked: Vec<String>,
    }

    impl Host {
        /// The tree that holds `files`, each a path and its bytes; a path
        /// that ends in `/` is a directory.
        fn new(name: &str, files: &[(&str, &str)]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-find-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for (path, bytes) in files {
                let at = root.join(path.trim_end_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, bytes).unwrap();
                }
            }
            Host {
                root,
                written: Vec::new(),
                said: String::new(),
                answers: Vec::new(),
                asked: Vec::new(),
            }
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            _ => Error::Unreadable,
        }
    }

    impl Tree for Host {
        fn info(&mut self, path: &str) -> Result<(bool, u64), Error> {
            let found = std::fs::metadata(self.root.join(path)).map_err(|e| error(&e))?;
            Ok((found.is_dir(), found.len()))
        }

        fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
            let listed = std::fs::read_dir(self.root.join(path)).map_err(|e| error(&e))?;
            let mut names: Vec<String> = listed
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort_unstable_by(|a, b| b.cmp(a));
            Ok(names)
        }

        fn out(&mut self, bytes: &[u8]) {
            self.written.extend_from_slice(bytes);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }

        fn ask(&mut self, question: &str) -> Option<String> {
            self.asked.push(question.into());
            (!self.answers.is_empty()).then(|| self.answers.remove(0))
        }

        fn run(&mut self, words: &[&str]) -> Ran {
            // A file of the tree is none of the kernel's .mexec files.
            if words[0].contains('/') && self.root.join(words[0]).is_file() {
                return Ran::Unstarted(Error::NotAProgram);
            }
            if !commands::knows(words[0]) {
                return Ran::NotThere;
            }
            let words: Vec<String> = words.iter().map(|&w| String::from(w)).collect();
            let status = commands::run(&words, self).unwrap_or(127);
            Ran::Ended(Ended::Exited(u64::from(status)))
        }
    }

    impl Streams for Host {
        fn out(&mut self, bytes: &[u8]) -> bool {
            self.written.extend_from_slice(bytes);
            true
        }

        fn err(&mut self, bytes: &[u8]) -> bool {
            self.said.push_str(&String::from_utf8_lossy(bytes));
            true
        }

        fn file(&mut self, path: &str) -> Option<File> {
            let found = std::fs::metadata(self.root.join(path)).ok()?;
            let kind = if found.is_dir() {
                Kind::Directory
            } else {
                Kind::File
            };
            let allowed = Allowed {
                read: true,
                write: true,
                run: found.is_dir(),
            };
            Some(File {
                kind,
                size: found.len(),
                allowed,
            })
        }

        fn terminal(&mut self, _: u32) -> bool {
            false
        }
    }

    /// How find ran on `host` with the words `text`: its status, what it
    /// wrote and what it said.
    fn ran(host: &mut Host, text: &str) -> (u64, String, String) {
        let given = words(text);
        let status = find_in(host, &asked(&given).unwrap());
        let written = String::from_utf8(core::mem::take(&mut host.written)).unwrap();
        (status, written, core::mem::take(&mut host.said))
    }

    fn wrote(text: &str) -> (u64, String, String) {
        (0, text.into(), String::new())
    }

    /// The tree GNU's find was tried on.
    fn tree(name: &str) -> Host {
        let big = "x".repeat(513);
        let mut host = Host::new(
            name,
            &[
                ("t/a", "x"),
                ("t/b/big", ""),
                ("t/b/c/e", ""),
                ("t/d/F", "y\n"),
            ],
        );
        std::fs::write(host.root.join("t/b/big"), big).unwrap();
        host.answers.clear();
        host
    }

    #[test]
    fn every_file_is_printed_before_or_after_what_it_holds() {
        let mut host = tree("order");
        assert_eq!(
            ran(&mut host, "t"),
            wrote("t\nt/a\nt/b\nt/b/big\nt/b/c\nt/b/c/e\nt/d\nt/d/F\n")
        );
        assert_eq!(
            ran(&mut host, "t -depth"),
            wrote("t/a\nt/b/big\nt/b/c/e\nt/b/c\nt/b\nt/d/F\nt/d\nt\n")
        );
        assert_eq!(
            ran(&mut host, "t/d/ t/a"),
            wrote("t/d/\nt/d/F\nt/a\n"),
            "a path as given"
        );
        assert_eq!(
            ran(&mut host, "t/d -print0"),
            wrote("t/d\0t/d\nt/d/F\0t/d/F\n")
        );
        assert_eq!(
            ran(&mut host, "t/d -print0 -print"),
            wrote("t/d\0t/d\nt/d/F\0t/d/F\n")
        );
    }

    #[test]
    fn files_are_tested_by_name_path_type_size_and_links() {
        // As GNU's find answers on these files.
        let mut host = tree("tests");
        assert_eq!(ran(&mut host, "t -iname f"), wrote("t/d/F\n"));
        assert_eq!(ran(&mut host, "t -name F"), wrote("t/d/F\n"));
        assert_eq!(ran(&mut host, "t -name f"), wrote(""));
        assert_eq!(ran(&mut host, "t -path */c*"), wrote("t/b/c\nt/b/c/e\n"));
        assert_eq!(ran(&mut host, "t -type d"), wrote("t\nt/b\nt/b/c\nt/d\n"));
        assert_eq!(ran(&mut host, "t -type f -size 2"), wrote("t/b/big\n"));
        assert_eq!(ran(&mut host, "t -type f -size 1"), wrote("t/a\nt/d/F\n"));
        assert_eq!(ran(&mut host, "t -type f -size 0"), wrote("t/b/c/e\n"));
        assert_eq!(ran(&mut host, "t -type f -size +1"), wrote("t/b/big\n"));
        assert_eq!(ran(&mut host, "t -type f -size -1"), wrote("t/b/c/e\n"));
        assert_eq!(ran(&mut host, "t -type f -size 2c"), wrote("t/d/F\n"));
        assert_eq!(
            ran(&mut host, "t -type f -size +1c"),
            wrote("t/b/big\nt/d/F\n")
        );
        assert_eq!(ran(&mut host, "t -type l"), wrote(""), "no file is a link");
        assert_eq!(ran(&mut host, "t/d -links 1"), wrote("t/d\nt/d/F\n"));
        assert_eq!(ran(&mut host, "t/d -links +1"), wrote(""));
        assert_eq!(ran(&mut host, "t/d -links -2"), wrote("t/d\nt/d/F\n"));
    }

    #[test]
    fn prune_keeps_find_out_of_a_directory_unless_files_come_first() {
        let mut host = tree("prune");
        assert_eq!(
            ran(&mut host, "t -name b -prune -o -print"),
            wrote("t\nt/a\nt/d\nt/d/F\n")
        );
        assert_eq!(
            ran(&mut host, "t -depth -name b -prune -o -print"),
            wrote("t/a\nt/b/big\nt/b/c/e\nt/b/c\nt/d/F\nt/d\nt\n"),
            "-prune does nothing with -depth"
        );
        assert_eq!(ran(&mut host, "t -prune"), wrote("t\n"));
    }

    #[test]
    fn not_and_and_or_choose_files() {
        let mut host = tree("operators");
        assert_eq!(ran(&mut host, "t/d ! -name F"), wrote("t/d\n"));
        assert_eq!(
            ran(&mut host, "t -name a -o -name F"),
            wrote("t/a\nt/d/F\n")
        );
        assert_eq!(
            ran(&mut host, "t -name a -o -name d -type f"),
            wrote("t/a\n"),
            "and before or: d is no file"
        );
        assert_eq!(
            ran(&mut host, "t ( -name a -o -name d ) -type d"),
            wrote("t/d\n")
        );
        assert_eq!(
            ran(&mut host, "t -name a -print -o -print"),
            wrote("t\nt/a\nt/b\nt/b/big\nt/b/c\nt/b/c/e\nt/d\nt/d/F\n")
        );
    }

    #[test]
    fn exec_runs_a_program_on_each_file_and_holds_when_it_ends_with_0() {
        let mut host = tree("exec");
        assert_eq!(
            ran(&mut host, "t -exec test -d {} ; -print"),
            wrote("t\nt/b\nt/b/c\nt/d\n")
        );
        assert_eq!(
            ran(&mut host, "t/d -exec echo <{}> ;"),
            wrote("<t/d>\n<t/d/F>\n")
        );
        assert_eq!(
            ran(&mut host, "t/d -print -exec echo {} +"),
            wrote("t/d\nt/d/F\nt/d t/d/F\n"),
            "+ runs it once on all, at the end"
        );
        assert_eq!(
            ran(&mut host, "t/d -exec false {} +"),
            (1, String::new(), String::new())
        );
        assert_eq!(
            ran(&mut host, "t/d -exec false {} ;"),
            (0, String::new(), String::new()),
            "; only answers false"
        );
        assert_eq!(
            ran(&mut host, "t/a -exec nope {} ; -print"),
            (1, String::new(), "find: nope: no such program\n".into()),
            "a program not there is a test that failed"
        );
        assert_eq!(
            ran(&mut host, "t/a -exec nope {} ;"),
            (1, String::new(), "find: nope: no such program\n".into())
        );
        assert_eq!(
            ran(&mut host, "t/a -exec t/a {} ; -print"),
            (
                1,
                String::new(),
                "find: t/a: not a program Meridian runs\n".into()
            ),
            "nor is one that does not start"
        );
        assert_eq!(
            ran(&mut host, "t/a -name x -exec echo {} +"),
            wrote(""),
            "no files, no run"
        );
    }

    #[test]
    fn exec_with_a_plus_runs_as_often_as_the_paths_take() {
        // Paths of 63 bytes, 64 with the blank after each, fill a run at 256.
        let names: Vec<String> = (0..300).map(|n| format!("d/{n:0>61}")).collect();
        let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "")).collect();
        let mut host = Host::new("batches", &files);
        let (status, written, said) = ran(&mut host, "d -type f -exec echo {} +");
        assert_eq!((status, said.as_str()), (0, ""));
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2, "19200 bytes of paths take two runs");
        assert_eq!(
            lines[0].split(' ').count(),
            256,
            "a run is full at 16384 bytes"
        );
        let all: Vec<&str> = lines.iter().flat_map(|l| l.split(' ')).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(all, sorted);
        assert!(
            lines[0].len() >= BATCH - 64 && lines[0].len() < BATCH + 64,
            "{}",
            lines[0].len()
        );
    }

    #[test]
    fn ok_asks_before_it_runs() {
        let mut host = tree("ok");
        host.answers = vec!["y".into(), "no".into()];
        assert_eq!(
            ran(&mut host, "t/d -ok echo {} ; -print"),
            wrote("t/d\nt/d\n")
        );
        assert_eq!(host.asked, ["< echo t/d > ? ", "< echo t/d/F > ? "]);
        assert_eq!(
            ran(&mut host, "t/a -ok echo {} ;").0,
            0,
            "no one to answer is a no"
        );
    }

    #[test]
    fn a_path_that_is_not_there_is_said_and_the_rest_walked() {
        let mut host = tree("missing");
        assert_eq!(
            ran(&mut host, "nope t/d"),
            (
                1,
                "t/d\nt/d/F\n".into(),
                "find: nope: no such file\n".into()
            )
        );
    }

    #[test]
    fn and_and_or_run_nothing_their_left_side_decided() {
        let mut host = tree("short");
        assert_eq!(
            ran(&mut host, "t/d -name nope -exec echo ran {} ;"),
            wrote("")
        );
        assert_eq!(
            ran(&mut host, "t/d -name d -o -exec echo ran {} ;"),
            wrote("ran t/d/F\n")
        );
        assert_eq!(
            ran(&mut host, "t/d ! -name d -exec echo ran {} ;"),
            wrote("ran t/d/F\n")
        );
        assert_eq!(
            ran(&mut host, "t/d -exec false ; -o -print"),
            wrote("t/d\nt/d/F\n"),
            "a program that failed is a test that failed"
        );
    }

    #[test]
    fn every_run_of_exec_with_a_plus_goes_at_the_end_wherever_it_is() {
        let mut host = tree("nested");
        assert_eq!(
            ran(
                &mut host,
                "t/d -name F -o ( -exec echo {} + ) -o ! -exec echo x {} +"
            ),
            wrote("t/d\n"),
            "inside parentheses and an or; the second is never reached, + being true"
        );
        assert_eq!(
            ran(&mut host, "t/d ! -exec echo {} + -print"),
            wrote("t/d t/d/F\n"),
            "! of + is false"
        );
        assert_eq!(
            ran(&mut host, "t/d -exec echo a {} + -exec echo b {} +"),
            wrote("a t/d t/d/F\nb t/d t/d/F\n")
        );
    }

    #[test]
    fn a_file_is_walked_alone_and_pruning_it_changes_nothing() {
        let mut host = tree("file");
        assert_eq!(ran(&mut host, "t/a"), wrote("t/a\n"));
        assert_eq!(ran(&mut host, "t/a -prune"), wrote("t/a\n"));
        assert_eq!(ran(&mut host, "t/a -depth"), wrote("t/a\n"));
        assert_eq!(
            ran(&mut host, "t/d -depth -exec echo {} ;"),
            wrote("t/d/F\nt/d\n")
        );
        assert_eq!(
            ran(&mut host, "t/b -name c -prune -o -type f -print"),
            wrote("t/b/big\n")
        );
    }

    #[test]
    fn a_directory_that_cannot_be_listed_is_said_and_the_rest_walked() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut host = tree("unlisted");
        let shut = host.root.join("t/b");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        // One who may read anything lists it all the same.
        let listable = std::fs::read_dir(&shut).is_ok();
        let found = ran(&mut host, "t");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !listable {
            assert_eq!(
                found,
                (
                    1,
                    "t\nt/a\nt/b\nt/d\nt/d/F\n".into(),
                    "find: t/b: could not be read\n".into()
                )
            );
        }
    }

    #[test]
    fn every_file_of_a_random_tree_is_found_once() {
        let mut seed = 0x3c6e_f372_fe94_f82b_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..30 {
            let mut paths = vec![String::from("r/")];
            for n in 0..next() % 40 {
                let parent = paths
                    .iter()
                    .filter(|p| p.ends_with('/'))
                    .nth(
                        usize::try_from(next()).unwrap()
                            % paths.iter().filter(|p| p.ends_with('/')).count(),
                    )
                    .unwrap()
                    .clone();
                let slash = if next() % 3 == 0 { "/" } else { "" };
                paths.push(format!("{parent}n{n}{slash}"));
            }
            let files: Vec<(&str, &str)> = paths.iter().map(|p| (p.as_str(), "")).collect();
            let mut host = Host::new(&format!("random-{round}"), &files);
            let (status, written, _) = ran(&mut host, "r");
            assert_eq!(status, 0);
            let mut found: Vec<&str> = written.lines().collect();
            let mut wanted: Vec<String> = paths
                .iter()
                .map(|p| p.trim_end_matches('/').to_string())
                .collect();
            found.sort_unstable();
            wanted.sort_unstable();
            assert_eq!(found, wanted);
            let (_, depth, _) = ran(&mut host, "r -depth");
            assert_eq!(
                depth.lines().last(),
                Some("r"),
                "the start comes last with -depth"
            );
        }
    }
}
