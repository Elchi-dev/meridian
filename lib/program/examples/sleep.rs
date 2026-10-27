//! sleep: waits as many seconds as it is given, as POSIX's sleep does. A
//! number with a fraction, as `0.5`, waits that part of a second too, in
//! hundredths, as long as a tick of the timer.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::Handle;
use program::abi::TICKS_A_SECOND;

program::main!(sleep);

/// The ticks `text` seconds come to: digits, and a point and digits after
/// it for a part of a second, of which hundredths count; none for a text
/// that is no such number, or one too long to wait.
fn ticks(text: &str) -> Option<u64> {
    let (whole, part) = text.split_once('.').unwrap_or((text, ""));
    let digits = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
    if (whole.is_empty() && part.is_empty()) || !digits(whole) || !digits(part) {
        return None;
    }
    let seconds: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let hundredths = part
        .bytes()
        .chain(core::iter::repeat(b'0'))
        .take(2)
        .fold(0, |value, digit| value * 10 + u64::from(digit - b'0'));
    seconds
        .checked_mul(TICKS_A_SECOND)?
        .checked_add(hundredths * TICKS_A_SECOND / 100)
}

/// The ticks sleep's words ask it to wait, after a `--` or not.
fn asked(arguments: &[String]) -> Result<u64, String> {
    let words = match arguments {
        [dashes, rest @ ..] if dashes == "--" => rest,
        words => words,
    };
    match words {
        [time] => ticks(time).ok_or_else(|| format!("sleep: {time} is no number of seconds\n")),
        [] => Err(String::from("sleep: needs a time to wait\n")),
        _ => Err(String::from("sleep: takes one time\n")),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn sleep(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(ticks) => {
            program::sleep(ticks);
            0
        }
        Err(why) => {
            let _ = program::write_error(&why);
            1
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

    #[test]
    fn seconds_are_ticks_of_the_timer() {
        assert_eq!(ticks("0"), Some(0));
        assert_eq!(ticks("1"), Some(TICKS_A_SECOND));
        assert_eq!(ticks("3"), Some(3 * TICKS_A_SECOND));
        assert_eq!(ticks("007"), Some(7 * TICKS_A_SECOND));
        assert_eq!(TICKS_A_SECOND, 100, "a tick is a hundredth of a second");
    }

    #[test]
    fn a_fraction_counts_in_hundredths() {
        assert_eq!(ticks("0.5"), Some(50));
        assert_eq!(ticks(".5"), Some(50));
        assert_eq!(ticks("1.25"), Some(125));
        assert_eq!(ticks("0.05"), Some(5));
        assert_eq!(ticks("0.009"), Some(0), "less than a tick");
        assert_eq!(ticks("2.999"), Some(299), "hundredths, not rounded up");
        assert_eq!(ticks("5."), Some(500));
    }

    #[test]
    fn what_is_no_number_of_seconds_is_none() {
        for text in [
            "", ".", "-1", "+1", "1e3", "1.2.3", "a", "1 ", "½", "٣", "1,5",
        ] {
            assert_eq!(ticks(text), None, "{text:?}");
        }
        assert_eq!(
            ticks("184467440737095517"),
            None,
            "longer than the ticks count"
        );
        assert_eq!(
            ticks("18446744073709551616"),
            None,
            "longer than a number holds"
        );
        assert_eq!(
            ticks("184467440737095516"),
            Some(18_446_744_073_709_551_600)
        );
    }

    #[test]
    fn it_takes_one_time_after_a_dash_dash_or_not() {
        assert_eq!(asked(&words("2")), Ok(200));
        assert_eq!(asked(&words("-- 2")), Ok(200));
        assert_eq!(
            asked(&words("--")),
            Err(String::from("sleep: needs a time to wait\n"))
        );
        assert_eq!(
            asked(&[]),
            Err(String::from("sleep: needs a time to wait\n"))
        );
        assert_eq!(
            asked(&words("1 2")),
            Err(String::from("sleep: takes one time\n"))
        );
        assert_eq!(
            asked(&words("-1")),
            Err(String::from("sleep: -1 is no number of seconds\n"))
        );
        assert_eq!(
            asked(&words("-- -- 1")),
            Err(String::from("sleep: takes one time\n")),
            "one -- only"
        );
    }
}
