//! A sed script run on lines, as POSIX has it: each line read into the
//! pattern space in turn, the commands whose addresses select it run on
//! it, and what is left written out, unless `-n` said not to.

use alloc::format;
use alloc::vec::Vec;

use regex::{Regex, Text};

use crate::script::{Address, Command, Kind, Piece, Script, Substitute};

/// What sed reads from and writes to.
pub trait World {
    /// The next line of the input, without its line feed; none at the end.
    fn line(&mut self) -> Option<Vec<u8>>;

    /// Whether no line comes after the one read last.
    fn last(&mut self) -> bool;

    /// Writes `bytes` to the standard output.
    fn write(&mut self, bytes: &[u8]);

    /// Adds `bytes` to the end of the file at `path`, which was made anew
    /// before sed read a line.
    fn append(&mut self, path: &str, bytes: &[u8]);

    /// What the file at `path` holds, nothing when it cannot be read.
    fn read(&mut self, path: &str) -> Vec<u8>;
}

/// Why a run stopped short.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// An empty expression before any other was used.
    NoExpression,
    /// A search with back-references went past its budget.
    TooComplex,
}

impl Trouble {
    /// What sed says when a run stopped so.
    #[must_use]
    pub fn says(self) -> &'static str {
        match self {
            Trouble::NoExpression => "an empty expression with none used before it",
            Trouble::TooComplex => regex::TooComplex.says(),
        }
    }
}

impl From<regex::TooComplex> for Trouble {
    fn from(_: regex::TooComplex) -> Trouble {
        Trouble::TooComplex
    }
}

/// How the commands for one line ended.
enum Ended {
    /// At the end of the script, which writes the pattern space.
    Script,
    /// With `d` or `c`, which do not.
    Deleted,
    /// With `D`, which goes on with what is left and reads no line.
    Again,
    /// With `q`, or `n` with no line after, which write the pattern space
    /// and end the run.
    Quit,
    /// With `N` with no line after, which ends the run writing nothing.
    Silent,
}

/// What `a` and `r` write when the commands for a line ended.
enum Queued<'s> {
    Text(&'s str),
    File(&'s str),
}

/// A run of a script.
pub struct Sed<'s> {
    script: &'s Script,
    quiet: bool,
    pattern: Vec<u8>,
    hold: Vec<u8>,
    /// The number of the line read last.
    number: usize,
    /// For each command, whether a range it selects is going on.
    ranges: Vec<bool>,
    /// Whether a substitution was made since a line was read or `t` went.
    substituted: bool,
    queued: Vec<Queued<'s>>,
    /// The expression used last, for an empty one.
    last_regex: Option<&'s Regex>,
}

impl<'s> Sed<'s> {
    /// A run of `script`, which writes the pattern space after each line
    /// unless `quiet` or the script's `#n` says not to.
    #[must_use]
    pub fn new(script: &'s Script, quiet: bool) -> Sed<'s> {
        Sed {
            script,
            quiet: quiet || script.quiet,
            pattern: Vec::new(),
            hold: Vec::new(),
            number: 0,
            ranges: alloc::vec![false; script.commands.len()],
            substituted: false,
            queued: Vec::new(),
            last_regex: None,
        }
    }

    /// Runs the script on every line `world` gives.
    ///
    /// # Errors
    ///
    /// When an empty expression comes before any other, or a search goes
    /// past its budget; what was written stays written.
    pub fn run(&mut self, world: &mut impl World) -> Result<(), Trouble> {
        let mut read = true;
        loop {
            if read {
                let Some(line) = world.line() else {
                    return Ok(());
                };
                self.number += 1;
                self.pattern = line;
                self.substituted = false;
            }
            let ended = self.lines(world)?;
            if matches!(ended, Ended::Script | Ended::Quit) && !self.quiet {
                self.put(world);
            }
            self.flush(world);
            match ended {
                Ended::Quit | Ended::Silent => return Ok(()),
                Ended::Again => read = false,
                Ended::Script | Ended::Deleted => read = true,
            }
        }
    }

    /// Writes the pattern space and a line feed.
    fn put(&self, world: &mut impl World) {
        world.write(&self.pattern);
        world.write(b"\n");
    }

    /// Writes what `a` and `r` left to write.
    fn flush(&mut self, world: &mut impl World) {
        for queued in core::mem::take(&mut self.queued) {
            match queued {
                Queued::Text(text) => {
                    world.write(text.as_bytes());
                    world.write(b"\n");
                }
                Queued::File(path) => {
                    let held = world.read(path);
                    world.write(&held);
                }
            }
        }
    }

    /// Runs the commands on the pattern space, and says how they ended.
    fn lines(&mut self, world: &mut impl World) -> Result<Ended, Trouble> {
        let script = self.script;
        let mut at = 0;
        while let Some(command) = script.commands.get(at) {
            if !self.selects(at, command, world)? {
                at = match command.kind {
                    Kind::Block(end) => end + 1,
                    _ => at + 1,
                };
                continue;
            }
            let index = at;
            at += 1;
            match &command.kind {
                Kind::Block(_) | Kind::End | Kind::Label => {}
                Kind::Append(text) => self.queued.push(Queued::Text(text)),
                Kind::Branch(to) => at = *to,
                Kind::Change(text) => {
                    // In a range only its last line puts the text out.
                    if !self.ranges[index] {
                        world.write(text.as_bytes());
                        world.write(b"\n");
                    }
                    return Ok(Ended::Deleted);
                }
                Kind::Delete => return Ok(Ended::Deleted),
                Kind::DeleteFirst => {
                    let Some(feed) = self.pattern.iter().position(|&b| b == b'\n') else {
                        return Ok(Ended::Deleted);
                    };
                    self.pattern.drain(..=feed);
                    return Ok(Ended::Again);
                }
                Kind::Get => self.pattern.clone_from(&self.hold),
                Kind::GetAppend => {
                    self.pattern.push(b'\n');
                    self.pattern.extend_from_slice(&self.hold);
                }
                Kind::Hold => self.hold.clone_from(&self.pattern),
                Kind::HoldAppend => {
                    self.hold.push(b'\n');
                    self.hold.extend_from_slice(&self.pattern);
                }
                Kind::Insert(text) => {
                    world.write(text.as_bytes());
                    world.write(b"\n");
                }
                Kind::List => world.write(&listed(&self.pattern)),
                Kind::Next => {
                    if world.last() {
                        return Ok(Ended::Quit);
                    }
                    if !self.quiet {
                        self.put(world);
                    }
                    self.flush(world);
                    self.pattern = world.line().unwrap_or_default();
                    self.number += 1;
                }
                Kind::NextAppend => {
                    if world.last() {
                        return Ok(Ended::Silent);
                    }
                    self.flush(world);
                    let line = world.line().unwrap_or_default();
                    self.pattern.push(b'\n');
                    self.pattern.extend_from_slice(&line);
                    self.number += 1;
                }
                Kind::Print => self.put(world),
                Kind::PrintFirst => {
                    let end = self.pattern.iter().position(|&b| b == b'\n');
                    world.write(&self.pattern[..end.unwrap_or(self.pattern.len())]);
                    world.write(b"\n");
                }
                Kind::Quit => return Ok(Ended::Quit),
                Kind::Read(path) => self.queued.push(Queued::File(path)),
                Kind::Substitute(substitute) => self.substitute(substitute, world)?,
                Kind::Test(to) => {
                    if self.substituted {
                        self.substituted = false;
                        at = *to;
                    }
                }
                Kind::Write(path) => {
                    let mut line = self.pattern.clone();
                    line.push(b'\n');
                    world.append(path, &line);
                }
                Kind::Exchange => core::mem::swap(&mut self.pattern, &mut self.hold),
                Kind::Translate(pairs) => self.pattern = translated(&self.pattern, pairs),
                Kind::Number => world.write(format!("{}\n", self.number).as_bytes()),
            }
        }
        Ok(Ended::Script)
    }

    /// Whether the addresses of `command`, the one at `index`, select the
    /// line read last; a range that starts or ends on it starts or ends.
    fn selects(
        &mut self,
        index: usize,
        command: &'s Command,
        world: &mut impl World,
    ) -> Result<bool, Trouble> {
        let selected = match (&command.first, &command.second) {
            (None, _) => true,
            (Some(first), None) => self.matches(first, world)?,
            (Some(first), Some(second)) => {
                if self.ranges[index] {
                    let ends = match second {
                        Address::Line(line) => self.number >= *line,
                        other => self.matches(other, world)?,
                    };
                    self.ranges[index] = !ends;
                    true
                } else if self.matches(first, world)? {
                    // A second address that is a line already reached
                    // ends the range on the line that starts it.
                    self.ranges[index] =
                        !matches!(second, Address::Line(line) if *line <= self.number);
                    true
                } else {
                    false
                }
            }
        };
        Ok(selected != command.negated)
    }

    fn matches(&mut self, address: &'s Address, world: &mut impl World) -> Result<bool, Trouble> {
        Ok(match address {
            Address::Line(line) => self.number == *line,
            Address::Last => world.last(),
            Address::Match(regex) => {
                let regex = self.regex(regex.as_ref())?;
                regex.find(&Text::new(&self.pattern), 0)?.is_some()
            }
        })
    }

    /// `regex`, or for none the expression used last, which it is from
    /// then on.
    fn regex(&mut self, regex: Option<&'s Regex>) -> Result<&'s Regex, Trouble> {
        let regex = regex.or(self.last_regex).ok_or(Trouble::NoExpression)?;
        self.last_regex = Some(regex);
        Ok(regex)
    }

    /// `s`: replaces its `nth` match in the pattern space, or with `g` each
    /// from that one on. An empty match right where one ended is none.
    fn substitute(
        &mut self,
        substitute: &'s Substitute,
        world: &mut impl World,
    ) -> Result<(), Trouble> {
        let regex = self.regex(substitute.regex.as_ref())?;
        let text = Text::new(&self.pattern);
        let mut out = Vec::new();
        let mut copied = 0;
        let mut from = 0;
        let mut count = 0;
        let mut ended = None;
        while let Some(spans) = regex.captures(&text, from)? {
            let Some(whole) = spans[0] else {
                break;
            };
            let empty = whole.start == whole.end;
            if !(empty && ended == Some(whole.start)) {
                count += 1;
                if count >= substitute.nth {
                    out.extend_from_slice(&self.pattern[copied..whole.start]);
                    for piece in &substitute.replacement {
                        let span = match piece {
                            Piece::Text(text) => {
                                out.extend_from_slice(text.as_bytes());
                                continue;
                            }
                            Piece::Whole => Some(whole),
                            Piece::Group(number) => spans.get(*number).copied().flatten(),
                        };
                        if let Some(span) = span {
                            out.extend_from_slice(&self.pattern[span.start..span.end]);
                        }
                    }
                    copied = whole.end;
                    if !substitute.every {
                        break;
                    }
                }
            }
            ended = Some(whole.end);
            // After an empty match the next starts a character further.
            from = whole.end + usize::from(empty);
            if from > self.pattern.len() {
                break;
            }
        }
        if count < substitute.nth {
            return Ok(());
        }
        out.extend_from_slice(&self.pattern[copied..]);
        self.pattern = out;
        self.substituted = true;
        if substitute.print {
            self.put(world);
        }
        if let Some(path) = &substitute.write {
            let mut line = self.pattern.clone();
            line.push(b'\n');
            world.append(path, &line);
        }
        Ok(())
    }
}

/// `pattern` with each character that is first in one of `pairs` made
/// the second.
fn translated(pattern: &[u8], pairs: &[(char, char)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pattern.len());
    for chunk in pattern.utf8_chunks() {
        for c in chunk.valid().chars() {
            let made = pairs
                .iter()
                .find(|(from, _)| *from == c)
                .map_or(c, |&(_, to)| to);
            let mut room = [0; 4];
            out.extend_from_slice(made.encode_utf8(&mut room).as_bytes());
        }
        out.extend_from_slice(chunk.invalid());
    }
    out
}

/// How long a line `l` writes may be before it folds it, its `\` after.
const FOLD: usize = 69;

/// The pattern space as `l` writes it: `\` and the characters POSIX
/// names as their escapes, a line feed as `\n`, every other byte that
/// prints nothing in octal, lines folded with a `\` after [`FOLD`]
/// characters, and a `$` at the end.
fn listed(pattern: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut width = 0;
    let mut put = |piece: &[u8], wide: usize| {
        if width + wide > FOLD {
            out.extend_from_slice(b"\\\n");
            width = 0;
        }
        out.extend_from_slice(piece);
        width += wide;
    };
    for chunk in pattern.utf8_chunks() {
        for c in chunk.valid().chars() {
            let escape: &[u8] = match c {
                '\\' => b"\\\\",
                '\u{7}' => b"\\a",
                '\u{8}' => b"\\b",
                '\u{c}' => b"\\f",
                '\n' => b"\\n",
                '\r' => b"\\r",
                '\t' => b"\\t",
                '\u{b}' => b"\\v",
                c if c.is_control() => {
                    let mut room = [0; 4];
                    for byte in c.encode_utf8(&mut room).bytes() {
                        put(format!("\\{byte:03o}").as_bytes(), 4);
                    }
                    continue;
                }
                c => {
                    let mut room = [0; 4];
                    put(c.encode_utf8(&mut room).as_bytes(), 1);
                    continue;
                }
            };
            put(escape, 2);
        }
        for byte in chunk.invalid() {
            put(format!("\\{byte:03o}").as_bytes(), 4);
        }
    }
    out.extend_from_slice(b"$\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::parse;
    use alloc::string::String;
    use regex::Syntax;

    /// Lines in memory to read, and what is written, to the standard
    /// output and to files.
    #[derive(Default)]
    struct Desk {
        lines: Vec<Vec<u8>>,
        next: usize,
        out: Vec<u8>,
        files: Vec<(String, Vec<u8>)>,
    }

    impl Desk {
        fn reading(input: &str) -> Desk {
            let mut lines: Vec<Vec<u8>> = input
                .split('\n')
                .map(|line| line.as_bytes().to_vec())
                .collect();
            if input.ends_with('\n') || input.is_empty() {
                lines.pop();
            }
            Desk {
                lines,
                ..Desk::default()
            }
        }

        fn file(&self, path: &str) -> Option<String> {
            self.files
                .iter()
                .find(|(name, _)| name == path)
                .map(|(_, held)| String::from_utf8(held.clone()).unwrap())
        }
    }

    impl World for Desk {
        fn line(&mut self) -> Option<Vec<u8>> {
            let line = self.lines.get(self.next)?.clone();
            self.next += 1;
            Some(line)
        }

        fn last(&mut self) -> bool {
            self.next >= self.lines.len()
        }

        fn write(&mut self, bytes: &[u8]) {
            self.out.extend_from_slice(bytes);
        }

        fn append(&mut self, path: &str, bytes: &[u8]) {
            match self.files.iter_mut().find(|(name, _)| name == path) {
                Some((_, held)) => held.extend_from_slice(bytes),
                None => self.files.push((String::from(path), bytes.to_vec())),
            }
        }

        fn read(&mut self, path: &str) -> Vec<u8> {
            self.files
                .iter()
                .find(|(name, _)| name == path)
                .map(|(_, held)| held.clone())
                .unwrap_or_default()
        }
    }

    /// What `script` makes of `input`, quiet when `quiet`, and the desk.
    fn ran(script: &str, quiet: bool, desk: &mut Desk, syntax: Syntax) -> Result<String, Trouble> {
        let script = parse(script, syntax).unwrap();
        let mut sed = Sed::new(&script, quiet);
        let done = sed.run(desk);
        let out = String::from_utf8(desk.out.clone()).unwrap();
        done.map(|()| out)
    }

    fn sed(script: &str, input: &str) -> String {
        ran(script, false, &mut Desk::reading(input), Syntax::Basic).unwrap()
    }

    fn quiet(script: &str, input: &str) -> String {
        ran(script, true, &mut Desk::reading(input), Syntax::Basic).unwrap()
    }

    #[test]
    fn each_line_is_written_unless_quiet() {
        assert_eq!(sed("", "a\nb\n"), "a\nb\n");
        assert_eq!(quiet("", "a\nb\n"), "");
        assert_eq!(sed("p", "a\nb\n"), "a\na\nb\nb\n");
        assert_eq!(quiet("p", "a\nb\n"), "a\nb\n");
        assert_eq!(sed("#n\np", "a\n"), "a\n", "#n is -n");
        assert_eq!(sed("", ""), "");
        assert_eq!(
            sed("", "no end"),
            "no end\n",
            "a last line without its line feed"
        );
    }

    #[test]
    fn addresses_select_lines_by_number_last_and_match() {
        let input = "one\ntwo\nthree\nfour\n";
        assert_eq!(sed("2d", input), "one\nthree\nfour\n");
        assert_eq!(sed("$d", input), "one\ntwo\nthree\n");
        assert_eq!(sed("/o/d", input), "three\n");
        assert_eq!(sed("2!d", input), "two\n");
        assert_eq!(quiet("/^t/p", input), "two\nthree\n");
        assert_eq!(quiet("$p", "only\n"), "only\n");
        assert_eq!(quiet("9p", input), "");
    }

    #[test]
    fn a_range_runs_from_its_first_line_to_its_second() {
        let input = "1\n2\n3\n4\n5\n6\n";
        assert_eq!(quiet("2,4p", input), "2\n3\n4\n");
        assert_eq!(quiet("2,4!p", input), "1\n5\n6\n");
        assert_eq!(quiet("/2/,/4/p", input), "2\n3\n4\n");
        assert_eq!(quiet("4,2p", input), "4\n", "a second line already passed");
        assert_eq!(quiet("3,3p", input), "3\n");
        assert_eq!(quiet("5,$p", input), "5\n6\n");
        assert_eq!(
            quiet("/5/,9p", input),
            "5\n6\n",
            "to the end when it never comes"
        );
        assert_eq!(
            quiet("/[24]/,/[0-9]/p", input),
            "2\n3\n4\n5\n",
            "the second is not tried on the first"
        );
        assert_eq!(
            quiet(
                "/1/,+p"
                    .split(',')
                    .next()
                    .unwrap()
                    .replace("/1/", "1,2p")
                    .as_str(),
                input
            ),
            "1\n2\n"
        );
        let again = "x\na\nx\nb\nx\n";
        assert_eq!(quiet("/a/,/x/p", again), "a\nx\n");
        assert_eq!(
            quiet("/x/,/x/p", again),
            "x\na\nx\nx\n",
            "a range starts again after it ended"
        );
    }

    #[test]
    fn a_block_runs_its_commands_on_the_lines_it_selects() {
        let input = "a\nb\nc\n";
        assert_eq!(quiet("2{p;p}", input), "b\nb\n");
        assert_eq!(quiet("2!{p}", input), "a\nc\n");
        assert_eq!(quiet("/a/,/b/{/b/p}", input), "b\n");
        assert_eq!(sed("1{\nd\n}\n3{s/c/C/}", input), "b\nC\n");
        assert_eq!(quiet("{{p}}", "z\n"), "z\n");
    }

    #[test]
    fn s_replaces_the_match_or_every_one() {
        assert_eq!(sed("s/o/0/", "foo\n"), "f0o\n");
        assert_eq!(sed("s/o/0/g", "foo\n"), "f00\n");
        assert_eq!(sed("s/o/0/2", "fooo\n"), "fo0o\n");
        assert_eq!(sed("s/o/0/2g", "fooo\n"), "fo00\n");
        assert_eq!(sed("s/o/0/4", "fooo\n"), "fooo\n", "no fourth");
        assert_eq!(sed("s/x/y/", "abc\n"), "abc\n");
        assert_eq!(sed("s/.*/[&]/", "ab\n"), "[ab]\n");
        assert_eq!(sed("s/\\(a*\\)\\(b*\\)/\\2\\1/", "aabbc\n"), "bbaac\n");
        assert_eq!(
            sed("s/a\\(x\\)*/[\\1]/", "ab\n"),
            "[]b\n",
            "a group that took no part"
        );
        assert_eq!(sed("s/b/\\n/", "abc\n"), "a\nc\n");
        assert_eq!(sed("s/ü/ue/g", "Tür über\n"), "Tuer ueber\n");
        assert_eq!(quiet("s/a/A/p", "a\nb\n"), "A\n");
    }

    #[test]
    fn an_empty_match_where_one_ended_is_no_match() {
        assert_eq!(sed("s/b*/x/g", "abc\n"), "xaxcx\n");
        assert_eq!(sed("s/x*/-/g", "abc\n"), "-a-b-c-\n");
        assert_eq!(sed("s/x*/-/2", "abc\n"), "a-bc\n");
        assert_eq!(sed("s/b*/x/2", "abc\n"), "axc\n");
        assert_eq!(sed("s/x*/-/g", "\n"), "-\n");
        assert_eq!(
            sed("s/x*/-/g", "äö\n"),
            "-ä-ö-\n",
            "a character further, not a byte"
        );
    }

    #[test]
    fn an_empty_expression_is_the_one_used_last() {
        assert_eq!(sed("/a/s//X/g", "banana\n"), "bXnXnX\n");
        assert_eq!(sed("s/n/N/;s//M/", "banana\n"), "baNaMa\n");
        assert_eq!(quiet("/b/{//p}", "abc\nxyz\n"), "abc\n");
        let mut desk = Desk::reading("a\n");
        assert_eq!(
            ran("s//x/", false, &mut desk, Syntax::Basic),
            Err(Trouble::NoExpression)
        );
    }

    #[test]
    fn t_branches_once_a_substitution_was_made() {
        assert_eq!(
            sed(":a\ns/^\\(x*\\)y/\\1x/\nta", "yyy\n"),
            "xyy\n".replace("xyy", "xxx").as_str()
        );
        assert_eq!(sed("s/a/A/\nt\ns/$/!/", "a\nb\n"), "A\nb!\n");
        assert_eq!(
            sed("s/a/A/\ns/z/Z/\nt done\ns/$/?/\n:done", "a\nb\n"),
            "A\nb?\n",
            "any substitution since the line was read"
        );
        assert_eq!(
            sed("s/a/A/\nt one\n:one\nt two\ns/$/!/\n:two", "a\n"),
            "A!\n",
            "t takes back what it went on"
        );
    }

    #[test]
    fn b_branches_to_its_label_or_the_end() {
        assert_eq!(sed("b\ns/a/X/", "a\n"), "a\n");
        assert_eq!(sed("/a/b skip\ns/$/!/\n:skip", "a\nb\n"), "a\nb!\n");
        assert_eq!(quiet(":top\n$!{N;b top}\np", "1\n2\n3\n"), "1\n2\n3\n");
    }

    #[test]
    fn y_changes_characters_one_for_one() {
        assert_eq!(sed("y/abc/xyz/", "aabbcc d\n"), "xxyyzz d\n");
        assert_eq!(sed("y/äö/ÄÖ/", "schön\n"), "schÖn\n");
        assert_eq!(sed("N;y/\\n/ /", "a\nb\n"), "a b\n");
        let mut desk = Desk {
            lines: Vec::from([Vec::from([b'a', 0xff, b'b'])]),
            ..Desk::default()
        };
        let script = parse("y/ab/xy/", Syntax::Basic).unwrap();
        Sed::new(&script, false).run(&mut desk).unwrap();
        assert_eq!(
            desk.out,
            [b'x', 0xff, b'y', b'\n'],
            "a byte that is no UTF-8 stays"
        );
    }

    #[test]
    fn a_i_and_c_put_text_out_around_and_instead_of_lines() {
        assert_eq!(sed("2i\\\nbefore", "1\n2\n"), "1\nbefore\n2\n");
        assert_eq!(sed("1a after", "1\n2\n"), "1\nafter\n2\n");
        assert_eq!(
            sed("a one\na two", "x\n"),
            "x\none\ntwo\n",
            "in their order"
        );
        assert_eq!(sed("1a after\n1d", "1\n2\n"), "after\n2\n", "after d too");
        assert_eq!(sed("2c\\\nchanged", "1\n2\n3\n"), "1\nchanged\n3\n");
        assert_eq!(
            sed("2,3c gone", "1\n2\n3\n4\n"),
            "1\ngone\n4\n",
            "once, at the end of a range"
        );
        assert_eq!(sed("$!c x", "1\n2\n"), "x\n2\n");
        assert_eq!(sed("i\\\n  kept", "a\n"), "  kept\na\n");
    }

    #[test]
    fn hold_space_commands_move_text_between_the_spaces() {
        assert_eq!(
            sed("1!G;h;$!d", "a\nb\nc\n"),
            "c\nb\na\n",
            "the lines turned round"
        );
        assert_eq!(sed("x", "a\nb\n"), "\na\n");
        assert_eq!(sed("h;s/./X/;G", "ab\n"), "Xb\nab\n");
        assert_eq!(quiet("H;${x;p}", "a\nb\n"), "\na\nb\n");
        assert_eq!(sed("g", "a\n"), "\n");
    }

    #[test]
    fn n_and_n_append_read_the_next_line_in_the_middle() {
        assert_eq!(sed("n;d", "1\n2\n3\n4\n5\n"), "1\n3\n5\n");
        assert_eq!(quiet("n;p", "1\n2\n3\n"), "2\n");
        assert_eq!(
            sed("n", "only\n"),
            "only\n",
            "no next line ends it, once written"
        );
        assert_eq!(sed("$!N;s/\\n/ /", "1\n2\n3\n4\n5\n"), "1 2\n3 4\n5\n");
        assert_eq!(
            sed("N;s/\\n/+/", "1\n2\n3\n"),
            "1+2\n",
            "N at the end writes nothing, as POSIX has it"
        );
        assert_eq!(quiet("N;N;=", "a\nb\nc\nd\n"), "3\n", "the number goes on");
        assert_eq!(
            sed("1a A\nN", "1\n2\n"),
            "A\n1\n2\n",
            "a's text is put out before N reads"
        );
    }

    #[test]
    fn d_and_p_first_take_the_pattern_space_up_to_its_first_line_feed() {
        assert_eq!(sed("$!N;P;D", "1\n2\n3\n"), "1\n2\n3\n");
        assert_eq!(
            sed("$!N;/^\\(.*\\)\\n\\1$/!P;D", "a\na\nb\nc\nc\n"),
            "a\nb\nc\n",
            "lines that come again once"
        );
        assert_eq!(quiet("N;P", "x\ny\n"), "x\n");
        assert_eq!(sed("D", "a\nb\n"), "", "with no line feed as d");
        assert_eq!(
            sed(
                "/\\n/!G;s/\\(.\\)\\(.*\\n\\)/&\\2\\1/;//D;s/.//",
                "abc\nxy\n"
            ),
            "cba\nyx\n",
            "each line turned round"
        );
    }

    #[test]
    fn q_ends_after_writing_and_equals_writes_the_number() {
        assert_eq!(sed("2q", "1\n2\n3\n"), "1\n2\n");
        assert_eq!(quiet("2q", "1\n2\n3\n"), "");
        assert_eq!(sed("1a after\n1q", "1\n2\n"), "1\nafter\n");
        assert_eq!(quiet("$=", "a\nb\nc\n"), "3\n");
        assert_eq!(sed("=", "a\nb\n"), "1\na\n2\nb\n");
        assert_eq!(sed("N;s/\\n/\t/", &sed("=", "a\nb\n")), "1\ta\n2\tb\n");
    }

    #[test]
    fn w_and_r_write_and_read_files() {
        let mut desk = Desk::reading("a\nb\nc\n");
        assert_eq!(
            ran("/b/w out\ns/c/C/w subs", true, &mut desk, Syntax::Basic).unwrap(),
            ""
        );
        assert_eq!(desk.file("out").as_deref(), Some("b\n"));
        assert_eq!(desk.file("subs").as_deref(), Some("C\n"));
        let mut desk = Desk::reading("1\n2\n");
        desk.files
            .push((String::from("note"), b"read in\n".to_vec()));
        assert_eq!(
            ran("1r note", false, &mut desk, Syntax::Basic).unwrap(),
            "1\nread in\n2\n"
        );
        let mut desk = Desk::reading("1\n");
        assert_eq!(
            ran("r none", false, &mut desk, Syntax::Basic).unwrap(),
            "1\n",
            "a file not there adds nothing"
        );
    }

    #[test]
    fn l_writes_the_pattern_space_so_nothing_hides() {
        assert_eq!(quiet("l", "a\tb\\c\n"), "a\\tb\\\\c$\n");
        assert_eq!(quiet("N;l", "x\ny\n"), "x\\ny$\n");
        assert_eq!(
            quiet("l", "\u{7}\u{8}\u{c}\r\u{b}\u{1b}\n"),
            "\\a\\b\\f\\r\\v\\033$\n"
        );
        assert_eq!(quiet("l", "schön\n"), "schön$\n");
        assert_eq!(
            quiet("l", "\u{85}\n"),
            "\\302\\205$\n",
            "a control past ASCII by its bytes"
        );
        let long = "x".repeat(70);
        assert_eq!(
            quiet("l", &(long.clone() + "\n")),
            "x".repeat(69) + "\\\nx$\n",
            "folded after 69"
        );
        assert_eq!(quiet("l", &("x".repeat(69) + "\n")), "x".repeat(69) + "$\n");
        assert_eq!(
            quiet("l", &("x".repeat(68) + "\t\n")),
            "x".repeat(68) + "\\\n\\t$\n",
            "an escape is not cut"
        );
        let mut desk = Desk {
            lines: Vec::from([Vec::from([b'a', 0xff])]),
            ..Desk::default()
        };
        assert_eq!(
            ran("l", true, &mut desk, Syntax::Basic).unwrap(),
            "a\\377$\n"
        );
    }

    #[test]
    fn extended_expressions_run_as_extended() {
        let mut desk = Desk::reading("aaa b\n");
        assert_eq!(
            ran("s/(a+) (b)/\\2 \\1/", false, &mut desk, Syntax::Extended).unwrap(),
            "b aaa\n"
        );
    }

    #[test]
    fn a_search_too_complex_ends_the_run_with_what_was_written() {
        let line = "a".repeat(40);
        let mut desk = Desk::reading(&("ok\n".to_owned() + &line + "\n"));
        assert_eq!(
            ran(
                "/ok/p;/\\(\\(a*\\)*\\)*\\1x/d",
                true,
                &mut desk,
                Syntax::Basic
            ),
            Err(Trouble::TooComplex)
        );
        assert_eq!(desk.out, b"ok\n");
        assert_ne!(Trouble::TooComplex.says(), Trouble::NoExpression.says());
    }

    #[test]
    fn the_pattern_space_prints_whole_or_to_its_first_line_feed() {
        assert_eq!(quiet("P", "abc\n"), "abc\n", "with no line feed all of it");
        assert_eq!(quiet("N;P;p", "a\nb\n"), "a\na\nb\n");
        assert_eq!(quiet("$!N;P", "a\nb\nc\n"), "a\nc\n");
        assert_eq!(quiet("G;P", "x\n"), "x\n");
        assert_eq!(
            quiet("G;l", "x\n"),
            "x\\n$\n",
            "an empty hold space after a line feed"
        );
        assert_eq!(quiet("G;G;P;p", "x\n"), "x\nx\n\n\n");
    }

    #[test]
    fn the_hold_space_starts_empty_and_keeps_across_lines() {
        assert_eq!(sed("G", "a\nb\n"), "a\n\nb\n\n", "double spaced");
        assert_eq!(sed("x;G", "a\nb\n"), "\na\na\nb\n");
        assert_eq!(quiet("H;$!d;x;s/\\n/,/g;p", "a\nb\nc\n"), ",a,b,c\n");
        assert_eq!(quiet("1h;1!H;${g;s/\\n/+/g;p}", "1\n2\n3\n"), "1+2+3\n");
        assert_eq!(sed("x;x", "a\nb\n"), "a\nb\n");
        assert_eq!(sed("h;d", "a\n"), "");
        assert_eq!(quiet("$!{h;d};x;p", "a\nb\n"), "a\n");
    }

    #[test]
    fn negation_and_blocks_select_together() {
        let input = "1\n2\n3\n4\n";
        assert_eq!(quiet("2,3!{p}", input), "1\n4\n");
        assert_eq!(quiet("2,3{2!p}", input), "3\n");
        assert_eq!(quiet("/1/!{/4/!p}", input), "2\n3\n");
        assert_eq!(quiet("$!{$!p}", input), "1\n2\n3\n");
        assert_eq!(sed("2,3!d", input), "2\n3\n");
        assert_eq!(
            quiet("1,/[0-9]/p", input),
            "1\n2\n",
            "the second address only from the next line on"
        );
    }

    #[test]
    fn substitution_flags_go_together() {
        let mut desk = Desk::reading("aaa\nbbb\n");
        assert_eq!(
            ran("s/a/x/2gpw both", true, &mut desk, Syntax::Basic).unwrap(),
            "axx\n"
        );
        assert_eq!(
            desk.file("both").as_deref(),
            Some("axx\n"),
            "only lines it changed"
        );
        assert_eq!(sed("s/a/&&/g", "ab\n"), "aab\n");
        assert_eq!(sed("s/[0-9]/<&>/3", "12345\n"), "12<3>45\n");
        assert_eq!(sed("s/$/!/", "x\n"), "x!\n");
        assert_eq!(sed("s/^/> /", "x\n"), "> x\n");
        assert_eq!(sed("s/^/> /g", "x\n"), "> x\n", "^ holds once");
        assert_eq!(sed("s/x*$/!/", "axx\n"), "a!\n");
        assert_eq!(sed("s/.//g", "abc\n"), "\n");
        assert_eq!(sed("s/\\(.\\)\\(.\\)/\\2\\1/g", "abcde\n"), "badce\n");
        assert_eq!(sed("s/a/b/;s/b/c/", "a\n"), "c\n", "one after the other");
    }

    #[test]
    fn a_substitution_spans_the_lines_in_the_pattern_space() {
        assert_eq!(sed("N;N;s/\\n/,/g", "a\nb\nc\n"), "a,b,c\n");
        assert_eq!(
            sed("N;s/^/>/g", "a\nb\n"),
            ">a\nb\n",
            "^ only at the start of the space"
        );
        assert_eq!(sed("N;s/$/</", "a\nb\n"), "a\nb<\n");
        assert_eq!(sed("N;s/a.b/X/", "a\nb\n"), "X\n", ". takes a line feed");
        assert_eq!(sed("N;s/[^a]/_/", "a\nb\n"), "a_b\n");
    }

    #[test]
    fn t_forgets_substitutions_when_a_new_line_is_read() {
        assert_eq!(sed("1s/a/A/;2t end;2s/$/!/;:end", "a\nb\n"), "A\nb!\n");
        assert_eq!(
            sed("s/a/A/;$!N;t end;s/$/?/;:end", "a\nb\n"),
            "A\nb\n",
            "N reads a line but keeps what t will see"
        );
    }

    #[test]
    fn a_and_i_write_texts_of_several_lines() {
        assert_eq!(sed("1i\\\nfirst\\\nsecond", "x\n"), "first\nsecond\nx\n");
        assert_eq!(sed("a\\\nend", "x\ny\n"), "x\nend\ny\nend\n");
        assert_eq!(sed("$a\\\nlast", "x\ny\n"), "x\ny\nlast\n");
        assert_eq!(sed("1i x\n1i y", "z\n"), "x\ny\nz\n");
        assert_eq!(
            quiet("a after", "x\n"),
            "after\n",
            "-n does not keep a's text in"
        );
        assert_eq!(sed("2q;a z", "1\n2\n3\n"), "1\nz\n2\n");
    }

    #[test]
    fn c_puts_its_text_out_for_each_line_it_does_not_take_as_a_range() {
        assert_eq!(sed("2!c X", "1\n2\n3\n"), "X\n2\nX\n");
        assert_eq!(sed("/b/,/c/!c -", "a\nb\nc\nd\n"), "-\nb\nc\n-\n");
        assert_eq!(
            sed("1,2{c X\n}", "1\n2\n3\n"),
            "X\nX\n3\n",
            "inside a block it has no range"
        );
        assert_eq!(sed("$c\\\nend", "1\n2\n"), "1\nend\n");
        assert_eq!(sed("c\\\n", "1\n"), "\n");
    }

    #[test]
    fn q_in_a_block_and_d_in_the_middle_end_as_they_should() {
        assert_eq!(sed("/stop/{s/stop/STOP/;q}", "a\nstop\nb\n"), "a\nSTOP\n");
        assert_eq!(sed("2{p;d};s/$/./", "1\n2\n3\n"), "1.\n2\n3.\n");
        assert_eq!(sed("$q;s/$/,/", "1\n2\n"), "1,\n2\n");
        assert_eq!(sed("d;s/^/never/", "1\n"), "");
    }

    #[test]
    fn r_puts_a_file_out_as_it_is_after_the_line() {
        let with = |script: &str, input: &str| {
            let mut desk = Desk::reading(input);
            desk.files
                .push((String::from("f"), b"no line feed".to_vec()));
            ran(script, false, &mut desk, Syntax::Basic).unwrap()
        };
        assert_eq!(with("r f", "a\nb\n"), "a\nno line feedb\nno line feed");
        assert_eq!(with("$r f", "a\nb\n"), "a\nb\nno line feed");
        assert_eq!(
            with("r f\nd", "a\n"),
            "no line feed",
            "d does not take it back"
        );
        assert_eq!(
            with("1r f\n1a text", "a\n"),
            "a\nno line feedtext\n",
            "with a's text in order"
        );
    }

    #[test]
    fn w_adds_each_line_to_its_file_in_order() {
        let mut desk = Desk::reading("a\nb\n");
        ran("w f\ns/a/A/w f", true, &mut desk, Syntax::Basic).unwrap();
        assert_eq!(desk.file("f").as_deref(), Some("a\nA\nb\n"));
        let mut desk = Desk::reading("a\n");
        ran("N;w g", false, &mut desk, Syntax::Basic).unwrap();
        assert_eq!(desk.file("g"), None, "N at the end ended it before w");
    }

    #[test]
    fn numbers_count_every_line_read() {
        assert_eq!(quiet("n;=", "a\nb\nc\nd\n"), "2\n4\n");
        assert_eq!(quiet("$!N;=", "a\nb\nc\n"), "2\n3\n");
        assert_eq!(quiet("/b/=", "a\nb\nc\nb\n"), "2\n4\n");
        assert_eq!(sed("$!d;=", "x\ny\nz\n"), "3\nz\n");
    }

    #[test]
    fn scripts_that_change_nothing_give_the_input_back() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut roll = |sides: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % 1024).unwrap() % sides
        };
        for _ in 0..150 {
            let lines: Vec<String> = (0..roll(6))
                .map(|_| {
                    (0..roll(5))
                        .map(|_| ['a', 'b', 'ä', ' ', '\t'][roll(5)])
                        .collect()
                })
                .collect();
            let mut input = lines.join("\n");
            if !lines.is_empty() {
                input.push('\n');
            }
            for script in [
                "",
                "p;d",
                "h;g",
                "x;x",
                "y/ab/ba/;y/ab/ba/",
                "s/a/&/g",
                "$!N;P;D",
                "s/\\(.*\\)/\\1/",
            ] {
                assert_eq!(sed(script, &input), input, "{script} on {input:?}");
            }
            assert_eq!(quiet("p", &input), input);
            assert_eq!(
                sed("1!G;h;$!d", &sed("1!G;h;$!d", &input)),
                input,
                "turned round twice"
            );
            let count = if lines.is_empty() {
                String::new()
            } else {
                alloc::format!("{}\n", lines.len())
            };
            assert_eq!(quiet("$=", &input), count, "{input:?}");
        }
    }

    #[test]
    fn a_range_may_start_on_the_last_line_or_open_again() {
        assert_eq!(quiet("$,/x/p", "a\nb\n"), "b\n");
        assert_eq!(
            quiet("/a/,2p", "1\na\n2\na\n3\n"),
            "a\na\n",
            "a line passed ends it on its first"
        );
        assert_eq!(quiet("2,/z/p", "1\n2\n3\n"), "2\n3\n");
        assert_eq!(sed("2,$c gone", "1\n2\n3\n"), "1\ngone\n");
    }

    #[test]
    fn l_marks_an_empty_space_and_lines_held_together() {
        assert_eq!(quiet("l", "\n"), "$\n");
        assert_eq!(quiet("G;G;l", "a\n"), "a\\n\\n$\n");
        assert_eq!(quiet("y/a/\\n/;l", "bab\n"), "b\\nb$\n");
    }

    #[test]
    fn groups_left_out_of_a_round_add_nothing() {
        assert_eq!(sed("s/\\(a\\)\\{0,1\\}b/[\\1]/g", "abb\n"), "[a][]\n");
        assert_eq!(sed("s/x\\(y\\)*/<\\1>/g", "xyyx\n"), "<y><>\n");
    }

    #[test]
    fn an_empty_or_too_complex_expression_stops_the_run_where_it_is() {
        let mut desk = Desk::reading("a\n");
        assert_eq!(
            ran("p;//p", true, &mut desk, Syntax::Basic),
            Err(Trouble::NoExpression)
        );
        assert_eq!(desk.out, b"a\n", "what came before it stays");
        let line = "a".repeat(40);
        let mut desk = Desk::reading(&(line + "\n"));
        assert_eq!(
            ran("s/\\(\\(a*\\)*\\)*\\1x/y/", false, &mut desk, Syntax::Basic),
            Err(Trouble::TooComplex)
        );
        assert_eq!(
            Trouble::NoExpression.says(),
            "an empty expression with none used before it"
        );
        assert_eq!(Trouble::TooComplex.says(), regex::TooComplex.says());
    }

    #[test]
    fn n_writes_what_it_leaves_and_puts_a_text_out_first() {
        assert_eq!(sed("1a A\nn;s/^/+/", "1\n2\n3\n"), "1\nA\n+2\n3\n");
        assert_eq!(quiet("n;n;p", "1\n2\n3\n4\n"), "3\n");
        assert_eq!(
            sed("$!n;d", "1\n2\n3\n"),
            "1\n",
            "the last line is no n's to write"
        );
    }

    #[test]
    fn expressions_match_in_the_case_they_are_written() {
        assert_eq!(quiet("/a/p", "A\na\n"), "a\n");
        assert_eq!(sed("s/a/x/g", "Aa\n"), "Ax\n");
        assert_eq!(quiet("/[a-z]/p", "X\nx\n"), "x\n");
    }

    #[test]
    fn well_known_one_liners_do_what_they_are_known_for() {
        let text = "  lead\ntrail  \n\n\n\nmid   dle\n";
        assert_eq!(sed("s/^[ \t]*//", text), "lead\ntrail  \n\n\n\nmid   dle\n");
        assert_eq!(sed("s/[ \t]*$//", text), "  lead\ntrail\n\n\n\nmid   dle\n");
        assert_eq!(sed("s/  */ /g", "a  b   c\n"), "a b c\n");
        assert_eq!(
            sed("/./,/^$/!d", "\n\na\nb\n\n\n\nc\n\n"),
            "a\nb\n\nc\n\n",
            "runs of blank lines squeezed to one, none at the top"
        );
        assert_eq!(
            sed("/./,$!d", "\n\na\n\nb\n"),
            "a\n\nb\n",
            "blank lines at the top gone"
        );
        assert_eq!(
            sed("$!N;$!D", "1\n2\n3\n4\n"),
            "3\n4\n",
            "the last two lines"
        );
        assert_eq!(sed("$!d", "1\n2\n3\n"), "3\n", "the last line");
        assert_eq!(
            quiet("/c/{g;1!p};h", "a\nb\nc\nd\n"),
            "b\n",
            "the line before a match"
        );
        assert_eq!(
            quiet("/b/{n;p}", "a\nb\nc\nd\n"),
            "c\n",
            "the line after one"
        );
        assert_eq!(
            sed(
                ":a\n/\\\\$/N\ns/\\\\\\n//\nta",
                "one \\\ntwo \\\nthree\nfour\n"
            ),
            "one two three\nfour\n",
            "lines that end with a backslash joined"
        );
        assert_eq!(
            sed("n;n;G", "1\n2\n3\n4\n5\n6\n"),
            "1\n2\n3\n\n4\n5\n6\n\n",
            "a blank line every three"
        );
        assert_eq!(quiet("2,4{=;p}", "a\nb\nc\nd\ne\n"), "2\nb\n3\nc\n4\nd\n");
        assert_eq!(sed("1d;4d", "1\n2\n3\n4\n5\n"), "2\n3\n5\n");
        assert_eq!(
            sed("/^$/d;G", "a\n\nb\n"),
            "a\n\nb\n\n",
            "double spaced with no blank line twice"
        );
        assert_eq!(
            sed(
                "y/abcdefghijklmnopqrstuvwxyz/ABCDEFGHIJKLMNOPQRSTUVWXYZ/",
                "Shout it\n"
            ),
            "SHOUT IT\n"
        );
        assert_eq!(
            sed("s/\\([^,]*\\),\\([^,]*\\)/\\2,\\1/", "last,first,rest\n"),
            "first,last,rest\n",
            "two fields swapped"
        );
        assert_eq!(
            quiet(
                "/start/,/end/{/start/n;/end/!p}",
                "x\nstart\nin 1\nin 2\nend\ny\n"
            ),
            "in 1\nin 2\n",
            "what lies between two markers"
        );
        assert_eq!(
            sed("s/\\(.*\\)foo/\\1bar/", "foo foo foo\n"),
            "foo foo bar\n",
            "the last foo"
        );
        assert_eq!(sed("$!N;s/\\n/ /", "a\nb\nc\n"), "a b\nc\n");
        assert_eq!(
            sed(
                ":a\ns/\\(.*[0-9]\\)\\([0-9]\\{3\\}\\)/\\1,\\2/\nta",
                "1234567\n"
            ),
            "1,234,567\n",
            "a comma every three digits"
        );
    }

    #[test]
    fn more_one_liners_with_answers_known_before_them() {
        assert_eq!(
            quiet(
                "G\ns/\\n/&&/\n/^\\([ -~]*\\n\\).*\\n\\1/d\ns/\\n//\nh\nP",
                "a\nb\na\nc\nb\n"
            ),
            "a\nb\nc\n",
            "lines seen before gone, wherever they were"
        );
        assert_eq!(
            sed(
                "$!N\ns/^\\(.*\\)\\n\\1$/\\1/\nt\nD",
                "a\na\nb\nc\nc\nc\nd\n"
            ),
            "a\nc\n",
            "a line its twin follows, once for each pair: the third c pairs with d"
        );
        assert_eq!(
            sed("N;N;s/\\n/,/g", "1\n2\n3\n4\n5\n6\n7\n"),
            "1,2,3\n4,5,6\n",
            "N with no line left ends it writing nothing"
        );
        assert_eq!(sed("$!N;$!N;s/\\n/,/g", "1\n2\n3\n4\n"), "1,2,3\n4\n");
        assert_eq!(sed("s/\r$//", "dos\r\nunix\n"), "dos\nunix\n");
        assert_eq!(
            sed("s/<[^>]*>//g", "<b>bold</b> and <i>it</i>\n"),
            "bold and it\n"
        );
        assert_eq!(sed("s/ .*//", "first second third\n"), "first\n");
        assert_eq!(sed("s/./&&/g", "abc\n"), "aabbcc\n");
        assert_eq!(
            quiet("/^.\\{4,\\}$/p", "ab\nabcd\nabcdef\n"),
            "abcd\nabcdef\n"
        );
        assert_eq!(quiet("/b/!p", "a\nb\nc\n"), "a\nc\n");
        assert_eq!(sed("3q", "1\n2\n3\n4\n5\n"), "1\n2\n3\n");
        assert_eq!(sed("1!G;h;$!d", ""), "", "nothing to turn round");
    }

    #[test]
    fn quiet_from_its_caller_or_its_script_is_quiet_both_ways() {
        let script = parse("#n\np", Syntax::Basic).unwrap();
        for asked in [false, true] {
            let mut desk = Desk::reading("a\n");
            Sed::new(&script, asked).run(&mut desk).unwrap();
            assert_eq!(desk.out, b"a\n", "asked {asked}");
        }
        let script = parse("p", Syntax::Basic).unwrap();
        let mut desk = Desk::reading("a\n");
        Sed::new(&script, false).run(&mut desk).unwrap();
        assert_eq!(desk.out, b"a\na\n");
    }

    #[test]
    fn last_holds_for_the_line_read_last_whoever_read_it() {
        assert_eq!(quiet("N;$p", "1\n2\n"), "1\n2\n", "after N the line N read");
        assert_eq!(
            quiet("n;$p", "1\n2\n3\n"),
            "",
            "after n the line n read, which was not the last"
        );
        assert_eq!(quiet("$!{n;$p}", "1\n2\n"), "2\n");
        assert_eq!(
            quiet("$p;N", "1\n2\n3\n"),
            "3\n",
            "the third line is read for its own round, and is the last"
        );
    }

    #[test]
    fn bytes_that_are_no_utf8_go_through_s_and_y_as_characters() {
        let run = |script: &str, line: &[u8]| {
            let script = parse(script, Syntax::Basic).unwrap();
            let mut desk = Desk {
                lines: Vec::from([line.to_vec()]),
                ..Desk::default()
            };
            Sed::new(&script, false).run(&mut desk).unwrap();
            desk.out
        };
        assert_eq!(run("s/./X/g", &[0xff, b'a', 0xfe]), b"XXX\n");
        assert_eq!(run("s/a/ä/", &[0xff, b'a']), [0xff, 0xc3, 0xa4, b'\n']);
        assert_eq!(run("s/x*/-/g", &[0xff]), b"-\xff-\n");
        assert_eq!(run("y/a/b/", &[b'a', 0xc3]), [b'b', 0xc3, b'\n']);
    }

    #[test]
    fn d_first_goes_round_again_with_the_hold_space_kept() {
        assert_eq!(
            sed("1{h;d};G;s/\\n/:/;P;D", "k\na\nb\n"),
            "a:k\nb:k\n",
            "with no line feed left D is d"
        );
        assert_eq!(
            quiet("1h;$!N;x;G;x;P;D", "1\n2\n3\n"),
            "1\n2\n3\n",
            "D goes round with what is left, reading no line, and the hold space stays"
        );
    }
}
