//! Arithmetic expansion: what stands in `$((...))` once its parameters and
//! commands were expanded, worked out as C works out signed long integers.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::lex::DEEPEST;
use crate::Code;

/// The variables an expression reads and sets.
pub trait Variables {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&mut self, name: &str, value: String);
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Number(i64),
    Name(String),
    /// An operator or a parenthesis.
    Op(&'static str),
}

/// The operators, longest first, so the first that fits is the longest.
const OPERATORS: [&str; 34] = [
    "<<=", ">>=", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "*=", "/=", "%=", "+=", "-=",
    "&=", "^=", "|=", "+", "-", "~", "!", "*", "/", "%", "<", ">", "&", "^", "|", "?", ":", "=",
    "(",
];

/// The integer a C constant without its sign is: decimal, octal after a
/// `0`, or hexadecimal after `0x`; past 64 bits it wraps, as other shells'
/// arithmetic does.
fn constant(text: &str) -> Option<i64> {
    let (digits, radix) = if let Some(hex) = text.strip_prefix("0x").or(text.strip_prefix("0X")) {
        (hex, 16)
    } else if text.len() > 1 && text.starts_with('0') {
        (&text[1..], 8)
    } else {
        (text, 10)
    };
    if !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u64::from_str_radix(digits, radix)
        .ok()
        .map(u64::cast_signed)
}

/// The integer a variable's value stands for: a constant, maybe signed,
/// maybe with blanks around it, and nothing for 0.
fn value(text: &str) -> Option<i64> {
    let text = text.trim_matches([' ', '\t', '\n']);
    if text.is_empty() {
        return Some(0);
    }
    let (negative, digits) = match text.as_bytes()[0] {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let n = constant(digits)?;
    Some(if negative { n.wrapping_neg() } else { n })
}

fn tokens(expression: &str) -> Result<Vec<Token>, Code> {
    let mut out = Vec::new();
    let mut rest = expression;
    loop {
        rest = rest.trim_start_matches([' ', '\t', '\n']);
        let Some(c) = rest.chars().next() else {
            return Ok(out);
        };
        let word = |rest: &str| {
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            (rest[..end].to_string(), end)
        };
        if c.is_ascii_digit() {
            let (text, end) = word(rest);
            out.push(Token::Number(constant(&text).ok_or(Code::BadArithmetic)?));
            rest = &rest[end..];
        } else if c.is_ascii_alphabetic() || c == '_' {
            let (text, end) = word(rest);
            out.push(Token::Name(text));
            rest = &rest[end..];
        } else if c == ')' {
            out.push(Token::Op(")"));
            rest = &rest[1..];
        } else {
            let op = OPERATORS
                .iter()
                .find(|op| rest.starts_with(**op))
                .ok_or(Code::BadArithmetic)?;
            out.push(Token::Op(op));
            rest = &rest[op.len()..];
        }
    }
}

/// The binary operators from the loosest to the tightest, each level with
/// the operators it holds.
const LEVELS: [&[&str]; 10] = [
    &["||"],
    &["&&"],
    &["|"],
    &["^"],
    &["&"],
    &["==", "!="],
    &["<", "<=", ">", ">="],
    &["<<", ">>"],
    &["+", "-"],
    &["*", "/", "%"],
];

struct Evaluation<'a, V> {
    tokens: Vec<Token>,
    at: usize,
    variables: &'a mut V,
    /// How many parentheses and signs the one being read stands inside.
    depth: usize,
}

impl<V: Variables> Evaluation<'_, V> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn took(&mut self, op: &str) -> bool {
        let next = matches!(self.peek(), Some(Token::Op(o)) if *o == op);
        if next {
            self.at += 1;
        }
        next
    }

    /// The value of the variable `name`.
    fn read(&self, name: &str) -> Result<i64, Code> {
        match self.variables.get(name) {
            None => Ok(0),
            Some(text) => value(&text).ok_or(Code::NotANumber),
        }
    }

    /// An assignment or a conditional; when `skip`, read without working
    /// anything out, as the branch `&&`, `||` or `?:` does not take is.
    fn assignment(&mut self, skip: bool) -> Result<i64, Code> {
        if let (Some(Token::Name(name)), Some(Token::Op(op))) =
            (self.tokens.get(self.at), self.tokens.get(self.at + 1))
            && op.ends_with('=')
            && !matches!(*op, "==" | "!=" | "<=" | ">=")
        {
            let (name, op) = (name.clone(), *op);
            self.at += 2;
            let right = self.assignment(skip)?;
            if skip {
                return Ok(0);
            }
            let value = if op == "=" {
                right
            } else {
                binary(&op[..op.len() - 1], self.read(&name)?, right)?
            };
            self.variables.set(&name, value.to_string());
            return Ok(value);
        }
        self.conditional(skip)
    }

    fn conditional(&mut self, skip: bool) -> Result<i64, Code> {
        let condition = self.binary(0, skip)?;
        if !self.took("?") {
            return Ok(condition);
        }
        let yes = self.assignment(skip || condition == 0)?;
        if !self.took(":") {
            return Err(Code::BadArithmetic);
        }
        let no = self.conditional(skip || condition != 0)?;
        Ok(if condition != 0 { yes } else { no })
    }

    /// The operators of `LEVELS[level]` and those that bind tighter.
    fn binary(&mut self, level: usize, skip: bool) -> Result<i64, Code> {
        let Some(ops) = LEVELS.get(level) else {
            return self.unary(skip);
        };
        let mut left = self.binary(level + 1, skip)?;
        while let Some(&op) = ops.iter().find(|&&op| self.peek() == Some(&Token::Op(op))) {
            self.at += 1;
            // The right side of && and || is not worked out once the left
            // decided.
            let decided = match op {
                "&&" => left == 0,
                "||" => left != 0,
                _ => false,
            };
            let right = self.binary(level + 1, skip || decided)?;
            left = match op {
                _ if skip => 0,
                "&&" => i64::from(left != 0 && right != 0),
                "||" => i64::from(left != 0 || right != 0),
                _ => binary(op, left, right)?,
            };
        }
        Ok(left)
    }

    /// Counts one more parenthesis or sign around what is read next, which
    /// keeps working it out well inside a program's stack.
    fn deeper(&mut self) -> Result<(), Code> {
        if self.depth >= DEEPEST {
            return Err(Code::TooNested);
        }
        self.depth += 1;
        Ok(())
    }

    fn unary(&mut self, skip: bool) -> Result<i64, Code> {
        for op in ["+", "-", "~", "!"] {
            if self.took(op) {
                self.deeper()?;
                let n = self.unary(skip)?;
                self.depth -= 1;
                return Ok(match op {
                    "+" => n,
                    "-" => n.wrapping_neg(),
                    "~" => !n,
                    _ => i64::from(n == 0),
                });
            }
        }
        let token = self.peek().cloned().ok_or(Code::BadArithmetic)?;
        self.at += 1;
        match token {
            Token::Number(n) => Ok(n),
            Token::Name(_) if skip => Ok(0),
            Token::Name(name) => self.read(&name),
            Token::Op("(") => {
                self.deeper()?;
                let n = self.assignment(skip)?;
                self.depth -= 1;
                if self.took(")") {
                    Ok(n)
                } else {
                    Err(Code::BadArithmetic)
                }
            }
            Token::Op(_) => Err(Code::BadArithmetic),
        }
    }
}

/// What `left op right` is, wrapping as C's signed integers do on the
/// machines sh runs on, and shifts taking the count modulo 64.
fn binary(op: &str, left: i64, right: i64) -> Result<i64, Code> {
    let shift = u32::from(right.to_le_bytes()[0] & 63);
    Ok(match op {
        "*" => left.wrapping_mul(right),
        "/" | "%" if right == 0 => return Err(Code::DivisionByZero),
        "/" => left.wrapping_div(right),
        "%" => left.wrapping_rem(right),
        "+" => left.wrapping_add(right),
        "-" => left.wrapping_sub(right),
        "<<" => left.wrapping_shl(shift),
        ">>" => left.wrapping_shr(shift),
        "<" => i64::from(left < right),
        "<=" => i64::from(left <= right),
        ">" => i64::from(left > right),
        ">=" => i64::from(left >= right),
        "==" => i64::from(left == right),
        "!=" => i64::from(left != right),
        "&" => left & right,
        "^" => left ^ right,
        _ => left | right,
    })
}

/// What `expression` works out to, the variables it names read and those
/// it assigns set; nothing works out to 0, as in other shells.
///
/// # Errors
///
/// `BadArithmetic` for what is not C's arithmetic, `DivisionByZero`, and
/// `NotANumber` for a variable whose value is no integer.
pub fn evaluate(expression: &str, variables: &mut impl Variables) -> Result<i64, Code> {
    let mut evaluation = Evaluation {
        tokens: tokens(expression)?,
        at: 0,
        variables,
        depth: 0,
    };
    if evaluation.tokens.is_empty() {
        return Ok(0);
    }
    let n = evaluation.assignment(false)?;
    if evaluation.at != evaluation.tokens.len() {
        return Err(Code::BadArithmetic);
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    /// Variables kept in a map, as the shell keeps its own.
    #[derive(Default)]
    struct Map(BTreeMap<String, String>);

    impl Variables for Map {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }

        fn set(&mut self, name: &str, value: String) {
            self.0.insert(name.into(), value);
        }
    }

    fn with(pairs: &[(&str, &str)]) -> Map {
        Map(pairs.iter().map(|&(n, v)| (n.into(), v.into())).collect())
    }

    fn worked(expression: &str) -> Result<i64, Code> {
        evaluate(expression, &mut Map::default())
    }

    #[test]
    fn constants_are_decimal_octal_or_hexadecimal() {
        for (text, n) in [
            ("42", 42),
            ("0x1F", 31),
            ("0X1f", 31),
            ("017", 15),
            ("0", 0),
            ("00", 0),
            ("18446744073709551615", -1),
            ("9223372036854775808", i64::MIN),
        ] {
            assert_eq!(worked(text), Ok(n), "{text}");
        }
        for text in [
            "08",
            "0x",
            "0xg",
            "1a",
            "0x+5",
            "18446744073709551616",
            "0b1",
        ] {
            assert_eq!(worked(text), Err(Code::BadArithmetic), "{text}");
        }
    }

    #[test]
    fn operators_bind_as_tightly_as_in_c() {
        for (text, n) in [
            ("1 + 2 * 3", 7),
            ("(1 + 2) * 3", 9),
            ("10 - 2 - 3", 5),
            ("2 * 3 % 4", 2),
            ("100 / 10 / 5", 2),
            ("1 << 2 + 1", 8),
            ("1 + 1 < 3", 1),
            ("1 == 1 < 2", 1),
            ("6 & 3 ^ 1 | 8", 11),
            ("1 || 0 && 0", 1),
            ("-2 * -3", 6),
            ("!0 + !5", 1),
            ("~0", -1),
            ("- - 4", 4),
            ("--4", 4),
            ("+-+3", -3),
            ("1 ++ 2", 3),
            ("((((7))))", 7),
            ("2*3+4*5", 26),
        ] {
            assert_eq!(worked(text), Ok(n), "{text}");
        }
    }

    /// What C makes of `x op y`, written out apart from the shell's.
    fn c(op: &str, x: i64, y: i64) -> i64 {
        let truth = |b: bool| i64::from(b);
        match op {
            "*" => x * y,
            "/" => x / y,
            "%" => x % y,
            "+" => x + y,
            "-" => x - y,
            "<<" => x << y,
            ">>" => x >> y,
            "<" => truth(x < y),
            "<=" => truth(x <= y),
            ">" => truth(x > y),
            ">=" => truth(x >= y),
            "==" => truth(x == y),
            "!=" => truth(x != y),
            "&" => x & y,
            "^" => x ^ y,
            "|" => x | y,
            "&&" => truth(x != 0 && y != 0),
            _ => truth(x != 0 || y != 0),
        }
    }

    #[test]
    fn every_two_operators_group_as_cs_table_says() {
        // C's binary operators from the tightest, those of a row binding
        // alike and from the left.
        let rows: [&[&str]; 10] = [
            &["*", "/", "%"],
            &["+", "-"],
            &["<<", ">>"],
            &["<", "<=", ">", ">="],
            &["==", "!="],
            &["&"],
            &["^"],
            &["|"],
            &["&&"],
            &["||"],
        ];
        let row = |op: &str| rows.iter().position(|r| r.contains(&op)).unwrap();
        let (a, b, d) = (7, 3, 2);
        for first in rows.concat() {
            for second in rows.concat() {
                let text = format!("{a} {first} {b} {second} {d}");
                let want = if row(first) <= row(second) {
                    c(second, c(first, a, b), d)
                } else {
                    c(first, a, c(second, b, d))
                };
                assert_eq!(worked(&text), Ok(want), "{text}");
            }
        }
    }

    #[test]
    fn division_truncates_toward_zero_and_refuses_zero() {
        for (text, n) in [
            ("7 / 2", 3),
            ("-7 / 2", -3),
            ("-7 % 2", -1),
            ("7 % -2", 1),
            ("(-9223372036854775807 - 1) / -1", i64::MIN),
            ("(-9223372036854775807 - 1) % -1", 0),
        ] {
            assert_eq!(worked(text), Ok(n), "{text}");
        }
        for text in ["1 / 0", "1 % 0", "5 / (2 - 2)"] {
            assert_eq!(worked(text), Err(Code::DivisionByZero), "{text}");
        }
    }

    #[test]
    fn comparisons_answer_one_or_zero() {
        for (text, n) in [
            ("3 > 2", 1),
            ("3 >= 3", 1),
            ("2 < 3", 1),
            ("2 <= 1", 0),
            ("2 != 2", 0),
            ("2 == 2", 1),
            ("-1 < 0", 1),
            ("5 && 7", 1),
            ("0 && 5", 0),
            ("3 && 0", 0),
            ("!0", 1),
            ("!5", 0),
            ("3 < 3", 0),
            ("3 > 3", 0),
            ("0 || -3", 1),
            ("0 || 0", 0),
        ] {
            assert_eq!(worked(text), Ok(n), "{text}");
        }
    }

    #[test]
    fn shifts_and_sums_wrap_as_sixty_four_bits_do() {
        for (text, n) in [
            ("1 << 63", i64::MIN),
            ("1 << 64", 1),
            ("1 << -1", i64::MIN),
            ("-8 >> 1", -4),
            ("9223372036854775807 + 1", i64::MIN),
            ("-(-9223372036854775807 - 1)", i64::MIN),
            ("4611686018427387904 * 4", 0),
            ("0 - 9223372036854775807 - 2", i64::MAX),
        ] {
            assert_eq!(worked(text), Ok(n), "{text}");
        }
    }

    #[test]
    fn a_name_stands_for_its_variable_as_a_number() {
        let mut v = with(&[
            ("x", "5"),
            ("e", ""),
            ("s", " 7 "),
            ("m", "-3"),
            ("p", "+3"),
            ("h", "0x10"),
        ]);
        for (text, n) in [
            ("x * 2", 10),
            ("unset + 1", 1),
            ("e", 0),
            ("s", 7),
            ("m", -3),
            ("p", 3),
            ("h", 16),
            ("_x1 + x", 5),
        ] {
            assert_eq!(evaluate(text, &mut v), Ok(n), "{text}");
        }
        for value in ["abc", "1 2", "5x", "--5", "++5", "0x+5", "0x", "1.5"] {
            let mut v = with(&[("bad", value)]);
            assert_eq!(
                evaluate("bad + 1", &mut v),
                Err(Code::NotANumber),
                "{value}"
            );
        }
    }

    #[test]
    fn assignments_set_their_variables() {
        let mut v = with(&[("x", "10")]);
        for (text, n, after) in [
            ("x = 3", 3, "3"),
            ("x += 2", 5, "5"),
            ("x -= 1", 4, "4"),
            ("x *= 6", 24, "24"),
            ("x /= 5", 4, "4"),
            ("x %= 3", 1, "1"),
            ("x <<= 4", 16, "16"),
            ("x >>= 2", 4, "4"),
            ("x &= 6", 4, "4"),
            ("x ^= 7", 3, "3"),
            ("x |= 8", 11, "11"),
            ("x = -1", -1, "-1"),
        ] {
            assert_eq!(evaluate(text, &mut v), Ok(n), "{text}");
            assert_eq!(v.get("x").as_deref(), Some(after), "{text}");
        }
        assert_eq!(evaluate("x = y = 4", &mut v), Ok(4));
        assert_eq!(
            (v.get("x"), v.get("y")),
            (Some("4".into()), Some("4".into()))
        );
        assert_eq!(evaluate("(z = 2) + z", &mut v), Ok(4), "left first");
        assert_eq!(evaluate("new += 3", &mut v), Ok(3), "unset counts as 0");
    }

    #[test]
    fn what_an_assignment_cannot_do_is_an_error() {
        let mut v = with(&[("bad", "x"), ("n", "1")]);
        assert_eq!(evaluate("n /= 0", &mut v), Err(Code::DivisionByZero));
        assert_eq!(evaluate("bad += 1", &mut v), Err(Code::NotANumber));
        assert_eq!(
            evaluate("bad = 1", &mut v),
            Ok(1),
            "a plain = reads nothing"
        );
        assert_eq!(evaluate("1 = 2", &mut v), Err(Code::BadArithmetic));
        assert_eq!(evaluate("(n) = 2", &mut v), Err(Code::BadArithmetic));
        assert_eq!(evaluate("n == 1", &mut v), Ok(1), "not an assignment");
        assert_eq!(evaluate("n <= 1", &mut v), Ok(1));
        assert_eq!(evaluate("n >= 2", &mut v), Ok(0));
        assert_eq!(evaluate("n != 1", &mut v), Ok(0));
        assert_eq!(v.get("n").as_deref(), Some("1"));
    }

    #[test]
    fn what_and_or_and_choices_skip_is_not_worked_out() {
        let mut v = with(&[("bad", "x")]);
        for (text, n) in [
            ("0 && (y = 1)", 0),
            ("1 || (y = 1)", 1),
            ("0 && 1 / 0", 0),
            ("1 || 1 / 0", 1),
            ("1 ? 2 : (y = 3)", 2),
            ("0 ? 1 / 0 : 5", 5),
            ("0 && bad", 0),
            ("1 ? 7 : bad", 7),
            ("0 ? (y += 1) : 6", 6),
            ("0 && (y <<= 1)", 0),
        ] {
            assert_eq!(evaluate(text, &mut v), Ok(n), "{text}");
        }
        assert_eq!(v.get("y"), None, "no skipped assignment happened");
        assert_eq!(evaluate("1 && (y = 2)", &mut v), Ok(1));
        assert_eq!(v.get("y").as_deref(), Some("2"));
    }

    #[test]
    fn a_choice_takes_one_side() {
        let mut v = Map::default();
        for (text, n) in [
            ("1 ? 2 : 3", 2),
            ("0 ? 2 : 3", 3),
            ("0 ? 1 : 0 ? 2 : 3", 3),
            ("1 ? 0 ? 4 : 5 : 6", 5),
            ("1 ? x = 4 : 5", 4),
            ("(0 ? 1 : 2) + 1", 3),
        ] {
            assert_eq!(evaluate(text, &mut v), Ok(n), "{text}");
        }
        assert_eq!(v.get("x").as_deref(), Some("4"));
        for text in ["1 ? 2", "1 : 2", "1 ? : 2", "0 ? 1 : 2 = 3"] {
            assert_eq!(evaluate(text, &mut v), Err(Code::BadArithmetic), "{text}");
        }
    }

    #[test]
    fn what_is_not_arithmetic_is_refused() {
        assert_eq!(worked(""), Ok(0), "nothing is 0");
        assert_eq!(worked(" \t\n"), Ok(0));
        for text in [
            "1 +", "(1", "1)", "1 2", "@", "x y", ")", "a.b", "1,2", "*", "1 ? 2 :", "()", "!",
            "$x",
        ] {
            assert_eq!(worked(text), Err(Code::BadArithmetic), "{text}");
        }
    }

    #[test]
    fn arithmetic_from_scripts_works_out() {
        let mut v = with(&[
            ("OPTIND", "3"),
            ("a", "2"),
            ("b", "5"),
            ("c", "4"),
            ("i", "9"),
        ]);
        for (text, n) in [
            ("OPTIND - 1", 2),
            ("(a + b) * c / 2", 14),
            ("i % 2 == 1", 1),
            ("i > 5 && i < 10", 1),
            ("(i + 1) % 10", 0),
            ("1 << c", 16),
            ("b > a ? b : a", 5),
        ] {
            assert_eq!(evaluate(text, &mut v), Ok(n), "{text}");
        }
        assert_eq!(evaluate("i = i - 1", &mut v), Ok(8));
        assert_eq!(v.get("i").as_deref(), Some("8"));
    }

    #[test]
    fn parentheses_and_signs_stand_sixteen_deep() {
        let parens = |n: usize| format!("{}1{}", "(".repeat(n), ")".repeat(n));
        assert_eq!(worked(&parens(16)), Ok(1));
        assert_eq!(worked(&parens(17)), Err(Code::TooNested));
        assert_eq!(worked(&format!("{}1", "-".repeat(16))), Ok(1));
        assert_eq!(
            worked(&format!("{}1", "!".repeat(17))),
            Err(Code::TooNested)
        );
        assert_eq!(
            worked(&format!("{}1{}", "-(".repeat(8), ")".repeat(8))),
            Ok(1)
        );
        assert_eq!(
            worked(&format!("{}1{}", "-(".repeat(9), ")".repeat(9))),
            Err(Code::TooNested)
        );
        let side_by_side = vec![parens(16); 50].join(" + ");
        assert_eq!(worked(&side_by_side), Ok(50), "one after another");
        assert_eq!(worked(&vec!["-1"; 20].join(" + ")), Ok(-20));
        let long = (1..=1000)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" + ");
        assert_eq!(worked(&long), Ok(500_500), "however long");
    }

    #[test]
    fn signs_work_on_names_and_parentheses() {
        let mut v = with(&[("x", "5"), ("z", "0")]);
        for (text, n) in [
            ("-x", -5),
            ("~x", -6),
            ("!x", 0),
            ("!z", 1),
            ("+x", 5),
            ("-(2 + 3)", -5),
            ("!(1 && 0)", 1),
            ("~-1", 0),
            ("1 +\n2", 3),
            ("\t3\t*\t4", 12),
        ] {
            assert_eq!(evaluate(text, &mut v), Ok(n), "{text}");
        }
    }

    #[test]
    fn assignments_inside_assignments_and_conditions_count() {
        let mut v = with(&[("b", "1")]);
        assert_eq!(evaluate("a = b += 2", &mut v), Ok(3));
        assert_eq!(
            (v.get("a"), v.get("b")),
            (Some("3".into()), Some("3".into()))
        );
        assert_eq!(evaluate("(x = 0) || (y = 5)", &mut v), Ok(1));
        assert_eq!(
            (v.get("x"), v.get("y")),
            (Some("0".into()), Some("5".into()))
        );
        assert_eq!(evaluate("(c = 4) * (c + 1)", &mut v), Ok(20));
        assert_eq!(evaluate("x1 = 1x", &mut v), Err(Code::BadArithmetic));
    }
}
