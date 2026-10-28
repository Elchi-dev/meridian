//! tr: its standard input with characters changed, deleted or squeezed,
//! as POSIX's tr has it: `tr SET1 SET2` puts each character of SET2 for
//! the one of SET1 at its place, `-d` deletes those of SET1, `-s`
//! squeezes runs of one of the last set into one, and `-c` takes every
//! character SET1 does not hold instead.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;

program::main!(tr);

/// Whether a character is in a class.
type Holds = fn(char) -> bool;

/// The character classes a set may name, `[:name:]`, as the POSIX locale
/// has them.
const CLASSES: [(&str, Holds); 12] = [
    ("alnum", |c| c.is_ascii_alphanumeric()),
    ("alpha", |c| c.is_ascii_alphabetic()),
    ("blank", |c| c == ' ' || c == '\t'),
    ("cntrl", |c| c.is_ascii_control()),
    ("digit", |c| c.is_ascii_digit()),
    ("graph", |c| c.is_ascii_graphic()),
    ("lower", |c| c.is_ascii_lowercase()),
    ("print", |c| c.is_ascii_graphic() || c == ' '),
    ("punct", |c| c.is_ascii_punctuation()),
    ("space", |c| c.is_ascii_whitespace() || c == '\u{b}'),
    ("upper", |c| c.is_ascii_uppercase()),
    ("xdigit", |c| c.is_ascii_hexdigit()),
];

/// The character a backslash in a set stands for with `rest` after it,
/// and how many characters of `rest` it took.
fn escaped(rest: &[char]) -> (char, usize) {
    let digits = rest
        .iter()
        .take(3)
        .take_while(|c| ('0'..='7').contains(c))
        .count();
    if digits > 0 {
        let value = rest[..digits]
            .iter()
            .fold(0u32, |n, c| n * 8 + c.to_digit(8).unwrap_or(0));
        return (char::from_u32(value & 0xff).unwrap_or('\0'), digits);
    }
    let c = match rest.first() {
        Some('a') => '\u{7}',
        Some('b') => '\u{8}',
        Some('f') => '\u{c}',
        Some('n') => '\n',
        Some('r') => '\r',
        Some('t') => '\t',
        Some('v') => '\u{b}',
        Some(&c) => c,
        None => return ('\\', 0),
    };
    (c, 1)
}

/// The characters `set` stands for, in its order: ranges and classes
/// written out, and `[c*n]` as `n` of `c`. Where `fill` is some, a `[c*]`
/// stands for as many of `c` as make the set that long; elsewhere it is
/// refused, as only SET2 may hold one.
///
/// # Errors
///
/// For a class that is none, a range that goes down, or a `[c*]` where it
/// may not stand.
fn characters(set: &str, fill: Option<usize>) -> Result<Vec<char>, String> {
    let written: Vec<char> = set.chars().collect();
    let mut out = Vec::new();
    let mut fill_at = None;
    let mut i = 0;
    while i < written.len() {
        let rest = &written[i..];
        if rest.starts_with(&['[', ':'])
            && let Some(end) = rest.windows(2).position(|w| w == [':', ']'])
        {
            let name: String = rest[2..end].iter().collect();
            let Some(&(_, holds)) = CLASSES.iter().find(|(n, _)| *n == name) else {
                return Err(format!("[:{name}:] is no class"));
            };
            out.extend((0..=127u8).map(char::from).filter(|&c| holds(c)));
            i += end + 2;
            continue;
        }
        if rest.len() >= 4
            && rest[0] == '['
            && rest[2] == '*'
            && let Some(end) = rest.iter().position(|&c| c == ']')
        {
            let count: String = rest[3..end].iter().collect();
            if count.is_empty() {
                if fill.is_none() || fill_at.is_some() {
                    return Err(format!("[{}*] stands only in SET2, once", rest[1]));
                }
                fill_at = Some((out.len(), rest[1]));
            } else {
                let n = count
                    .parse::<usize>()
                    .map_err(|_| format!("[{}*{count}] holds no count", rest[1]))?;
                out.extend(core::iter::repeat_n(rest[1], n));
            }
            i += end + 1;
            continue;
        }
        let (first, took) = if rest[0] == '\\' {
            let (c, took) = escaped(&rest[1..]);
            (c, took + 1)
        } else {
            (rest[0], 1)
        };
        i += took;
        if written.get(i) == Some(&'-') && i + 1 < written.len() {
            let (last, took) = if written[i + 1] == '\\' {
                let (c, took) = escaped(&written[i + 2..]);
                (c, took + 1)
            } else {
                (written[i + 1], 1)
            };
            if last < first {
                return Err(format!("{first}-{last} goes down"));
            }
            out.extend(first..=last);
            i += 1 + took;
            continue;
        }
        out.push(first);
    }
    if let (Some((at, c)), Some(length)) = (fill_at, fill) {
        let n = length.saturating_sub(out.len());
        out.splice(at..at, core::iter::repeat_n(c, n));
    }
    Ok(out)
}

/// What tr was asked to do.
#[derive(Debug, PartialEq, Eq)]
struct Asked {
    complement: bool,
    delete: bool,
    squeeze: bool,
    first: Vec<char>,
    second: Vec<char>,
}

/// What tr's words ask of it.
///
/// # Errors
///
/// For options it does not know, sets that do not read, or too few or
/// too many sets for the options.
fn asked(arguments: &[String]) -> Result<Asked, String> {
    let (mut complement, mut delete, mut squeeze) = (false, false, false);
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        for letter in letters.chars() {
            match letter {
                'c' | 'C' => complement = true,
                'd' => delete = true,
                's' => squeeze = true,
                _ => return Err(format!("-{letter} is no option")),
            }
        }
        words = rest;
    }
    // -d takes SET1 alone, and -s SET2 too or not; -ds and changing take
    // both.
    let wanted = match (delete, squeeze) {
        (true, false) => 1,
        (false, true) => 1 + usize::from(words.len() == 2),
        _ => 2,
    };
    if words.len() != wanted {
        return Err(String::from(
            "takes SET1 and SET2, or SET1 alone with -d or -s",
        ));
    }
    let first = characters(&words[0], None)?;
    let second = match words.get(1) {
        Some(set) if !delete || squeeze => characters(set, Some(first.len()))?,
        _ => Vec::new(),
    };
    if !delete && !squeeze && second.is_empty() && !first.is_empty() {
        return Err(String::from("SET2 holds nothing to put in"));
    }
    Ok(Asked {
        complement,
        delete,
        squeeze,
        first,
        second,
    })
}

/// tr at work on a text that comes in pieces.
struct Tr {
    asked: Asked,
    /// The bytes of a character a piece ended in the middle of.
    cut: Vec<u8>,
    /// The character written last, for squeezing.
    last: Option<char>,
}

impl Tr {
    fn new(asked: Asked) -> Tr {
        Tr {
            asked,
            cut: Vec::new(),
            last: None,
        }
    }

    /// Whether SET1, or what it does not hold with `-c`, takes `c`.
    fn chosen(&self, c: char) -> bool {
        self.asked.first.contains(&c) != self.asked.complement
    }

    /// What `c` becomes: none when it is deleted.
    fn changed(&self, c: char) -> Option<char> {
        if self.asked.delete {
            return (!self.chosen(c)).then_some(c);
        }
        if self.asked.second.is_empty() || !self.chosen(c) {
            return Some(c);
        }
        let second = &self.asked.second;
        // A character the second list is too short for becomes its last.
        let last = second.len() - 1;
        let at = if self.asked.complement {
            last
        } else {
            self.asked
                .first
                .iter()
                .rposition(|&f| f == c)
                .map_or(last, |at| at.min(last))
        };
        Some(second[at])
    }

    /// Whether a run of `c` written goes on as one, as `-s` asks: the last
    /// set holds it, SET2 or SET1 when it stands alone.
    fn squeezed(&self, c: char) -> bool {
        self.asked.squeeze
            && if self.asked.second.is_empty() {
                self.chosen(c)
            } else {
                self.asked.second.contains(&c)
            }
    }

    fn put(&mut self, c: char, out: &mut Vec<u8>) {
        let Some(c) = self.changed(c) else {
            return;
        };
        if self.last == Some(c) && self.squeezed(c) {
            return;
        }
        self.last = Some(c);
        let mut room = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut room).as_bytes());
    }

    /// What `piece` becomes; a character it ends in the middle of waits
    /// for the next piece, and bytes that are no UTF-8 go through as they
    /// are.
    fn feed(&mut self, piece: &[u8]) -> Vec<u8> {
        let mut bytes = core::mem::take(&mut self.cut);
        bytes.extend_from_slice(piece);
        let mut out = Vec::new();
        let mut rest = bytes.as_slice();
        while !rest.is_empty() {
            match core::str::from_utf8(rest) {
                Ok(text) => {
                    text.chars().for_each(|c| self.put(c, &mut out));
                    rest = &[];
                }
                Err(e) => {
                    let (good, bad) = rest.split_at(e.valid_up_to());
                    if let Ok(text) = core::str::from_utf8(good) {
                        text.chars().for_each(|c| self.put(c, &mut out));
                    }
                    if let Some(len) = e.error_len() {
                        out.extend_from_slice(&bad[..len]);
                        self.last = None;
                        rest = &bad[len..];
                    } else {
                        self.cut = bad.to_vec();
                        rest = &[];
                    }
                }
            }
        }
        out
    }

    /// What is left at the end: the bytes of a character cut short.
    fn finish(self) -> Vec<u8> {
        self.cut
    }
}

#[cfg_attr(test, allow(dead_code))]
fn tr(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&format!("tr: {why}\n"));
            return 2;
        }
    };
    let mut tr = Tr::new(asked);
    let mut room = [0u8; abi::MESSAGE_BYTES];
    loop {
        match program::input(&mut room) {
            Ok(0) => break,
            Ok(len) => {
                if program::write_bytes(&tr.feed(&room[..len])).is_err() {
                    return 1;
                }
            }
            Err(_) => {
                let _ = program::write_error("tr: its input could not be read\n");
                return 1;
            }
        }
    }
    u64::from(program::write_bytes(&tr.finish()).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| String::from(*w)).collect()
    }

    /// What tr with `arguments` makes of `pieces`.
    fn run(arguments: &[&str], pieces: &[&[u8]]) -> Vec<u8> {
        let mut tr = Tr::new(asked(&words(arguments)).unwrap());
        let mut out = Vec::new();
        for piece in pieces {
            out.extend(tr.feed(piece));
        }
        out.extend(tr.finish());
        out
    }

    fn text(arguments: &[&str], input: &str) -> String {
        String::from_utf8(run(arguments, &[input.as_bytes()])).unwrap()
    }

    #[test]
    fn a_set_writes_out_ranges_classes_and_escapes() {
        let set = |s: &str| characters(s, None).map(|c| c.into_iter().collect::<String>());
        assert_eq!(set("a-e"), Ok("abcde".into()));
        assert_eq!(set("ab-dz"), Ok("abcdz".into()));
        assert_eq!(set("-a-"), Ok("-a-".into()), "a dash at an end is itself");
        assert_eq!(set("[:digit:]"), Ok("0123456789".into()));
        assert_eq!(set("[:upper:]").map(|s| s.len()), Ok(26));
        assert_eq!(
            set("[:blank:]"),
            Ok("\t ".into()),
            "in the order of their codes"
        );
        assert_eq!(set("\\n\\t\\\\\\101\\7x"), Ok("\n\t\\A\u{7}x".into()));
        assert_eq!(set("\\"), Ok("\\".into()));
        assert_eq!(set("[a*3]b"), Ok("aaab".into()));
        assert_eq!(set("[:nope:]"), Err("[:nope:] is no class".into()));
        assert_eq!(set("z-a"), Err("z-a goes down".into()));
        assert_eq!(set("[a*]"), Err("[a*] stands only in SET2, once".into()));
        assert_eq!(set("[a*x]"), Err("[a*x] holds no count".into()));
        assert_eq!(set("[:"), Ok("[:".into()), "no class without its end");
    }

    #[test]
    fn a_fill_makes_set2_as_long_as_set1() {
        let fill = |s: &str, n| characters(s, Some(n)).map(|c| c.into_iter().collect::<String>());
        assert_eq!(fill("[x*]", 4), Ok("xxxx".into()));
        assert_eq!(fill("a[x*]b", 5), Ok("axxxb".into()));
        assert_eq!(
            fill("abc[x*]", 2),
            Ok("abc".into()),
            "none where it is long enough"
        );
        assert_eq!(
            fill("[x*][y*]", 3),
            Err("[y*] stands only in SET2, once".into())
        );
    }

    #[test]
    fn its_words_say_what_to_do() {
        let a = asked(&words(&["-ds", "ab", "c"])).unwrap();
        assert!(a.delete && a.squeeze && !a.complement);
        assert_eq!(a.second, ['c']);
        assert!(asked(&words(&["-C", "a", "b"])).unwrap().complement);
        assert!(asked(&words(&["--", "-a", "b"])).is_ok());
        for (args, why) in [
            (&["-x", "a", "b"][..], "-x is no option"),
            (&["a"], "takes SET1 and SET2, or SET1 alone with -d or -s"),
            (
                &["-d", "a", "b"],
                "takes SET1 and SET2, or SET1 alone with -d or -s",
            ),
            (
                &["a", "b", "c"],
                "takes SET1 and SET2, or SET1 alone with -d or -s",
            ),
            (
                &["-ds", "a"],
                "takes SET1 and SET2, or SET1 alone with -d or -s",
            ),
            (&["a", ""], "SET2 holds nothing to put in"),
        ] {
            assert_eq!(asked(&words(args)), Err(String::from(why)), "{args:?}");
        }
    }

    #[test]
    fn set2_takes_the_place_of_set1() {
        assert_eq!(text(&["a-z", "A-Z"], "hello, World!\n"), "HELLO, WORLD!\n");
        assert_eq!(
            text(&["abc", "x"], "aabbcc"),
            "xxxxxx",
            "the last of SET2 again"
        );
        assert_eq!(
            text(&["aa", "xy"], "a"),
            "y",
            "the last place a character has"
        );
        assert_eq!(text(&["a-c", "[x*]"], "abcd"), "xxxd");
        assert_eq!(text(&["é", "e"], "café"), "cafe");
        assert_eq!(text(&["", "x"], "a"), "a");
    }

    #[test]
    fn d_deletes_and_c_takes_the_rest() {
        assert_eq!(text(&["-d", "aeiou"], "education\n"), "dctn\n");
        assert_eq!(text(&["-cd", "0-9\n"], "a1b2\nc3"), "12\n3");
        assert_eq!(text(&["-c", "a-z\n", "_"], "ab c!\n"), "ab_c_\n");
    }

    #[test]
    fn s_squeezes_runs_into_one() {
        assert_eq!(text(&["-s", " "], "a   b  c "), "a b c ");
        assert_eq!(text(&["-s", "a-z", "A-Z"], "aabbb cc"), "AB C");
        assert_eq!(text(&["-ds", "x", "a"], "xaxax  b"), "a  b");
        assert_eq!(text(&["-s", "\\n"], "a\n\n\nb\n"), "a\nb\n");
        assert_eq!(
            text(&["-cs", "a-z", "\n"], "one  two..three"),
            "one\ntwo\nthree"
        );
    }

    #[test]
    fn a_character_cut_between_pieces_comes_whole() {
        let e = "é".as_bytes();
        assert_eq!(
            run(&["é", "e"], &[b"caf", &e[..1], &e[1..], b"!"]),
            b"cafe!"
        );
        assert_eq!(run(&["-s", "é"], &[&e[..1], &e[1..], e]), "é".as_bytes());
        assert_eq!(
            run(&["a", "b"], &[b"a", &e[..1]]),
            [b'b', e[0]],
            "cut short at the end"
        );
    }

    #[test]
    fn bytes_that_are_no_text_go_through() {
        assert_eq!(run(&["a", "b"], &[&[b'a', 0xff, b'a']]), [b'b', 0xff, b'b']);
        assert_eq!(
            run(&["-s", "a"], &[&[b'a', 0xff, b'a', b'a']]),
            [b'a', 0xff, b'a']
        );
    }

    #[test]
    fn every_class_holds_what_the_posix_locale_puts_in_it() {
        let class = |name: &str| -> String {
            characters(&format!("[:{name}:]"), None)
                .unwrap()
                .into_iter()
                .collect()
        };
        assert_eq!(class("digit"), "0123456789");
        assert_eq!(class("xdigit"), "0123456789ABCDEFabcdef");
        assert_eq!(class("lower"), "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(class("upper"), "ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        assert_eq!(class("alpha").len(), 52);
        assert_eq!(class("alnum").len(), 62);
        assert_eq!(class("space"), "\t\n\u{b}\u{c}\r ");
        assert_eq!(class("blank"), "\t ");
        assert_eq!(class("cntrl").len(), 33, "0 to 31 and 127");
        assert_eq!(class("punct").len(), 32);
        assert_eq!(class("graph").len(), 94);
        assert_eq!(class("print").len(), 95, "graph and the space");
        for (name, _) in CLASSES {
            assert!(class(name).is_ascii(), "{name}");
        }
    }

    #[test]
    fn an_octal_escape_takes_three_digits_at_most() {
        let set = |s: &str| characters(s, None).unwrap();
        assert_eq!(set("\\0"), ['\0']);
        assert_eq!(set("\\1011"), ['A', '1'], "the fourth digit is a character");
        assert_eq!(set("\\18"), ['\u{1}', '8'], "8 is no octal digit");
        assert_eq!(set("\\777"), ['\u{ff}'], "its low eight bits");
        assert_eq!(set("\\x"), ['x'], "any other escaped character is itself");
        assert_eq!(set("\\-"), ['-']);
        assert_eq!(
            set("a\\-z"),
            ['a', '-', 'z'],
            "an escaped dash makes no range"
        );
        assert_eq!(
            set("\\a\\b\\f\\r\\v"),
            ['\u{7}', '\u{8}', '\u{c}', '\r', '\u{b}']
        );
    }

    #[test]
    fn a_range_takes_escapes_at_either_end() {
        let set = |s: &str| characters(s, None).map(|c| c.into_iter().collect::<String>());
        assert_eq!(set("\\101-C"), Ok("ABC".into()));
        assert_eq!(set("A-\\103"), Ok("ABC".into()));
        assert_eq!(set("a-a"), Ok("a".into()));
        assert_eq!(set("α-γ"), Ok("αβγ".into()), "past ASCII too");
        assert_eq!(set("a-"), Ok("a-".into()));
        assert_eq!(set("-"), Ok("-".into()));
        assert_eq!(set(""), Ok(String::new()));
    }

    #[test]
    fn a_repetition_and_a_class_mix_with_the_rest() {
        let set = |s: &str| characters(s, None).map(|c| c.into_iter().collect::<String>());
        assert_eq!(set("x[:digit:]y"), Ok("x0123456789y".into()));
        assert_eq!(set("[a*2][b*0]c"), Ok("aac".into()), "none of b");
        assert_eq!(set("[a*10]"), Ok("a".repeat(10)));
        assert_eq!(
            set("[ab]"),
            Ok("[ab]".into()),
            "brackets alone are characters"
        );
        assert_eq!(
            set("[a*2"),
            Ok("[a*2".into()),
            "no repetition without its end"
        );
        let fill = |s: &str, n| characters(s, Some(n)).map(|c| c.into_iter().collect::<String>());
        assert_eq!(
            fill("[x*]y", 1),
            Ok("y".into()),
            "none of x where y is enough"
        );
        assert_eq!(
            fill("[:upper:][x*]", 28),
            Ok(String::from("ABCDEFGHIJKLMNOPQRSTUVWXYZxx"))
        );
    }

    #[test]
    fn complement_and_squeeze_go_together() {
        assert_eq!(text(&["-cs", "[:alnum:]", " "], "a,,b;;c"), "a b c");
        assert_eq!(text(&["-sc", "x", "-"], "xyzzx"), "x-x");
        assert_eq!(
            text(&["-c", "a", "[-*]"], "abc"),
            "a--",
            "a fill in the complement's SET2"
        );
        assert_eq!(text(&["-cd", "[:digit:]"], "tel: 0123-45\n"), "012345");
        assert_eq!(
            text(&["-dc", "a-z"], "A1b2c"),
            "bc",
            "the letters may come in any order"
        );
    }

    #[test]
    fn squeezing_looks_at_what_was_written_last() {
        assert_eq!(
            text(&["-s", "ab", "xx"], "abab"),
            "x",
            "a and b both become x"
        );
        assert_eq!(
            text(&["-s", "a", "b"], "aab"),
            "b",
            "what changing made counts"
        );
        assert_eq!(text(&["-s", " "], " "), " ");
        assert_eq!(text(&["-s", "a"], ""), "");
        assert_eq!(
            text(&["-ds", "x", "a"], "axa"),
            "a",
            "deleting brings the two together"
        );
        assert_eq!(text(&["-s", "ab"], "aabbaa"), "aba", "each its own run");
    }

    #[test]
    fn what_tr_keeps_and_drops_is_its_own_choice() {
        let tr = Tr::new(asked(&words(&["-d", "ab"])).unwrap());
        assert!(tr.chosen('a') && tr.chosen('b') && !tr.chosen('c'));
        assert_eq!(tr.changed('a'), None);
        assert_eq!(tr.changed('c'), Some('c'));
        assert!(!tr.squeezed('a'), "no -s");
        let tr = Tr::new(asked(&words(&["-c", "ab", "x"])).unwrap());
        assert!(!tr.chosen('a') && tr.chosen('z'));
        assert_eq!(tr.changed('z'), Some('x'));
        assert_eq!(tr.changed('a'), Some('a'));
        let tr = Tr::new(asked(&words(&["-s", "ab", "xy"])).unwrap());
        assert!(
            tr.squeezed('x') && tr.squeezed('y') && !tr.squeezed('a'),
            "the last set"
        );
        let tr = Tr::new(asked(&words(&["-s", "ab"])).unwrap());
        assert!(tr.squeezed('a') && !tr.squeezed('x'));
    }

    #[test]
    fn a_long_text_in_many_pieces_comes_out_whole() {
        let line = "the quick brown fox jumps over the lazy dog, 日本語\n".repeat(100);
        let bytes = line.as_bytes();
        let pieces: Vec<&[u8]> = bytes.chunks(7).collect();
        let out = run(&["a-z", "A-Z"], &pieces);
        assert_eq!(String::from_utf8(out).unwrap(), line.to_uppercase());
        let four = "𝄞".as_bytes();
        assert_eq!(four.len(), 4);
        let one_by_one: Vec<&[u8]> = four.chunks(1).collect();
        assert_eq!(run(&["𝄞", "x"], &one_by_one), b"x");
        let mut cut = Tr::new(asked(&words(&["a", "b"])).unwrap());
        assert_eq!(cut.feed(&four[..2]), b"");
        assert_eq!(
            cut.finish(),
            &four[..2],
            "what was cut short comes out at the end"
        );
    }

    #[test]
    fn options_go_together_in_one_word_or_several() {
        for (args, (complement, delete, squeeze)) in [
            (&["-cds", "a", "b"][..], (true, true, true)),
            (&["-c", "-d", "-s", "a", "b"], (true, true, true)),
            (&["-sd", "a", "b"], (false, true, true)),
            (&["-s", "a", "b"], (false, false, true)),
            (&["-s", "a"], (false, false, true)),
            (&["-Cd", "a"], (true, true, false)),
            (&["a", "b"], (false, false, false)),
        ] {
            let a = asked(&words(args)).unwrap();
            assert_eq!(
                (a.complement, a.delete, a.squeeze),
                (complement, delete, squeeze),
                "{args:?}"
            );
        }
        let a = asked(&words(&["-d", "x"])).unwrap();
        assert_eq!(a.second, Vec::<char>::new(), "-d alone takes no SET2");
        let a = asked(&words(&["-s", "x"])).unwrap();
        assert_eq!(a.second, Vec::<char>::new());
        assert_eq!(
            asked(&words(&["-", "b"])).map(|a| a.first),
            Ok(Vec::from(['-'])),
            "a dash alone is a set"
        );
    }

    #[test]
    fn a_set_that_does_not_read_is_said_whichever_it_is() {
        assert_eq!(
            asked(&words(&["z-a", "x"])),
            Err(String::from("z-a goes down"))
        );
        assert_eq!(
            asked(&words(&["a", "z-a"])),
            Err(String::from("z-a goes down"))
        );
        assert_eq!(
            asked(&words(&["-d", "[:x:]"])),
            Err(String::from("[:x:] is no class"))
        );
        assert_eq!(
            asked(&words(&["[a*]", "b"])),
            Err(String::from("[a*] stands only in SET2, once"))
        );
        assert!(
            asked(&words(&["-d", "a", "[b*]"])).is_err(),
            "-d takes SET1 alone"
        );
        assert!(
            asked(&words(&["-ds", "a", "[b*]"])).is_ok(),
            "-ds may fill SET2"
        );
    }
}
