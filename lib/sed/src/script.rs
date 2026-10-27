//! A sed script read into its commands, as POSIX writes them: addresses
//! before each, `!`, blocks in braces, and the commands with what each
//! takes. A label is found once the whole script is read, so a branch may
//! go forward.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use regex::{Regex, Syntax};

/// A line a command applies to.
#[derive(Debug)]
pub enum Address {
    /// The line of this number, counted from 1 across all the input.
    Line(usize),
    /// `$`: the last line.
    Last,
    /// The lines an expression matches; none for `//`, the one used last.
    Match(Option<Regex>),
}

/// What a command does.
#[derive(Debug)]
pub enum Kind {
    /// `{`: the commands up to the `}` at the index it holds.
    Block(usize),
    /// `}`.
    End,
    Append(String),
    /// `b`: on at the command of that index, one past the last for the
    /// end of the script.
    Branch(usize),
    Change(String),
    Delete,
    DeleteFirst,
    Get,
    GetAppend,
    Hold,
    HoldAppend,
    Insert(String),
    List,
    Next,
    NextAppend,
    Print,
    PrintFirst,
    Quit,
    Read(String),
    Substitute(Box<Substitute>),
    /// `t`: as `b`, when a substitution was made.
    Test(usize),
    Write(String),
    Exchange,
    /// `y`: each character that is first in a pair to the second.
    Translate(Vec<(char, char)>),
    Label,
    Number,
}

/// A command with the addresses that say which lines it applies to.
#[derive(Debug)]
pub struct Command {
    pub first: Option<Address>,
    pub second: Option<Address>,
    /// `!`: the lines the addresses do not select.
    pub negated: bool,
    pub kind: Kind,
}

/// `s`: what it replaces, and with what.
#[derive(Debug)]
pub struct Substitute {
    /// None for `s//.../`, which uses the expression used last.
    pub regex: Option<Regex>,
    pub replacement: Vec<Piece>,
    /// `g`: every match from the `nth` on, not only that one.
    pub every: bool,
    pub nth: usize,
    pub print: bool,
    pub write: Option<String>,
}

/// A part of a replacement.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    /// `&`: what matched.
    Whole,
    /// `\n`: what the subexpression `n` matched.
    Group(usize),
}

/// A script read.
#[derive(Debug)]
pub struct Script {
    pub commands: Vec<Command>,
    /// `#n` alone on the script's first line, which stands for `-n`.
    pub quiet: bool,
}

impl Script {
    /// The files `w` and `s///w` write, which sed makes anew before it
    /// reads a line.
    #[must_use]
    pub fn written(&self) -> Vec<&str> {
        let mut paths = Vec::new();
        for command in &self.commands {
            let path = match &command.kind {
                Kind::Write(path) => path,
                Kind::Substitute(s) => match &s.write {
                    Some(path) => path,
                    None => continue,
                },
                _ => continue,
            };
            if !paths.contains(&path.as_str()) {
                paths.push(path.as_str());
            }
        }
        paths
    }
}

/// What is wrong with a script, and the character it was found at,
/// counted from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub at: usize,
    pub why: Why,
}

/// The ways a script can be wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Unknown(char),
    NoCommand,
    Addresses,
    Unended,
    Regex(regex::Code),
    Flag,
    NoLabel,
    UnknownLabel,
    Brace,
    Lengths,
    Extra,
    LineZero,
    NoFile,
    Escape,
    Reference,
    Delimiter,
}

impl Why {
    /// What sed says about a script wrong in this way.
    #[must_use]
    pub fn says(self) -> String {
        let said = match self {
            Why::Unknown(c) => return format!("{c} is no command"),
            Why::Regex(code) => code.says(),
            Why::NoCommand => "an address without a command",
            Why::Addresses => "more addresses than the command takes",
            Why::Unended => "an expression or a text without the character that ends it",
            Why::Flag => "a flag s does not have",
            Why::NoLabel => "a : without a label",
            Why::UnknownLabel => "a branch to a label no : sets",
            Why::Brace => "a brace without its other half",
            Why::Lengths => "y's two texts are not as long as each other",
            Why::Extra => "more after a command than it takes",
            Why::LineZero => "lines are counted from 1",
            Why::NoFile => "r and w need the name of a file",
            Why::Escape => "an escape POSIX gives no meaning there",
            Why::Reference => "a reference to a subexpression the expression does not have",
            Why::Delimiter => "a backslash or a line feed cannot end an expression",
        };
        String::from(said)
    }
}

fn fail<T>(at: usize, why: Why) -> Result<T, Error> {
    Err(Error { at, why })
}

/// Reads `text`, the expressions in it as `syntax` says.
///
/// # Errors
///
/// Where the script breaks POSIX's rules for sed, or an expression in it
/// breaks those for expressions.
pub fn parse(text: &str, syntax: Syntax) -> Result<Script, Error> {
    let mut reader = Reader {
        chars: text.chars().collect(),
        at: 0,
        syntax,
    };
    let quiet =
        reader.chars.starts_with(&['#', 'n']) && matches!(reader.chars.get(2), None | Some('\n'));
    let mut commands: Vec<Command> = Vec::new();
    // Each `{` still open: its command and its place.
    let mut open = Vec::new();
    let mut labels = Vec::new();
    // Each `b` and `t`: its command, its label and its place.
    let mut branches = Vec::new();
    loop {
        reader.skip(|c| matches!(c, ' ' | '\t' | '\n' | ';'));
        let at = reader.at;
        let Some(c) = reader.peek() else {
            break;
        };
        if c == '#' {
            reader.skip(|c| c != '\n');
            continue;
        }
        let (first, second, negated) = reader.addresses()?;
        let letter_at = reader.at;
        let Some(letter) = reader.next() else {
            return fail(at, Why::NoCommand);
        };
        let addresses = usize::from(first.is_some()) + usize::from(second.is_some());
        let most = match letter {
            ':' | '}' => 0,
            'q' => 1,
            _ => 2,
        };
        if addresses > most {
            return fail(at, Why::Addresses);
        }
        let kind = match letter {
            '{' => {
                open.push((commands.len(), at));
                Kind::Block(0)
            }
            '}' => {
                let Some((block, _)) = open.pop() else {
                    return fail(letter_at, Why::Brace);
                };
                commands[block].kind = Kind::Block(commands.len());
                Kind::End
            }
            'a' => Kind::Append(reader.text()),
            'i' => Kind::Insert(reader.text()),
            'c' => Kind::Change(reader.text()),
            'b' | 't' => {
                branches.push((commands.len(), reader.label(), letter_at));
                if letter == 'b' {
                    Kind::Branch(0)
                } else {
                    Kind::Test(0)
                }
            }
            ':' => {
                let label = reader.label();
                if label.is_empty() {
                    return fail(letter_at, Why::NoLabel);
                }
                labels.push((label, commands.len()));
                Kind::Label
            }
            'r' => Kind::Read(reader.file()?),
            'w' => Kind::Write(reader.file()?),
            's' => Kind::Substitute(Box::new(reader.substitute()?)),
            'y' => Kind::Translate(reader.translate()?),
            other => plain(other).ok_or(Error {
                at: letter_at,
                why: Why::Unknown(other),
            })?,
        };
        // A block's commands may start on its line.
        if letter != '{' {
            reader.ended()?;
        }
        commands.push(Command {
            first,
            second,
            negated,
            kind,
        });
    }
    if let Some(&(_, at)) = open.last() {
        return fail(at, Why::Brace);
    }
    resolve(&mut commands, &labels, branches)?;
    Ok(Script { commands, quiet })
}

/// The command a letter that takes nothing stands for.
fn plain(letter: char) -> Option<Kind> {
    Some(match letter {
        'd' => Kind::Delete,
        'D' => Kind::DeleteFirst,
        'g' => Kind::Get,
        'G' => Kind::GetAppend,
        'h' => Kind::Hold,
        'H' => Kind::HoldAppend,
        'l' => Kind::List,
        'n' => Kind::Next,
        'N' => Kind::NextAppend,
        'p' => Kind::Print,
        'P' => Kind::PrintFirst,
        'q' => Kind::Quit,
        'x' => Kind::Exchange,
        '=' => Kind::Number,
        _ => return None,
    })
}

/// Sends each of `branches`, its command, label and place, to the command
/// its label is on in `labels`, or to the end for no label.
fn resolve(
    commands: &mut [Command],
    labels: &[(String, usize)],
    branches: Vec<(usize, String, usize)>,
) -> Result<(), Error> {
    for (command, label, at) in branches {
        let target = if label.is_empty() {
            commands.len()
        } else {
            labels
                .iter()
                .find(|(name, _)| *name == label)
                .map(|&(_, index)| index)
                .ok_or(Error {
                    at,
                    why: Why::UnknownLabel,
                })?
        };
        if let Kind::Branch(to) | Kind::Test(to) = &mut commands[command].kind {
            *to = target;
        }
    }
    Ok(())
}

fn blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

struct Reader {
    chars: Vec<char>,
    at: usize,
    syntax: Syntax,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += 1;
        Some(c)
    }

    fn skip(&mut self, over: impl Fn(char) -> bool) {
        while self.peek().is_some_and(&over) {
            self.at += 1;
        }
    }

    /// What may follow a command: blanks, then the end of its line, a `;`,
    /// a `}` or a comment.
    fn ended(&mut self) -> Result<(), Error> {
        self.skip(blank);
        match self.peek() {
            None | Some('\n' | ';' | '}' | '#') => Ok(()),
            Some(_) => fail(self.at, Why::Extra),
        }
    }

    /// The addresses before a command, and whether a `!` takes the lines
    /// they leave out.
    fn addresses(&mut self) -> Result<(Option<Address>, Option<Address>, bool), Error> {
        let first = self.address()?;
        let second = match first {
            Some(_) if self.peek() == Some(',') => {
                self.at += 1;
                self.skip(blank);
                let at = self.at;
                Some(self.address()?.ok_or(Error {
                    at,
                    why: Why::NoCommand,
                })?)
            }
            _ => None,
        };
        self.skip(blank);
        let negated = self.peek() == Some('!');
        if negated {
            self.at += 1;
            self.skip(blank);
        }
        Ok((first, second, negated))
    }

    /// The address at the reader, none when no address starts there.
    fn address(&mut self) -> Result<Option<Address>, Error> {
        let at = self.at;
        let address = match self.peek() {
            Some(c) if c.is_ascii_digit() => {
                let mut line: usize = 0;
                while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
                    line = line.saturating_mul(10).saturating_add(digit as usize);
                    self.at += 1;
                }
                if line == 0 {
                    return fail(at, Why::LineZero);
                }
                Address::Line(line)
            }
            Some('$') => {
                self.at += 1;
                Address::Last
            }
            Some('/') => {
                self.at += 1;
                Address::Match(self.regex('/')?)
            }
            Some('\\') => {
                self.at += 1;
                let delimiter = self.delimiter()?;
                Address::Match(self.regex(delimiter)?)
            }
            _ => return Ok(None),
        };
        Ok(Some(address))
    }

    /// The character that ends the expression or texts that start here.
    fn delimiter(&mut self) -> Result<char, Error> {
        match self.peek() {
            None => fail(self.at, Why::Unended),
            Some('\\' | '\n') => fail(self.at, Why::Delimiter),
            Some(c) => {
                self.at += 1;
                Ok(c)
            }
        }
    }

    /// The expression up to `delimiter`, read past it: none when it is
    /// empty, which stands for the one used last. A delimiter after a
    /// backslash is that character as it is.
    fn regex(&mut self, delimiter: char) -> Result<Option<Regex>, Error> {
        let start = self.at;
        let mut pattern = String::new();
        loop {
            match self.next() {
                None | Some('\n') => return fail(start, Why::Unended),
                Some(c) if c == delimiter => break,
                Some('\\') => match self.next() {
                    None => return fail(start, Why::Unended),
                    Some(c) if c == delimiter => {
                        if special(c, self.syntax) {
                            pattern.push('\\');
                        }
                        pattern.push(c);
                    }
                    Some(c) => {
                        pattern.push('\\');
                        pattern.push(c);
                    }
                },
                Some(c) => pattern.push(c),
            }
        }
        if pattern.is_empty() {
            return Ok(None);
        }
        Regex::new(&pattern, self.syntax, false)
            .map(Some)
            .map_err(|e| Error {
                at: start,
                why: Why::Regex(e.code),
            })
    }

    /// The text `a`, `i` and `c` take: after `\` and a line feed as POSIX
    /// writes it, or on the same line; a backslash takes the character
    /// after it as it is, a line feed too, which goes on to the next line.
    fn text(&mut self) -> String {
        self.skip(blank);
        if self.peek() == Some('\\') {
            self.at += 1;
            if self.peek() == Some('\n') {
                self.at += 1;
            }
        }
        let mut text = String::new();
        while let Some(c) = self.peek().filter(|&c| c != '\n') {
            self.at += 1;
            if c != '\\' {
                text.push(c);
            } else if let Some(after) = self.next() {
                text.push(after);
            }
        }
        text
    }

    /// The label after `b`, `t` or `:`: up to the end of the line, a `;`
    /// or a `}`, without blanks at either end.
    fn label(&mut self) -> String {
        self.skip(blank);
        let start = self.at;
        self.skip(|c| !matches!(c, '\n' | ';' | '}'));
        let label: String = self.chars[start..self.at].iter().collect();
        String::from(label.trim_end_matches(blank))
    }

    /// The file `r` and `w` name: the rest of the line.
    fn file(&mut self) -> Result<String, Error> {
        self.skip(blank);
        let start = self.at;
        self.skip(|c| c != '\n');
        if self.at == start {
            return fail(start, Why::NoFile);
        }
        Ok(self.chars[start..self.at].iter().collect())
    }

    /// `s`: its expression, its replacement and its flags.
    fn substitute(&mut self) -> Result<Substitute, Error> {
        let delimiter = self.delimiter()?;
        let regex = self.regex(delimiter)?;
        let groups = regex.as_ref().map(Regex::groups);
        let replacement = self.replacement(delimiter, groups)?;
        let mut substitute = Substitute {
            regex,
            replacement,
            every: false,
            nth: 1,
            print: false,
            write: None,
        };
        loop {
            let at = self.at;
            match self.peek() {
                Some('g') => substitute.every = true,
                Some('p') => substitute.print = true,
                Some('w') => {
                    self.at += 1;
                    substitute.write = Some(self.file()?);
                    return Ok(substitute);
                }
                Some(c) if c.is_ascii_digit() => {
                    let mut nth: usize = 0;
                    while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
                        nth = nth.saturating_mul(10).saturating_add(digit as usize);
                        self.at += 1;
                    }
                    if nth == 0 {
                        return fail(at, Why::Flag);
                    }
                    substitute.nth = nth;
                    continue;
                }
                Some(c) if c.is_alphanumeric() => return fail(at, Why::Flag),
                _ => return Ok(substitute),
            }
            self.at += 1;
        }
    }

    /// The replacement up to `delimiter`, read past it, for an expression
    /// with `groups` subexpressions, unknown for the one used last.
    fn replacement(&mut self, delimiter: char, groups: Option<usize>) -> Result<Vec<Piece>, Error> {
        let start = self.at;
        let mut pieces = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, pieces: &mut Vec<Piece>| {
            if !text.is_empty() {
                pieces.push(Piece::Text(core::mem::take(text)));
            }
        };
        loop {
            let at = self.at;
            match self.next() {
                None | Some('\n') => return fail(start, Why::Unended),
                Some(c) if c == delimiter => break,
                Some('&') => {
                    flush(&mut text, &mut pieces);
                    pieces.push(Piece::Whole);
                }
                Some('\\') => match self.next() {
                    None => return fail(start, Why::Unended),
                    Some(c) if c == delimiter => text.push(c),
                    Some(digit @ '1'..='9') => {
                        let number = digit as usize - '0' as usize;
                        if groups.is_some_and(|groups| number > groups) {
                            return fail(at, Why::Reference);
                        }
                        flush(&mut text, &mut pieces);
                        pieces.push(Piece::Group(number));
                    }
                    Some('n') => text.push('\n'),
                    // Other tools give these meanings POSIX does not.
                    Some(c) if c.is_alphanumeric() => return fail(at, Why::Escape),
                    Some(c) => text.push(c),
                },
                Some(c) => text.push(c),
            }
        }
        flush(&mut text, &mut pieces);
        Ok(pieces)
    }

    /// `y`: its two texts, as pairs of characters.
    fn translate(&mut self) -> Result<Vec<(char, char)>, Error> {
        let start = self.at;
        let delimiter = self.delimiter()?;
        let from = self.characters(delimiter)?;
        let to = self.characters(delimiter)?;
        if from.len() != to.len() {
            return fail(start, Why::Lengths);
        }
        Ok(from.into_iter().zip(to).collect())
    }

    /// A text of `y` up to `delimiter`, read past it: `\n` a line feed and
    /// `\\` and the delimiter after a backslash themselves.
    fn characters(&mut self, delimiter: char) -> Result<Vec<char>, Error> {
        let start = self.at;
        let mut characters = Vec::new();
        loop {
            let at = self.at;
            match self.next() {
                None | Some('\n') => return fail(start, Why::Unended),
                Some(c) if c == delimiter => return Ok(characters),
                Some('\\') => match self.next() {
                    Some('n') => characters.push('\n'),
                    Some(c) if c == '\\' || c == delimiter => characters.push(c),
                    _ => return fail(at, Why::Escape),
                },
                Some(c) => characters.push(c),
            }
        }
    }
}

/// Whether `c` means more than itself in an expression of `syntax`, so a
/// delimiter that is `c` keeps its backslash to stay itself.
fn special(c: char, syntax: Syntax) -> bool {
    match syntax {
        Syntax::Basic => matches!(c, '.' | '[' | '*' | '^' | '$'),
        Syntax::Extended => matches!(
            c,
            '.' | '[' | '(' | ')' | '*' | '+' | '?' | '{' | '|' | '^' | '$'
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::{Span, Text};

    fn read(text: &str) -> Script {
        parse(text, Syntax::Basic).unwrap()
    }

    fn failed(text: &str) -> (usize, Why) {
        let e = parse(text, Syntax::Basic).unwrap_err();
        (e.at, e.why)
    }

    /// The letter of each command of `text`.
    fn letters(text: &str) -> String {
        read(text)
            .commands
            .iter()
            .map(|command| match &command.kind {
                Kind::Block(_) => '{',
                Kind::End => '}',
                Kind::Append(_) => 'a',
                Kind::Branch(_) => 'b',
                Kind::Change(_) => 'c',
                Kind::Delete => 'd',
                Kind::DeleteFirst => 'D',
                Kind::Get => 'g',
                Kind::GetAppend => 'G',
                Kind::Hold => 'h',
                Kind::HoldAppend => 'H',
                Kind::Insert(_) => 'i',
                Kind::List => 'l',
                Kind::Next => 'n',
                Kind::NextAppend => 'N',
                Kind::Print => 'p',
                Kind::PrintFirst => 'P',
                Kind::Quit => 'q',
                Kind::Read(_) => 'r',
                Kind::Substitute(_) => 's',
                Kind::Test(_) => 't',
                Kind::Write(_) => 'w',
                Kind::Exchange => 'x',
                Kind::Translate(_) => 'y',
                Kind::Label => ':',
                Kind::Number => '=',
            })
            .collect()
    }

    fn only(text: &str) -> Command {
        let mut script = read(text);
        assert_eq!(script.commands.len(), 1, "{text}");
        script.commands.remove(0)
    }

    fn substitute(text: &str) -> Substitute {
        match only(text).kind {
            Kind::Substitute(s) => *s,
            other => panic!("{other:?}"),
        }
    }

    /// Where `regex` matches `text` first.
    fn matched(regex: &Regex, text: &str) -> Option<Span> {
        regex.find(&Text::new(text.as_bytes()), 0).unwrap()
    }

    #[test]
    fn every_command_reads_as_its_kind() {
        assert_eq!(
            letters(
                "{\n}\na x\nb\nc x\nd\nD\ng\nG\nh\nH\ni x\nl\nn\nN\np\nP\nq\nr f\ns/a/b/\nt\nw f\nx\ny/a/b/\n:l\n="
            ),
            "{}abcdDgGhHilnNpPqrstwxy:="
        );
    }

    #[test]
    fn commands_part_at_line_feeds_and_semicolons() {
        assert_eq!(letters("p;d\nx"), "pdx");
        assert_eq!(letters(" ; p ;; "), "p");
        assert_eq!(letters("p # what it does\nd"), "pd");
        assert_eq!(letters("# nothing but this"), "");
        assert_eq!(letters(""), "");
        assert_eq!(letters("\n\n\tp\t\n"), "p");
        assert_eq!(letters("{p;d};x"), "{pd}x");
    }

    #[test]
    fn hash_n_alone_on_the_first_line_is_quiet() {
        assert!(read("#n\np").quiet);
        assert!(read("#n").quiet);
        assert!(!read("#np").quiet, "a comment that goes on");
        assert_eq!(letters("#np"), "");
        assert!(!read(" #n\np").quiet, "not at the very start");
        assert!(!read("p\n#n").quiet);
        assert!(!read("#N\np").quiet);
    }

    #[test]
    fn addresses_name_lines_by_number_last_or_expression() {
        assert!(matches!(only("5p").first, Some(Address::Line(5))));
        assert!(matches!(only("123p").first, Some(Address::Line(123))));
        assert!(matches!(only("$p").first, Some(Address::Last)));
        assert!(only("p").first.is_none());
        let Some(Address::Match(Some(regex))) = only("/x.z/p").first else {
            panic!("an expression");
        };
        assert_eq!(matched(&regex, "axyz"), Some(Span { start: 1, end: 4 }));
        let Some(Address::Match(Some(regex))) = only("\\%a%p").first else {
            panic!("an expression with its own delimiter");
        };
        assert!(matched(&regex, "ba").is_some());
        assert!(matches!(only("//p").first, Some(Address::Match(None))));
        let range = only("1,3p");
        assert!(matches!(
            (range.first, range.second),
            (Some(Address::Line(1)), Some(Address::Line(3)))
        ));
        let range = only("/a/,$p");
        assert!(matches!(
            (range.first, range.second),
            (Some(Address::Match(Some(_))), Some(Address::Last))
        ));
        assert!(only("2!p").negated);
        assert!(!only("2p").negated);
        let spaced = only("1,$ ! p");
        assert!(spaced.negated && matches!(spaced.second, Some(Address::Last)));
        assert!(only("!p").negated, "! without an address");
    }

    #[test]
    fn an_address_read_wrong_says_where() {
        assert_eq!(failed("0p"), (0, Why::LineZero));
        assert_eq!(failed("1,0p"), (2, Why::LineZero));
        assert_eq!(failed("1,p"), (2, Why::NoCommand));
        assert_eq!(failed("5"), (0, Why::NoCommand));
        assert_eq!(failed("/a"), (1, Why::Unended));
        assert_eq!(failed("/a\n/p"), (1, Why::Unended));
        assert_eq!(failed("\\"), (1, Why::Unended));
        assert_eq!(failed("\\\\a\\p"), (1, Why::Delimiter));
        assert_eq!(failed("/a\\{2/p"), (1, Why::Regex(regex::Code::Interval)));
        assert_eq!(failed("1k"), (1, Why::Unknown('k')));
    }

    #[test]
    fn a_command_takes_only_as_many_addresses_as_it_can() {
        assert_eq!(failed("1:a"), (0, Why::Addresses));
        assert_eq!(failed("1,2q"), (0, Why::Addresses));
        assert_eq!(failed("{p};1}"), (4, Why::Addresses));
        assert_eq!(letters("1q"), "q");
        assert_eq!(letters("1,2{p}"), "{p}");
        assert_eq!(letters("/a/,/b/a x"), "a");
    }

    #[test]
    fn text_comes_after_a_backslash_and_a_line_feed_or_on_the_same_line() {
        let text = |script| match only(script).kind {
            Kind::Append(text) | Kind::Insert(text) | Kind::Change(text) => text,
            other => panic!("{other:?}"),
        };
        assert_eq!(text("a hello"), "hello");
        assert_eq!(text("a    hello"), "hello", "blanks before it go");
        assert_eq!(text("a\\\nhello"), "hello");
        assert_eq!(text("a\\\n  two"), "  two", "after the line feed they stay");
        assert_eq!(text("a\\hello"), "hello");
        assert_eq!(text("a one\\\ntwo"), "one\ntwo");
        assert_eq!(
            text("i x\\y\\\\z"),
            "xy\\z",
            "a backslash takes what follows as it is"
        );
        assert_eq!(text("c\\\n"), "");
        assert_eq!(text("c"), "");
        assert_eq!(letters("a x\np"), "ap", "the text ends with its line");
        assert_eq!(letters("i x;p"), "i", "a ; is part of it");
    }

    #[test]
    fn a_branch_goes_to_its_label_or_the_end() {
        let script = read(":top\np\nb top");
        assert!(matches!(script.commands[2].kind, Kind::Branch(0)));
        let script = read("b\np");
        assert!(
            matches!(script.commands[0].kind, Kind::Branch(2)),
            "the end"
        );
        let script = read("t on\np\n:on\nd");
        assert!(matches!(script.commands[0].kind, Kind::Test(2)), "forward");
        let script = read(":a;p;b a");
        assert!(
            matches!(script.commands[2].kind, Kind::Branch(0)),
            "; ends a label"
        );
        let script = read(":a b  \nb a b");
        assert!(
            matches!(script.commands[1].kind, Kind::Branch(0)),
            "blanks inside stay"
        );
        assert_eq!(failed("p\nb nope"), (2, Why::UnknownLabel));
        assert_eq!(failed("t x"), (0, Why::UnknownLabel));
        assert_eq!(failed(":"), (0, Why::NoLabel));
        assert_eq!(failed("p; :  \n"), (3, Why::NoLabel));
    }

    #[test]
    fn braces_hold_the_commands_between_them() {
        let script = read("1{p;p}");
        assert!(matches!(script.commands[0].kind, Kind::Block(3)));
        assert!(matches!(script.commands[3].kind, Kind::End));
        let nested = read("{{p}\nd}");
        assert!(matches!(nested.commands[0].kind, Kind::Block(5)));
        assert!(matches!(nested.commands[1].kind, Kind::Block(3)));
        assert_eq!(letters("/x/!{\np\n}"), "{p}");
        assert_eq!(failed("{p"), (0, Why::Brace));
        assert_eq!(failed("p}"), (1, Why::Brace));
        assert_eq!(failed("{p}}"), (3, Why::Brace));
        assert_eq!(failed("p\n{\n{p}"), (2, Why::Brace), "the one left open");
    }

    #[test]
    fn r_and_w_name_a_file_with_the_rest_of_the_line() {
        assert!(matches!(only("r /a/b c").kind, Kind::Read(path) if path == "/a/b c"));
        assert!(matches!(only("w out;p").kind, Kind::Write(path) if path == "out;p"));
        assert_eq!(letters("w out\np"), "wp");
        assert_eq!(failed("r"), (1, Why::NoFile));
        assert_eq!(failed("w   \np"), (4, Why::NoFile));
    }

    #[test]
    fn s_reads_its_expression_replacement_and_flags() {
        let plain = substitute("s/a/b/");
        assert_eq!(
            (
                plain.replacement,
                plain.every,
                plain.nth,
                plain.print,
                plain.write
            ),
            (Vec::from([Piece::Text("b".into())]), false, 1, false, None)
        );
        assert!(plain.regex.is_some());
        assert!(substitute("s/a/b/g").every);
        assert_eq!(substitute("s/a/b/3").nth, 3);
        let both = substitute("s/a/b/2g");
        assert_eq!((both.nth, both.every), (2, true));
        assert!(substitute("s/a/b/p").print);
        assert_eq!(substitute("s/a/b/w f").write.as_deref(), Some("f"));
        let all = substitute("s/a/b/gpw out file");
        assert_eq!(
            (all.every, all.print, all.write.as_deref()),
            (true, true, Some("out file"))
        );
        assert_eq!(substitute("s/a//").replacement, []);
        assert!(
            substitute("s//x/").regex.is_none(),
            "the expression used last"
        );
        assert_eq!(letters("s/a/b/ ; p"), "sp");
        assert_eq!(letters("s/a/b/g}"[..7].as_ref()), "s");
    }

    #[test]
    fn s_read_wrong_says_where() {
        assert_eq!(failed("s/a/b/0"), (6, Why::Flag));
        assert_eq!(failed("s/a/b/x"), (6, Why::Flag));
        assert_eq!(failed("s/a/b/g x"), (8, Why::Extra));
        assert_eq!(failed("s/a/b"), (4, Why::Unended));
        assert_eq!(failed("s/a"), (2, Why::Unended));
        assert_eq!(failed("s"), (1, Why::Unended));
        assert_eq!(failed("s/a\nb/c/"), (2, Why::Unended));
        assert_eq!(
            failed("s/a/x\ny/"),
            (4, Why::Unended),
            "a line feed needs its backslash"
        );
        assert_eq!(failed("s\\a\\b\\"), (1, Why::Delimiter));
        assert_eq!(failed("s/[a/b/"), (2, Why::Regex(regex::Code::OpenBracket)));
        assert_eq!(failed("s/a/\\1/"), (4, Why::Reference));
        assert_eq!(failed("s/\\(a\\)/\\2/"), (8, Why::Reference));
    }

    #[test]
    fn a_replacement_holds_texts_the_match_and_subexpressions() {
        assert_eq!(
            substitute("s/\\(a\\)\\(b\\)/[\\2&\\1]/").replacement,
            [
                Piece::Text("[".into()),
                Piece::Group(2),
                Piece::Whole,
                Piece::Group(1),
                Piece::Text("]".into())
            ]
        );
        assert_eq!(
            substitute("s/a/\\&\\\\/").replacement,
            [Piece::Text("&\\".into())]
        );
        assert_eq!(
            substitute("s/a/x\\ny/").replacement,
            [Piece::Text("x\ny".into())]
        );
        assert_eq!(
            substitute("s/a/x\\\ny/").replacement,
            [Piece::Text("x\ny".into())],
            "a backslash and a line feed"
        );
        assert_eq!(
            substitute("s|a|x\\|y|").replacement,
            [Piece::Text("x|y".into())]
        );
        assert_eq!(
            substitute("s//\\7/").replacement,
            [Piece::Group(7)],
            "unknown yet"
        );
        assert_eq!(
            substitute("s/a/\\%\\-/").replacement,
            [Piece::Text("%-".into())]
        );
        assert_eq!(
            failed("s/a/\\t/"),
            (4, Why::Escape),
            "a tab is written as itself"
        );
        assert_eq!(failed("s/a/\\0/"), (4, Why::Escape));
    }

    #[test]
    fn a_delimiter_after_a_backslash_is_itself_in_the_expression() {
        let regex = |text, syntax| {
            let script = parse(text, syntax).unwrap();
            match &script.commands[0].kind {
                Kind::Substitute(s) => s.regex.as_ref().map(|r| matched(r, "a|b a.b axb")),
                other => panic!("{other:?}"),
            }
        };
        let span = |start, end| Some(Some(Span { start, end }));
        assert_eq!(
            regex("s|a\\|b|X|", Syntax::Basic),
            span(0, 3),
            "| is plain in basic ones"
        );
        assert_eq!(
            regex("s|a\\|b|X|", Syntax::Extended),
            span(0, 3),
            "and kept as \\| in extended ones"
        );
        assert_eq!(
            regex("s.a\\.b.X.", Syntax::Basic),
            span(4, 7),
            "a dot that stays a dot"
        );
        assert_eq!(
            regex("s/a.b/X/", Syntax::Basic),
            span(0, 3),
            "a dot that is any character"
        );
        assert_eq!(regex("sxa\\xbxXx", Syntax::Basic), span(8, 11));
        assert_eq!(regex("s/a\\/b/X/", Syntax::Basic), Some(None));
    }

    #[test]
    fn y_pairs_characters_of_texts_as_long_as_each_other() {
        assert!(
            matches!(only("y/abc/xyz/").kind, Kind::Translate(pairs) if pairs == [('a', 'x'), ('b', 'y'), ('c', 'z')])
        );
        assert!(
            matches!(only("y/a\\nb/\\\\x\\n/").kind, Kind::Translate(pairs) if pairs == [('a', '\\'), ('\n', 'x'), ('b', '\n')])
        );
        assert!(
            matches!(only("y,a\\,b,x\\,y,").kind, Kind::Translate(pairs) if pairs == [('a', 'x'), (',', ','), ('b', 'y')])
        );
        assert!(
            matches!(only("y/äö/ÄÖ/").kind, Kind::Translate(pairs) if pairs == [('ä', 'Ä'), ('ö', 'Ö')])
        );
        assert!(matches!(only("y///").kind, Kind::Translate(pairs) if pairs.is_empty()));
        assert_eq!(failed("y/ab/x/"), (1, Why::Lengths));
        assert_eq!(failed("y/a\\tb/xyz/"), (3, Why::Escape));
        assert_eq!(failed("y/a/b"), (4, Why::Unended));
        assert_eq!(failed("y/a\n/b/"), (2, Why::Unended));
    }

    #[test]
    fn extended_expressions_are_read_as_extended() {
        let script = parse("/a+/p\ns/(a)(b)/\\2\\1/", Syntax::Extended).unwrap();
        let Some(Address::Match(Some(regex))) = &script.commands[0].first else {
            panic!("an expression");
        };
        assert_eq!(matched(regex, "baa"), Some(Span { start: 1, end: 3 }));
        assert!(
            matches!(&script.commands[1].kind, Kind::Substitute(s) if s.replacement == [Piece::Group(2), Piece::Group(1)])
        );
        let basic = parse("/a+/p", Syntax::Basic).unwrap();
        let Some(Address::Match(Some(regex))) = &basic.commands[0].first else {
            panic!("an expression");
        };
        assert_eq!(
            matched(regex, "baa"),
            None,
            "+ is a character in a basic one"
        );
        assert_eq!(
            parse("/(/p", Syntax::Extended).unwrap_err(),
            Error {
                at: 1,
                why: Why::Regex(regex::Code::Parenthesis)
            }
        );
    }

    #[test]
    fn only_blanks_and_an_end_may_follow_a_command() {
        assert_eq!(failed("p x"), (2, Why::Extra));
        assert_eq!(failed("q 5"), (2, Why::Extra));
        assert_eq!(failed("dp"), (1, Why::Extra));
        assert_eq!(failed("y/a/b/c"), (6, Why::Extra));
        assert_eq!(letters("p  # yes"), "p");
        assert_eq!(letters("p\t;\td"), "pd");
        assert_eq!(failed("k"), (0, Why::Unknown('k')));
        assert_eq!(failed("1,2 !\tz"), (6, Why::Unknown('z')));
    }

    #[test]
    fn the_files_w_writes_are_named_once_each() {
        assert_eq!(read("w a\ns/x/y/w b\nw a\np").written(), ["a", "b"]);
        assert_eq!(read("p;s/x/y/").written(), Vec::<&str>::new());
    }

    #[test]
    fn every_trouble_says_something_of_its_own() {
        let whys = [
            Why::Unknown('k'),
            Why::NoCommand,
            Why::Addresses,
            Why::Unended,
            Why::Regex(regex::Code::Range),
            Why::Flag,
            Why::NoLabel,
            Why::UnknownLabel,
            Why::Brace,
            Why::Lengths,
            Why::Extra,
            Why::LineZero,
            Why::NoFile,
            Why::Escape,
            Why::Reference,
            Why::Delimiter,
        ];
        for (i, a) in whys.iter().enumerate() {
            for b in &whys[i + 1..] {
                assert_ne!(a.says(), b.says());
            }
        }
        assert_eq!(Why::Unknown('k').says(), "k is no command");
        assert_eq!(
            Why::Regex(regex::Code::Range).says(),
            regex::Code::Range.says()
        );
    }

    #[test]
    fn a_place_counts_characters_across_lines() {
        assert_eq!(failed("p\n\n  k"), (5, Why::Unknown('k')));
        assert_eq!(failed("p\ns/a"), (4, Why::Unended));
        assert_eq!(failed("1,2p\n3,4:x"), (5, Why::Addresses));
        assert_eq!(failed("ä"), (0, Why::Unknown('ä')));
        assert_eq!(failed("p;€"), (2, Why::Unknown('€')));
        assert_eq!(
            failed("s/ä/ö/q"),
            (6, Why::Flag),
            "counted in characters, not bytes"
        );
    }

    #[test]
    fn an_address_may_take_any_delimiter_and_escape_it() {
        let first = |text| match only(text).first {
            Some(Address::Match(regex)) => regex,
            other => panic!("{other:?}"),
        };
        let regex = first("\\,a\\,b,p").unwrap();
        assert!(matched(&regex, "a,b").is_some());
        assert!(matched(&regex, "ab").is_none());
        let regex = first("/a\\/b/p").unwrap();
        assert!(matched(&regex, "xa/b").is_some());
        let regex = first("\\|a|p").unwrap();
        assert!(matched(&regex, "a").is_some());
        assert!(first("\\%%p").is_none(), "empty, so the one used last");
        let regex = first("/a\\nb/p").unwrap();
        assert!(matched(&regex, "a\nb").is_some(), "\\n is a line feed");
        assert!(matches!(
            only("99999999999999999999999p").first,
            Some(Address::Line(usize::MAX))
        ));
    }

    #[test]
    fn a_label_lives_inside_a_block_and_may_be_any_word() {
        let script = read("{:a;N;ba}");
        assert!(matches!(script.commands[3].kind, Kind::Branch(1)));
        assert!(
            matches!(script.commands[4].kind, Kind::End),
            "the brace ends the label"
        );
        let script = read(":1\nb1");
        assert!(matches!(script.commands[1].kind, Kind::Branch(0)));
        let script = read(":x-y.z\nt x-y.z");
        assert!(matches!(script.commands[1].kind, Kind::Test(0)));
        let script = read(":a\n:a\nb a");
        assert!(
            matches!(script.commands[2].kind, Kind::Branch(0)),
            "the first of two"
        );
        let script = read("b end\n:end");
        assert!(matches!(script.commands[0].kind, Kind::Branch(1)));
    }

    #[test]
    fn blanks_may_stand_after_a_comma_and_around_a_bang_only() {
        assert!(matches!(only("1,  3p").second, Some(Address::Line(3))));
        assert!(only("1,3 !  p").negated);
        assert_eq!(failed("1 ,3p"), (2, Why::Unknown(',')));
        assert_eq!(letters("  \t1p"), "p");
    }

    #[test]
    fn a_text_holds_braces_and_semicolons_as_they_are() {
        let script = read("1{a x};y\n}");
        assert!(matches!(&script.commands[1].kind, Kind::Append(text) if text == "x};y"));
        assert!(matches!(script.commands[2].kind, Kind::End));
        let script = read("1{s/a/b/}");
        assert!(
            matches!(script.commands[2].kind, Kind::End),
            "a brace right after flags"
        );
        let script = read("w out  \np");
        assert!(matches!(&script.commands[0].kind, Kind::Write(path) if path == "out  "));
    }

    #[test]
    fn flags_may_come_twice_and_counts_have_digits() {
        assert_eq!(substitute("s/a/b/12").nth, 12);
        assert!(substitute("s/a/b/pp").print);
        assert!(substitute("s/a/b/gg").every);
        let script = read("s/a/b/w f\np");
        assert_eq!(letters("s/a/b/w f\np"), "sp");
        assert_eq!(script.written(), ["f"]);
        assert_eq!(failed("s/a/b/1x"), (7, Why::Flag));
        assert_eq!(letters("s/a/b/#note"), "s");
    }

    #[test]
    fn a_delimiter_special_in_one_syntax_stays_itself_in_both() {
        let regex = |text, syntax| {
            let script = parse(text, syntax).unwrap();
            let Kind::Substitute(s) = &script.commands[0].kind else {
                panic!("s");
            };
            let regex = s.regex.as_ref().unwrap();
            (
                matched(regex, "a+b").is_some(),
                matched(regex, "aab").is_some(),
            )
        };
        assert_eq!(regex("s+a\\+b+X+", Syntax::Basic), (true, false));
        assert_eq!(regex("s+a\\+b+X+", Syntax::Extended), (true, false));
        assert_eq!(regex("s/a+b/X/", Syntax::Extended), (false, true));
        assert!(!regex("s(a\\(b(X(", Syntax::Extended).0);
        assert_eq!(
            regex("s*a\\*b*X*", Syntax::Basic),
            (false, false),
            "a star that stays a star matches neither"
        );
    }

    #[test]
    fn an_address_on_a_closing_brace_is_refused() {
        assert_eq!(failed("1{p;2}"), (4, Why::Addresses));
        assert_eq!(failed("{p;$}"), (3, Why::Addresses));
        assert_eq!(letters("{p;}"), "{p}");
    }

    #[test]
    fn y_counts_characters_not_bytes() {
        assert!(matches!(only("y/ä/a/").kind, Kind::Translate(pairs) if pairs == [('ä', 'a')]));
        assert_eq!(failed("y/ä/ab/"), (1, Why::Lengths));
        assert!(matches!(only("y/a/\\//").kind, Kind::Translate(pairs) if pairs == [('a', '/')]));
    }
}
