//! grep: the lines of each file it is given, or of its standard input
//! without one, that match one of its patterns, as POSIX's grep has it.
//! A pattern is a basic regular expression, an extended one with `-E`,
//! or with `-F` a text as it is; `-e` gives one, as often as wanted, and
//! a line feed in one parts it in two. `-i` matches in either case, `-x`
//! only whole lines, and `-v` takes the lines that match none. `-c` says
//! only how many lines it took, `-n` puts each one's number before it,
//! `-l` names only the files that hold one, and `-q` says nothing at all;
//! `-s` keeps quiet about files it cannot read. With more than one file,
//! each line comes after its file's path. It ends with 0 when it took a
//! line, 1 when it took none, and 2 when something went wrong, but with
//! `-q` with 0 as soon as it took one.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::text::{self, Lines};
use regex::{Regex, Syntax, Text, TooComplex};

// A search with back-references goes deep; lib/regex keeps the deepest
// within half of this.
program::main!(grep, stack = 256);

/// What grep writes for the lines it takes, each one asked for winning
/// over those before it here, as POSIX has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Show {
    #[default]
    Lines,
    Numbered,
    Count,
    Names,
    Quiet,
}

/// How grep reads its patterns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Basic,
    Extended,
    Fixed,
}

/// How a line must match: in either case or as written, and all of it
/// or anywhere in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct How {
    either_case: bool,
    whole: bool,
}

/// What grep was asked for.
#[derive(Debug, Default, PartialEq, Eq)]
struct Asked<'a> {
    patterns: Vec<&'a str>,
    files: &'a [String],
    kind: Kind,
    how: How,
    without: bool,
    show: Show,
    silent: bool,
}

fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked::default();
    let mut lists = Vec::new();
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
                'E' => asked.kind = Kind::Extended,
                'F' => asked.kind = Kind::Fixed,
                'c' => asked.show = asked.show.max(Show::Count),
                'i' => asked.how.either_case = true,
                'l' => asked.show = asked.show.max(Show::Names),
                'n' => asked.show = asked.show.max(Show::Numbered),
                'q' => asked.show = Show::Quiet,
                's' => asked.silent = true,
                'v' => asked.without = true,
                'x' => asked.how.whole = true,
                'e' => {
                    // The rest of the word is the pattern, or else the
                    // next word.
                    let after = &letters[at + 1..];
                    if after.is_empty() {
                        let (list, rest) = words
                            .split_first()
                            .ok_or_else(|| String::from("grep: -e wants a pattern\n"))?;
                        lists.push(list.as_str());
                        words = rest;
                    } else {
                        lists.push(after);
                    }
                    break;
                }
                _ => return Err(format!("grep: -{letter} is no option\n")),
            }
        }
    }
    if lists.is_empty() {
        let (list, rest) = words
            .split_first()
            .ok_or_else(|| String::from("grep: needs a pattern to look for\n"))?;
        lists.push(list.as_str());
        words = rest;
    }
    asked.patterns = lists.iter().flat_map(|list| list.split('\n')).collect();
    asked.files = words;
    Ok(asked)
}

/// What grep matches lines with.
#[derive(Debug)]
enum Matcher {
    /// Texts as they are, in upper case when in either case.
    Fixed(Vec<String>),
    Expressions(Vec<Regex>),
}

impl Matcher {
    /// The matcher for the patterns `asked` has.
    ///
    /// # Errors
    ///
    /// What grep says of a pattern that is no expression.
    fn new(asked: &Asked) -> Result<Matcher, String> {
        let syntax = match asked.kind {
            Kind::Basic => Syntax::Basic,
            Kind::Extended => Syntax::Extended,
            Kind::Fixed => {
                let texts = asked.patterns.iter().map(|text| {
                    if asked.how.either_case {
                        text.to_uppercase()
                    } else {
                        String::from(*text)
                    }
                });
                return Ok(Matcher::Fixed(texts.collect()));
            }
        };
        let mut expressions = Vec::new();
        for pattern in &asked.patterns {
            let regex = Regex::new(pattern, syntax, asked.how.either_case)
                .map_err(|e| format!("grep: {pattern}: {}\n", e.code.says()))?;
            expressions.push(regex);
        }
        Ok(Matcher::Expressions(expressions))
    }

    /// Whether `line` matches one of the patterns as `how` says.
    ///
    /// # Errors
    ///
    /// For an expression with back-references that would take too long.
    fn matches(&self, line: &[u8], how: How) -> Result<bool, TooComplex> {
        let whole = how.whole;
        match self {
            Matcher::Fixed(texts) if how.either_case => {
                let line = String::from_utf8_lossy(line).to_uppercase();
                Ok(texts.iter().any(|text| {
                    if whole {
                        line == *text
                    } else {
                        line.contains(text.as_str())
                    }
                }))
            }
            Matcher::Fixed(texts) => Ok(texts.iter().any(|text| {
                let text = text.as_bytes();
                if whole {
                    line == text
                } else {
                    text.is_empty() || line.windows(text.len()).any(|w| w == text)
                }
            })),
            Matcher::Expressions(expressions) => {
                let text = Text::new(line);
                for regex in expressions {
                    let Some(span) = regex.find(&text, 0)? else {
                        continue;
                    };
                    // Of matches as long as the line, the leftmost-longest
                    // is one.
                    if !whole || (span.start == 0 && span.end == line.len()) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

/// The lines grep takes from one text that comes in pieces.
struct Found<'a> {
    asked: &'a Asked<'a>,
    matcher: &'a Matcher,
    /// The file's path, none for the standard input.
    path: Option<&'a str>,
    /// Whether each line comes after its file's path.
    named: bool,
    number: usize,
    matched: usize,
    out: Vec<u8>,
    lines: Lines,
    /// Set once nothing more of the text is wanted.
    done: bool,
    trouble: Option<TooComplex>,
}

impl<'a> Found<'a> {
    fn new(
        asked: &'a Asked<'a>,
        matcher: &'a Matcher,
        path: Option<&'a str>,
        named: bool,
    ) -> Found<'a> {
        Found {
            asked,
            matcher,
            path,
            named,
            number: 0,
            matched: 0,
            out: Vec::new(),
            lines: Lines::default(),
            done: false,
            trouble: None,
        }
    }

    /// Takes in `piece`, and says whether more is wanted.
    fn feed(&mut self, piece: &[u8]) -> bool {
        let mut lines = core::mem::take(&mut self.lines);
        lines.feed(piece, |line| self.take(line));
        self.lines = lines;
        !self.done
    }

    /// Takes `line` when it is one grep wants, and says whether more are
    /// wanted.
    fn take(&mut self, line: &[u8]) -> bool {
        self.number += 1;
        let asked = self.asked;
        match self.matcher.matches(line, asked.how) {
            Ok(matches) if matches == asked.without => return true,
            Ok(_) => {}
            Err(trouble) => {
                self.trouble = Some(trouble);
                self.done = true;
                return false;
            }
        }
        self.matched += 1;
        match asked.show {
            // One line is enough to say.
            Show::Names | Show::Quiet => {
                self.done = true;
                return false;
            }
            Show::Count => return true,
            Show::Lines | Show::Numbered => {}
        }
        if let Some(path) = self.path.filter(|_| self.named) {
            self.out.extend_from_slice(path.as_bytes());
            self.out.push(b':');
        }
        if asked.show == Show::Numbered {
            self.out
                .extend_from_slice(format!("{}:", self.number).as_bytes());
        }
        self.out.extend_from_slice(line);
        self.out.push(b'\n');
        true
    }

    /// What grep writes for the text, how many lines it took, and what
    /// went wrong matching them.
    fn finish(mut self) -> (Vec<u8>, usize, Option<TooComplex>) {
        let lines = core::mem::take(&mut self.lines);
        if !self.done {
            lines.finish(|line| {
                self.take(line);
            });
        }
        let path = self.path.unwrap_or("(standard input)");
        match self.asked.show {
            Show::Count if self.named => {
                self.out = format!("{path}:{}\n", self.matched).into_bytes();
            }
            Show::Count => self.out = format!("{}\n", self.matched).into_bytes(),
            Show::Names if self.matched > 0 => self.out = format!("{path}\n").into_bytes(),
            _ => {}
        }
        (self.out, self.matched, self.trouble)
    }
}

#[cfg_attr(test, allow(dead_code))]
fn grep(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let matcher = match Matcher::new(&asked) {
        Ok(matcher) => matcher,
        Err(why) => {
            let _ = program::write_error(&why);
            return 2;
        }
    };
    let sources = text::sources(asked.files);
    let named = sources.len() > 1;
    let (mut taken, mut failed) = (0, false);
    for path in sources {
        let mut found = Found::new(&asked, &matcher, path, named);
        if let Err(e) = text::pieces(path, |piece| found.feed(piece)) {
            failed = true;
            if !asked.silent {
                let _ = program::write_error(&text::trouble("grep", path.unwrap_or("-"), e));
            }
            continue;
        }
        let (said, count, trouble) = found.finish();
        if let Some(trouble) = trouble {
            failed = true;
            let _ = program::write_error(&format!(
                "grep: {}: {}\n",
                path.unwrap_or("-"),
                trouble.says()
            ));
        }
        taken += count;
        failed |= program::write_bytes(&said).is_err();
        if asked.show == Show::Quiet && taken > 0 {
            return 0;
        }
    }
    ending(failed, taken)
}

/// What grep ends with: 2 when something `failed`, else 0 when it
/// `matched` a line and 1 when it found none.
fn ending(failed: bool, matched: usize) -> u64 {
    match (failed, matched) {
        (true, _) => 2,
        (false, 0) => 1,
        (false, _) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    /// What grep asked with `arguments` writes for `pieces` read as the
    /// file `path`, among several when `named`, and how many lines it
    /// took.
    fn found_in(
        arguments: &[String],
        path: Option<&str>,
        named: bool,
        pieces: &[&[u8]],
    ) -> (Vec<u8>, usize, Option<TooComplex>) {
        let asked = asked(arguments).unwrap();
        let matcher = Matcher::new(&asked).unwrap();
        let mut found = Found::new(&asked, &matcher, path, named);
        for piece in pieces {
            if !found.feed(piece) {
                break;
            }
        }
        found.finish()
    }

    /// The same with the words of `arguments`, named by `name` among
    /// several when that is some.
    fn found(arguments: &str, name: Option<&str>, pieces: &[&str]) -> (String, usize) {
        let pieces: Vec<&[u8]> = pieces.iter().map(|piece| piece.as_bytes()).collect();
        let (out, count, trouble) = found_in(&words(arguments), name, name.is_some(), &pieces);
        assert_eq!(trouble, None);
        (String::from_utf8(out).unwrap(), count)
    }

    /// Whether grep asked with `arguments`, its last word the pattern,
    /// takes `line`.
    fn takes(arguments: &str, line: &[u8]) -> bool {
        let words = words(arguments);
        let asked = asked(&words).unwrap();
        let matcher = Matcher::new(&asked).unwrap();
        matcher.matches(line, asked.how).unwrap()
    }

    #[test]
    fn options_come_before_the_pattern_and_the_files() {
        let w = words("-in -c two /a /b");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.patterns, a.files, a.how.either_case, a.show, a.without),
            (Vec::from(["two"]), &w[3..], true, Show::Count, false)
        );
        let w = words("-v x");
        assert!(asked(&w).unwrap().without);
        let w = words("-- -v /a");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.patterns, a.files, a.without),
            (Vec::from(["-v"]), &w[2..], false),
            "after --"
        );
        let w = words("- /a");
        assert_eq!(
            asked(&w).unwrap().patterns,
            ["-"],
            "a dash alone is a pattern"
        );
        let w = words("-Exs a");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.kind, a.how.whole, a.silent),
            (Kind::Extended, true, true)
        );
        assert_eq!(asked(&words("-F a")).unwrap().kind, Kind::Fixed);
        assert_eq!(
            asked(&words("-EF a")).unwrap().kind,
            Kind::Fixed,
            "the last wins"
        );
        assert_eq!(asked(&words("a")).unwrap().kind, Kind::Basic);
        assert_eq!(asked(&words("-y a")), Err("grep: -y is no option\n".into()));
        assert_eq!(
            asked(&words("-i")),
            Err("grep: needs a pattern to look for\n".into())
        );
        assert_eq!(
            asked(&[]),
            Err("grep: needs a pattern to look for\n".into())
        );
    }

    #[test]
    fn each_e_gives_a_pattern_and_leaves_the_words_after_to_files() {
        let w = words("-e a -e b /f");
        let a = asked(&w).unwrap();
        assert_eq!((a.patterns, a.files), (Vec::from(["a", "b"]), &w[4..]));
        let w = words("-i -ex /f");
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.patterns, a.files, a.how.either_case),
            (Vec::from(["x"]), &w[2..], true),
            "the rest of the word"
        );
        let w = words("-ve -n");
        assert_eq!(
            asked(&w).unwrap().patterns,
            ["-n"],
            "the next word, dash or not"
        );
        assert_eq!(
            asked(&words("-e")),
            Err("grep: -e wants a pattern\n".into())
        );
        let w = [String::from("one\ntwo"), String::from("/f")];
        let a = asked(&w).unwrap();
        assert_eq!(
            (a.patterns, a.files),
            (Vec::from(["one", "two"]), &w[1..]),
            "a line feed parts a pattern"
        );
        let w = [
            String::from("-e"),
            String::from("x\n"),
            String::from("-e"),
            String::new(),
        ];
        assert_eq!(asked(&w).unwrap().patterns, ["x", "", ""]);
    }

    #[test]
    fn of_what_to_show_the_one_asked_for_later_here_wins() {
        let show = |arguments| asked(&words(arguments)).unwrap().show;
        assert_eq!(show("x"), Show::Lines);
        assert_eq!(show("-n x"), Show::Numbered);
        assert_eq!(show("-cn x"), Show::Count, "a count wins over numbers");
        assert_eq!(show("-nc x"), Show::Count);
        assert_eq!(show("-cl x"), Show::Names);
        assert_eq!(show("-lc x"), Show::Names);
        assert_eq!(show("-ql x"), Show::Quiet);
        assert_eq!(show("-lqn x"), Show::Quiet);
    }

    #[test]
    fn a_pattern_is_a_basic_expression_unless_asked_otherwise() {
        assert!(takes("a.c", b"xabcx"));
        assert!(!takes("a.c", b"ac"));
        assert!(takes("a+", b"a+"), "+ is a character in a basic one");
        assert!(!takes("a+", b"aa"));
        assert!(takes("-E a+", b"aa"));
        assert!(takes("-E (ab|cd)e", b"xcde"));
        assert!(takes("\\(ab\\)\\1", b"xababx"));
        assert!(!takes("\\(ab\\)\\1", b"xabx"));
        assert!(takes("^[0-9]*$", b"2026"));
        assert!(!takes("^[0-9]*$", b"20x6"));
        assert!(takes("-F a.c", b"a.c"));
        assert!(!takes("-F a.c", b"abc"), "a dot as it is");
        assert!(takes("-i ÉTÉ", "été".as_bytes()));
        assert!(takes("-iF ÉTÉ", "l'été".as_bytes()));
        assert!(takes("-i A", &[0xff, b'a']), "lines that are no text too");
        assert!(takes("-iF A", &[0xff, b'a']));
    }

    #[test]
    fn x_takes_only_lines_matched_whole() {
        assert!(takes("-x a*", b"aaa"));
        assert!(!takes("-x a*", b"aab"));
        let empty = [String::from("-x"), String::new()];
        let (out, count, _) = found_in(&empty, None, false, &[b"\na\n\n"]);
        assert_eq!(
            (out, count),
            (b"\n\n".to_vec(), 2),
            "the empty pattern matches only empty lines whole"
        );
        assert!(
            takes("-Ex a|ab", b"ab"),
            "the leftmost-longest match is the whole"
        );
        assert!(takes("-xF a.b", b"a.b"));
        assert!(!takes("-xF a.b", b"a.bc"));
        assert!(takes("-xiF AB", b"ab"));
        assert!(!takes("-xiF AB", b"abc"));
        assert!(!takes("-x b", b"abc"), "a match inside is not enough");
    }

    #[test]
    fn any_of_several_patterns_takes_a_line() {
        let either = words("-e cat -e dog");
        let (out, count, _) = found_in(&either, None, false, &[b"a cat\na cow\na dog\n"]);
        assert_eq!((out, count), (b"a cat\na dog\n".to_vec(), 2));
        let fixed = words("-F -e . -e *");
        let (out, _, _) = found_in(&fixed, None, false, &[b"a.\nab\n*b\n"]);
        assert_eq!(out, b"a.\n*b\n");
        let lines = [String::from("^a\nb$")];
        let (out, _, _) = found_in(&lines, None, false, &[b"ax\nxb\nxx\n"]);
        assert_eq!(out, b"ax\nxb\n");
    }

    #[test]
    fn a_pattern_that_is_no_expression_is_named_with_why() {
        let wrong = |arguments| Matcher::new(&asked(&words(arguments)).unwrap()).unwrap_err();
        assert_eq!(
            wrong("a\\{2"),
            "grep: a\\{2: an interval that is no count from 0 to 255\n"
        );
        assert_eq!(
            wrong("-E ("),
            "grep: (: a parenthesis without its other half\n"
        );
        assert_eq!(
            wrong("-e x -e [a"),
            "grep: [a: a [ without the ] that ends it\n",
            "the one that is wrong"
        );
        assert!(
            Matcher::new(&asked(&words("-F [a")).unwrap()).is_ok(),
            "as it is"
        );
    }

    #[test]
    fn the_lines_found_come_as_asked() {
        let text = ["one\ntwo\nthr", "ee two\nfour"];
        assert_eq!(found("two", None, &text), ("two\nthree two\n".into(), 2));
        assert_eq!(found("-v two", None, &text), ("one\nfour\n".into(), 2));
        assert_eq!(
            found("-n two", None, &text),
            ("2:two\n3:three two\n".into(), 2)
        );
        assert_eq!(found("-c two", None, &text), ("2\n".into(), 2));
        assert_eq!(found("-c two", Some("/a"), &text), ("/a:2\n".into(), 2));
        assert_eq!(
            found("-n o", Some("/a"), &text),
            ("/a:1:one\n/a:2:two\n/a:3:three two\n/a:4:four\n".into(), 4)
        );
        assert_eq!(
            found("four", None, &text),
            ("four\n".into(), 1),
            "the last too"
        );
        assert_eq!(found("five", None, &text), (String::new(), 0));
        assert_eq!(found("-c five", None, &text), ("0\n".into(), 0));
        assert_eq!(found("-i TWO", None, &text), ("two\nthree two\n".into(), 2));
        assert_eq!(found("-E o$", None, &text), ("two\nthree two\n".into(), 2));
    }

    #[test]
    fn l_and_q_stop_at_the_first_line_taken() {
        let pieces: [&[u8]; 2] = [b"no\nyes\nyes again\n", b"never read\n"];
        let (out, count, _) = found_in(&words("-l yes"), Some("/f"), false, &pieces);
        assert_eq!(
            (out, count),
            (b"/f\n".to_vec(), 1),
            "the file named once, even alone"
        );
        let (out, count, _) = found_in(&words("-l yes"), None, false, &pieces);
        assert_eq!((out, count), (b"(standard input)\n".to_vec(), 1));
        let (out, count, _) = found_in(&words("-l nothing"), Some("/f"), true, &pieces);
        assert_eq!((out, count), (Vec::new(), 0));
        let (out, count, _) = found_in(&words("-q yes"), None, false, &pieces);
        assert_eq!((out, count), (Vec::new(), 1));
        let quiet = words("-q yes");
        let asked = asked(&quiet).unwrap();
        let matcher = Matcher::new(&asked).unwrap();
        let mut found = Found::new(&asked, &matcher, None, false);
        assert!(found.feed(b"no\n"));
        assert!(!found.feed(b"yes\nyes\n"), "nothing more is wanted");
        assert_eq!(
            found.number, 2,
            "the line after the first taken is not even read"
        );
        let mut held = Found::new(&asked, &matcher, None, false);
        assert!(!held.feed(b"yes\nyes"));
        assert_eq!(
            held.finish().1,
            1,
            "a line held back after the first is not taken"
        );
    }

    #[test]
    fn counts_and_numbers_go_on_across_pieces_and_files() {
        let text = ["a\nb", "\nc\n", "a b\n"];
        assert_eq!(found("-vc a", None, &text), ("2\n".into(), 2), "b and c");
        assert_eq!(found("-n a", None, &text), ("1:a\n4:a b\n".into(), 2));
        assert_eq!(found("-c a", Some("/x"), &[]), ("/x:0\n".into(), 0));
        assert_eq!(found("-c z", Some("/x"), &text), ("/x:0\n".into(), 0));
        assert_eq!(
            found("-v z", Some("/x"), &text),
            ("/x:a\n/x:b\n/x:c\n/x:a b\n".into(), 4)
        );
    }

    #[test]
    fn a_search_too_complex_ends_the_text_with_its_trouble() {
        let line = "a".repeat(40) + "\nnext\n";
        let (out, count, trouble) = found_in(
            &[String::from("\\(\\(a*\\)*\\)*\\1x")],
            None,
            false,
            &[line.as_bytes()],
        );
        assert_eq!((out, count, trouble), (Vec::new(), 0, Some(TooComplex)));
    }

    #[test]
    fn it_ends_with_what_it_found_unless_something_went_wrong() {
        assert_eq!(ending(false, 3), 0);
        assert_eq!(ending(false, 0), 1);
        assert_eq!(ending(true, 0), 2);
        assert_eq!(ending(true, 5), 2, "even with lines found");
    }

    #[test]
    fn nothing_to_look_for_is_in_every_line() {
        let (out, count, _) = found_in(&[String::new()], None, false, &[b"x\n\ny"]);
        assert_eq!((out, count), (b"x\n\ny\n".to_vec(), 3));
        let without = [String::from("-v"), String::new()];
        let (out, count, _) = found_in(&without, None, false, &[b"x\ny\n"]);
        assert_eq!((out, count), (Vec::new(), 0));
        let fixed = [String::from("-F"), String::new()];
        assert_eq!(found_in(&fixed, None, false, &[b"x\n"]).1, 1);
    }

    #[test]
    fn a_character_cut_between_pieces_is_found_in_either_case() {
        let e = "\u{e9}".as_bytes();
        let w = [String::from("-i"), String::from("\u{c9}T")];
        let (out, count, _) = found_in(&w, None, false, &[&[b'x', e[0]], &[e[1], b't', b'\n']]);
        assert_eq!((out, count), ("x\u{e9}t\n".as_bytes().to_vec(), 1));
        assert!(!takes("-F longer_than_it", b"short"));
        assert!(takes("-F same", b"same"));
    }

    /// The next of a run of numbers that look random, always the same.
    fn shuffled(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// Random lines of a few letters, and the text they make.
    fn random_lines(seed: &mut u64) -> (Vec<String>, String) {
        let lines: Vec<String> = (0..shuffled(seed) % 12)
            .map(|_| {
                (0..shuffled(seed) % 6)
                    .map(|_| {
                        ['a', 'b', 'A', '.', ' '][usize::try_from(shuffled(seed) % 5).unwrap()]
                    })
                    .collect()
            })
            .collect();
        let text = lines.iter().fold(String::new(), |all, l| all + l + "\n");
        (lines, text)
    }

    #[test]
    fn a_text_as_it_is_finds_the_lines_that_hold_it() {
        let mut seed = 0x0bad_cafe_dead_beef_u64;
        for _ in 0..300 {
            let (lines, text) = random_lines(&mut seed);
            let needle =
                ["a", "ab", ".", "A", "b a"][usize::try_from(shuffled(&mut seed) % 5).unwrap()];
            let piece = 1 + usize::try_from(shuffled(&mut seed) % 7).unwrap();
            let pieces: Vec<&str> = text
                .as_bytes()
                .chunks(piece)
                .map(|c| core::str::from_utf8(c).unwrap())
                .collect();
            let holding: Vec<&String> = lines.iter().filter(|l| l.contains(needle)).collect();
            let without: Vec<&String> = lines.iter().filter(|l| !l.contains(needle)).collect();
            let found_words = |options: &str| {
                let mut given = words(options);
                given.extend(["-e".into(), needle.into()]);
                let pieces: Vec<&[u8]> = pieces.iter().map(|p| p.as_bytes()).collect();
                let (out, count, trouble) = found_in(&given, None, false, &pieces);
                assert_eq!(trouble, None);
                (String::from_utf8(out).unwrap(), count)
            };
            let joined =
                |picked: &[&String]| picked.iter().fold(String::new(), |all, l| all + l + "\n");
            assert_eq!(
                found_words("-F").0,
                joined(&holding),
                "{needle:?} in {text:?}"
            );
            assert_eq!(found_words("-F -v").0, joined(&without));
            assert_eq!(
                found_words("-F -c").0,
                alloc::format!("{}\n", holding.len())
            );
            let whole: Vec<&String> = lines.iter().filter(|l| l.as_str() == needle).collect();
            assert_eq!(found_words("-F -x").0, joined(&whole));
            let either: Vec<&String> = lines
                .iter()
                .filter(|l| {
                    l.to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
                .collect();
            assert_eq!(found_words("-F -i").0, joined(&either));
        }
    }

    #[test]
    fn numbered_lines_say_where_they_are() {
        let mut seed = 0x1234_5678_9abc_def0_u64;
        for _ in 0..200 {
            let (lines, text) = random_lines(&mut seed);
            let want = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.contains('a'))
                .fold(String::new(), |all, (n, l)| {
                    all + &(n + 1).to_string() + ":" + l + "\n"
                });
            assert_eq!(found("-n -F a", None, &[&text]).0, want);
        }
    }
}
