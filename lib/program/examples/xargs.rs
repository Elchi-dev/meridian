//! xargs: runs a program with the words of its standard input after the
//! arguments it is given, as many as fit on one command, and again for
//! the rest, as POSIX's xargs does; `echo` when it is given no program.
//! Words are parted by blanks and line feeds, and quotes and backslashes
//! hold them together. `-n` sets how many words a command takes at most,
//! `-L` how many lines, a line that ends in a blank going on into the
//! next, and `-s` how many bytes, each word and the program's name with
//! one more; `-x` ends xargs when a command cannot take what `-n` or
//! `-L` asks. `-I` runs the program once for each line, the whole line
//! put where its arguments hold the text `-I` names. `-E` names a word
//! that ends the input, `-t` writes each command on the standard error
//! before it runs, and `-p` asks at the console whether to run it. xargs
//! ends with 123 when a program ended with anything else than 0, stops
//! with 124 at once when one ended with 255 and with 125 when one was
//! stopped, 126 when one could not start and 127 when it was not there.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use program::start::{Given, Sink};
use program::{Ended, Error, Handle, abi, text};

program::main!(xargs);

/// Where programs named without a path are.
const PROGRAMS: &str = "/System/Boot/meridian/programs";

/// How many bytes a command may take unless `-s` says otherwise.
const SIZE: usize = 16384;

/// How the words of the input make up commands.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// At most so many words a command, or as many as fit.
    Words(Option<usize>),
    /// The words of so many lines a command.
    Lines(usize),
    /// A command for each line, put where the arguments hold this text.
    Insert(String),
}

/// What xargs's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked {
    mode: Mode,
    size: usize,
    exact: bool,
    end: Option<String>,
    trace: bool,
    prompt: bool,
    /// The program and the arguments it is given before the words.
    command: Vec<String>,
}

/// A count of at least 1 as `-n`, `-L` and `-s` take it.
fn count(letter: char, value: &str) -> Result<usize, String> {
    Some(value)
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("xargs: -{letter} wants a number from 1, not {value}\n"))
}

/// What xargs's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked, String> {
    let mut asked = Asked {
        mode: Mode::Words(None),
        size: SIZE,
        exact: false,
        end: None,
        trace: false,
        prompt: false,
        command: Vec::new(),
    };
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        words = rest;
        for (at, letter) in letters.char_indices() {
            match letter {
                't' => asked.trace = true,
                'p' => asked.prompt = true,
                'x' => asked.exact = true,
                'n' | 'L' | 's' | 'I' | 'E' => {
                    let after = &letters[at + 1..];
                    let value = if after.is_empty() {
                        let (value, rest) = words
                            .split_first()
                            .ok_or_else(|| format!("xargs: -{letter} wants its word\n"))?;
                        words = rest;
                        value.as_str()
                    } else {
                        after
                    };
                    match letter {
                        'n' => asked.mode = Mode::Words(Some(count('n', value)?)),
                        'L' => asked.mode = Mode::Lines(count('L', value)?),
                        's' => asked.size = count('s', value)?,
                        'I' if value.is_empty() => {
                            return Err(String::from(
                                "xargs: -I wants the text to put the line in for\n",
                            ));
                        }
                        'I' => {
                            asked.mode = Mode::Insert(String::from(value));
                            asked.exact = true;
                        }
                        _ => asked.end = Some(String::from(value)),
                    }
                    break;
                }
                _ => return Err(format!("xargs: -{letter} is no option\n")),
            }
        }
    }
    asked.command = if words.is_empty() {
        Vec::from([String::from("echo")])
    } else {
        words.to_vec()
    };
    Ok(asked)
}

/// What the input says, a piece at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Read {
    /// A whole word.
    Word(Vec<u8>),
    /// The end of a line that had words, unless a blank at its end ran it
    /// on into the next.
    LineEnd,
}

/// Cuts the input into words as it comes: parted by blanks and line
/// feeds, held together by quotes, which a line feed may not be inside,
/// and by a backslash before any character. Taking whole lines, blanks
/// a line starts with are left out and the others belong to its word.
#[derive(Default)]
struct Words {
    whole_lines: bool,
    word: Vec<u8>,
    in_word: bool,
    quote: Option<u8>,
    escaped: bool,
    line: Line,
}

/// Where the input is in its line: whether a word came on it, and
/// whether the last byte was a blank outside any word.
#[derive(Default)]
struct Line {
    words: bool,
    blank_last: bool,
}

impl Words {
    fn new(whole_lines: bool) -> Words {
        Words {
            whole_lines,
            ..Words::default()
        }
    }

    /// Hands out the word being read, if one began.
    fn end_word(&mut self, out: &mut impl FnMut(Read)) {
        if self.in_word {
            out(Read::Word(core::mem::take(&mut self.word)));
            self.in_word = false;
            self.line.words = true;
        }
    }

    /// Takes the next `bytes` of the input, and hands out what they make.
    ///
    /// # Errors
    ///
    /// When a line feed comes inside quotes.
    fn feed(&mut self, bytes: &[u8], out: &mut impl FnMut(Read)) -> Result<(), String> {
        for &byte in bytes {
            if self.escaped {
                self.word.push(byte);
                self.escaped = false;
                self.line.blank_last = false;
                continue;
            }
            if let Some(quote) = self.quote {
                match byte {
                    b'\n' => return Err(unmatched(quote)),
                    _ if byte == quote => self.quote = None,
                    _ => self.word.push(byte),
                }
                continue;
            }
            match byte {
                b'\n' => {
                    self.end_word(out);
                    if self.line.words && (self.whole_lines || !self.line.blank_last) {
                        out(Read::LineEnd);
                        self.line.words = false;
                    }
                }
                b' ' | b'\t' if self.whole_lines => {
                    if self.in_word {
                        self.word.push(byte);
                    }
                }
                b' ' | b'\t' => self.end_word(out),
                b'\\' => {
                    self.escaped = true;
                    self.in_word = true;
                }
                b'\'' | b'"' => {
                    self.quote = Some(byte);
                    self.in_word = true;
                }
                _ => {
                    self.word.push(byte);
                    self.in_word = true;
                }
            }
            self.line.blank_last = matches!(byte, b' ' | b'\t');
        }
        Ok(())
    }

    /// Ends the input, handing out the word and the line it was in.
    ///
    /// # Errors
    ///
    /// When it ends inside quotes.
    fn finish(mut self, out: &mut impl FnMut(Read)) -> Result<(), String> {
        if let Some(quote) = self.quote {
            return Err(unmatched(quote));
        }
        self.end_word(out);
        if self.line.words {
            out(Read::LineEnd);
        }
        Ok(())
    }
}

/// What xargs says of a quote without its other half.
fn unmatched(quote: u8) -> String {
    let name = if quote == b'\'' { "single" } else { "double" };
    format!("xargs: a {name} quote without its other half\n")
}

/// Why a command could not be made.
#[derive(Debug, PartialEq, Eq)]
enum TooLong {
    /// A word, or the command alone, is longer than `-s` allows.
    Word,
    /// `-x` asked for more than fits.
    Exact,
}

/// Gathers the words into commands, as `asked` says.
struct Gathering<'a> {
    asked: &'a Asked,
    /// The bytes of the program and its arguments, each with one more.
    base: usize,
    words: Vec<String>,
    lines: usize,
    used: usize,
    made: bool,
}

impl<'a> Gathering<'a> {
    fn new(asked: &'a Asked) -> Gathering<'a> {
        let base = asked.command.iter().map(|w| w.len() + 1).sum();
        Gathering {
            asked,
            base,
            words: Vec::new(),
            lines: 0,
            used: base,
            made: false,
        }
    }

    /// The command of the words gathered so far, which start again.
    fn command(&mut self) -> Vec<String> {
        self.made = true;
        self.lines = 0;
        self.used = self.base;
        let mut command = self.asked.command.clone();
        command.append(&mut self.words);
        command
    }

    /// Takes what the input said next, and answers the commands it
    /// completes.
    ///
    /// # Errors
    ///
    /// When a command cannot be made as asked.
    fn take(&mut self, read: Read) -> Result<Vec<Vec<String>>, TooLong> {
        let mut done = Vec::new();
        match (&self.asked.mode, read) {
            (Mode::Insert(text), Read::Word(line)) => {
                let line = String::from_utf8_lossy(&line).into_owned();
                let command: Vec<String> = self
                    .asked
                    .command
                    .iter()
                    .map(|word| word.replace(text.as_str(), &line))
                    .collect();
                if command.iter().map(|w| w.len() + 1).sum::<usize>() > self.asked.size {
                    return Err(TooLong::Word);
                }
                self.made = true;
                done.push(command);
            }
            (mode, Read::Word(word)) => {
                let word = String::from_utf8_lossy(&word).into_owned();
                let adds = word.len() + 1;
                if self.base + adds > self.asked.size {
                    return Err(TooLong::Word);
                }
                if self.used + adds > self.asked.size {
                    if self.asked.exact && !matches!(mode, Mode::Words(None)) {
                        return Err(TooLong::Exact);
                    }
                    done.push(self.command());
                }
                self.words.push(word);
                self.used += adds;
                if matches!(mode, Mode::Words(Some(n)) if self.words.len() == *n) {
                    done.push(self.command());
                }
            }
            (Mode::Lines(n), Read::LineEnd) => {
                self.lines += 1;
                if self.lines == *n {
                    done.push(self.command());
                }
            }
            (Mode::Insert(_) | Mode::Words(_), Read::LineEnd) => {}
        }
        Ok(done)
    }

    /// The last command, of the words left, or of none when the input
    /// gave none at all, unless the lines are put into commands.
    fn finish(mut self) -> Option<Vec<String>> {
        let last =
            !self.words.is_empty() || !self.made && !matches!(self.asked.mode, Mode::Insert(_));
        last.then(|| self.command())
    }
}

/// How one command went.
#[derive(Debug, PartialEq, Eq)]
enum Ran {
    Ended(Ended),
    /// It was not run, as the one asked at the console said.
    Declined,
    /// Its program is not there, or could not start.
    NotThere,
    Unstarted(Error),
}

/// The status xargs ends with after a command went so, and whether it
/// stops there.
fn status(ran: &Ran) -> (u64, bool) {
    match ran {
        Ran::Ended(Ended::Exited(0)) | Ran::Declined => (0, false),
        Ran::Ended(Ended::Exited(255)) => (124, true),
        Ran::Ended(Ended::Exited(_)) => (123, false),
        Ran::Ended(Ended::Faulted(_)) => (125, true),
        Ran::NotThere => (127, true),
        Ran::Unstarted(_) => (126, true),
    }
}

/// What SPAWN is asked for to start the program `name`: the path as it
/// is, else the file of that name among the programs, and where `has`
/// finds no such file, as on a CD, the program of that name the loader
/// brought.
fn program_path(name: &str, has: impl FnOnce(&str) -> bool) -> String {
    if name.contains('/') {
        return name.to_string();
    }
    let path = format!("{PROGRAMS}/{name}.mexec");
    if has(&path) {
        path
    } else {
        format!("{name}.mexec")
    }
}

/// A handle for `sink` of the child's own.
#[cfg_attr(test, allow(dead_code))]
fn handed(sink: Option<Sink>) -> Option<Sink> {
    match sink? {
        Sink::Channel(end) => program::duplicate(end).ok().map(Sink::Channel),
        Sink::File(file) => program::duplicate(file).ok().map(Sink::File),
    }
}

/// Runs `command`: writes it first with `-t` or `-p`, asks with `-p`,
/// and waits for it to end. Its input is one that has ended, its output
/// and errors are xargs's own.
#[cfg_attr(test, allow(dead_code))]
fn run(command: &[String], asked: &Asked) -> Ran {
    if asked.trace || asked.prompt {
        let shown = command.join(" ");
        let asking = if asked.prompt { " ?..." } else { "\n" };
        let _ = program::write_error(&format!("{shown}{asking}"));
        if asked.prompt
            && !program::typed_line().is_some_and(|answer| answer.starts_with(['y', 'Y']))
        {
            return Ran::Declined;
        }
    }
    let Ok((mine, theirs)) = program::channel() else {
        return Ran::Unstarted(Error::Full);
    };
    let _ = program::close(mine);
    let given = Given {
        input: Some(theirs),
        output: handed(program::start::given().output),
        error: handed(program::start::given().error),
        line: None,
    };
    let path = program_path(&command[0], |path| {
        program::open(path, abi::file::READ).is_ok_and(|file| {
            let _ = program::close(file);
            true
        })
    });
    let arguments: Vec<&str> = command[1..].iter().map(String::as_str).collect();
    match program::spawn_handing(&path, &arguments, given) {
        Ok((child, start)) => {
            let _ = program::close(start);
            let ended = program::wait(child);
            let _ = program::close(child);
            ended.map_or_else(Ran::Unstarted, Ran::Ended)
        }
        Err(Error::NotFound | Error::NoSuchProgram) => Ran::NotThere,
        Err(e) => Ran::Unstarted(e),
    }
}

/// A run of xargs: the commands it makes, how the worst of them ended,
/// and whether it stopped before the input ended.
struct Going<'a> {
    asked: &'a Asked,
    gathering: Gathering<'a>,
    worst: u64,
    stopped: Option<String>,
    /// Whether the word -E names came.
    ended: bool,
}

#[cfg_attr(test, allow(dead_code))]
impl<'a> Going<'a> {
    fn new(asked: &'a Asked) -> Going<'a> {
        Going {
            asked,
            gathering: Gathering::new(asked),
            worst: 0,
            stopped: None,
            ended: false,
        }
    }

    /// Whether it reads on.
    fn reading(&self) -> bool {
        !self.ended && self.stopped.is_none()
    }

    /// Takes what the input said, runs the commands it completes, and
    /// says whether to read on.
    fn take(&mut self, reads: Vec<Read>) -> bool {
        for read in reads {
            if !self.reading() {
                break;
            }
            if let (Some(end), Read::Word(word)) = (&self.asked.end, &read)
                && word == end.as_bytes()
            {
                self.ended = true;
                break;
            }
            match self.gathering.take(read) {
                Ok(commands) => {
                    for command in commands {
                        if !self.run(&command) {
                            break;
                        }
                    }
                }
                Err(e) => self.stopped = Some(too_long(&e)),
            }
        }
        self.reading()
    }

    /// Runs `command`, and says whether to go on.
    fn run(&mut self, command: &[String]) -> bool {
        let ran = run(command, self.asked);
        let (status, stop) = status(&ran);
        self.worst = self.worst.max(status);
        if stop {
            self.stopped = Some(match ran {
                Ran::NotThere => format!("xargs: {}: no such program\n", command[0]),
                Ran::Unstarted(e) => text::trouble("xargs", &command[0], e),
                _ => format!("xargs: {} ended so that xargs stops\n", command[0]),
            });
        }
        !stop
    }

    /// Runs the last command, unless it stopped, and answers the status
    /// xargs ends with and why it stopped.
    fn finish(mut self) -> (u64, Option<String>) {
        if self.stopped.is_none()
            && let Some(command) =
                core::mem::replace(&mut self.gathering, Gathering::new(self.asked)).finish()
        {
            self.run(&command);
        }
        (self.worst, self.stopped)
    }
}

#[cfg_attr(test, allow(dead_code))]
fn xargs(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut words = Words::new(matches!(asked.mode, Mode::Insert(_)));
    let mut going = Going::new(&asked);
    // What the input broke on; the words before it still run.
    let mut broken = None;
    let read = text::pieces(None, |piece| {
        let mut reads = Vec::new();
        if let Err(why) = words.feed(piece, &mut |r| reads.push(r)) {
            broken = Some(why);
        }
        going.take(reads) && broken.is_none()
    });
    if broken.is_none() && going.reading() {
        let mut reads = Vec::new();
        match core::mem::take(&mut words).finish(&mut |r| reads.push(r)) {
            Ok(()) => {
                going.take(reads);
            }
            Err(why) => broken = Some(why),
        }
    }
    let (worst, stopped) = going.finish();
    let said = match (read, broken, stopped) {
        (_, _, Some(why)) | (_, Some(why), None) => why,
        (Err(e), None, None) => text::trouble("xargs", "-", e),
        (Ok(()), None, None) => return worst,
    };
    let _ = program::write_error(&said);
    worst.max(1)
}

/// What xargs says when a command could not be made.
fn too_long(why: &TooLong) -> String {
    String::from(match why {
        TooLong::Word => "xargs: a word is longer than -s lets a command be\n",
        TooLong::Exact => "xargs: a command cannot take what -n or -L asks within -s\n",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// What `input`, read in pieces of `piece` bytes, says.
    fn read_in(whole_lines: bool, input: &str, piece: usize) -> Result<Vec<Read>, String> {
        let mut words = Words::new(whole_lines);
        let mut said = Vec::new();
        for piece in input.as_bytes().chunks(piece.max(1)) {
            words.feed(piece, &mut |r| said.push(r))?;
        }
        words.finish(&mut |r| said.push(r))?;
        Ok(said)
    }

    fn word(text: &str) -> Read {
        Read::Word(text.as_bytes().to_vec())
    }

    /// The commands xargs asked with `options` makes of `input`.
    fn commands(options: &str, input: &str) -> Result<Vec<String>, TooLong> {
        let given = args(options);
        let asked = asked(&given).unwrap();
        let mut gathering = Gathering::new(&asked);
        let mut made = Vec::new();
        for read in read_in(matches!(asked.mode, Mode::Insert(_)), input, 3).unwrap() {
            made.extend(gathering.take(read)?);
        }
        made.extend(gathering.finish());
        Ok(made.iter().map(|c| c.join(" ")).collect())
    }

    #[test]
    fn words_are_parted_by_blanks_and_held_by_quotes_and_backslashes() {
        assert_eq!(
            read_in(false, "a 'b c' \"d e\" f\\ g\n", 2),
            Ok(Vec::from([
                word("a"),
                word("b c"),
                word("d e"),
                word("f g"),
                Read::LineEnd
            ]))
        );
        assert_eq!(
            read_in(false, "  a\t\tb  ", 1),
            Ok(Vec::from([word("a"), word("b"), Read::LineEnd]))
        );
        assert_eq!(
            read_in(false, "''\n", 1),
            Ok(Vec::from([word(""), Read::LineEnd])),
            "an empty word"
        );
        assert_eq!(
            read_in(false, "\n\n \n", 1),
            Ok(Vec::new()),
            "lines without words end none"
        );
        assert_eq!(
            read_in(false, "a\\\nb\n", 1),
            Ok(Vec::from([word("a\nb"), Read::LineEnd]))
        );
        assert_eq!(
            read_in(false, "\"it's\"", 1),
            Ok(Vec::from([word("it's"), Read::LineEnd]))
        );
    }

    #[test]
    fn a_line_feed_inside_quotes_or_an_open_quote_at_the_end_is_wrong() {
        assert_eq!(read_in(false, "a 'b\n", 1), Err(unmatched(b'\'')));
        assert_eq!(read_in(false, "a \"b", 1), Err(unmatched(b'"')));
        assert_eq!(
            unmatched(b'\''),
            "xargs: a single quote without its other half\n"
        );
        assert_eq!(
            unmatched(b'"'),
            "xargs: a double quote without its other half\n"
        );
        let mut words = Words::new(false);
        let mut said = Vec::new();
        assert!(words.feed(b"a 'b\nc", &mut |r| said.push(r)).is_err());
        assert_eq!(said, [word("a")], "the words before it came");
    }

    #[test]
    fn a_line_ending_in_a_blank_goes_on_into_the_next() {
        assert_eq!(
            read_in(false, "a b \nc\nd\n", 1),
            Ok(Vec::from([
                word("a"),
                word("b"),
                word("c"),
                Read::LineEnd,
                word("d"),
                Read::LineEnd
            ]))
        );
        assert_eq!(
            read_in(true, "  x y \n z\n", 2),
            Ok(Vec::from([
                word("x y "),
                Read::LineEnd,
                word("z"),
                Read::LineEnd
            ])),
            "whole lines keep their blanks but the first ones, and end where they end"
        );
    }

    #[test]
    fn commands_take_as_many_words_as_asked_or_as_fit() {
        assert_eq!(
            commands("", "a b\nc\n"),
            Ok(Vec::from(["echo a b c".into()]))
        );
        assert_eq!(
            commands("-n 2", "a b\nc\n"),
            Ok(Vec::from(["echo a b".into(), "echo c".into()]))
        );
        assert_eq!(
            commands("-n1 printf %s", "x y"),
            Ok(Vec::from(["printf %s x".into(), "printf %s y".into()]))
        );
        assert_eq!(
            commands("-s 15", "aaaa bbbb cccc\n"),
            Ok(Vec::from(["echo aaaa bbbb".into(), "echo cccc".into()])),
            "each word and the name with one byte more"
        );
        assert_eq!(
            commands("-s 15", "aaaaaaaaaaaaaaaaaaaa\n"),
            Err(TooLong::Word)
        );
        assert_eq!(
            commands("-s 4", ""),
            Ok(Vec::from(["echo".into()])),
            "the program alone may be at the edge"
        );
        assert_eq!(commands("-n 3 -s 10 -x", "a b c d\n"), Err(TooLong::Exact));
        assert_eq!(
            commands("-n 3 -s 10", "a b c d\n"),
            Ok(Vec::from(["echo a b".into(), "echo c d".into()])),
            "without -x what fits"
        );
        assert_eq!(
            commands("-s 9 -x", "a b c\n"),
            Ok(Vec::from(["echo a b".into(), "echo c".into()])),
            "-x alone holds -n or -L"
        );
    }

    #[test]
    fn lines_make_a_command_so_many_at_a_time() {
        assert_eq!(
            commands("-L 1", "a b \nc\nd\n"),
            Ok(Vec::from(["echo a b c".into(), "echo d".into()]))
        );
        assert_eq!(
            commands("-L 2", "a\nb\nc\n"),
            Ok(Vec::from(["echo a b".into(), "echo c".into()]))
        );
        assert_eq!(commands("-L 1 -s 9 -x", "a b c\n"), Err(TooLong::Exact));
        assert_eq!(commands("-L1", ""), Ok(Vec::from(["echo".into()])));
    }

    #[test]
    fn i_puts_each_line_where_the_arguments_ask() {
        assert_eq!(
            commands("-I {} echo [{}] <{}>", "  x y\n z\n"),
            Ok(Vec::from([
                "echo [x y] <x y>".into(),
                "echo [z] <z>".into()
            ]))
        );
        assert_eq!(
            commands("-I % echo %", "a 'b c'\n"),
            Ok(Vec::from(["echo a b c".into()]))
        );
        assert_eq!(
            commands("-I %", "q\n"),
            Ok(Vec::from(["echo".into()])),
            "nowhere to put it"
        );
        assert_eq!(
            commands("-I % echo %", ""),
            Ok(Vec::new()),
            "no line, no command"
        );
        assert_eq!(commands("-I % -s 8 echo %", "abcd\n"), Err(TooLong::Word));
    }

    #[test]
    fn options_and_the_program() {
        let given = args("-tpx -n 3 -s 100 -E end printf %s");
        let asked = asked(&given).unwrap();
        assert!(asked.trace && asked.prompt && asked.exact);
        assert_eq!(
            (asked.mode, asked.size, asked.end.as_deref()),
            (Mode::Words(Some(3)), 100, Some("end"))
        );
        assert_eq!(asked.command, ["printf", "%s"]);
        let given = args("");
        let plain = super::asked(&given).unwrap();
        assert_eq!(
            (plain.mode, plain.size, plain.command),
            (Mode::Words(None), SIZE, Vec::from([String::from("echo")]))
        );
        assert!(!plain.trace && !plain.prompt && !plain.exact && plain.end.is_none());
        let given = args("-I{} -- -n {}");
        let insert = super::asked(&given).unwrap();
        assert_eq!(insert.mode, Mode::Insert(String::from("{}")));
        assert!(insert.exact, "-I means -x");
        assert_eq!(insert.command, ["-n", "{}"]);
        assert_eq!(super::asked(&args("-L2")).unwrap().mode, Mode::Lines(2));
        let wrong = |options: &str| super::asked(&args(options)).unwrap_err();
        assert_eq!(wrong("-q"), "xargs: -q is no option\n");
        assert_eq!(wrong("-n"), "xargs: -n wants its word\n");
        assert_eq!(wrong("-n 0"), "xargs: -n wants a number from 1, not 0\n");
        assert_eq!(wrong("-L x"), "xargs: -L wants a number from 1, not x\n");
        assert_eq!(wrong("-s -1"), "xargs: -s wants a number from 1, not -1\n");
    }

    #[test]
    fn how_a_command_went_sets_the_status_and_whether_to_stop() {
        assert_eq!(status(&Ran::Ended(Ended::Exited(0))), (0, false));
        assert_eq!(status(&Ran::Declined), (0, false));
        assert_eq!(status(&Ran::Ended(Ended::Exited(1))), (123, false));
        assert_eq!(status(&Ran::Ended(Ended::Exited(254))), (123, false));
        assert_eq!(status(&Ran::Ended(Ended::Exited(255))), (124, true));
        assert_eq!(status(&Ran::Ended(Ended::Faulted(14))), (125, true));
        assert_eq!(status(&Ran::Unstarted(Error::Denied)), (126, true));
        assert_eq!(status(&Ran::NotThere), (127, true));
        let on_disk = |path: &str| path == "/System/Boot/meridian/programs/echo.mexec";
        assert_eq!(
            program_path("echo", on_disk),
            "/System/Boot/meridian/programs/echo.mexec"
        );
        assert_eq!(
            program_path("wc", on_disk),
            "wc.mexec",
            "a program not on the disk is one the loader brought"
        );
        assert_eq!(program_path("/x/y.mexec", |_| false), "/x/y.mexec");
        assert_eq!(program_path("./y", |_| false), "./y");
        assert_eq!(
            too_long(&TooLong::Word),
            "xargs: a word is longer than -s lets a command be\n"
        );
    }

    #[test]
    fn random_words_come_back_in_order_within_every_limit() {
        let mut seed = 0x0123_4567_89ab_cdef_u64;
        for _ in 0..300 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let all: Vec<String> = (0..seed % 30)
                .map(|n| "w".repeat(1 + usize::try_from(n % 5).unwrap()))
                .collect();
            let input = all.join(if seed.is_multiple_of(2) { " " } else { "\n" });
            let n = 1 + seed % 4;
            let size = 12 + seed % 20;
            let made = commands(&alloc::format!("-n {n} -s {size}"), &input).unwrap();
            let words: Vec<&str> = made.iter().flat_map(|c| c.split(' ').skip(1)).collect();
            assert_eq!(words, all.iter().map(String::as_str).collect::<Vec<_>>());
            for command in &made {
                let parts: Vec<&str> = command.split(' ').collect();
                assert!(parts.len() - 1 <= usize::try_from(n).unwrap());
                assert!(
                    parts.iter().map(|p| p.len() + 1).sum::<usize>()
                        <= usize::try_from(size).unwrap()
                );
            }
        }
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// `word` written for the input in one of four ways, as `how` picks:
    /// as it is, in single quotes, in double quotes, or with a backslash
    /// before each character.
    fn written(word: &str, how: u64) -> String {
        match how % 4 {
            0 if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric()) => {
                word.to_string()
            }
            0 | 1 if !word.contains('\'') => alloc::format!("'{word}'"),
            2 if !word.contains('"') => alloc::format!("\"{word}\""),
            // A backslash before each character writes no empty word.
            _ if word.is_empty() => String::from("''"),
            _ => word
                .chars()
                .fold(String::new(), |all, c| alloc::format!("{all}\\{c}")),
        }
    }

    #[test]
    fn random_words_however_written_come_back_as_they_were() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let letters = ['a', 'b', ' ', '\'', '"', '\\', 'z', '\t'];
        for _ in 0..300 {
            let words: Vec<String> = (0..shuffled(&mut seed) % 10)
                .map(|_| {
                    (0..shuffled(&mut seed) % 4)
                        .map(|_| letters[usize::try_from(shuffled(&mut seed) % 8).unwrap()])
                        .collect()
                })
                .collect();
            let mut input = String::new();
            for word in &words {
                input.push_str(&written(word, shuffled(&mut seed)));
                input.push_str(
                    [" ", "\t", "\n", "  \n "][usize::try_from(shuffled(&mut seed) % 4).unwrap()],
                );
            }
            let piece = 1 + usize::try_from(shuffled(&mut seed) % 7).unwrap();
            let read = read_in(false, &input, piece).unwrap();
            let back: Vec<String> = read
                .into_iter()
                .filter_map(|r| match r {
                    Read::Word(w) => Some(String::from_utf8(w).unwrap()),
                    Read::LineEnd => None,
                })
                .collect();
            assert_eq!(back, words, "{input:?}");
        }
    }

    #[test]
    fn random_lines_make_commands_of_so_many_lines() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..300 {
            let lines: Vec<Vec<String>> = (0..shuffled(&mut seed) % 8)
                .map(|n| {
                    (0..=shuffled(&mut seed) % 3)
                        .map(|w| alloc::format!("l{n}w{w}"))
                        .collect()
                })
                .collect();
            let input: String = lines.iter().map(|l| l.join(" ") + "\n").collect();
            let per = 1 + shuffled(&mut seed) % 3;
            let made = commands(&alloc::format!("-L {per}"), &input).unwrap();
            let want: Vec<String> = if lines.is_empty() {
                Vec::from([String::from("echo")])
            } else {
                lines
                    .chunks(usize::try_from(per).unwrap())
                    .map(|chunk| {
                        alloc::format!(
                            "echo {}",
                            chunk
                                .iter()
                                .map(|l| l.join(" "))
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                    })
                    .collect()
            };
            assert_eq!(made, want, "-L {per} {input:?}");
        }
    }

    #[test]
    fn random_lines_go_where_i_says_without_the_blanks_they_start_with() {
        let mut seed = 0x5851_f42d_4c95_7f2d_u64;
        for _ in 0..300 {
            let lines: Vec<String> = (0..shuffled(&mut seed) % 6)
                .map(|n| {
                    alloc::format!(
                        "{}x{n} y",
                        " ".repeat(usize::try_from(shuffled(&mut seed) % 3).unwrap())
                    )
                })
                .collect();
            let input = lines.iter().fold(String::new(), |all, l| all + l + "\n");
            let made = commands("-I @ echo <@> @", &input).unwrap();
            let want: Vec<String> = lines
                .iter()
                .map(|l| {
                    let line = l.trim_start();
                    alloc::format!("echo <{line}> {line}")
                })
                .collect();
            assert_eq!(made, want);
        }
    }
}
