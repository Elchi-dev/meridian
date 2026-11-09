//! date: writes the date and the time of day, as POSIX's date does, in
//! the C locale's form or in the one a word starting with `+` gives, with
//! the conversions of C's strftime. Meridian knows no zones yet, so the
//! time is UTC's, and `-u`, which asks for it, changes nothing; the time
//! cannot be set yet either. It ends with 0, or with 1 when the time is
//! not known or a word cannot be taken.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use program::Handle;
use program::date::{self, Moment};

program::main!(date);

/// What date writes for its words `arguments` at `now`, nanoseconds
/// since 1970 began, or what it says when it cannot.
fn said(arguments: &[String], now: Option<u64>) -> Result<String, String> {
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
    let form = match words {
        [] => date::DEFAULT,
        [word] => match word.strip_prefix('+') {
            Some(form) => form,
            None => return Err(String::from("date: the time cannot be set yet\n")),
        },
        _ => return Err(String::from("date: takes one +FORMAT at most\n")),
    };
    let Some(now) = now else {
        return Err(String::from(
            "date: Meridian does not know the time of day\n",
        ));
    };
    let mut text = date::formatted(form, &Moment::at(now / 1_000_000_000));
    text.push('\n');
    Ok(text)
}

#[cfg_attr(test, allow(dead_code))]
fn date(_: Handle) -> u64 {
    match said(program::arguments(), program::now()) {
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
            Err(String::from("date: the time cannot be set yet\n")),
            "after -- a word is no option"
        );
    }

    #[test]
    fn what_it_cannot_do_is_said() {
        for (line, why) in [
            ("-x", "date: -x is no option\n"),
            ("-ux +%Y", "date: -x is no option\n"),
            ("1109120026", "date: the time cannot be set yet\n"),
            ("+%Y +%m", "date: takes one +FORMAT at most\n"),
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
}
