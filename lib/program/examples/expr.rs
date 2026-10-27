//! expr: evaluates the expression its words make and writes what it comes
//! to, as POSIX's expr does: `|` and `&`, comparisons, `+`, `-`, `*`, `/`
//! and `%` on integers, and `:`, which matches a basic regular expression
//! at the start of a text. It ends with 0 when the result is neither empty
//! nor zero, 1 when it is, 2 for an expression it cannot read, and 3 for
//! one it cannot work out, as a division by zero.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::Handle;
use regex::{Regex, Syntax, Text};

// lib/regex keeps its deepest search within half of this.
program::main!(expr, stack = 256);

/// Why an expression has no value.
#[derive(Debug, PartialEq, Eq)]
enum Wrong {
    /// It does not read as an expression: what is wrong.
    Read(String),
    /// It reads, but cannot be worked out: why.
    Worked(String),
}

/// The words of an expression, read one after another.
struct Words<'a> {
    words: &'a [String],
    at: usize,
}

impl<'a> Words<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.words.get(self.at).map(String::as_str)
    }

    /// Takes the next word when it is one of `operators`.
    fn operator(&mut self, operators: &[&str]) -> Option<&'a str> {
        let word = self.peek().filter(|word| operators.contains(word))?;
        self.at += 1;
        Some(word)
    }

    /// `|` between `&`s: the first that is neither empty nor zero, or else
    /// the last.
    fn or(&mut self) -> Result<String, Wrong> {
        let mut value = self.and()?;
        while self.operator(&["|"]).is_some() {
            let other = self.and()?;
            if empty_or_zero(&value) {
                value = other;
            }
        }
        Ok(value)
    }

    /// `&` between comparisons: the first when neither is empty nor zero,
    /// or else 0.
    fn and(&mut self) -> Result<String, Wrong> {
        let mut value = self.comparison()?;
        while self.operator(&["&"]).is_some() {
            let other = self.comparison()?;
            if empty_or_zero(&value) || empty_or_zero(&other) {
                value = String::from("0");
            }
        }
        Ok(value)
    }

    /// A comparison, of integers when both are and of texts otherwise: 1
    /// when it holds and 0 when not.
    fn comparison(&mut self) -> Result<String, Wrong> {
        let mut value = self.sum()?;
        while let Some(operator) = self.operator(&["=", "!=", "<", "<=", ">", ">="]) {
            let other = self.sum()?;
            let order = match (integer(&value), integer(&other)) {
                (Some(a), Some(b)) => a.cmp(&b),
                _ => value.as_bytes().cmp(other.as_bytes()),
            };
            let holds = match operator {
                "=" => order.is_eq(),
                "!=" => order.is_ne(),
                "<" => order.is_lt(),
                "<=" => order.is_le(),
                ">" => order.is_gt(),
                _ => order.is_ge(),
            };
            value = String::from(if holds { "1" } else { "0" });
        }
        Ok(value)
    }

    fn sum(&mut self) -> Result<String, Wrong> {
        let mut value = self.product()?;
        while let Some(operator) = self.operator(&["+", "-"]) {
            let other = self.product()?;
            value = arithmetic(&value, operator, &other)?;
        }
        Ok(value)
    }

    fn product(&mut self) -> Result<String, Wrong> {
        let mut value = self.matched()?;
        while let Some(operator) = self.operator(&["*", "/", "%"]) {
            let other = self.matched()?;
            value = arithmetic(&value, operator, &other)?;
        }
        Ok(value)
    }

    /// `:`: the text on its left matched by the expression on its right.
    fn matched(&mut self) -> Result<String, Wrong> {
        let mut value = self.primary()?;
        while self.operator(&[":"]).is_some() {
            let pattern = self.primary()?;
            value = matching(&value, &pattern)?;
        }
        Ok(value)
    }

    /// A word, or an expression in parentheses.
    fn primary(&mut self) -> Result<String, Wrong> {
        let Some(word) = self.peek() else {
            return Err(Wrong::Read(String::from("an expression ends too soon")));
        };
        self.at += 1;
        if word != "(" {
            return Ok(String::from(word));
        }
        let value = self.or()?;
        if self.operator(&[")"]).is_none() {
            return Err(Wrong::Read(String::from("a ( without its )")));
        }
        Ok(value)
    }
}

/// The value of the expression `words` make.
///
/// # Errors
///
/// For words that make no expression, and one that cannot be worked out.
fn evaluated(words: &[String]) -> Result<String, Wrong> {
    let mut reader = Words { words, at: 0 };
    let value = reader.or()?;
    match reader.peek() {
        None => Ok(value),
        Some(word) => Err(Wrong::Read(format!(
            "{word} is more than an expression takes"
        ))),
    }
}

/// `text` as an integer: decimal digits, a `-` before them or not.
fn integer(text: &str) -> Option<i64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Whether `value` is what expr counts as false: empty, or an integer
/// that is zero.
fn empty_or_zero(value: &str) -> bool {
    value.is_empty() || integer(value) == Some(0)
}

/// `left` and `right`, integers, with `operator` between them.
fn arithmetic(left: &str, operator: &str, right: &str) -> Result<String, Wrong> {
    let not_a_number = |text: &str| Wrong::Read(format!("{text} is no integer"));
    let a = integer(left).ok_or_else(|| not_a_number(left))?;
    let b = integer(right).ok_or_else(|| not_a_number(right))?;
    if matches!(operator, "/" | "%") && b == 0 {
        return Err(Wrong::Worked(String::from("a division by zero")));
    }
    let value = match operator {
        "+" => a.checked_add(b),
        "-" => a.checked_sub(b),
        "*" => a.checked_mul(b),
        "/" => a.checked_div(b),
        _ => a.checked_rem(b),
    };
    value
        .map(|value| format!("{value}"))
        .ok_or_else(|| Wrong::Worked(String::from("a result too big for 64 bits")))
}

/// `text` matched by the basic expression `pattern` at its start: what
/// the first subexpression took when it has one, or else how many
/// characters matched, 0 for none.
fn matching(text: &str, pattern: &str) -> Result<String, Wrong> {
    let regex = Regex::new(pattern, Syntax::Basic, false)
        .map_err(|e| Wrong::Read(format!("{pattern}: {}", e.code.says())))?;
    let line = Text::new(text.as_bytes());
    let spans = regex
        .captures(&line, 0)
        .map_err(|e| Wrong::Worked(String::from(e.says())))?
        .filter(|spans| spans[0].is_some_and(|whole| whole.start == 0));
    let Some(spans) = spans else {
        return Ok(String::from(if regex.groups() > 0 { "" } else { "0" }));
    };
    if regex.groups() > 0 {
        let taken = spans[1].map_or("", |span| &text[span.start..span.end]);
        return Ok(String::from(taken));
    }
    let whole = spans[0].map_or("", |span| &text[span.start..span.end]);
    Ok(format!("{}", whole.chars().count()))
}

/// What expr ends with for the value it wrote.
fn ending(value: &str) -> u64 {
    u64::from(empty_or_zero(value))
}

#[cfg_attr(test, allow(dead_code))]
fn expr(_: Handle) -> u64 {
    match evaluated(program::arguments()) {
        Ok(value) => {
            let written = program::write(&format!("{value}\n"));
            if written.is_err() { 3 } else { ending(&value) }
        }
        Err(Wrong::Read(why)) => {
            let _ = program::write_error(&format!("expr: {why}\n"));
            2
        }
        Err(Wrong::Worked(why)) => {
            let _ = program::write_error(&format!("expr: {why}\n"));
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> alloc::vec::Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    fn value(text: &str) -> Result<String, Wrong> {
        evaluated(&words(text))
    }

    fn is(text: &str, want: &str) {
        assert_eq!(value(text), Ok(String::from(want)), "{text}");
    }

    fn read_wrong(text: &str, why: &str) {
        assert_eq!(value(text), Err(Wrong::Read(String::from(why))), "{text}");
    }

    #[test]
    fn arithmetic_goes_by_precedence_and_parentheses() {
        is("1 + 2", "3");
        is("1 + 2 * 3", "7");
        is("( 1 + 2 ) * 3", "9");
        is("10 - 2 - 3", "5");
        is("20 / 3", "6");
        is("20 % 3", "2");
        is("-7 / 2", "-3");
        is("-7 % 2", "-1");
        is("2 * -3", "-6");
        is("5", "5");
        is("007 + 1", "8");
        is("( ( 4 ) )", "4");
    }

    #[test]
    fn or_and_and_give_an_operand_not_a_truth() {
        is("a | b", "a");
        is("0 | b", "b");
        let empty_first = [String::new(), String::from("|"), String::from("b")];
        assert_eq!(
            evaluated(&empty_first),
            Ok(String::from("b")),
            "an empty one is false"
        );
        is("0 | 0", "0");
        is("a & b", "a");
        is("a & 0", "0");
        is("0 & b", "0");
        assert_eq!(value("1 | 2 & 0"), Ok(String::from("1")), "& before |");
    }

    #[test]
    fn comparisons_are_of_integers_when_both_are_and_of_texts_else() {
        is("10 > 9", "1");
        assert_eq!(
            value("10 > 9a"),
            Ok(String::from("0")),
            "as texts, 1 before 9"
        );
        is("abc < abd", "1");
        is("abc = abc", "1");
        is("abc != abc", "0");
        is("-1 < 0", "1");
        assert_eq!(value("010 = 10"), Ok(String::from("1")), "as integers");
        is("2 <= 2", "1");
        is("3 >= 4", "0");
        assert_eq!(
            value("1 + 1 = 2"),
            Ok(String::from("1")),
            "arithmetic before a comparison"
        );
        assert_eq!(value("a = a = 1"), Ok(String::from("1")), "from the left");
    }

    #[test]
    fn integers_are_digits_with_a_minus_at_most() {
        assert_eq!(integer("12"), Some(12));
        assert_eq!(integer("-12"), Some(-12));
        assert_eq!(integer("+12"), None);
        assert_eq!(integer("-"), None);
        assert_eq!(integer(""), None);
        assert_eq!(integer("1e3"), None);
        assert_eq!(integer("99999999999999999999"), None);
        assert!(empty_or_zero(""));
        assert!(empty_or_zero("0"));
        assert!(empty_or_zero("-0"));
        assert!(empty_or_zero("000"));
        assert!(!empty_or_zero("0.0"));
        assert!(!empty_or_zero("a"));
    }

    #[test]
    fn a_match_counts_characters_or_gives_the_first_subexpression() {
        is("abcdef : abc", "3");
        is("abcdef : .*", "6");
        is("abcdef : x", "0");
        assert_eq!(
            value("abcdef : bcd"),
            Ok(String::from("0")),
            "only at the start"
        );
        is("abcdef : a\\(..\\)", "bc");
        assert_eq!(
            value("abcdef : x\\(..\\)"),
            Ok(String::new()),
            "none, so nothing"
        );
        assert_eq!(
            value("überall : .*"),
            Ok(String::from("7")),
            "characters, not bytes"
        );
        is("v1.2.3 : v\\([0-9]*\\)", "1");
        assert_eq!(
            value("abc : a* : 1"),
            Ok(String::from("1")),
            "from the left"
        );
        is("aaa : a*", "3");
        is("ab : \\(a\\)*b", "a");
        let empty = [String::new(), String::from(":"), String::from("a*")];
        assert_eq!(
            evaluated(&empty),
            Ok(String::from("0")),
            "an empty text matches at its start, taking nothing"
        );
    }

    #[test]
    fn what_cannot_be_read_or_worked_out_is_said() {
        read_wrong("", "an expression ends too soon");
        read_wrong("1 +", "an expression ends too soon");
        read_wrong("( 1", "a ( without its )");
        read_wrong("1 2", "2 is more than an expression takes");
        read_wrong("a + 1", "a is no integer");
        read_wrong("1 - x", "x is no integer");
        read_wrong("a : \\(", "\\(: a parenthesis without its other half");
        assert_eq!(
            value("1 / 0"),
            Err(Wrong::Worked(String::from("a division by zero")))
        );
        assert_eq!(
            value("1 % 0"),
            Err(Wrong::Worked(String::from("a division by zero")))
        );
        assert_eq!(
            value("9223372036854775807 + 1"),
            Err(Wrong::Worked(String::from("a result too big for 64 bits")))
        );
        assert_eq!(
            value("-9223372036854775807 - 2"),
            Err(Wrong::Worked(String::from("a result too big for 64 bits")))
        );
        is("9223372036854775807 + 0", "9223372036854775807");
    }

    #[test]
    fn it_ends_with_0_for_a_value_that_is_neither_empty_nor_zero() {
        assert_eq!(ending("1"), 0);
        assert_eq!(ending("a"), 0);
        assert_eq!(ending("-1"), 0);
        assert_eq!(ending("0"), 1);
        assert_eq!(ending(""), 1);
        assert_eq!(ending("-0"), 1, "zero as an integer is zero");
        assert_eq!(ending("00"), 1);
        assert_eq!(ending("0.0"), 0, "a text, not an integer");
    }

    #[test]
    fn texts_compare_by_their_bytes() {
        is("ä > z", "1");
        is("Z < a", "1");
        is("a < ab", "1");
        is("abc > ab", "1");
        is("x = x", "1");
        let empty = [String::new(), String::from("<"), String::from("a")];
        assert_eq!(
            evaluated(&empty),
            Ok(String::from("1")),
            "the empty text first"
        );
    }

    #[test]
    fn a_match_takes_all_of_a_basic_expression() {
        is("x : ^x", "1");
        is("abc : .*c$", "3");
        is("abd : .*c$", "0");
        is("aXb : a[A-Z]b", "3");
        is("aaa : a\\{2\\}", "2");
        is("abab : \\(ab\\)\\1", "ab");
        is("a.b : a\\.b", "3");
        is("axb : a\\.b", "0");
        is("x : \\(\\)", "");
        assert_eq!(
            value("ab : \\(a\\)\\(b\\)"),
            Ok(String::from("a")),
            "the first subexpression of two"
        );
    }

    #[test]
    fn a_match_binds_tighter_than_arithmetic_and_comparisons() {
        is("2 + 3 : 1", "2");
        assert_eq!(
            value("13 : 1 + 1"),
            Ok(String::from("2")),
            "13 : 1 is 1, then 1 + 1"
        );
        is("abc : ab = 2", "1");
        assert_eq!(
            value("a < b < 2"),
            Ok(String::from("1")),
            "(a < b) is 1, and 1 < 2"
        );
    }

    #[test]
    fn an_operator_where_a_value_stands_is_a_value() {
        is("= = =", "1");
        is("( + )", "+");
        is(")", ")");
        is("* : .", "1");
        read_wrong("( )", "a ( without its )");
        read_wrong("1 + ( 2", "a ( without its )");
    }

    #[test]
    fn division_overflows_only_at_the_least_integer_by_minus_one() {
        is("9223372036854775807 * -1", "-9223372036854775807");
        is("-9223372036854775808 + 0", "-9223372036854775808");
        is("-9223372036854775808 / 1", "-9223372036854775808");
        let too_big = Err(Wrong::Worked(String::from("a result too big for 64 bits")));
        assert_eq!(value("-9223372036854775808 / -1"), too_big);
        assert_eq!(value("-9223372036854775808 % -1"), too_big);
        assert_eq!(value("-9223372036854775808 * -1"), too_big);
        is("-0 | x", "x");
    }

    #[test]
    fn a_match_too_complex_to_work_out_is_said() {
        let text = format!("{} : \\(\\(a*\\)*\\)*\\1x", "a".repeat(40));
        assert_eq!(
            value(&text),
            Err(Wrong::Worked(String::from(regex::TooComplex.says())))
        );
    }

    #[test]
    fn gnu_s_words_are_no_operators_here() {
        read_wrong("length abc", "abc is more than an expression takes");
        read_wrong("substr abc 1 2", "abc is more than an expression takes");
        read_wrong("index abc b", "abc is more than an expression takes");
        is("length", "length");
    }

    #[test]
    fn only_ascii_digits_make_an_integer() {
        read_wrong("1 + ٣", "٣ is no integer");
        read_wrong("1 + ½", "½ is no integer");
        read_wrong("1 + --1", "--1 is no integer");
        read_wrong("1 + -", "- is no integer");
        is("٣ = ٣", "1");
        is("٣ < ٤", "1");
    }
}
