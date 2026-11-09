//! Days and dates from 1970 on, in the calendar of today carried back,
//! as clocks, file systems and programs count time: Howard Hinnant's
//! ways, from years that start in March, so that a leap day comes last.

#![cfg_attr(not(test), no_std)]

/// Whether `year` has a 29 February.
#[must_use]
pub fn leap(year: u64) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// How many days `month` of `year` has.
#[must_use]
pub fn days_in(year: u64, month: u64) -> u64 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The year, month and day `days` after 1970 began.
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

/// The days from the start of 1970 to `day` of `month` of `year`, the
/// other way of [`civil`]: for a year from 1970 on, and a month and a day
/// it has.
#[must_use]
pub fn days(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_days_are_the_dates_they_were() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_675), (2023, 11, 14));
        assert_eq!(civil(11_016), (2000, 2, 29), "a leap day");
        assert_eq!(civil(47_540), (2100, 2, 28));
        assert_eq!(civil(47_541), (2100, 3, 1), "2100 has no 29 February");
        assert_eq!(civil(2_932_896), (9999, 12, 31));
        assert_eq!(days(1970, 1, 1), 0);
        assert_eq!(days(1971, 1, 1), 365);
        assert_eq!(days(1973, 1, 1), 1096, "1972 was a leap year");
        assert_eq!(days(2026, 11, 9), 20_766);
        assert_eq!(days(9999, 12, 31), 2_932_896);
    }

    #[test]
    fn every_day_to_9999_goes_there_and_back() {
        let mut previous = (1969, 12, 31);
        for day in 0..=2_932_896 {
            let date = civil(day);
            let (year, month, of) = date;
            assert_eq!(days(year, month, of), day, "{date:?}");
            let next_day = (previous.0, previous.1, previous.2 + 1);
            let next_month = (previous.0, previous.1 + 1, 1);
            let next_year = (previous.0 + 1, 1, 1);
            assert!(
                date == next_day || date == next_month || date == next_year,
                "{previous:?} then {date:?}"
            );
            assert!((1..=days_in(year, month)).contains(&of), "{date:?}");
            previous = date;
        }
    }

    #[test]
    fn leap_years_are_those_of_today_s_calendar() {
        for (year, is) in [
            (1972, true),
            (1900, false),
            (2000, true),
            (2024, true),
            (2026, false),
            (2100, false),
            (2400, true),
        ] {
            assert_eq!(leap(year), is, "{year}");
        }
        assert_eq!(days_in(2024, 2), 29);
        assert_eq!(days_in(2026, 2), 28);
        for (month, length) in [
            (1, 31),
            (3, 31),
            (4, 30),
            (6, 30),
            (7, 31),
            (8, 31),
            (9, 30),
            (11, 30),
            (12, 31),
        ] {
            assert_eq!(days_in(2026, month), length, "month {month}");
        }
    }

    #[test]
    fn random_days_are_the_dates_date_gives() {
        use std::fmt::Write as _;
        use std::io::Write as _;
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let picked: Vec<u64> = (0..500)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed % 2_932_897
            })
            .collect();
        let mut asked = String::new();
        for day in &picked {
            let _ = writeln!(asked, "@{}", day * 86_400);
        }
        let mut child = std::process::Command::new("date")
            .args(["-u", "-f", "-", "+%Y %m %d"])
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
        for (day, date) in picked.iter().zip(said.lines()) {
            let (year, month, of) = civil(*day);
            assert_eq!(format!("{year:04} {month:02} {of:02}"), date, "day {day}");
        }
        assert_eq!(said.lines().count(), picked.len());
    }
}
