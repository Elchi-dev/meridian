//! cal: writes a calendar, as POSIX's cal does: of the month the time of
//! day falls in with no operands, of a whole year with one, and of a
//! month of a year with two, for the years 1 to 9999, in the calendar
//! England and its colonies kept: Julius Caesar's to 2 September 1752,
//! Pope Gregory's from the 14th that followed it. Its weeks start on
//! Sunday, laid out as util-linux's cal lays them out. It ends with 0, or
//! with 1 when an operand is no month or year it has, or the time of day
//! is not known.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::Handle;
use program::date::Moment;

program::main!(cal);

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// How wide a month is, and the room between months in a year.
const WIDTH: usize = 20;
const GAP: &str = "   ";

/// Whether the day lies before Gregory's calendar came in, on 14
/// September 1752.
fn julian(year: i64, month: i64, day: i64) -> bool {
    (year, month, day) < (1752, 9, 14)
}

/// The Julian day number of the day, by the calendar in force on it.
fn day_number(year: i64, month: i64, day: i64) -> i64 {
    // Fliegel and Van Flandern's ways, with division toward zero.
    if julian(year, month, day) {
        367 * year - (7 * (year + 5001 + (month - 9) / 7)) / 4 + (275 * month) / 9 + day + 1_729_777
    } else {
        let shift = (month - 14) / 12;
        (1461 * (year + 4800 + shift)) / 4 + (367 * (month - 2 - 12 * shift)) / 12
            - (3 * ((year + 4900 + shift) / 100)) / 4
            + day
            - 32_075
    }
}

/// The day of the week, from 0 for Sunday.
fn weekday(year: i64, month: i64, day: i64) -> usize {
    usize::try_from((day_number(year, month, day) + 1) % 7).unwrap_or(0)
}

/// The days a month has, by their numbers.
fn days(year: i64, month: i64) -> Vec<i64> {
    if (year, month) == (1752, 9) {
        // The eleven days the change of calendar took.
        return [1, 2].into_iter().chain(14..=30).collect();
    }
    let leap = if julian(year, 2, 29) {
        year % 4 == 0
    } else {
        year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
    };
    let last = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=last).collect()
}

/// `text` in the middle of `width` columns, the odd one of room before it.
fn centred(text: &str, width: usize) -> String {
    let room = width.saturating_sub(text.len());
    let before = room.div_ceil(2);
    format!("{}{text}{}", " ".repeat(before), " ".repeat(room - before))
}

/// The eight lines of a month: its name, with its year when `titled`, the
/// days of the week, and six weeks, each line as wide as a month is.
fn month_lines(year: i64, month: i64, titled: bool) -> [String; 8] {
    let name = MONTHS[usize::try_from(month - 1).unwrap_or(0)];
    let title = if titled {
        format!("{name} {year:04}")
    } else {
        String::from(name)
    };
    let mut lines: [String; 8] = Default::default();
    lines[0] = centred(&title, WIDTH);
    lines[1] = String::from("Su Mo Tu We Th Fr Sa");
    let mut cells: Vec<String> = alloc::vec![String::from("  "); weekday(year, month, 1)];
    cells.extend(days(year, month).iter().map(|day| format!("{day:2}")));
    cells.resize(42, String::from("  "));
    for (line, week) in lines[2..].iter_mut().zip(cells.chunks(7)) {
        *line = week.join(" ");
    }
    lines
}

/// The calendar of `month` of `year`.
fn month(year: i64, month: i64) -> String {
    let mut out = month_lines(year, month, true).join("\n");
    out.push('\n');
    out
}

/// The calendar of all of `year`, three months to a row.
fn year(year: i64) -> String {
    let mut out = centred(&format!("{year:04}"), 3 * WIDTH + 2 * GAP.len());
    out.push_str("\n\n");
    for first in [1, 4, 7, 10] {
        let months = [first, first + 1, first + 2].map(|m| month_lines(year, m, false));
        for row in 0..8 {
            let line: Vec<&str> = months.iter().map(|lines| lines[row].as_str()).collect();
            out.push_str(&line.join(GAP));
            out.push('\n');
        }
    }
    out
}

/// The number `word` writes, from `least` to `most`.
fn number(word: &str, least: i64, most: i64) -> Option<i64> {
    word.parse::<i64>()
        .ok()
        .filter(|n| (least..=most).contains(n))
}

/// What cal writes for its words `arguments` at `now`, nanoseconds since
/// 1970 began, or what it says when it cannot.
fn said(arguments: &[String], now: Option<u64>) -> Result<String, String> {
    let operands = match arguments.split_first() {
        Some((first, rest)) if first == "--" => rest,
        Some((first, _)) if first.starts_with('-') && first.len() > 1 => {
            return Err(format!("cal: {first} is no option\n"));
        }
        _ => arguments,
    };
    let as_year = |word: &str| {
        number(word, 1, 9999).ok_or_else(|| format!("cal: {word} is no year from 1 to 9999\n"))
    };
    match operands {
        [] => {
            let Some(now) = now else {
                return Err(String::from(
                    "cal: Meridian does not know the time of day\n",
                ));
            };
            let at = Moment::at(now / 1_000_000_000);
            let (y, m) = (
                i64::try_from(at.year).unwrap_or(0),
                i64::try_from(at.month).unwrap_or(0),
            );
            Ok(month(y, m))
        }
        [only] => Ok(year(as_year(only)?)),
        [m, y] => {
            let m =
                number(m, 1, 12).ok_or_else(|| format!("cal: {m} is no month from 1 to 12\n"))?;
            Ok(month(as_year(y)?, m))
        }
        _ => Err(String::from("cal: takes a month and a year at most\n")),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn cal(_: Handle) -> u64 {
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

    #[test]
    fn months_and_years_are_laid_out_as_util_linux_cal_lays_them_out() {
        let months = [
            (1752, 9),
            (1, 1),
            (1900, 2),
            (2000, 2),
            (9999, 12),
            (1582, 10),
        ]
        .map(|(y, m)| month(y, m))
        .concat();
        assert_eq!(months, include_str!("../testdata/cal/months.txt"));
        assert_eq!(year(1752), include_str!("../testdata/cal/1752.txt"));
        assert_eq!(year(2026), include_str!("../testdata/cal/2026.txt"));
    }

    #[test]
    fn the_calendar_changes_on_14_september_1752() {
        assert_eq!(
            day_number(1752, 9, 14),
            day_number(1752, 9, 2) + 1,
            "the day after the 2nd was the 14th"
        );
        assert_eq!((weekday(1752, 9, 2), weekday(1752, 9, 14)), (3, 4));
        assert!(julian(1752, 9, 13) && !julian(1752, 9, 14) && !julian(1753, 1, 1));
        assert_eq!(days(1752, 9).len(), 19);
        assert_eq!(
            days(1700, 2).len(),
            29,
            "a Julian year divisible by 100 leaps"
        );
        assert_eq!(days(1800, 2).len(), 28, "a Gregorian one does not");
        assert_eq!(days(2000, 2).len(), 29, "unless divisible by 400");
        assert_eq!(days(1752, 2).len(), 29);
        assert_eq!(days(2026, 11).len(), 30);
        assert_eq!(
            weekday(1, 1, 1),
            6,
            "1 January of the year 1 was a Saturday"
        );
        assert_eq!(
            day_number(2000, 1, 1),
            2_451_545,
            "the day astronomers count from J2000"
        );
        assert_eq!(
            day_number(1582, 10, 15),
            2_299_171,
            "Julian's 15 October 1582"
        );
    }

    #[test]
    fn gregorian_days_fall_on_the_weekdays_date_gives() {
        use std::io::Write as _;
        let mut seed = 0x6a09_e667_f3bc_c908_u64;
        let mut next = |below: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            i64::try_from(seed % u64::try_from(below).unwrap()).unwrap()
        };
        let dates: Vec<(i64, i64, i64)> = (0..400)
            .map(|_| {
                let year = 1753 + next(8247);
                let month = 1 + next(12);
                let last = i64::try_from(days(year, month).len()).unwrap();
                (year, month, 1 + next(last))
            })
            .collect();
        let asked: String = dates
            .iter()
            .map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}\n"))
            .collect::<Vec<_>>()
            .concat();
        let mut child = std::process::Command::new("date")
            .args(["-u", "-f", "-", "+%w"])
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
        let said = String::from_utf8(done.stdout).unwrap();
        for (&(y, m, d), theirs) in dates.iter().zip(said.lines()) {
            assert_eq!(weekday(y, m, d).to_string(), theirs, "{y}-{m}-{d}");
        }
        assert_eq!(said.lines().count(), dates.len());
    }

    /// 12:00 UTC on 9 November 2026, in nanoseconds.
    const NOON: u64 = 1_794_225_600_000_000_000;

    #[test]
    fn operands_say_which_calendar() {
        assert_eq!(
            said(&words(""), Some(NOON)),
            Ok(month(2026, 11)),
            "the month at hand"
        );
        assert_eq!(said(&words("2026"), Some(NOON)), Ok(year(2026)));
        assert_eq!(
            said(&words("11 2026"), None),
            Ok(month(2026, 11)),
            "no clock needed"
        );
        assert_eq!(said(&words("-- 9 1752"), None), Ok(month(1752, 9)));
        assert_eq!(
            said(&words("12"), None),
            Ok(year(12)),
            "one operand is a year"
        );
        assert_eq!(
            said(&words("12 9999"), None),
            Ok(month(9999, 12)),
            "the last month of the last year"
        );
        assert_eq!(said(&words("1 1"), None), Ok(month(1, 1)), "and the first");
        for (line, why) in [
            ("13 2026", "cal: 13 is no month from 1 to 12\n"),
            ("0 2026", "cal: 0 is no month from 1 to 12\n"),
            ("x 2026", "cal: x is no month from 1 to 12\n"),
            ("1 0", "cal: 0 is no year from 1 to 9999\n"),
            ("1 10000", "cal: 10000 is no year from 1 to 9999\n"),
            ("-1", "cal: -1 is no option\n"),
            ("-y", "cal: -y is no option\n"),
            ("1 2 3", "cal: takes a month and a year at most\n"),
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
                "cal: Meridian does not know the time of day\n"
            ))
        );
        assert_eq!(
            said(&words("-"), None),
            Err(String::from("cal: - is no year from 1 to 9999\n"))
        );
    }

    #[test]
    fn each_month_starts_the_day_after_the_last_one_ended() {
        let total = |year| (1..=12).map(|m| days(year, m).len()).sum::<usize>();
        assert_eq!(total(2026), 365);
        assert_eq!(total(2024), 366);
        assert_eq!(total(1700), 366, "a Julian leap year");
        assert_eq!(total(1800), 365);
        assert_eq!(total(1752), 355, "eleven days gone");
        for year in [
            1, 4, 100, 1600, 1700, 1751, 1752, 1753, 1900, 2000, 2026, 9998,
        ] {
            for month in 1..=12 {
                let last = *days(year, month).last().unwrap();
                let next = if month == 12 {
                    day_number(year + 1, 1, 1)
                } else {
                    day_number(year, month + 1, 1)
                };
                assert_eq!(
                    next,
                    day_number(year, month, last) + 1,
                    "after {year}-{month}-{last}"
                );
            }
        }
    }

    #[test]
    fn a_year_is_four_rows_of_three_months_each_line_as_wide() {
        let text = year(2024);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 34);
        for (at, line) in lines.iter().enumerate() {
            assert_eq!(line.len(), if at == 1 { 0 } else { 66 }, "line {at}");
        }
        assert_eq!(
            &lines[8][23..43],
            "25 26 27 28 29      ",
            "2024's 29 February"
        );
    }

    #[test]
    fn titles_sit_in_the_middle_the_odd_column_before() {
        assert_eq!(centred("ab", 6), "  ab  ");
        assert_eq!(centred("abc", 6), "  abc ");
        assert_eq!(centred("abcdefg", 6), "abcdefg", "too long for its room");
        assert_eq!(centred("", 3), "   ");
    }
}
