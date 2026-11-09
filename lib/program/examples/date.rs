//! date: writes the date and the time of day, as POSIX's date does, in
//! the C locale's form or in the one a word starting with `+` gives, with
//! the conversions of C's strftime; or sets the time to the one a word
//! gives as `mmddhhmm[[cc]yy]` and writes it, which a token with
//! `SeSystemtimePrivilege` may, until the machine stops (decision record
//! 0031). Meridian knows no zones yet, so the time is UTC's, and `-u`,
//! which asks for it, changes nothing. It ends with 0, or with 1 when the
//! time is not known or cannot be set, or a word cannot be taken.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::date::{self, Moment};
use program::{Error, Handle};

program::main!(date);

/// What date's words ask for.
#[derive(Debug, PartialEq, Eq)]
enum Asked<'w> {
    /// The time of day, written in the form given.
    Shown(&'w str),
    /// The time of day set to the one the word gives.
    Set(&'w str),
}

/// What date's words `arguments` ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        if let Some(letter) = letters.chars().find(|&c| c != 'u') {
            return Err(format!("date: -{letter} is no option\n"));
        }
        words = rest;
    }
    match words {
        [] => Ok(Asked::Shown(date::DEFAULT)),
        [word] => Ok(word
            .strip_prefix('+')
            .map_or(Asked::Set(word), Asked::Shown)),
        _ => Err(String::from("date: takes one +FORMAT or time at most\n")),
    }
}

/// What date writes of `now`, nanoseconds since 1970 began, in `form`, or
/// what it says when the time is not known.
fn shown(form: &str, now: Option<u64>) -> Result<String, String> {
    let Some(now) = now else {
        return Err(String::from(
            "date: Meridian does not know the time of day\n",
        ));
    };
    let mut text = date::formatted(form, &Moment::at(now / 1_000_000_000));
    text.push('\n');
    Ok(text)
}

/// The nanoseconds since 1970 began at the time `word` gives as
/// `mmddhhmm[[cc]yy]`: in the year of `now` when it names none, and for a
/// `yy` alone in the 1900s from 69 on and in the 2000s below, as POSIX
/// has; or why it cannot be taken.
fn set_to(word: &str, now: Option<u64>) -> Result<u64, String> {
    let wrong = || format!("date: {word} is no time as mmddhhmm[[cc]yy]\n");
    if !word.bytes().all(|b| b.is_ascii_digit()) {
        return Err(wrong());
    }
    let two = |at: usize| {
        let b = word.as_bytes();
        u64::from(b[at] - b'0') * 10 + u64::from(b[at + 1] - b'0')
    };
    let year = match word.len() {
        8 => {
            let now = now.ok_or_else(|| {
                String::from(
                    "date: Meridian does not know the time of day, so the year must be given\n",
                )
            })?;
            calendar::civil(now / 1_000_000_000 / 86_400).0
        }
        10 => {
            let short = two(8);
            short + if short >= 69 { 1900 } else { 2000 }
        }
        12 => two(8) * 100 + two(10),
        _ => return Err(wrong()),
    };
    let (month, day, hour, minute) = (two(0), two(2), two(4), two(6));
    let fits = year >= 1970
        && (1..=12).contains(&month)
        && (1..=calendar::days_in(year, month)).contains(&day)
        && hour < 24
        && minute < 60;
    if !fits {
        return Err(wrong());
    }
    let seconds = calendar::days(year, month, day) * 86_400 + hour * 3600 + minute * 60;
    // Past 2554 the nanoseconds take more than 64 bits.
    seconds.checked_mul(1_000_000_000).ok_or_else(wrong)
}

/// What date says once it asked for the time to be `nanoseconds` since
/// 1970 began and was answered `set`: the time, as it writes it with no
/// form.
fn after_setting(nanoseconds: u64, set: Result<(), Error>) -> Result<String, String> {
    match set {
        Ok(()) => shown(date::DEFAULT, Some(nanoseconds)),
        Err(Error::Denied) => Err(String::from(
            "date: setting the time takes SeSystemtimePrivilege\n",
        )),
        Err(_) => Err(String::from("date: the time could not be set\n")),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn date(_: Handle) -> u64 {
    let said = asked(program::arguments()).and_then(|asked| match asked {
        Asked::Shown(form) => shown(form, program::now()),
        Asked::Set(word) => {
            set_to(word, program::now()).and_then(|at| after_setting(at, program::set_time(at)))
        }
    });
    match said {
        Ok(text) => u64::from(program::write_bytes(text.as_bytes()).is_err()),
        Err(why) => {
            let _ = program::write_error(&why);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// 12:00 UTC on 9 November 2026 and a little, in nanoseconds.
    const NOON: u64 = 1_794_225_600_000_000_000 + 999_999_999;

    /// What date writes for the words `line` at `now`, or for a time to
    /// set the nanoseconds it is.
    fn said(line: &[String], now: Option<u64>) -> Result<String, String> {
        match asked(line)? {
            Asked::Shown(form) => shown(form, now),
            Asked::Set(word) => set_to(word, now).map(|at| format!("set {at}")),
        }
    }

    #[test]
    fn it_writes_the_time_in_the_c_locale_s_form_or_the_one_given() {
        assert_eq!(
            said(&words(""), Some(NOON)).as_deref(),
            Ok("Mon Nov  9 12:00:00 UTC 2026\n"),
            "the nanoseconds are cut"
        );
        assert_eq!(
            said(&words("-u"), Some(NOON)),
            said(&words(""), Some(NOON)),
            "-u changes nothing"
        );
        assert_eq!(
            said(&[String::from("+%F %H:%M, day %j")], Some(NOON)).as_deref(),
            Ok("2026-11-09 12:00, day 313\n"),
            "a form is one word, blanks and all"
        );
        assert_eq!(
            said(&words("-uu -- +%Y"), Some(NOON)).as_deref(),
            Ok("2026\n")
        );
        assert_eq!(
            said(&words("+"), Some(NOON)).as_deref(),
            Ok("\n"),
            "an empty form"
        );
        assert_eq!(
            said(&words("-- -u"), Some(NOON)),
            Err(String::from("date: -u is no time as mmddhhmm[[cc]yy]\n")),
            "after -- a word is no option"
        );
    }

    #[test]
    fn what_it_cannot_do_is_said() {
        for (line, why) in [
            ("-x", "date: -x is no option\n"),
            ("-ux +%Y", "date: -x is no option\n"),
            ("+%Y +%m", "date: takes one +FORMAT or time at most\n"),
            (
                "1109120026 +%Y",
                "date: takes one +FORMAT or time at most\n",
            ),
        ] {
            assert_eq!(
                said(&words(line), Some(NOON)),
                Err(String::from(why)),
                "{line}"
            );
        }
        assert_eq!(
            said(&words(""), None),
            Err(String::from(
                "date: Meridian does not know the time of day\n"
            ))
        );
        assert_eq!(
            said(&words("-x"), None),
            Err(String::from("date: -x is no option\n")),
            "the words come first"
        );
    }

    #[test]
    fn a_time_to_set_is_mmddhhmm_with_a_year_or_this_one() {
        let at = |seconds: u64| Ok(seconds * 1_000_000_000);
        for (word, seconds) in [
            ("11091230", at(1_794_227_400)),
            ("1109123026", at(1_794_227_400)),
            ("110912302026", at(1_794_227_400)),
            ("0101000070", at(0)),
            ("0101000068", at(3_092_601_600)),
            ("1231235999", at(946_684_740)),
            ("0229000024", at(1_709_164_800)),
            ("010100002554", at(18_429_292_800)),
        ] {
            assert_eq!(set_to(word, Some(NOON)), seconds, "{word}");
        }
        assert_eq!(
            set_to(
                "01010000",
                Some(315_532_800_000_000_000 + 86_400_000_000_000 * 400)
            ),
            at(347_155_200),
            "the year of the time of day, 1981 here"
        );
        for word in [
            "123123591969",
            "0101000069",
            "1301000026",
            "0001000026",
            "0100000026",
            "0229000026",
            "1109240026",
            "1109126026",
            "110912",
            "11091230x",
            "110912302",
            "11091230202",
            "1109123020261",
            "",
            "010100002555",
        ] {
            assert_eq!(
                set_to(word, Some(NOON)),
                Err(format!("date: {word} is no time as mmddhhmm[[cc]yy]\n")),
                "{word:?}"
            );
        }
        assert_eq!(
            set_to("11091230", None),
            Err(String::from(
                "date: Meridian does not know the time of day, so the year must be given\n"
            ))
        );
        assert_eq!(
            set_to("1109123026", None),
            at(1_794_227_400),
            "a year needs no time of day"
        );
    }

    #[test]
    fn times_to_set_are_the_ones_gnu_s_date_reads() {
        use core::fmt::Write as _;
        use std::io::Write as _;
        let mut seed = 0x8f1b_bcdc_bfa5_3e0b_u64;
        let mut next = |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        let mut words = Vec::new();
        let mut asked = String::new();
        for _ in 0..300 {
            let year = 1970 + next(580);
            let month = 1 + next(12);
            let day = 1 + next(calendar::days_in(year, month));
            let (hour, minute) = (next(24), next(60));
            words.push(format!("{month:02}{day:02}{hour:02}{minute:02}{year}"));
            let _ = writeln!(
                asked,
                "{year}-{month:02}-{day:02} {hour:02}:{minute:02} UTC"
            );
        }
        let mut child = std::process::Command::new("date")
            .args(["-u", "-f", "-", "+%s"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(asked.as_bytes())
            .unwrap();
        let done = child.wait_with_output().unwrap();
        assert!(done.status.success());
        let theirs = String::from_utf8(done.stdout).unwrap();
        for (word, seconds) in words.iter().zip(theirs.lines()) {
            let seconds: u64 = seconds.parse().unwrap();
            assert_eq!(
                set_to(word, Some(NOON)),
                seconds
                    .checked_mul(1_000_000_000)
                    .ok_or_else(|| format!("date: {word} is no time as mmddhhmm[[cc]yy]\n")),
                "{word}"
            );
        }
        assert_eq!(theirs.lines().count(), words.len());
    }

    #[test]
    fn a_time_set_is_written_or_why_not_said() {
        assert_eq!(
            after_setting(NOON, Ok(())).as_deref(),
            Ok("Mon Nov  9 12:00:00 UTC 2026\n")
        );
        assert_eq!(
            after_setting(NOON, Err(Error::Denied)),
            Err(String::from(
                "date: setting the time takes SeSystemtimePrivilege\n"
            ))
        );
        assert_eq!(
            after_setting(NOON, Err(Error::BadHandle)),
            Err(String::from("date: the time could not be set\n"))
        );
    }
}
