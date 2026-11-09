//! Moments in UTC from the seconds since 1970 began, and written as
//! POSIX's `date` and C's `strftime` write them in the C locale.

use alloc::format;
use alloc::string::String;

/// A moment in UTC, by the calendar of today carried back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moment {
    /// The seconds since 1970 began.
    pub seconds: u64,
    pub year: u64,
    /// From 1, January.
    pub month: u64,
    /// From 1.
    pub day: u64,
    pub hour: u64,
    pub minute: u64,
    pub second: u64,
    /// From 0, Sunday.
    pub weekday: u64,
    /// The day of its year, from 0.
    pub day_of_year: u64,
}

/// The year, month and day `days` after 1970 began: Howard Hinnant's way,
/// from years that start in March, so that a leap day comes last.
#[must_use]
pub fn civil(days: u64) -> (u64, u64, u64) {
    let from_march = days + 719_468;
    let era = from_march / 146_097;
    let day_of_era = from_march % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    (year_of_era + era * 400 + u64::from(month <= 2), month, day)
}

/// Whether `year` has a 29 February.
#[must_use]
pub fn leap(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// The days before the first of `month` in a year, leap or not.
fn days_before(month: u64, leap: bool) -> u64 {
    const BEFORE: [u64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    BEFORE[usize::try_from(month - 1).unwrap_or(0)] + u64::from(leap && month > 2)
}

impl Moment {
    /// The moment `seconds` after 1970 began.
    #[must_use]
    pub fn at(seconds: u64) -> Moment {
        let days = seconds / 86_400;
        let of_day = seconds % 86_400;
        let (year, month, day) = civil(days);
        Moment {
            seconds,
            year,
            month,
            day,
            hour: of_day / 3600,
            minute: of_day / 60 % 60,
            second: of_day % 60,
            // 1 January 1970 was a Thursday.
            weekday: (days + 4) % 7,
            day_of_year: days_before(month, leap(year)) + day - 1,
        }
    }

    /// The week of ISO 8601 and the year it belongs to: weeks start on
    /// Monday, and the first of a year holds its first Thursday.
    #[must_use]
    pub fn iso_week(&self) -> (u64, u64) {
        // How many weeks a year has: 53 when it starts on a Thursday, or
        // on a Wednesday in a leap year.
        let weeks = |year: u64| {
            let starts = ((new_year(year) + 4) % 7, leap(year));
            if matches!(starts, (4, _) | (3, true)) {
                53
            } else {
                52
            }
        };
        let monday_based = (self.weekday + 6) % 7 + 1;
        let week = (self.day_of_year + 11 - monday_based) / 7;
        if week == 0 {
            (self.year - 1, weeks(self.year - 1))
        } else if week > weeks(self.year) {
            (self.year + 1, 1)
        } else {
            (self.year, week)
        }
    }
}

/// The days from the start of 1970 to the first of January of `year`,
/// from 1970 on: the other way of [`civil`], 1 January being day 306 of
/// a year that starts in March.
fn new_year(year: u64) -> u64 {
    let from_march = year - 1;
    let era = from_march / 400;
    let year_of_era = from_march - era * 400;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + 306 - 719_468
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

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

/// What `date` writes with no format: the C locale's `%a %b %e %H:%M:%S
/// %Z %Y`.
pub const DEFAULT: &str = "%a %b %e %H:%M:%S %Z %Y";

/// `form` with each conversion of POSIX's `date` written for `moment` in
/// UTC, as the C locale writes it; one it does not know stays as it is.
#[must_use]
pub fn formatted(form: &str, moment: &Moment) -> String {
    let mut out = String::new();
    let mut letters = form.chars();
    while let Some(c) = letters.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(letter) = letters.next() else {
            out.push('%');
            break;
        };
        if let Some(text) = conversion(letter, moment) {
            out.push_str(&text);
        } else {
            out.push('%');
            out.push(letter);
        }
    }
    out
}

/// What the conversion `letter` writes for `moment`.
fn conversion(letter: char, m: &Moment) -> Option<String> {
    let day_name = DAYS[usize::try_from(m.weekday).unwrap_or(0)];
    let month_name = MONTHS[usize::try_from(m.month - 1).unwrap_or(0)];
    let twelve = (m.hour + 11) % 12 + 1;
    Some(match letter {
        'a' => String::from(&day_name[..3]),
        'A' => String::from(day_name),
        'b' | 'h' => String::from(&month_name[..3]),
        'B' => String::from(month_name),
        'c' => formatted("%a %b %e %H:%M:%S %Y", m),
        'C' => format!("{:02}", m.year / 100),
        'd' => format!("{:02}", m.day),
        'D' | 'x' => formatted("%m/%d/%y", m),
        'e' => format!("{:2}", m.day),
        'F' => formatted("%Y-%m-%d", m),
        'g' => format!("{:02}", m.iso_week().0 % 100),
        'G' => format!("{}", m.iso_week().0),
        'H' => format!("{:02}", m.hour),
        'I' => format!("{twelve:02}"),
        'j' => format!("{:03}", m.day_of_year + 1),
        'm' => format!("{:02}", m.month),
        'M' => format!("{:02}", m.minute),
        'n' => String::from("\n"),
        'p' => String::from(if m.hour < 12 { "AM" } else { "PM" }),
        'r' => formatted("%I:%M:%S %p", m),
        'R' => formatted("%H:%M", m),
        's' => format!("{}", m.seconds),
        'S' => format!("{:02}", m.second),
        't' => String::from("\t"),
        'T' | 'X' => formatted("%H:%M:%S", m),
        'u' => format!("{}", (m.weekday + 6) % 7 + 1),
        'U' => format!("{:02}", (m.day_of_year + 7 - m.weekday) / 7),
        'V' => format!("{:02}", m.iso_week().1),
        'w' => format!("{}", m.weekday),
        'W' => format!("{:02}", (m.day_of_year + 7 - (m.weekday + 6) % 7) / 7),
        'y' => format!("{:02}", m.year % 100),
        'Y' => format!("{}", m.year),
        'z' => String::from("+0000"),
        'Z' => String::from("UTC"),
        '%' => String::from("%"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn days_and_seconds_become_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(47_541), (2100, 3, 1), "2100 has no 29 February");
        let noon = Moment::at(1_794_225_600);
        assert_eq!(
            (
                noon.year,
                noon.month,
                noon.day,
                noon.hour,
                noon.minute,
                noon.second
            ),
            (2026, 11, 9, 12, 0, 0)
        );
        assert_eq!(
            (noon.weekday, noon.day_of_year),
            (1, 312),
            "a Monday, its 313th day"
        );
        let first = Moment::at(0);
        assert_eq!((first.weekday, first.day_of_year), (4, 0), "a Thursday");
        assert_eq!(Moment::at(86_399).second, 59);
        assert!(leap(2000) && leap(2024) && !leap(1900) && !leap(2100) && !leap(2026));
        assert_eq!(new_year(1970), 0);
        assert_eq!(new_year(1971), 365);
        assert_eq!(new_year(1973), 365 * 3 + 1, "1972 was a leap year");
        assert_eq!(new_year(2026), 20_454);
    }

    #[test]
    fn the_days_of_a_year_count_on_through_its_leap_day() {
        // 2024 was a leap year: its last day was its 366th.
        let last = Moment::at((new_year(2025) - 1) * 86_400);
        assert_eq!(
            (last.year, last.month, last.day, last.day_of_year),
            (2024, 12, 31, 365)
        );
        let leap_day = Moment::at((new_year(2024) + 59) * 86_400);
        assert_eq!(
            (leap_day.month, leap_day.day, leap_day.day_of_year),
            (2, 29, 59)
        );
        let after = Moment::at((new_year(2023) + 59) * 86_400);
        assert_eq!((after.month, after.day), (3, 1), "2023 had none");
        for (month, leap, before) in [
            (1, false, 0),
            (2, true, 31),
            (3, false, 59),
            (3, true, 60),
            (12, false, 334),
            (12, true, 335),
        ] {
            assert_eq!(days_before(month, leap), before, "{month}, {leap}");
        }
        assert_eq!(Moment::at(253_402_300_799).year, 9999);
    }

    #[test]
    fn date_writes_what_the_c_locale_writes() {
        assert_eq!(
            formatted(DEFAULT, &Moment::at(0)),
            "Thu Jan  1 00:00:00 UTC 1970"
        );
        assert_eq!(
            formatted(DEFAULT, &Moment::at(1_794_225_600)),
            "Mon Nov  9 12:00:00 UTC 2026"
        );
        let m = Moment::at(1_794_225_600 + 13 * 3600 + 5);
        assert_eq!(formatted("%I %p %r", &m), "01 AM 01:00:05 AM");
        assert_eq!(
            formatted("%q|%%|%", &m),
            "%q|%|%",
            "what is no conversion stays"
        );
        assert_eq!(formatted("a%nb%tc", &m), "a\nb\tc");
        assert_eq!(formatted("", &m), "");
    }

    #[test]
    fn each_conversion_writes_its_part() {
        // Noon on a Monday in its 46th week of ISO's, and on a Friday that
        // ISO counts in the 53rd week of the year before, as 2026 began on
        // a Thursday.
        let monday = Moment::at(1_794_225_600);
        let friday = Moment::at(1_798_804_800);
        for (form, on_monday, on_friday) in [
            ("%a %A", "Mon Monday", "Fri Friday"),
            ("%b %B %h", "Nov November Nov", "Jan January Jan"),
            ("%c", "Mon Nov  9 12:00:00 2026", "Fri Jan  1 12:00:00 2027"),
            ("%C %y %Y", "20 26 2026", "20 27 2027"),
            (
                "%d|%e|%D|%x",
                "09| 9|11/09/26|11/09/26",
                "01| 1|01/01/27|01/01/27",
            ),
            ("%F %j %m", "2026-11-09 313 11", "2027-01-01 001 01"),
            ("%g %G %V", "26 2026 46", "26 2026 53"),
            ("%H %I %M %S %p", "12 12 00 00 PM", "12 12 00 00 PM"),
            (
                "%r|%R|%T|%X",
                "12:00:00 PM|12:00|12:00:00|12:00:00",
                "12:00:00 PM|12:00|12:00:00|12:00:00",
            ),
            ("%u %w %U %W", "1 1 45 45", "5 5 00 00"),
            (
                "%s %z %Z %%",
                "1794225600 +0000 UTC %",
                "1798804800 +0000 UTC %",
            ),
        ] {
            assert_eq!(formatted(form, &monday), on_monday, "{form}");
            assert_eq!(formatted(form, &friday), on_friday, "{form}");
        }
        let midnight = Moment::at(1_798_761_600);
        assert_eq!(
            formatted("%H %I %p", &midnight),
            "00 12 AM",
            "midnight is 12 AM"
        );
        let late = Moment::at(1_798_761_600 - 1);
        assert_eq!(
            formatted("%F %T %p %V %G", &late),
            "2026-12-31 23:59:59 PM 53 2026"
        );
    }

    /// Every conversion, parted by bars.
    const ALL: &str = "%a|%A|%b|%B|%c|%C|%d|%D|%e|%F|%g|%G|%h|%H|%I|%j|%m|%M|%p|%r|%R|%s|%S|%T|%u|%U|%V|%w|%W|%x|%X|%y|%Y|%z|%Z|%%";

    #[test]
    fn every_conversion_matches_gnu_date() {
        use core::fmt::Write as _;
        use std::io::Write as _;
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut moments: Vec<u64> = (0..600)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed % 253_402_300_800
            })
            .collect();
        // The days around each new year, where ISO's weeks change years.
        for year in 1971..2040 {
            for day in 0..8 {
                moments.push((new_year(year) + day) * 86_400 - 4 * 86_400 + 43_210);
            }
        }
        let mut asked = String::new();
        for seconds in &moments {
            let _ = writeln!(asked, "@{seconds}");
        }
        let mut child = std::process::Command::new("date")
            .args(["-u", "-f", "-", &format!("+{ALL}")])
            .env("LC_ALL", "C")
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
        for (seconds, theirs) in moments.iter().zip(said.lines()) {
            assert_eq!(formatted(ALL, &Moment::at(*seconds)), theirs, "@{seconds}");
        }
        assert_eq!(said.lines().count(), moments.len());
    }
}
