//! A word of a sh script expanded, as POSIX's word expansions have it:
//! tilde expansion, parameter expansion, command substitution and
//! arithmetic expansion, field splitting by `IFS`, and quote removal.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::arith::{self, Variables};
use super::lex::{self, Expansion, Part, Word};
use super::pattern::Pattern;
use super::run::{Session, Stop};
use crate::parse::is_name;
use crate::run::Machine;
use crate::{Code, Error, Pos};

/// A character of a word on its way to fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Char {
    pub c: char,
    pub kind: Kind,
}

/// Where a character of a word came from, which says what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// Quoted, so it stands for itself and is never split.
    Quoted,
    /// Written in the word unquoted: a pattern's, but never split.
    Written,
    /// Brought by an expansion unquoted, so it is split by `IFS`.
    Brought,
}

/// The kind of what an expansion brings, quoted or not.
fn brought(quoted: bool) -> Kind {
    if quoted { Kind::Quoted } else { Kind::Brought }
}

/// A word expanded so far: the fields `"$@"` ended, the one it is on, and
/// whether anything quoted stood in it, which keeps a field that is empty.
#[derive(Debug, Default)]
struct Expanded {
    done: Vec<Vec<Char>>,
    current: Vec<Char>,
    quoted: bool,
    /// Whether a `"$@"` with no parameters stood in it.
    nothing: bool,
}

impl Expanded {
    fn push(&mut self, text: &str, kind: Kind) {
        self.current.extend(text.chars().map(|c| Char { c, kind }));
    }

    /// Ends the field it is on, as `"$@"` does between parameters.
    fn split(&mut self) {
        self.done.push(core::mem::take(&mut self.current));
    }

    /// All its fields.
    fn fields(mut self) -> Vec<Vec<Char>> {
        self.split();
        self.done
    }
}

/// What a parameter stands for: its value, the positional parameters as
/// `@` and `*` give them, or nothing when it is not set.
enum Value {
    One(String),
    Each(Vec<String>),
    Unset,
}

/// The value of the special parameter, positional parameter or variable
/// `name`.
fn value(session: &Session, name: &str) -> Value {
    match name {
        "@" | "*" => Value::Each(session.positional().to_vec()),
        "#" => Value::One(session.positional().len().to_string()),
        "?" => Value::One(session.status().to_string()),
        "$" => Value::One(session.process().to_string()),
        "!" => session
            .background()
            .map_or(Value::Unset, |job| Value::One(job.to_string())),
        "-" => Value::One(session.flags()),
        "0" => Value::One(session.zero().into()),
        digits if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            let n: usize = digits.parse().unwrap_or(usize::MAX);
            session
                .positional()
                .get(n.wrapping_sub(1))
                .map_or(Value::Unset, |v| Value::One(v.clone()))
        }
        name => session.variable(name).map_or(Value::Unset, Value::One),
    }
}

/// The operators of `${name...}` that come after the name, the longest
/// first where one starts another.
const OPERATORS: [&str; 12] = [
    ":-", "-", ":=", "=", ":?", "?", ":+", "+", "%%", "%", "##", "#",
];

/// The parameter a braced expansion starts with, and the rest of it.
fn parameter(text: &str) -> Option<(&str, &str)> {
    let first = text.chars().next()?;
    let end = if first.is_ascii_alphabetic() || first == '_' {
        text.find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(text.len())
    } else if first.is_ascii_digit() {
        text.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len())
    } else if "@*#?-$!".contains(first) {
        1
    } else {
        return None;
    };
    Some(text.split_at(end))
}

/// What stops a script with the error `code` at `at`.
pub(super) fn failed(at: Pos, code: Code) -> Stop {
    Stop::Error(Error { at, code })
}

/// The parts of a text inside an expansion at `at`, read as a word.
fn inner(text: &str, at: Pos) -> Result<Vec<Part>, Stop> {
    lex::text(text).map_err(|e| failed(at, e.code))
}

/// The variables of a session, for arithmetic to read and set.
struct Arithmetic<'a>(&'a mut Session);

impl Variables for Arithmetic<'_> {
    fn get(&self, name: &str) -> Option<String> {
        self.0.variable(name)
    }

    fn set(&mut self, name: &str, value: String) {
        // A read-only variable keeps its value: arithmetic has no way to
        // say it could not set one, and POSIX leaves what happens open.
        let _ = self.0.assign(name, value);
    }
}

impl Session {
    /// `word` expanded into fields, split by `IFS` where an expansion
    /// brought what is not quoted, as a command's words are.
    pub(super) fn fields(
        &mut self,
        word: &Word,
        machine: &mut impl Machine,
    ) -> Result<Vec<Vec<Char>>, Stop> {
        let mut expanded = Expanded::default();
        self.parts(&word.parts, word.at, false, &mut expanded, machine)?;
        let (quoted, nothing) = (expanded.quoted, expanded.nothing);
        let fields = expanded.fields();
        if nothing && fields.iter().all(Vec::is_empty) {
            return Ok(Vec::new());
        }
        let ifs = self.ifs();
        let mut out = Vec::new();
        for field in fields {
            let before = out.len();
            out.extend(split(&field, &ifs));
            if out.len() == before && quoted {
                out.push(Vec::new());
            }
        }
        Ok(out)
    }

    /// `word` expanded into one text, not split, as an assignment's value,
    /// a redirection's path and a case's word are; `"$@"` joins its
    /// parameters with spaces there.
    pub(super) fn text(&mut self, word: &Word, machine: &mut impl Machine) -> Result<String, Stop> {
        let chars = self.characters(&word.parts, word.at, false, machine)?;
        Ok(chars.iter().map(|c| c.c).collect())
    }

    /// `parts` expanded into one run of characters, as [`Session::text`]
    /// takes them, every one quoted when `quoted`.
    pub(super) fn characters(
        &mut self,
        parts: &[Part],
        at: Pos,
        quoted: bool,
        machine: &mut impl Machine,
    ) -> Result<Vec<Char>, Stop> {
        let mut expanded = Expanded::default();
        self.parts(parts, at, quoted, &mut expanded, machine)?;
        let mut out = Vec::new();
        for (i, field) in expanded.fields().into_iter().enumerate() {
            if i > 0 {
                out.push(Char {
                    c: ' ',
                    kind: Kind::Quoted,
                });
            }
            out.extend(field);
        }
        Ok(out)
    }

    /// `word` expanded into a pattern, its quoted characters standing for
    /// themselves, as `case` takes it.
    pub(super) fn pattern(
        &mut self,
        word: &Word,
        machine: &mut impl Machine,
    ) -> Result<Pattern, Stop> {
        let chars = self.characters(&word.parts, word.at, false, machine)?;
        Ok(as_pattern(&chars))
    }

    /// What `IFS` splits at: its value, or space, tab and newline when it
    /// is not set.
    pub(super) fn ifs(&self) -> String {
        self.variable("IFS").unwrap_or_else(|| " \t\n".into())
    }

    /// Expands `parts`, all of them quoted when `quoted`, onto `out`.
    fn parts(
        &mut self,
        parts: &[Part],
        at: Pos,
        quoted: bool,
        out: &mut Expanded,
        machine: &mut impl Machine,
    ) -> Result<(), Stop> {
        for (i, part) in parts.iter().enumerate() {
            match part {
                Part::Text { text, quoted: q } => {
                    let q = *q || quoted;
                    out.quoted |= q;
                    let home = if i == 0 && !q {
                        self.tilde(text, parts.len() == 1)
                    } else {
                        None
                    };
                    match home {
                        Some((home, rest)) => {
                            out.push(&home, Kind::Quoted);
                            out.push(rest, Kind::Written);
                        }
                        None if q => out.push(text, Kind::Quoted),
                        None => out.push(text, Kind::Written),
                    }
                }
                Part::Expand {
                    expansion,
                    quoted: q,
                } => {
                    let q = *q || quoted;
                    out.quoted |= q;
                    self.expansion(expansion, at, q, out, machine)?;
                }
            }
        }
        Ok(())
    }

    /// The home `text`, the first part of a word, starts with a tilde for,
    /// and the rest of it: `~` alone or before a `/` stands for `$HOME`.
    /// Meridian keeps no homes for other users, so `~name` stays as it is.
    fn tilde<'t>(&self, text: &'t str, alone: bool) -> Option<(String, &'t str)> {
        let rest = text.strip_prefix('~')?;
        if !(rest.is_empty() && alone || rest.starts_with('/')) {
            return None;
        }
        Some((self.variable("HOME").unwrap_or_default(), rest))
    }

    /// Expands one expansion onto `out`.
    fn expansion(
        &mut self,
        expansion: &Expansion,
        at: Pos,
        quoted: bool,
        out: &mut Expanded,
        machine: &mut impl Machine,
    ) -> Result<(), Stop> {
        match expansion {
            Expansion::Parameter(name) => {
                let found = value(self, name);
                self.put(name, found, at, quoted, out)
            }
            Expansion::Braced(text) => self.braced(text, at, quoted, out, machine),
            Expansion::Command(text) => {
                let said = self.substituted(text, at, machine)?;
                out.push(&said, brought(quoted));
                Ok(())
            }
            Expansion::Arithmetic(text) => {
                let n = self.arithmetic(text, at, machine)?;
                out.push(&n.to_string(), brought(quoted));
                Ok(())
            }
        }
    }

    /// Puts what the parameter `name` stands for onto `out`: an error for
    /// one that is not set under `set -u`, but for `@` and `*`.
    fn put(
        &mut self,
        name: &str,
        found: Value,
        at: Pos,
        quoted: bool,
        out: &mut Expanded,
    ) -> Result<(), Stop> {
        match found {
            Value::One(text) => out.push(&text, brought(quoted)),
            Value::Unset if self.flag('u') => return Err(failed(at, Code::UnsetParameter)),
            Value::Unset => {}
            // Unquoted, `$*` is as `$@`: each parameter a field of its own,
            // split after.
            Value::Each(each) if !quoted || name == "@" => {
                if each.is_empty() && quoted {
                    out.nothing = true;
                }
                for (i, one) in each.iter().enumerate() {
                    if i > 0 {
                        out.split();
                    }
                    out.push(one, brought(quoted));
                }
            }
            Value::Each(each) => {
                let between: String = self.ifs().chars().take(1).collect();
                out.push(&each.join(&between), Kind::Quoted);
            }
        }
        Ok(())
    }

    /// Expands `${...}`, which holds `text`.
    fn braced(
        &mut self,
        text: &str,
        at: Pos,
        quoted: bool,
        out: &mut Expanded,
        machine: &mut impl Machine,
    ) -> Result<(), Stop> {
        if text == "#" {
            return self.put("#", value(self, "#"), at, quoted, out);
        }
        if let Some(name) = text.strip_prefix('#')
            && let Some((name, "")) = parameter(name)
        {
            let length = match value(self, name) {
                Value::One(v) => v.chars().count(),
                Value::Each(each) => each.len(),
                Value::Unset if self.flag('u') => return Err(failed(at, Code::UnsetParameter)),
                Value::Unset => 0,
            };
            out.push(&length.to_string(), brought(quoted));
            return Ok(());
        }
        let Some((name, rest)) = parameter(text) else {
            return Err(failed(at, Code::BadSubstitution));
        };
        if rest.is_empty() {
            return self.put(name, value(self, name), at, quoted, out);
        }
        let Some(op) = OPERATORS.iter().find(|op| rest.starts_with(**op)) else {
            return Err(failed(at, Code::BadSubstitution));
        };
        let word = &rest[op.len()..];
        let found = value(self, name);
        // With a colon, a parameter that is empty is wanting as well.
        let wanting = match &found {
            Value::One(v) => op.starts_with(':') && v.is_empty(),
            Value::Each(each) => op.starts_with(':') && each.is_empty(),
            Value::Unset => true,
        };
        match *op {
            ":-" | "-" if wanting => self.word(word, at, quoted, out, machine),
            ":=" | "=" if wanting => {
                if !is_name(name) {
                    return Err(failed(at, Code::BadSubstitution));
                }
                let given = self.inner_text(word, at, machine)?;
                if self.assign(name, given.clone()).is_err() {
                    return Err(failed(at, Code::ReadOnly));
                }
                out.push(&given, brought(quoted));
                Ok(())
            }
            ":?" | "?" if wanting => {
                let said = self.inner_text(word, at, machine)?;
                let said = match (said.is_empty(), op.starts_with(':')) {
                    (false, _) => said,
                    (true, true) => String::from("empty or not set"),
                    (true, false) => String::from("not set"),
                };
                let _ = machine.err(format!("{name}: {said}\n").as_bytes());
                Err(Stop::Said(1))
            }
            ":+" | "+" if !wanting => self.word(word, at, quoted, out, machine),
            ":+" | "+" => Ok(()),
            "%" | "%%" | "#" | "##" => {
                let Value::One(v) = found else {
                    return self.put(name, found, at, quoted, out);
                };
                let parts = inner(word, at)?;
                let chars = self.characters(&parts, at, false, machine)?;
                let pattern = as_pattern(&chars);
                let longest = op.len() == 2;
                let kept = if op.starts_with('%') {
                    pattern.without_end(&v, longest)
                } else {
                    pattern.without_start(&v, longest)
                };
                out.push(&kept, brought(quoted));
                Ok(())
            }
            _ => self.put(name, found, at, quoted, out),
        }
    }

    /// Expands the word of `${name:-word}` and its kin onto `out`.
    fn word(
        &mut self,
        word: &str,
        at: Pos,
        quoted: bool,
        out: &mut Expanded,
        machine: &mut impl Machine,
    ) -> Result<(), Stop> {
        let parts = inner(word, at)?;
        // Inside double quotes, all the word brings is quoted; outside, it
        // is split as what the parameter would have brought.
        let mut brought = Expanded::default();
        self.parts(&parts, at, quoted, &mut brought, machine)?;
        out.quoted |= brought.quoted;
        for (i, field) in brought.fields().into_iter().enumerate() {
            if i > 0 {
                out.split();
            }
            out.current.extend(field.into_iter().map(|c| match c.kind {
                Kind::Written => Char {
                    kind: Kind::Brought,
                    ..c
                },
                _ => c,
            }));
        }
        Ok(())
    }

    /// The word of `${name:=word}` or `${name:?word}`, as one text.
    fn inner_text(
        &mut self,
        word: &str,
        at: Pos,
        machine: &mut impl Machine,
    ) -> Result<String, Stop> {
        let parts = inner(word, at)?;
        let chars = self.characters(&parts, at, false, machine)?;
        Ok(chars.iter().map(|c| c.c).collect())
    }

    /// What `$((text))` works out to.
    fn arithmetic(&mut self, text: &str, at: Pos, machine: &mut impl Machine) -> Result<i64, Stop> {
        let parts = inner(text, at)?;
        let expression: String = self
            .characters(&parts, at, true, machine)?
            .iter()
            .map(|c| c.c)
            .collect();
        arith::evaluate(&expression, &mut Arithmetic(self)).map_err(|code| failed(at, code))
    }
}

/// `chars` as a pattern, its quoted characters standing for themselves.
pub(super) fn as_pattern(chars: &[Char]) -> Pattern {
    let written: Vec<(char, bool)> = chars
        .iter()
        .map(|c| (c.c, c.kind == Kind::Quoted))
        .collect();
    Pattern::new(&written)
}

/// What `read` gives `count` names from `line`: its fields split at the
/// characters of `ifs` as [`split`] splits them, and for the last name all
/// that is left of the line but the white space of `ifs` at its ends;
/// names past the fields get nothing.
pub(super) fn read_values(line: &[Char], ifs: &str, count: usize) -> Vec<String> {
    let breaks = |c: &Char| c.kind == Kind::Brought && ifs.contains(c.c);
    let white = |c: &Char| breaks(c) && matches!(c.c, ' ' | '\t' | '\n');
    let skip_white = |mut i: usize| {
        while line.get(i).is_some_and(white) {
            i += 1;
        }
        i
    };
    let mut values = Vec::new();
    let mut i = skip_white(0);
    // Past the end of the line each name before the last gets nothing.
    while values.len() + 1 < count {
        let start = i;
        while line.get(i).is_some_and(|c| !breaks(c)) {
            i += 1;
        }
        values.push(line[start..i].iter().map(|c| c.c).collect());
        // A break is white space, at most one other character of `ifs`,
        // and white space again.
        i = skip_white(i);
        if line.get(i).is_some_and(|c| breaks(c) && !white(c)) {
            i = skip_white(i + 1);
        }
    }
    let mut end = line.len();
    while end > i && white(&line[end - 1]) {
        end -= 1;
    }
    values.push(line[i.min(end)..end].iter().map(|c| c.c).collect());
    values.resize(count, String::new());
    values
}

/// `field` split at the characters of `ifs` an expansion brought
/// unquoted, as POSIX splits: a run of white space in `ifs` is one break
/// and none at the ends, and each other character of `ifs` breaks once,
/// so two side by side leave an empty field between them.
pub(super) fn split(field: &[Char], ifs: &str) -> Vec<Vec<Char>> {
    let breaks = |c: &Char| c.kind == Kind::Brought && ifs.contains(c.c);
    let white = |c: &Char| breaks(c) && matches!(c.c, ' ' | '\t' | '\n');
    let mut fields = Vec::new();
    let mut current = Vec::new();
    let mut started = false;
    let mut i = 0;
    while i < field.len() {
        if !breaks(&field[i]) {
            current.push(field[i]);
            started = true;
            i += 1;
            continue;
        }
        while i < field.len() && white(&field[i]) {
            i += 1;
        }
        let hard = i < field.len() && breaks(&field[i]) && !white(&field[i]);
        if hard {
            i += 1;
            while i < field.len() && white(&field[i]) {
                i += 1;
            }
        }
        if started || hard {
            fields.push(core::mem::take(&mut current));
        }
        started = false;
    }
    if started {
        fields.push(current);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh::desk::{out, ran, said, stopped};
    use crate::sh::run::Flow;

    /// The fields `script`'s words make, one in brackets for each, as
    /// printf shows them, with `positional` as the parameters.
    fn fields_of(script: &str, positional: &[&str]) -> String {
        said(&format!("{script}; echo"), positional)
            .0
            .trim_end()
            .into()
    }

    #[test]
    fn positional_parameters_count_from_1() {
        let ten = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"];
        assert_eq!(
            said("echo $1 ${10} $10 ${11} ${011}", &ten).0,
            "a j a0 k k\n"
        );
        assert_eq!(said("echo \"[$3]\" \"[${12}]\"", &["x"]).0, "[] []\n");
        assert_eq!(said("echo $#", &ten).0, "11\n");
        assert_eq!(
            said("echo \"[${00}]\"", &ten).0,
            "[]\n",
            "there is no parameter 0 but $0"
        );
    }

    #[test]
    fn special_parameters_say_the_session() {
        assert_eq!(out("echo $0 $$"), "script 42\n");
        assert_eq!(out("false; echo $?; echo $?"), "1\n0\n");
        assert_eq!(out("set -u; echo $-"), "u\n");
        assert_eq!(out("echo \"[$!]\""), "[]\n");
        assert_eq!(out("echo ${0} ${$} ${?} ${-}"), "script 42 0\n");
    }

    #[test]
    fn quoted_at_makes_a_field_of_each_parameter() {
        let p = ["a b", "", "c"];
        assert_eq!(fields_of("printf '[%s]' \"$@\"", &p), "[a b][][c]");
        assert_eq!(fields_of("printf '[%s]' \"x$@y\"", &p), "[xa b][][cy]");
        assert_eq!(
            fields_of("printf '[%s]' \"$@\"", &[]),
            "[]",
            "no field at all"
        );
        assert_eq!(
            out("n() { echo $#; }; n \"$@\"; n \"x$@\"; n \"$@\" ''"),
            "0\n1\n1\n"
        );
        assert_eq!(fields_of("printf '[%s]' \"${@}\"", &p), "[a b][][c]");
    }

    #[test]
    fn unquoted_at_and_star_make_a_field_of_each_parameter_then_split() {
        let p = ["a b", "", "c"];
        assert_eq!(fields_of("printf '[%s]' $@", &p), "[a][b][c]");
        assert_eq!(fields_of("printf '[%s]' $*", &p), "[a][b][c]");
        assert_eq!(fields_of("IFS=:; printf '[%s]' $*", &p), "[a b][c]");
        assert_eq!(fields_of("IFS=:; printf '[%s]' $@", &p), "[a b][c]");
        assert_eq!(fields_of("IFS=; printf '[%s]' $*", &p), "[a b][c]");
        assert_eq!(fields_of("printf '[%s]' x$@y", &["1", "2"]), "[x1][2y]");
        assert_eq!(out("n() { echo $#; }; n $@; n $*"), "0\n0\n");
    }

    #[test]
    fn quoted_star_joins_the_parameters_with_ifs_s_first_character() {
        let p = ["a b", "", "c"];
        assert_eq!(fields_of("printf '[%s]' \"$*\"", &p), "[a b  c]");
        assert_eq!(fields_of("IFS=:-; printf '[%s]' \"$*\"", &p), "[a b::c]");
        assert_eq!(fields_of("IFS=; printf '[%s]' \"$*\"", &p), "[a bc]");
        assert_eq!(fields_of("unset IFS; printf '[%s]' \"$*\"", &p), "[a b  c]");
        assert_eq!(
            fields_of("printf '[%s]' \"$*\"", &[]),
            "[]",
            "one empty field"
        );
        assert_eq!(
            fields_of("IFS=é; printf '[%s]' \"$*\"", &["x", "y"]),
            "[xéy]"
        );
    }

    #[test]
    fn at_and_star_in_a_text_join_with_spaces() {
        assert_eq!(said("a=$@; echo \"[$a]\"", &["x", "y"]).0, "[x y]\n");
        assert_eq!(said("a=\"$@\"; echo \"[$a]\"", &["x", "y"]).0, "[x y]\n");
        assert_eq!(
            said("IFS=:; a=\"$*\"; echo \"[$a]\"", &["x", "y"]).0,
            "[x:y]\n"
        );
        assert_eq!(
            said("case \"$@\" in 'x y') echo whole;; esac", &["x", "y"]).0,
            "whole\n"
        );
    }

    #[test]
    fn what_an_expansion_brings_unquoted_is_split_by_ifs() {
        assert_eq!(fields_of("v=' a  b '; printf '[%s]' $v", &[]), "[a][b]");
        assert_eq!(
            fields_of("v=' a  b '; printf '[%s]' \"$v\"", &[]),
            "[ a  b ]"
        );
        assert_eq!(
            fields_of("IFS=:; v=':a::b:'; printf '[%s]' $v", &[]),
            "[][a][][b]"
        );
        assert_eq!(
            fields_of("IFS=' :'; v=' a : b '; printf '[%s]' $v", &[]),
            "[a][b]"
        );
        assert_eq!(
            fields_of("IFS=' :'; v='a :: b'; printf '[%s]' $v", &[]),
            "[a][][b]"
        );
        assert_eq!(
            fields_of("IFS=; v=' a  b '; printf '[%s]' $v", &[]),
            "[ a  b ]"
        );
        assert_eq!(
            fields_of("unset IFS; v=' a  b '; printf '[%s]' $v", &[]),
            "[a][b]"
        );
        assert_eq!(
            fields_of("IFS=a; v=banana; printf '[%s]' $v", &[]),
            "[b][n][n]"
        );
        assert_eq!(
            fields_of("v=\"a\tb\nc\"; printf '[%s]' $v", &[]),
            "[a][b][c]"
        );
    }

    #[test]
    fn what_is_written_is_never_split() {
        assert_eq!(fields_of("IFS=a; printf '[%s]' banana", &[]), "[banana]");
        assert_eq!(
            fields_of("IFS=a; printf '[%s]' b${v}anana", &[]),
            "[banana]"
        );
        assert_eq!(
            fields_of("v='a b'; printf '[%s]' x$v\"y z\"", &[]),
            "[xa][by z]"
        );
        assert_eq!(fields_of("v=' '; printf '[%s]' a${v}b", &[]), "[a][b]");
        assert_eq!(
            fields_of("v=' '; printf '[%s]' \"a\"$v\"b\"", &[]),
            "[a][b]"
        );
    }

    #[test]
    fn an_empty_expansion_unquoted_makes_no_field() {
        let n = "n() { echo $#; }; ";
        assert_eq!(
            out(&format!("{n}n $e; n \"$e\"; n a\"$e\"; n ''; n $e$e")),
            "0\n1\n1\n1\n0\n"
        );
        assert_eq!(out(&format!("{n}e=' '; n $e; n \"\"$e")), "0\n1\n");
        assert_eq!(out(&format!("{n}n ${{e:-}} ${{e-}}")), "0\n");
    }

    #[test]
    fn a_tilde_at_a_word_s_start_is_home() {
        let home = "HOME=/home/me; ";
        assert_eq!(
            out(&format!("{home}echo ~ ~/x a~ \"~\" ~me \\~ '~' ~:")),
            "/home/me /home/me/x a~ ~ ~me ~ ~ ~:\n"
        );
        assert_eq!(out(&format!("{home}x=~/a; echo $x")), "/home/me/a\n");
        assert_eq!(
            fields_of("HOME='/a b'; printf '[%s]' ~ ~/c", &[]),
            "[/a b][/a b/c]"
        );
        assert_eq!(out("echo \"[\"~\"]\" ~$HOME"), "[~] ~\n");
        assert_eq!(
            out(&format!("{home}echo ${{u:-~}} ${{u:-~/x}}")),
            "/home/me /home/me/x\n"
        );
        assert_eq!(out("unset HOME; echo \"[$(echo ~/x)]\""), "[/x]\n");
    }

    #[test]
    fn a_length_counts_characters() {
        assert_eq!(out("v=héllo; echo ${#v}"), "5\n");
        assert_eq!(out("echo ${#u} ${#IFS}"), "0 3\n");
        assert_eq!(
            said("echo ${#@} ${#*} ${#} ${#1}", &["ab", "c"]).0,
            "2 2 2 2\n"
        );
        assert_eq!(said("echo ${#-} ${#?}", &[]).0, "0 1\n");
        assert_eq!(said("set -eu; echo ${#-}", &[]).0, "2\n");
    }

    #[test]
    fn a_default_stands_in_as_its_operator_says() {
        let set = "s=x; e=; ";
        for (expansion, want) in [
            ("${s:-w}", "x"),
            ("${e:-w}", "w"),
            ("${u:-w}", "w"),
            ("${s-w}", "x"),
            ("${e-w}", ""),
            ("${u-w}", "w"),
            ("${s:+w}", "w"),
            ("${e:+w}", ""),
            ("${u:+w}", ""),
            ("${s+w}", "w"),
            ("${e+w}", "w"),
            ("${u+w}", ""),
            ("${s:=w}", "x"),
            ("${s=w}", "x"),
            ("${e=w}", ""),
        ] {
            assert_eq!(
                out(&format!("{set}echo \"[{expansion}]\"")),
                format!("[{want}]\n"),
                "{expansion}"
            );
        }
    }

    #[test]
    fn a_default_of_at_holds_when_there_are_none() {
        assert_eq!(said("echo ${@:-none} ${@-unset}", &[]).0, "none\n");
        assert_eq!(said("echo ${@:-none} ${*:+some}", &["a"]).0, "a some\n");
        assert_eq!(said("echo ${1:-one} ${2-two}", &["", "b"]).0, "one b\n");
    }

    #[test]
    fn assigning_a_default_sets_the_variable() {
        assert_eq!(out("echo ${u:=w}; echo $u"), "w\nw\n");
        assert_eq!(out("e=; echo \"[${e=w}]\"; echo \"[$e]\""), "[]\n[]\n");
        assert_eq!(out("e=; echo ${e:=w}; echo $e"), "w\nw\n");
        assert_eq!(out("echo ${u=a b}; printf '[%s]' \"$u\""), "a b\n[a b]");
        assert_eq!(stopped("echo ${1:=x}"), (1, 6, Code::BadSubstitution));
        assert_eq!(stopped("echo ${@:=x}"), (1, 6, Code::BadSubstitution));
        assert_eq!(
            said("echo ${@=x}", &["a"]).0,
            "a\n",
            "set, so nothing is assigned"
        );
        assert_eq!(
            stopped("readonly r=; echo ${r:=x}"),
            (1, 19, Code::ReadOnly)
        );
        assert_eq!(out("readonly r=1; echo ${r:=x}"), "1\n");
    }

    #[test]
    fn a_question_mark_stops_the_script_for_a_parameter_not_there() {
        for (script, said_) in [
            ("echo ${u:?}", "u: empty or not set\n"),
            ("echo ${u?}", "u: not set\n"),
            ("e=; echo ${e:?}", "e: empty or not set\n"),
            ("s=x; echo ${u:?no $s here}", "u: no x here\n"),
            ("echo ${1?}", "1: not set\n"),
        ] {
            let (ended, _, desk) = ran(script, &[]);
            assert_eq!(ended, Err(Stop::Said(1)), "{script}");
            assert_eq!(
                (desk.out, desk.err),
                (Vec::new(), said_.as_bytes().to_vec()),
                "{script}"
            );
        }
        assert_eq!(
            out("e=; echo \"[${e?}]\""),
            "[]\n",
            "only one not set is wanting"
        );
    }

    #[test]
    fn a_pattern_takes_off_the_shortest_or_the_longest_match() {
        let v = "v=path/to/file.tar.gz; ";
        for (expansion, want) in [
            ("${v%.*}", "path/to/file.tar"),
            ("${v%%.*}", "path/to/file"),
            ("${v#*/}", "to/file.tar.gz"),
            ("${v##*/}", "file.tar.gz"),
            ("${v%x}", "path/to/file.tar.gz"),
            ("${v#path}", "/to/file.tar.gz"),
            ("${v%%*}", ""),
            ("${v%*}", "path/to/file.tar.gz"),
            ("${v#}", "path/to/file.tar.gz"),
            ("${v##p?t?}", "/to/file.tar.gz"),
            ("${v%[a-z][a-z]}", "path/to/file.tar."),
        ] {
            assert_eq!(
                out(&format!("{v}echo \"{expansion}\"")),
                format!("{want}\n"),
                "{expansion}"
            );
        }
    }

    #[test]
    fn a_pattern_s_quoted_characters_stand_for_themselves() {
        assert_eq!(
            out("s='a*b*'; echo \"${s%\"*\"}\" ${s%\\*} ${s%'*'}"),
            "a*b a*b a*b\n"
        );
        assert_eq!(out("s='a*b*'; echo \"${s%%*}\""), "\n");
        assert_eq!(out("p='*.'; v=a.b; echo ${v#$p} ${v#\"$p\"}"), "b a.b\n");
        assert_eq!(out("v=aXb; echo ${v%X*} ${v%\"X\"*}"), "a a\n");
    }

    #[test]
    fn a_pattern_on_a_parameter_not_there_takes_nothing() {
        assert_eq!(out("echo \"[${u%x}]\" \"[${u#x}]\""), "[] []\n");
        assert_eq!(said("echo ${@%b}", &["ab", "b"]).0, "ab b\n");
        assert_eq!(
            stopped("set -u; echo ${u%x}"),
            (1, 14, Code::UnsetParameter)
        );
    }

    #[test]
    fn a_default_is_split_unless_quoted() {
        assert_eq!(
            fields_of(
                "v='a b'; printf '[%s]' \"${u:-$v}\" ${u:-$v} ${u:-\"$v\"}",
                &[]
            ),
            "[a b][a][b][a b]"
        );
        assert_eq!(
            fields_of("printf '[%s]' ${u:-x y} \"${u:-x y}\"", &[]),
            "[x][y][x y]"
        );
        assert_eq!(
            fields_of("printf '[%s]' ${u:-\"$@\"}", &["a b", "c"]),
            "[a b][c]"
        );
        assert_eq!(fields_of("printf '[%s]' ${u:-}x", &[]), "[x]");
        assert_eq!(fields_of("printf '[%s]' ${u:-''}", &[]), "[]");
        assert_eq!(out("n() { echo $#; }; n ${u:-''} ${u:-}"), "1\n");
    }

    #[test]
    fn a_word_inside_braces_may_hold_other_expansions() {
        assert_eq!(out("a=1; echo ${u:-$a$((a + 1))$(echo 3)}"), "123\n");
        assert_eq!(out("echo ${u:-${w:-deep}}"), "deep\n");
        assert_eq!(out("echo ${u:-a  b}"), "a b\n");
        assert_eq!(out("echo \"${u:-a  b}\""), "a  b\n");
    }

    #[test]
    fn a_bad_substitution_stops_the_script_where_it_is() {
        assert_eq!(stopped("echo ${a;b}"), (1, 6, Code::BadSubstitution));
        assert_eq!(stopped("echo ${a!}"), (1, 6, Code::BadSubstitution));
        assert_eq!(stopped("echo ${a:}"), (1, 6, Code::BadSubstitution));
        assert_eq!(stopped("echo x${!a}"), (1, 6, Code::BadSubstitution));
        assert_eq!(stopped("echo ${a b}"), (1, 6, Code::BadSubstitution));
        assert_eq!(stopped("echo ${.}"), (1, 6, Code::BadSubstitution));
        assert_eq!(stopped("echo ${}"), (1, 6, Code::BadSubstitution));
    }

    #[test]
    fn set_u_stops_the_script_at_a_parameter_not_set() {
        assert_eq!(stopped("set -u; echo $u"), (1, 14, Code::UnsetParameter));
        assert_eq!(stopped("set -u; echo ${u}"), (1, 14, Code::UnsetParameter));
        assert_eq!(stopped("set -u; echo ${#u}"), (1, 14, Code::UnsetParameter));
        assert_eq!(stopped("set -u; echo $1"), (1, 14, Code::UnsetParameter));
        assert_eq!(stopped("set -u; echo $!"), (1, 14, Code::UnsetParameter));
        assert_eq!(
            stopped("set -u\necho \"a$u\""),
            (2, 6, Code::UnsetParameter)
        );
        assert_eq!(
            out("set -u; e=; echo \"[$e]\" ${u-x} ${u:-y} \"[$@]\" \"[$*]\" $# ${u+z}"),
            "[] x y [] [] 0\n"
        );
        assert_eq!(out("set -u; set +u; echo \"[$u]\""), "[]\n");
    }

    #[test]
    fn a_command_substitution_is_what_its_commands_write() {
        assert_eq!(out("echo $(echo a)b"), "ab\n");
        assert_eq!(
            fields_of("printf '[%s]' \"$(printf 'a\\n\\n')\"", &[]),
            "[a]"
        );
        assert_eq!(
            fields_of("printf '[%s]' \"$(printf 'a\\nb\\n')\"", &[]),
            "[a\nb]"
        );
        assert_eq!(
            fields_of("printf '[%s]' $(echo a b) \"$(echo c d)\"", &[]),
            "[a][b][c d]"
        );
        assert_eq!(fields_of("printf '[%s]' \"$(printf '\\n\\n')\"", &[]), "[]");
        assert_eq!(
            out("n() { echo $#; }; n $(printf '\\n\\n'); n \"$(printf '\\n\\n')\""),
            "0\n1\n"
        );
        assert_eq!(out("echo `echo old`"), "old\n");
    }

    #[test]
    fn arithmetic_works_out_a_number() {
        assert_eq!(out("echo $((1 + 2 * 3)) $(( (1 + 2) * 3 ))"), "7 9\n");
        assert_eq!(out("a=4; echo $((a * 2)) $(($a + 1))"), "8 5\n");
        assert_eq!(out("echo $((b = 5)) $b"), "5 5\n");
        assert_eq!(out("i=1; : $((i += 1)); echo $i"), "2\n");
        assert_eq!(
            out("x='1 + 2'; echo $(($x * 2))"),
            "5\n",
            "the text comes first, then the sum"
        );
        assert_eq!(said("echo $(($1 + $#))", &["40"]).0, "41\n");
        assert_eq!(out("echo $((\"1\" + '2'))"), "3\n");
        assert_eq!(out("echo $(( $(echo 6) * 7 ))"), "42\n");
    }

    #[test]
    fn arithmetic_s_errors_are_said_at_its_word() {
        assert_eq!(stopped("echo $((1 / 0))"), (1, 6, Code::DivisionByZero));
        assert_eq!(stopped("a=x; echo $((a))"), (1, 11, Code::NotANumber));
        assert_eq!(stopped("echo a$((1 +))"), (1, 6, Code::BadArithmetic));
        assert_eq!(stopped("echo $((2 2))"), (1, 6, Code::BadArithmetic));
        assert_eq!(out("echo $(($u))"), "0\n", "nothing works out to 0");
    }

    #[test]
    fn arithmetic_s_result_is_split_as_an_expansion_s_is() {
        assert_eq!(
            fields_of("IFS=0; printf '[%s]' $((100)) \"$((100))\"", &[]),
            "[1][][100]"
        );
    }

    #[test]
    fn a_parameter_is_read_up_to_where_its_name_ends() {
        assert_eq!(parameter("abc_1-x"), Some(("abc_1", "-x")));
        assert_eq!(parameter("_"), Some(("_", "")));
        assert_eq!(parameter("12:-x"), Some(("12", ":-x")));
        assert_eq!(parameter("1a"), Some(("1", "a")));
        assert_eq!(parameter("a1b"), Some(("a1b", "")));
        for special in ["@", "*", "#", "?", "-", "$", "!"] {
            assert_eq!(parameter(&format!("{special}x")), Some((special, "x")));
        }
        assert_eq!(parameter(""), None);
        assert_eq!(parameter(".a"), None);
        assert_eq!(parameter("éa"), None);
    }

    /// `text` as the characters of a field: those in capitals, spaces
    /// and colons brought by an expansion, the others written; none of
    /// them quoted.
    fn field(text: &str) -> Vec<Char> {
        text.chars()
            .map(|c| Char {
                c: c.to_ascii_lowercase(),
                kind: if c.is_ascii_uppercase() || c == ' ' || c == ':' {
                    Kind::Brought
                } else {
                    Kind::Written
                },
            })
            .collect()
    }

    fn split_into(text: &str, ifs: &str) -> Vec<String> {
        split(&field(text), ifs)
            .iter()
            .map(|f| f.iter().map(|c| c.c).collect())
            .collect()
    }

    #[test]
    fn splitting_breaks_only_at_what_an_expansion_brought() {
        assert_eq!(split_into("A B", " "), ["a", "b"]);
        assert_eq!(split_into("A:B", ":"), ["a", "b"]);
        assert_eq!(split_into("aAa", "a"), ["a", "a"]);
        assert_eq!(split_into("ab", "ab"), ["ab"]);
        assert_eq!(split_into("", " "), Vec::<String>::new());
        assert_eq!(split_into("   ", " "), Vec::<String>::new());
        assert_eq!(split_into(":", ":"), [""]);
        assert_eq!(split_into("::", ":"), ["", ""]);
        assert_eq!(split_into(" : ", " :"), [""]);
        assert_eq!(split_into("A: :B", " :"), ["a", "", "b"]);
        assert_eq!(split_into("A :B :", " :"), ["a", "b"]);
    }

    #[test]
    fn splitting_leaves_quoted_characters_whole() {
        let mut quoted = field("A B");
        quoted[1].kind = Kind::Quoted;
        assert_eq!(split(&quoted, " ").len(), 1);
        let mut tab = field("A\tB");
        tab[1].kind = Kind::Brought;
        assert_eq!(split(&tab, " \t").len(), 2);
        assert_eq!(split(&tab, " ").len(), 1, "only what IFS holds breaks");
    }

    #[test]
    fn expanding_leaves_the_session_going() {
        let (ended, session, _) = ran("v=${u:-x}; w=$((2 + 3))", &[]);
        assert_eq!(ended, Ok(Flow::Next));
        assert_eq!(session.variable("v").as_deref(), Some("x"));
        assert_eq!(session.variable("w").as_deref(), Some("5"));
    }

    #[test]
    fn what_a_tilde_stands_for_is_quoted_and_what_follows_it_is_not() {
        let home = "HOME='/a*'; ";
        assert_eq!(
            out(&format!(
                "{home}case /ab in ~) echo glob;; *) echo literal;; esac"
            )),
            "literal\n"
        );
        assert_eq!(
            out("HOME=/h; case /h/abc in ~/*) echo glob;; *) echo literal;; esac"),
            "glob\n"
        );
        assert_eq!(
            fields_of("HOME=/h; IFS=/; printf '[%s]' ~/a/b", &[]),
            "[/h/a/b]"
        );
        assert_eq!(fields_of("HOME='/h i'; printf '[%s]' ~", &[]), "[/h i]");
    }

    #[test]
    fn what_an_operator_brings_unquoted_is_split() {
        assert_eq!(
            fields_of("IFS=1; v=abcdefghijk; printf '[%s]' ${#v}", &[]),
            "[][]"
        );
        assert_eq!(
            fields_of("IFS=1; v=abcdefghijk; printf '[%s]' \"${#v}\"", &[]),
            "[11]"
        );
        assert_eq!(
            fields_of("printf '[%s]' ${u:=a b} \"${w:=c d}\"", &[]),
            "[a][b][c d]"
        );
        assert_eq!(
            fields_of("v='a b'; printf '[%s]' ${v%x} \"${v#x}\"", &[]),
            "[a][b][a b]"
        );
        assert_eq!(
            fields_of("IFS=4; printf '[%s]' $((141)) $(echo 545)", &[]),
            "[1][1][5][5]"
        );
    }

    #[test]
    fn an_empty_at_keeps_a_quoted_empty_field_beside_it() {
        let n = "n() { echo $#; }; ";
        assert_eq!(
            said(&format!("{n}n \"\"$@; n $@\"\"; n \"$@\"$@"), &[]).0,
            "1\n1\n0\n"
        );
        assert_eq!(
            said(&format!("{n}n \"$@\"; n \"$@\" \"$@\""), &[""]).0,
            "1\n2\n"
        );
        assert_eq!(said(&format!("{n}n $@"), &[""]).0, "0\n");
    }

    #[test]
    fn the_word_of_an_assigning_default_has_its_tilde_expanded() {
        assert_eq!(
            out("HOME=/h; echo ${u:=~}; echo $u ${w:=~/x}"),
            "/h\n/h /h/x\n"
        );
        assert_eq!(said("set -u; HOME=/h; echo ${u:?~}", &[]).1, "u: /h\n");
    }

    #[test]
    fn arithmetic_reads_its_text_as_inside_double_quotes() {
        assert_eq!(said("IFS=+; echo $(($*))", &["1", "2"]).0, "3\n");
        assert_eq!(said("echo $(($@))", &["7"]).0, "7\n");
        assert_eq!(out("HOME=5; echo $((~0)) $((1+~0))"), "-1 0\n");
    }

    #[test]
    fn a_tilde_with_a_quote_before_its_slash_stays() {
        assert_eq!(
            out("HOME=/h; echo ~\"/x\" ~/\"x\" ~'' \"\"~"),
            "~/x /h/x ~ ~\n"
        );
    }

    #[test]
    fn all_the_parameters_take_operators_as_one_does() {
        assert_eq!(said("echo ${#@} ${*:-none}", &[]).0, "0 none\n");
        assert_eq!(
            fields_of("printf '[%s]' ${*%b}", &["ab", "c d"]),
            "[ab][c][d]"
        );
        assert_eq!(
            fields_of("printf '[%s]' \"${@%b}\"", &["ab", "c d"]),
            "[ab][c d]"
        );
        assert_eq!(said("echo ${*+set} ${@-unset}", &["x"]).0, "set x\n");
    }

    #[test]
    fn a_command_substitution_is_split_by_ifs_as_it_is_then() {
        assert_eq!(
            fields_of("IFS=,; printf '[%s]' $(echo 'a,b c')", &[]),
            "[a][b c]"
        );
        assert_eq!(
            fields_of("printf '[%s]' $(IFS=,; echo 'a,b c')", &[]),
            "[a,b][c]",
            "IFS set inside stays inside"
        );
    }

    #[test]
    fn an_option_set_shows_in_the_dash_parameter() {
        assert_eq!(out("set -C; echo $-; set -f; echo $-"), "C\nCf\n");
        assert_eq!(
            out("set -e; echo \"$(echo $-)\""),
            "e\n",
            "a subshell has them too"
        );
    }

    #[test]
    fn a_text_joins_the_fields_of_at_with_spaces() {
        assert_eq!(
            said("a=\"x$@y\"; echo \"[$a]\"", &["1", "2"]).0,
            "[x1 2y]\n"
        );
        assert_eq!(said("a=$*; echo \"[$a]\"", &["1", "", "2"]).0, "[1  2]\n");
        assert_eq!(
            said(": > \"$@\"", &["a", "b"]).1,
            "a b: no file here to write\n"
        );
    }

    #[test]
    fn an_assignment_s_value_is_never_split() {
        assert_eq!(
            fields_of("v='a  b'; w=$v; printf '[%s]' \"$w\"", &[]),
            "[a  b]"
        );
        assert_eq!(
            fields_of("w=$(echo '1  2'); printf '[%s]' \"$w\"", &[]),
            "[1  2]"
        );
        assert_eq!(
            fields_of("IFS=1; w=$((111)); printf '[%s]' \"$w\"", &[]),
            "[111]"
        );
    }

    #[test]
    fn a_parameter_s_digits_are_read_whole() {
        let p: Vec<String> = (1..=12).map(|n| n.to_string()).collect();
        let p: Vec<&str> = p.iter().map(String::as_str).collect();
        assert_eq!(said("echo ${12} ${012} $12", &p).0, "12 12 12\n");
        assert_eq!(
            said("echo \"[${13}]\" \"[${99999999999999999999}]\"", &p).0,
            "[] []\n"
        );
    }
}
