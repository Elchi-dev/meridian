//! The commands both dialects run alike everywhere, which touch nothing
//! but their words, where they write, and what `test` asks of files
//! (decision record 0021): `echo`, `printf`, `test` and `[`, `true`,
//! `false` and `:`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
}

/// What the one asking may do with a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allowed {
    pub read: bool,
    pub write: bool,
    /// Run it as a program, or go into it as a directory.
    pub run: bool,
}

/// What `test` learns of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct File {
    pub kind: Kind,
    pub size: u64,
    pub allowed: Allowed,
}

/// What a command reaches: where it writes, where it says what went
/// wrong, and what it may ask of the system.
pub trait Streams {
    /// Writes `bytes` to the output, and says whether all of them went.
    fn out(&mut self, bytes: &[u8]) -> bool;
    /// Writes `bytes` where what went wrong is said, and says whether all
    /// of them went.
    fn err(&mut self, bytes: &[u8]) -> bool;
    /// What is at `path`, none when nothing is.
    fn file(&mut self, path: &str) -> Option<File>;
    /// Whether stream `stream` of the command is a terminal.
    fn terminal(&mut self, stream: u32) -> bool;
}

/// Runs the command `words` name, when it is one of these, and answers
/// its status; none when the first word names none of them.
pub fn run(words: &[String], streams: &mut impl Streams) -> Option<u8> {
    let (name, rest) = words.split_first()?;
    Some(match name.as_str() {
        "echo" => u8::from(!streams.out(&echoed(rest))),
        "printf" => printf(rest, streams),
        "test" => test(name, rest, streams),
        "[" => match rest.split_last() {
            Some((last, words)) if last == "]" => test(name, words, streams),
            _ => {
                let _ = streams.err(b"[: needs a ] at its end\n");
                2
            }
        },
        "true" | ":" => 0,
        "false" => 1,
        _ => return None,
    })
}

/// What `echo` writes for its words: them with a space between each two,
/// and a line's end after, which a first word `-n` leaves off.
fn echoed(words: &[String]) -> Vec<u8> {
    let (end, words) = match words.split_first() {
        Some((first, rest)) if first == "-n" => ("", rest),
        _ => ("\n", words),
    };
    let mut out = words.join(" ").into_bytes();
    out.extend_from_slice(end.as_bytes());
    out
}

/// The byte the escape after a `\` stands for, taken off `text`, as in a
/// format of printf: `\\`, the six letters C names, and up to three octal
/// digits; none for one there is not, which leaves the `\` as it is.
fn escape(text: &[u8]) -> Option<(u8, usize)> {
    let byte = match text.first()? {
        b'\\' => b'\\',
        b'a' => 0x07,
        b'b' => 0x08,
        b'f' => 0x0c,
        b'n' => b'\n',
        b'r' => b'\r',
        b't' => b'\t',
        b'v' => 0x0b,
        b'0'..=b'7' => return Some(octal(text, 3)),
        _ => return None,
    };
    Some((byte, 1))
}

/// The byte up to `most` octal digits at the start of `text` make, its
/// low eight bits, and how many digits there were.
fn octal(text: &[u8], most: usize) -> (u8, usize) {
    let digits = text
        .iter()
        .take(most)
        .take_while(|b| (b'0'..=b'7').contains(b))
        .count();
    let value = text[..digits]
        .iter()
        .fold(0u32, |n, b| n * 8 + u32::from(b - b'0'));
    (value.to_le_bytes()[0], digits)
}

/// The widest a conversion is padded or cut to; a width or a precision
/// above it counts as it, so that no format asks for more memory than a
/// program has.
const WIDEST: usize = 4096;

/// A conversion of a format: its flags, width and precision, and its
/// letter.
#[derive(Debug, PartialEq, Eq)]
struct Spec {
    left: bool,
    /// What `+` or a space asks for before a number that is not
    /// negative, `+` over a space as in C.
    positive: Option<char>,
    alternate: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
    letter: u8,
}

/// The conversion that starts after a `%` in `format`, and how long it
/// is; none when it does not end in a letter printf knows.
fn spec(format: &[u8]) -> Option<(Spec, usize)> {
    let mut spec = Spec {
        left: false,
        positive: None,
        alternate: false,
        zero: false,
        width: 0,
        precision: None,
        letter: 0,
    };
    let mut i = 0;
    while let Some(&flag) = format.get(i) {
        match flag {
            b'-' => spec.left = true,
            b'+' => spec.positive = Some('+'),
            b' ' => spec.positive = spec.positive.or(Some(' ')),
            b'#' => spec.alternate = true,
            b'0' => spec.zero = true,
            _ => break,
        }
        i += 1;
    }
    let number = |i: &mut usize| {
        let mut n = 0usize;
        while let Some(d @ b'0'..=b'9') = format.get(*i) {
            n = n.saturating_mul(10).saturating_add(usize::from(d - b'0'));
            *i += 1;
        }
        n.min(WIDEST)
    };
    spec.width = number(&mut i);
    if format.get(i) == Some(&b'.') {
        i += 1;
        spec.precision = Some(number(&mut i));
    }
    let letter = *format.get(i)?;
    if !b"diouxXcsb%".contains(&letter) {
        return None;
    }
    spec.letter = letter;
    Some((spec, i + 1))
}

/// What is wrong with a word printf takes as a number.
const NOT_A_NUMBER: &str = "not all a number";
const TOO_BIG: &str = "too big for 64 bits";

/// The integer an argument of a numeric conversion stands for: a C
/// constant, maybe signed, or after a quote the code of the character
/// after it, as far as it reads; and what is wrong with it, if anything.
/// A number past 64 bits stops at the most 64 bits hold.
fn integer(text: &str) -> (i128, Option<&'static str>) {
    if let Some(quoted) = text.strip_prefix(['\'', '"']) {
        let c = quoted
            .chars()
            .next()
            .map_or(0, |c| i128::from(u32::from(c)));
        return (c, None);
    }
    let text = text.trim_start_matches([' ', '\t']);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (digits, radix) = if let Some(hex) = digits.strip_prefix("0x").or(digits.strip_prefix("0X"))
    {
        (hex, 16)
    } else if digits.starts_with('0') {
        (digits, 8)
    } else {
        (digits, 10)
    };
    let taken = digits.chars().take_while(|c| c.is_digit(radix)).count();
    let mut wrong =
        (taken < digits.len() || digits.is_empty() && !text.is_empty()).then_some(NOT_A_NUMBER);
    let mut magnitude = 0u64;
    for c in digits[..taken].chars() {
        let next = magnitude
            .checked_mul(u64::from(radix))
            .and_then(|n| n.checked_add(u64::from(c.to_digit(radix).unwrap_or(0))));
        let Some(next) = next else {
            magnitude = u64::MAX;
            wrong = wrong.or(Some(TOO_BIG));
            break;
        };
        magnitude = next;
    }
    let n = i128::from(magnitude);
    (if negative { -n } else { n }, wrong)
}

/// `body` with the sign, the `0x` and the like before it, padded to the
/// spec's width as its flags ask.
fn padded(spec: &Spec, sign: &str, body: &str) -> String {
    let len = sign.chars().count() + body.chars().count();
    let fill = spec.width.saturating_sub(len);
    if spec.left {
        format!("{sign}{body}{}", " ".repeat(fill))
    } else if spec.zero && spec.precision.is_none() {
        format!("{sign}{}{body}", "0".repeat(fill))
    } else {
        format!("{}{sign}{body}", " ".repeat(fill))
    }
}

/// What the numeric conversion `spec` makes of `n`, and whether `n` fit
/// it: `d` and `i` take signed 64 bits, the others unsigned 64 bits, a
/// negative number turned round as C turns it; past that it stops at the
/// most they hold.
fn number(spec: &Spec, n: i128) -> (String, bool) {
    let signed = matches!(spec.letter, b'd' | b'i');
    let (lowest, highest) = if signed {
        (i128::from(i64::MIN), i128::from(i64::MAX))
    } else {
        (i128::from(i64::MIN), i128::from(u64::MAX))
    };
    let fits = (lowest..=highest).contains(&n);
    let fitted = n.clamp(lowest, highest);
    // A negative number turns round modulo 2 to the 64th, as C's does.
    let n = if signed {
        fitted
    } else {
        fitted.rem_euclid(1 << 64)
    };
    let digits = match spec.letter {
        b'o' => format!("{n:o}"),
        b'x' => format!("{n:x}"),
        b'X' => format!("{n:X}"),
        _ => format!("{}", n.unsigned_abs()),
    };
    let digits = match spec.precision {
        Some(0) if n == 0 => String::new(),
        Some(p) => format!("{}{digits}", "0".repeat(p.saturating_sub(digits.len()))),
        None => digits,
    };
    let signed = matches!(spec.letter, b'd' | b'i');
    let sign = match () {
        () if signed && n < 0 => String::from("-"),
        () if signed => spec.positive.map(String::from).unwrap_or_default(),
        () if spec.alternate && spec.letter == b'x' && n != 0 => "0x".into(),
        () if spec.alternate && spec.letter == b'X' && n != 0 => "0X".into(),
        () if spec.alternate && spec.letter == b'o' && !digits.starts_with('0') => "0".into(),
        () => String::new(),
    };
    (padded(spec, &sign, &digits), fits)
}

/// What `%b` makes of `text`: its escapes read as in a format, but octal
/// after a `\0` as up to three digits more, and with whether `\c` stopped
/// all output there.
fn backslashed(text: &str) -> (Vec<u8>, bool) {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        i += 1;
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match bytes.get(i) {
            Some(b'c') => return (out, true),
            Some(b'0') => {
                let (byte, digits) = octal(&bytes[i + 1..], 3);
                out.push(byte);
                i += 1 + digits;
            }
            _ => match escape(&bytes[i..]) {
                Some((byte, len)) => {
                    out.push(byte);
                    i += len;
                }
                None => out.push(b'\\'),
            },
        }
    }
    (out, false)
}

/// Runs printf with its words, and answers its status: 1 when a word was
/// not all a number where one was asked for, or no format came.
fn printf(words: &[String], streams: &mut impl Streams) -> u8 {
    let Some((format, mut arguments)) = words.split_first() else {
        let _ = streams.err(b"printf: needs a format\n");
        return 1;
    };
    let format = format.as_bytes();
    let mut out = Vec::new();
    let mut status = 0;
    loop {
        let before = arguments.len();
        let mut i = 0;
        while let Some(&b) = format.get(i) {
            i += 1;
            match b {
                b'\\' => match escape(&format[i..]) {
                    Some((byte, len)) => {
                        out.push(byte);
                        i += len;
                    }
                    None => out.push(b'\\'),
                },
                b'%' => {
                    let Some((spec, len)) = spec(&format[i..]) else {
                        out.push(b'%');
                        continue;
                    };
                    i += len;
                    if spec.letter == b'%' {
                        out.push(b'%');
                        continue;
                    }
                    let argument = match arguments.split_first() {
                        Some((first, rest)) => {
                            arguments = rest;
                            first.as_str()
                        }
                        None => "",
                    };
                    match spec.letter {
                        b's' => {
                            let text: String = match spec.precision {
                                Some(p) => argument.chars().take(p).collect(),
                                None => argument.into(),
                            };
                            out.extend(padded(&spec, "", &text).bytes());
                        }
                        b'c' => {
                            let first: String = argument.chars().take(1).collect();
                            out.extend(padded(&spec, "", &first).bytes());
                        }
                        b'b' => {
                            // As they are, which may be no UTF-8, padded by
                            // the characters they make when they are.
                            let (bytes, stopped) = backslashed(argument);
                            let len = core::str::from_utf8(&bytes)
                                .map_or(bytes.len(), |text| text.chars().count());
                            let fill = " ".repeat(spec.width.saturating_sub(len));
                            if !spec.left {
                                out.extend(fill.bytes());
                            }
                            out.extend(&bytes);
                            if spec.left {
                                out.extend(fill.bytes());
                            }
                            if stopped {
                                return if streams.out(&out) { status } else { 1 };
                            }
                        }
                        _ => {
                            let (n, read_wrong) = integer(argument);
                            let (text, fit) = number(&spec, n);
                            let wrong = read_wrong.or((!fit).then_some(TOO_BIG));
                            if let Some(wrong) = wrong {
                                let _ = streams
                                    .err(format!("printf: {argument}: {wrong}\n").as_bytes());
                                status = 1;
                            }
                            out.extend(text.bytes());
                        }
                    }
                }
                _ => out.push(b),
            }
        }
        // The format goes round again while words are left for it, so
        // long as it took one.
        if arguments.is_empty() || arguments.len() == before {
            break;
        }
    }
    if !streams.out(&out) {
        return 1;
    }
    status
}

/// The primaries `test` takes before one word.
const UNARY: [&str; 18] = [
    "-b", "-c", "-d", "-e", "-f", "-g", "-h", "-L", "-n", "-p", "-r", "-S", "-s", "-t", "-u", "-w",
    "-x", "-z",
];

/// The primaries `test` takes between two words.
const BINARY: [&str; 15] = [
    "=", "!=", "<", ">", "-eq", "-ne", "-gt", "-ge", "-lt", "-le", "-a", "-o", "-nt", "-ot", "-ef",
];

/// Runs `test`, called `name`, on `words`, and answers 0 when they hold,
/// 1 when they do not, and 2 when they are no test.
fn test(name: &str, words: &[String], streams: &mut impl Streams) -> u8 {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match holds(&words, streams) {
        Ok(held) => u8::from(!held),
        Err(why) => {
            let _ = streams.err(format!("{name}: {why}\n").as_bytes());
            2
        }
    }
}

/// Whether `words` hold, as POSIX reads them by how many there are, and
/// past four by the precedence of `!`, `-a` and `-o`; what is wrong with
/// them when they are no test.
fn holds(words: &[&str], streams: &mut impl Streams) -> Result<bool, String> {
    match *words {
        [] => Ok(false),
        [one] => Ok(!one.is_empty()),
        ["!", one] => Ok(one.is_empty()),
        [op, operand] if UNARY.contains(&op) => unary(op, operand, streams),
        [first, _] => Err(format!("{first}: no test of one word")),
        [left, op, right] if BINARY.contains(&op) => binary(left, op, right),
        ["!", a, b] => holds(&[a, b], streams).map(|held| !held),
        ["(", one, ")"] => holds(&[one], streams),
        ["!", a, b, c] => holds(&[a, b, c], streams).map(|held| !held),
        ["(", a, b, ")"] => holds(&[a, b], streams),
        _ => {
            let mut expression = Expression {
                words,
                at: 0,
                depth: 0,
            };
            let held = expression.or(streams)?;
            match words.get(expression.at) {
                None => Ok(held),
                Some(extra) => Err(format!("{extra}: more than a test takes")),
            }
        }
    }
}

/// Words of `test` read by precedence: `-o` looser than `-a`, `!` and
/// parentheses tightest.
struct Expression<'a> {
    words: &'a [&'a str],
    at: usize,
    /// How many `!` and parentheses the word being read stands inside,
    /// which go 16 deep, as the shell's other nesting does.
    depth: usize,
}

impl<'a> Expression<'a> {
    fn next(&mut self) -> Option<&'a str> {
        let word = self.words.get(self.at).copied();
        self.at += 1;
        word
    }

    fn took(&mut self, word: &str) -> bool {
        let next = self.words.get(self.at) == Some(&word);
        if next {
            self.at += 1;
        }
        next
    }

    fn or(&mut self, streams: &mut impl Streams) -> Result<bool, String> {
        let mut held = self.and(streams)?;
        while self.took("-o") {
            held |= self.and(streams)?;
        }
        Ok(held)
    }

    fn and(&mut self, streams: &mut impl Streams) -> Result<bool, String> {
        let mut held = self.not(streams)?;
        while self.took("-a") {
            held &= self.not(streams)?;
        }
        Ok(held)
    }

    fn not(&mut self, streams: &mut impl Streams) -> Result<bool, String> {
        let bang = self.took("!");
        if bang || self.took("(") {
            if self.depth >= crate::sh::lex::DEEPEST {
                return Err("this nests past 16 levels".into());
            }
            self.depth += 1;
            let held = if bang {
                self.not(streams).map(|held| !held)
            } else {
                self.or(streams)
            };
            self.depth -= 1;
            let held = held?;
            if !bang && !self.took(")") {
                return Err("( without its )".into());
            }
            return Ok(held);
        }
        let Some(word) = self.next() else {
            return Err("a test is missing at its end".into());
        };
        let following = self.words.get(self.at).copied();
        let after = self.words.get(self.at + 1).copied();
        match (following, after) {
            (Some(op), Some(right)) if BINARY.contains(&op) && op != "-a" && op != "-o" => {
                self.at += 2;
                binary(word, op, right)
            }
            (Some(operand), _) if UNARY.contains(&word) => {
                self.at += 1;
                unary(word, operand, streams)
            }
            _ => Ok(!word.is_empty()),
        }
    }
}

/// Whether the primary `op` holds for `operand`.
fn unary(op: &str, operand: &str, streams: &mut impl Streams) -> Result<bool, String> {
    let file = |streams: &mut _| Streams::file(streams, operand);
    Ok(match op {
        "-n" => !operand.is_empty(),
        "-z" => operand.is_empty(),
        "-t" => streams.terminal(whole(operand)?.try_into().unwrap_or(u32::MAX)),
        "-e" => file(streams).is_some(),
        "-f" => file(streams).is_some_and(|f| f.kind == Kind::File),
        "-d" => file(streams).is_some_and(|f| f.kind == Kind::Directory),
        "-s" => file(streams).is_some_and(|f| f.size > 0),
        "-r" => file(streams).is_some_and(|f| f.allowed.read),
        "-w" => file(streams).is_some_and(|f| f.allowed.write),
        "-x" => file(streams).is_some_and(|f| f.allowed.run),
        // Meridian has no links, devices, pipes, sockets or set-user bits.
        _ => false,
    })
}

/// The whole number `word` is, blanks around it allowed, for `test`.
fn whole(word: &str) -> Result<i64, String> {
    word.trim_matches([' ', '\t'])
        .parse()
        .map_err(|_| format!("{word}: not a whole number"))
}

/// Whether `left op right` holds.
fn binary(left: &str, op: &str, right: &str) -> Result<bool, String> {
    Ok(match op {
        "=" => left == right,
        "!=" => left != right,
        "<" => left.as_bytes() < right.as_bytes(),
        ">" => left.as_bytes() > right.as_bytes(),
        "-a" => !left.is_empty() && !right.is_empty(),
        "-o" => !left.is_empty() || !right.is_empty(),
        "-nt" | "-ot" | "-ef" => {
            return Err(format!(
                "{op}: Meridian keeps no times or links of files to compare"
            ));
        }
        _ => {
            let (a, b) = (whole(left)?, whole(right)?);
            match op {
                "-eq" => a == b,
                "-ne" => a != b,
                "-gt" => a > b,
                "-ge" => a >= b,
                "-lt" => a < b,
                _ => a <= b,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Streams that keep what is written, or refuse it all when `gone`, as
    /// an output whose reader left does.
    #[derive(Default)]
    struct Kept {
        out: Vec<u8>,
        err: Vec<u8>,
        gone: bool,
    }

    impl Streams for Kept {
        fn out(&mut self, bytes: &[u8]) -> bool {
            self.out.extend_from_slice(bytes);
            !self.gone
        }

        fn err(&mut self, bytes: &[u8]) -> bool {
            self.err.extend_from_slice(bytes);
            true
        }

        /// What the host's own files are, which the tests make in a
        /// folder of their own.
        fn file(&mut self, path: &str) -> Option<File> {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(path).ok()?;
            let mode = meta.permissions().mode();
            Some(File {
                kind: if meta.is_dir() {
                    Kind::Directory
                } else {
                    Kind::File
                },
                size: meta.len(),
                allowed: Allowed {
                    read: mode & 0o400 != 0,
                    write: mode & 0o200 != 0,
                    run: mode & 0o100 != 0,
                },
            })
        }

        /// What is kept here is no terminal.
        fn terminal(&mut self, _: u32) -> bool {
            false
        }
    }

    /// What running `words` answers, writes and says went wrong.
    fn ran(words: &[&str]) -> (Option<u8>, String, String) {
        let mut kept = Kept::default();
        let words: Vec<String> = words.iter().map(|w| String::from(*w)).collect();
        let status = run(&words, &mut kept);
        let text = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
        (status, text(kept.out), text(kept.err))
    }

    /// What printf writes for `words`, which must end it with 0.
    fn printed(words: &[&str]) -> String {
        let mut all = alloc::vec!["printf"];
        all.extend_from_slice(words);
        let (status, out, err) = ran(&all);
        assert_eq!((status, err.as_str()), (Some(0), ""), "{words:?}");
        out
    }

    #[test]
    fn echo_writes_its_words_and_a_line_end() {
        for (words, want) in [
            (&["echo"][..], "\n"),
            (&["echo", "a", "b"], "a b\n"),
            (&["echo", "-n", "a", "b"], "a b"),
            (&["echo", "-n"], ""),
            (&["echo", "a", "-n"], "a -n\n"),
            (&["echo", "--", "-n"], "-- -n\n"),
            (&["echo", "a\\nb", "c\\"], "a\\nb c\\\n"),
            (&["echo", ""], "\n"),
            (&["echo", "", ""], " \n"),
        ] {
            assert_eq!(
                ran(words),
                (Some(0), want.into(), String::new()),
                "{words:?}"
            );
        }
    }

    #[test]
    fn true_false_and_the_colon_answer_their_statuses() {
        assert_eq!(ran(&["true", "x"]).0, Some(0));
        assert_eq!(ran(&[":", "x"]).0, Some(0));
        assert_eq!(ran(&["false"]).0, Some(1));
        assert_eq!(ran(&["ls"]).0, None, "not one of these");
        assert_eq!(ran(&[]).0, None);
        assert_eq!(ran(&["Echo"]).0, None, "names as written");
    }

    #[test]
    fn a_command_whose_output_is_gone_ends_with_1() {
        for words in [&["echo", "a"][..], &["printf", "a"]] {
            let mut kept = Kept {
                gone: true,
                ..Kept::default()
            };
            let words: Vec<String> = words.iter().map(|w| String::from(*w)).collect();
            assert_eq!(run(&words, &mut kept), Some(1), "{words:?}");
        }
    }

    #[test]
    fn printf_reads_the_escapes_of_its_format() {
        assert_eq!(printed(&["hello"]), "hello");
        assert_eq!(printed(&["a\\nb\\t\\\\c"]), "a\nb\t\\c");
        assert_eq!(printed(&["\\a\\b\\f\\r\\v"]), "\u{7}\u{8}\u{c}\r\u{b}");
        assert_eq!(
            printed(&["\\101\\0x\\1012"]),
            "A\0xA2",
            "three octal digits at most"
        );
        assert_eq!(printed(&["\\q\\c\\"]), "\\q\\c\\", "others as they are");
        assert_eq!(printed(&["100%%"]), "100%");
        assert_eq!(printed(&["%y%"]), "%y%", "what is no conversion");
    }

    #[test]
    fn printf_puts_its_words_in_strings_and_characters() {
        assert_eq!(printed(&["%s-%s\\n", "a", "b"]), "a-b\n");
        assert_eq!(printed(&["[%s]"]), "[]", "a word that is missing is empty");
        assert_eq!(
            printed(&["[%5s][%-5s][%.2s][%5.1s]", "abc", "abc", "abc", "abc"]),
            "[  abc][abc  ][ab][    a]"
        );
        assert_eq!(printed(&["%c%c|", "hello", ""]), "h|");
        assert_eq!(printed(&["%3s|%c", "é", "日本"]), "  é|日");
    }

    #[test]
    fn printf_writes_numbers_as_c_does() {
        assert_eq!(printed(&["%d %i", "42", "-7"]), "42 -7");
        assert_eq!(
            printed(&["%+d|% d|%+ d|% +d", "5", "5", "5", "5"]),
            "+5| 5|+5|+5"
        );
        assert_eq!(
            printed(&["%05d|%-5d|%5d", "42", "42", "42"]),
            "00042|42   |   42"
        );
        assert_eq!(
            printed(&["%.3d|%5.3d|%.0d|", "7", "-7", "0"]),
            "007| -007||"
        );
        assert_eq!(
            printed(&["%o %u %x %X", "8", "42", "255", "255"]),
            "10 42 ff FF"
        );
        assert_eq!(
            printed(&["%#o %#x %#X %#x %#o", "8", "255", "255", "0", "0"]),
            "010 0xff 0XFF 0 0"
        );
        assert_eq!(
            printed(&["%u|%x", "-1", "-1"]),
            "18446744073709551615|ffffffffffffffff"
        );
        assert_eq!(printed(&["%05d|%-05d", "-42", "-42"]), "-0042|-42  ");
    }

    #[test]
    fn printf_reads_numbers_as_c_constants_or_characters() {
        assert_eq!(
            printed(&["%d %d %d %d %d", "0x10", "010", "'A", "\"a", ""]),
            "16 8 65 97 0"
        );
        assert_eq!(printed(&["%d %d %d", " 12", "+3", "0X1f"]), "12 3 31");
        assert_eq!(printed(&["%d", "'"]), "0", "a quote alone is nothing");
    }

    #[test]
    fn printf_says_which_word_is_not_all_a_number() {
        for (word, shown) in [
            ("abc", "0"),
            ("12abc", "12"),
            ("08", "0"),
            ("-", "0"),
            ("0x", "0"),
        ] {
            let (status, out, err) = ran(&["printf", "%d|", word]);
            assert_eq!(status, Some(1), "{word}");
            assert_eq!(out, format!("{shown}|"), "{word}");
            assert_eq!(err, format!("printf: {word}: not all a number\n"), "{word}");
        }
        assert_eq!(
            ran(&["printf"]),
            (Some(1), String::new(), "printf: needs a format\n".into())
        );
    }

    #[test]
    fn percent_b_reads_escapes_in_its_word_and_backslash_c_stops_all() {
        assert_eq!(printed(&["%b|", "a\\tb"]), "a\tb|");
        assert_eq!(printed(&["%b|%b", "\\0101\\101", "\\q"]), "AA|\\q");
        assert_eq!(
            printed(&["[%b]%s", "a\\nb\\cnot this", "nor this"]),
            "[a\nb"
        );
        assert_eq!(printed(&["%b\\n", "x"]), "x\n");
        assert_eq!(printed(&["%5b|", "\\t"]), "    \t|");
    }

    #[test]
    fn printf_goes_round_its_format_while_words_are_left() {
        assert_eq!(printed(&["%s\\n", "a", "b", "c"]), "a\nb\nc\n");
        assert_eq!(printed(&["%s=%s\\n", "a", "1", "b"]), "a=1\nb=\n");
        assert_eq!(
            printed(&["x\\n", "a", "b"]),
            "x\n",
            "a format that takes none"
        );
        assert_eq!(printed(&["%%%s", "a", "b"]), "%a%b");
    }

    #[test]
    fn widths_and_precisions_stop_at_4096() {
        let wide = printed(&["%5000d", "1"]);
        assert_eq!(wide.len(), WIDEST);
        assert!(wide.ends_with(" 1"));
        assert_eq!(printed(&["%.9999s", "ab"]), "ab");
        assert_eq!(printed(&["%.5000d", "1"]).len(), WIDEST);
    }

    /// What `test` answers for `words`, and what it says went wrong.
    fn tested(words: &[&str]) -> (u8, String) {
        let mut all = alloc::vec!["test"];
        all.extend_from_slice(words);
        let (status, out, err) = ran(&all);
        assert_eq!(out, "", "test writes nothing");
        (status.unwrap(), err)
    }

    #[test]
    fn test_reads_one_word_as_whether_it_is_empty() {
        assert_eq!(tested(&[]).0, 1);
        assert_eq!(tested(&["x"]).0, 0);
        assert_eq!(tested(&[""]).0, 1);
        for word in ["-n", "-z", "!", "=", "(", "-a"] {
            assert_eq!(tested(&[word]).0, 0, "{word} alone is a word");
        }
    }

    #[test]
    fn test_compares_strings_and_whole_numbers() {
        for (words, status) in [
            (&["a", "=", "a"][..], 0),
            (&["a", "=", "b"], 1),
            (&["a", "!=", "b"], 0),
            (&["a", "!=", "a"], 1),
            (&["a", "<", "b"], 0),
            (&["b", "<", "a"], 1),
            (&["B", "<", "a"], 0),
            (&["b", ">", "a"], 0),
            (&["a", ">", "a"], 1),
            (&["10", "-eq", "10"], 0),
            (&["10", "-eq", "010"], 0),
            (&["-3", "-lt", "2"], 0),
            (&["3", "-gt", "2"], 0),
            (&["2", "-gt", "2"], 1),
            (&["2", "-ge", "2"], 0),
            (&["2", "-le", "1"], 1),
            (&["1", "-le", "1"], 0),
            (&["1", "-lt", "1"], 1),
            (&["1", "-ne", "1"], 1),
            (&["1", "-ne", "2"], 0),
            (&[" 5 ", "-eq", "5"], 0),
            (&["", "=", ""], 0),
            (&["-n", "=", "-n"], 0),
            (&["!", "=", "!"], 0),
            (&["(", "=", ")"], 1),
            (&["a", "-a", "b"], 0),
            (&["a", "-a", ""], 1),
            (&["", "-o", "b"], 0),
            (&["", "-o", ""], 1),
        ] {
            assert_eq!(tested(words).0, status, "{words:?}");
        }
    }

    #[test]
    fn test_turns_round_with_a_bang_and_groups_with_parentheses() {
        for (words, status) in [
            (&["!", ""][..], 0),
            (&["!", "x"], 1),
            (&["!", "a", "=", "a"], 1),
            (&["!", "a", "=", "b"], 0),
            (&["!", "-z", ""], 1),
            (&["!", "-n", ""], 0),
            (&["(", "x", ")"], 0),
            (&["(", "", ")"], 1),
            (&["(", "-n", "x", ")"], 0),
            (&["(", "!", "", ")"], 0),
            (&["!", "!", "!", "!", "x"], 0),
        ] {
            assert_eq!(tested(words).0, status, "{words:?}");
        }
    }

    #[test]
    fn test_reads_longer_tests_by_precedence() {
        for (words, status) in [
            (&["x", "-o", "", "-a", ""][..], 0),
            (&["", "-a", "x", "-o", "x"], 0),
            (&["", "-o", "", "-o", "x"], 0),
            (&["x", "-a", "x", "-a", ""], 1),
            (&["!", "", "-a", "x"], 0),
            (&["(", "x", "-o", "", ")", "-a", "x"], 0),
            (&["(", "", ")", "-o", "x"], 0),
            (&["a", "=", "a", "-a", "b", "!=", "c"], 0),
            (&["-n", "a", "-a", "-z", ""], 0),
            (&["-n", "", "-o", "-z", "x"], 1),
            (&["!", "a", "=", "b", "-a", "c", "=", "c"], 0),
            (&["!", "x", "-o", "x", "-a", "x"], 0),
        ] {
            assert_eq!(tested(words).0, status, "{words:?}");
        }
    }

    #[test]
    fn test_says_what_is_no_number_or_no_test() {
        for (words, said) in [
            (&["a", "-eq", "1"][..], "a: not a whole number"),
            (&["1", "-lt", "1.5"], "1.5: not a whole number"),
            (&["-t", "x"], "x: not a whole number"),
            (&["a", "b"], "a: no test of one word"),
            (&["a", "b", "c", "d", "e"], "b: more than a test takes"),
            (&["(", "a", "-a", "b"], "( without its )"),
            (&["x", "-a", "x", "-a"], "a test is missing at its end"),
            (
                &["a", "-nt", "b"],
                "-nt: Meridian keeps no times or links of files to compare",
            ),
            (
                &["a", "-ef", "b"],
                "-ef: Meridian keeps no times or links of files to compare",
            ),
        ] {
            assert_eq!(tested(words), (2, format!("test: {said}\n")), "{words:?}");
        }
    }

    #[test]
    fn a_bracket_needs_its_closing_bracket() {
        assert_eq!(ran(&["[", "a", "=", "a", "]"]).0, Some(0));
        assert_eq!(ran(&["[", "a", "=", "b", "]"]).0, Some(1));
        assert_eq!(ran(&["[", "]"]).0, Some(1));
        assert_eq!(
            ran(&["[", "]", "]"]).0,
            Some(0),
            "a ] before the last is a word"
        );
        assert_eq!(
            ran(&["[", "a", "=", "a"]),
            (Some(2), String::new(), "[: needs a ] at its end\n".into())
        );
        assert_eq!(ran(&["["]).0, Some(2));
        assert_eq!(
            ran(&["[", "a", "-eq", "1", "]"]).2,
            "[: a: not a whole number\n"
        );
    }

    #[test]
    fn minus_t_asks_whether_a_stream_is_a_terminal() {
        assert_eq!(tested(&["-t", "1"]).0, 1, "what is kept here is none");
        assert_eq!(tested(&["-t", " 0 "]).0, 1);
        assert_eq!(tested(&["-t", "99999999999"]).0, 1);
    }

    #[test]
    fn test_asks_about_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("meridian-test-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let made = |name: &str, bytes: &[u8], mode: u32| {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            String::from(path.to_str().unwrap())
        };
        let file = made("f", b"x", 0o644);
        let empty = made("e", b"", 0o644);
        let tool = made("run", b"#!", 0o755);
        let locked = made("locked", b"x", 0o000);
        let folder = String::from(dir.to_str().unwrap());
        let missing = format!("{folder}/missing");
        for (op, path, status) in [
            ("-e", &file, 0),
            ("-e", &missing, 1),
            ("-e", &folder, 0),
            ("-f", &file, 0),
            ("-f", &folder, 1),
            ("-f", &missing, 1),
            ("-d", &folder, 0),
            ("-d", &file, 1),
            ("-s", &file, 0),
            ("-s", &empty, 1),
            ("-r", &file, 0),
            ("-r", &locked, 1),
            ("-w", &file, 0),
            ("-w", &locked, 1),
            ("-x", &tool, 0),
            ("-x", &file, 1),
            ("-x", &folder, 0),
            ("-x", &missing, 1),
            ("-h", &file, 1),
            ("-L", &file, 1),
            ("-b", &file, 1),
            ("-c", &file, 1),
            ("-p", &file, 1),
            ("-S", &file, 1),
            ("-g", &file, 1),
            ("-u", &file, 1),
        ] {
            assert_eq!(tested(&[op, path]).0, status, "{op} {path}");
        }
        assert_eq!(tested(&["!", "-e", &missing]).0, 0);
        assert_eq!(tested(&["-f", &file, "-a", "-d", &folder]).0, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_string_is_not_before_or_after_itself() {
        assert_eq!(tested(&["a", "<", "a"]).0, 1);
        assert_eq!(tested(&["a", ">", "a"]).0, 1);
        assert_eq!(tested(&["ab", "<", "b"]).0, 0, "byte by byte");
        assert_eq!(tested(&["é", ">", "z"]).0, 0, "bytes past ASCII come after");
    }

    #[test]
    fn printf_combines_flags_as_c_does() {
        for (format, argument, want) in [
            ("%+.3d", "7", "+007"),
            ("%-+5d|", "7", "+7   |"),
            ("% 05d", "7", " 0007"),
            ("%#.3o", "8", "010"),
            ("%#.0o", "0", "0"),
            ("%#5x", "255", " 0xff"),
            ("%#05x", "255", "0x0ff"),
            ("%.0x|", "0", "|"),
            ("%-#6o|", "8", "010   |"),
            ("%08.3d", "-5", "    -005"),
            ("%i", "-0", "0"),
            ("%3c|", "", "   |"),
            ("%-3c|", "ab", "a  |"),
            ("%-6.2s|", "abc", "ab    |"),
            ("%.0s|", "abc", "|"),
        ] {
            assert_eq!(printed(&[format, argument]), want, "{format} {argument}");
        }
    }

    #[test]
    fn printf_says_which_number_is_too_big_and_stops_it_at_64_bits() {
        assert_eq!(
            printed(&["%d", "9223372036854775807"]),
            "9223372036854775807"
        );
        assert_eq!(
            printed(&["%d", "-9223372036854775808"]),
            "-9223372036854775808"
        );
        assert_eq!(
            printed(&["%u", "18446744073709551615"]),
            "18446744073709551615"
        );
        assert_eq!(printed(&["%x", "-9223372036854775808"]), "8000000000000000");
        for (format, word, shown) in [
            ("%d", "9223372036854775808", "9223372036854775807"),
            ("%d", "-9223372036854775809", "-9223372036854775808"),
            ("%u", "18446744073709551616", "18446744073709551615"),
            ("%x", "-9223372036854775809", "8000000000000000"),
            ("%d", "0x10000000000000000", "9223372036854775807"),
            ("%o", "02000000000000000000000", "1777777777777777777777"),
        ] {
            let (status, out, err) = ran(&["printf", format, word]);
            assert_eq!(status, Some(1), "{format} {word}");
            assert_eq!(out, shown, "{format} {word}");
            assert_eq!(
                err,
                format!("printf: {word}: too big for 64 bits\n"),
                "{word}"
            );
        }
        let (_, _, err) = ran(&["printf", "%d", "99999999999999999999x"]);
        assert_eq!(
            err, "printf: 99999999999999999999x: not all a number\n",
            "first wrong first"
        );
    }

    #[test]
    fn printf_writes_bytes_as_they_are() {
        let mut kept = Kept::default();
        let words: Vec<String> = ["printf", "\\377\\0%b", "\\0200a"]
            .iter()
            .map(|w| String::from(*w))
            .collect();
        assert_eq!(run(&words, &mut kept), Some(0));
        assert_eq!(kept.out, [0xff, 0, 0x80, b'a']);
    }

    #[test]
    fn echo_takes_only_a_first_minus_n() {
        for (words, want) in [
            (&["echo", "-e", "a"][..], "-e a\n"),
            (&["echo", "-nn", "a"], "-nn a\n"),
            (&["echo", "-n", "-n", "a"], "-n a"),
            (&["echo", "-", "a"], "- a\n"),
            (&["echo", "日本", "語"], "日本 語\n"),
        ] {
            assert_eq!(ran(words).1, want, "{words:?}");
        }
    }

    #[test]
    fn padding_fills_to_the_width_on_the_side_asked() {
        for width in 0..8 {
            for len in 0..5 {
                let word = "x".repeat(len);
                let right = printed(&[&format!("%{width}s"), &word]);
                let left = printed(&[&format!("%-{width}s"), &word]);
                let size = width.max(len);
                assert_eq!(
                    right,
                    format!("{}{word}", " ".repeat(size - len)),
                    "{width} {len}"
                );
                assert_eq!(
                    left,
                    format!("{word}{}", " ".repeat(size - len)),
                    "{width} {len}"
                );
                let zeros = printed(&[&format!("%0{width}d"), &len.to_string()]);
                assert_eq!(zeros.len(), width.max(1), "{width} {len}");
                assert!(
                    zeros.trim_start_matches('0').is_empty() || zeros.ends_with(&len.to_string())
                );
            }
        }
    }

    /// Lists of words to test with, of every count up to four and past it.
    const TESTS: [&[&str]; 16] = [
        &[],
        &["x"],
        &[""],
        &["-n", ""],
        &["-z", ""],
        &["a", "=", "a"],
        &["a", "!=", "a"],
        &["1", "-lt", "2"],
        &["2", "-le", "1"],
        &["(", "x", ")"],
        &["(", "", ")"],
        &["(", "-z", "x", ")"],
        &["x", "-a", ""],
        &["x", "-o", ""],
        &["a", "=", "a", "-a", "b", "=", "b"],
        &["", "-o", "(", "x", ")"],
    ];

    #[test]
    fn a_bracket_answers_as_test_does() {
        for words in TESTS {
            let mut bracketed = alloc::vec!["["];
            bracketed.extend_from_slice(words);
            bracketed.push("]");
            assert_eq!(ran(&bracketed).0, Some(tested(words).0), "{words:?}");
        }
    }

    #[test]
    fn a_bang_before_a_short_test_turns_it_round() {
        for words in TESTS.iter().filter(|w| (1..=3).contains(&w.len())) {
            let mut turned = alloc::vec!["!"];
            turned.extend_from_slice(words);
            assert_eq!(tested(&turned).0, 1 - tested(words).0, "{words:?}");
        }
    }

    #[test]
    fn whole_numbers_compare_as_numbers() {
        for a in -3i64..=3 {
            for b in -3i64..=3 {
                for (op, holds) in [
                    ("-eq", a == b),
                    ("-ne", a != b),
                    ("-gt", a > b),
                    ("-ge", a >= b),
                    ("-lt", a < b),
                    ("-le", a <= b),
                ] {
                    let (left, right) = (a.to_string(), b.to_string());
                    assert_eq!(
                        tested(&[&left, op, &right]).0,
                        u8::from(!holds),
                        "{a} {op} {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_escape_means_the_same_in_a_format_and_in_percent_b() {
        for escape in [
            "\\\\", "\\a", "\\b", "\\f", "\\n", "\\r", "\\t", "\\v", "\\101", "\\7",
        ] {
            let in_format = printed(&[escape]);
            let in_b = printed(&["%b", escape]);
            assert_eq!(in_format, in_b, "{escape}");
            assert_eq!(in_format.len(), 1, "{escape} is one byte");
        }
    }

    #[test]
    fn octal_escapes_keep_their_low_eight_bits() {
        let bytes = |format: &str| {
            let mut kept = Kept::default();
            run(&[String::from("printf"), format.into()], &mut kept);
            kept.out
        };
        assert_eq!(bytes("\\400"), [0]);
        assert_eq!(bytes("\\0"), [0]);
        assert_eq!(bytes("\\08"), [0, b'8']);
        assert_eq!(bytes("\\1234"), [0o123, b'4']);
        assert_eq!(bytes("\\777"), [0xff]);
    }

    #[test]
    fn percent_b_pads_bytes_by_what_they_make() {
        let mut kept = Kept::default();
        let words: Vec<String> = ["printf", "%-4b|%3b|", "\\0377", "日"]
            .iter()
            .map(|w| String::from(*w))
            .collect();
        assert_eq!(run(&words, &mut kept), Some(0));
        let mut want = alloc::vec![0xff, b' ', b' ', b' ', b'|', b' ', b' '];
        want.extend_from_slice("日|".as_bytes());
        assert_eq!(kept.out, want);
        assert_eq!(
            printed(&["%b|", "a", "b\\cx", "c"]),
            "a|b",
            "\\c stops the next round too"
        );
    }

    #[test]
    fn a_stop_by_backslash_c_with_the_output_gone_ends_with_1() {
        let mut kept = Kept {
            gone: true,
            ..Kept::default()
        };
        let words: Vec<String> = ["printf", "%b", "a\\c"]
            .iter()
            .map(|w| String::from(*w))
            .collect();
        assert_eq!(run(&words, &mut kept), Some(1));
        assert_eq!(kept.out, b"a");
    }

    #[test]
    fn bangs_and_parentheses_nest_sixteen_deep() {
        let bangs = |n: usize| {
            let mut words = alloc::vec!["!"; n];
            words.push("x");
            tested(&words)
        };
        assert_eq!(bangs(16).0, 0);
        assert_eq!(bangs(15).0, 1);
        assert_eq!(bangs(17), (2, "test: this nests past 16 levels\n".into()));
        let parens = |n: usize| {
            let mut words = alloc::vec!["("; n];
            words.push("x");
            words.extend(alloc::vec![")"; n]);
            tested(&words)
        };
        assert_eq!(parens(16).0, 0);
        assert_eq!(parens(17).0, 2);
        let side_by_side: Vec<&str> = (0..20)
            .flat_map(|i| {
                if i == 0 {
                    &["(", "x", ")"][..]
                } else {
                    &["-a", "(", "x", ")"][..]
                }
            })
            .copied()
            .collect();
        assert_eq!(tested(&side_by_side).0, 0, "one after another");
    }

    #[test]
    fn whole_numbers_may_be_signed_but_not_empty() {
        assert_eq!(tested(&["+5", "-eq", "5"]).0, 0);
        assert_eq!(tested(&["-0", "-eq", "0"]).0, 0);
        assert_eq!(
            tested(&["", "-eq", "0"]),
            (2, "test: : not a whole number\n".into())
        );
        assert_eq!(tested(&["-t", "-1"]).0, 1);
        assert_eq!(
            tested(&["99999999999999999999", "-gt", "1"]),
            (2, "test: 99999999999999999999: not a whole number\n".into())
        );
    }

    #[test]
    fn a_word_printf_misses_is_nothing_or_0() {
        for (format, want) in [
            ("[%d]", "[0]"),
            ("[%i]", "[0]"),
            ("[%o]", "[0]"),
            ("[%u]", "[0]"),
            ("[%x]", "[0]"),
            ("[%X]", "[0]"),
            ("[%s]", "[]"),
            ("[%c]", "[]"),
            ("[%b]", "[]"),
            ("[%3d]", "[  0]"),
            ("[%-3s]", "[   ]"),
        ] {
            assert_eq!(printed(&[format]), want, "{format}");
        }
        assert_eq!(printed(&["%s|%d|%s", "a"]), "a|0|", "the rest of a round");
    }

    #[test]
    fn operators_where_words_go_are_words() {
        for (words, status) in [
            (&["-n", "-n"][..], 0),
            (&["-z", "-z"], 1),
            (&["-n", "=", "x"], 1),
            (&["!", "-n", "x"], 1),
            (&["!", "-z", "x"], 0),
            (&["=", "=", "="], 0),
            (&["!", "!", "x"], 0),
            (&["-a", "=", "-a"], 0),
        ] {
            assert_eq!(tested(words).0, status, "{words:?}");
        }
        assert_eq!(
            tested(&["=", "x"]),
            (2, "test: =: no test of one word\n".into())
        );
    }

    #[test]
    fn other_commands_are_left_to_the_shell() {
        for name in [
            "cd", "ls", "export", "set", "read", "exit", "[[", "]", "test2", "echo2",
        ] {
            assert_eq!(ran(&[name, "x"]).0, None, "{name}");
        }
    }

    #[test]
    fn the_specification_names_each_of_these_commands() {
        let spec = include_str!("../../../docs/formats/shell.md");
        for name in ["echo", "printf", "test", "[", "true", "false", ":"] {
            assert!(spec.contains(&format!("`{name}`")), "{name} is named");
            let words = [String::from(name)];
            assert!(run(&words, &mut Kept::default()).is_some(), "{name} runs");
        }
        assert!(
            spec.contains(&format!("at most {WIDEST},")),
            "the specification gives the widest"
        );
    }

    #[test]
    fn what_is_no_conversion_stays_as_it_is_written() {
        for format in ["%-5y", "%5", "%.", "%#", "abc%"] {
            assert_eq!(printed(&[format]), format, "{format}");
        }
    }

    #[test]
    fn printf_lines_up_a_table_one_round_a_row() {
        assert_eq!(
            printed(&[
                "%-8s|%6s|%5.1s\\n",
                "name",
                "size",
                "kind",
                "boot.meri",
                "1024",
                "file"
            ]),
            "name    |  size|    k\nboot.meri|  1024|    f\n"
        );
    }
}
