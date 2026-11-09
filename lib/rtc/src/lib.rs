//! The PC's real-time clock, as the IBM PC AT's MC146818 keeps the time
//! in its CMOS RAM (decision record 0033): where each part lies, and the
//! time in the registers and back, in binary or in binary-coded decimal
//! and in 24 hours or 12, as status register B says.

#![cfg_attr(not(test), no_std)]

/// The registers of the seconds, the minutes, the hours, the day of the
/// month, the month and the year in its century.
pub const SECONDS: u8 = 0x00;
pub const MINUTES: u8 = 0x02;
pub const HOURS: u8 = 0x04;
pub const DAY: u8 = 0x07;
pub const MONTH: u8 = 0x08;
pub const YEAR: u8 = 0x09;
/// Status register A, whose top bit says the clock is updating the
/// time, and B, which says the form of the time and holds the clock still.
pub const STATUS_A: u8 = 0x0a;
pub const STATUS_B: u8 = 0x0b;

/// Register A's bit that says an update is in progress.
pub const UPDATING: u8 = 1 << 7;
/// Register B's bits: the clock held still to be set, its numbers in
/// binary rather than in binary-coded decimal, and its hours in 24.
pub const SET: u8 = 1 << 7;
pub const BINARY: u8 = 1 << 2;
pub const HOURS_24: u8 = 1 << 1;
/// The bit of the hours that says afternoon, in 12 hours.
pub const PM: u8 = 1 << 7;

/// The time in the clock's registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registers {
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub day: u8,
    pub month: u8,
    /// The year in its century, and the century, which a register the
    /// ACPI FADT names keeps when the machine has one.
    pub year: u8,
    pub century: u8,
}

/// `n`, below 100, in binary-coded decimal: its tens in the high four
/// bits and its ones in the low.
fn bcd(n: u8) -> u8 {
    ((n / 10) << 4) | (n % 10)
}

/// The number the binary-coded decimal `b` holds; none for one that is no
/// two decimal digits.
fn from_bcd(b: u8) -> Option<u8> {
    let (tens, ones) = (b >> 4, b & 0x0f);
    (tens < 10 && ones < 10).then_some(tens * 10 + ones)
}

impl Registers {
    /// What the registers hold for `seconds` since 1970 began, in UTC and
    /// in the form `status_b` gives; none for a year past 9999.
    #[must_use]
    pub fn of(seconds: u64, status_b: u8) -> Option<Registers> {
        let (year, month, day) = calendar::civil(seconds / 86_400);
        let of_day = seconds % 86_400;
        let [hour, minute, second] =
            [of_day / 3600, of_day / 60 % 60, of_day % 60].map(|n| u8::try_from(n).unwrap_or(0));
        if year > 9999 {
            return None;
        }
        let [century, year, month, day] =
            [year / 100, year % 100, month, day].map(|n| u8::try_from(n).unwrap_or(0));
        let form = |n: u8| if status_b & BINARY == 0 { bcd(n) } else { n };
        let hours = if status_b & HOURS_24 == 0 {
            // Midnight is 12 in the morning, noon 12 in the afternoon.
            let twelve = (hour + 11) % 12 + 1;
            form(twelve) | if hour >= 12 { PM } else { 0 }
        } else {
            form(hour)
        };
        Some(Registers {
            seconds: form(second),
            minutes: form(minute),
            hours,
            day: form(day),
            month: form(month),
            year: form(year),
            century: form(century),
        })
    }

    /// The seconds since 1970 began the registers say, read in the form
    /// `status_b` gives; the century from its register when `century`
    /// says there is one, else the 1900s for years from 70 on and the
    /// 2000s below. None for registers that make no time, or one before
    /// 1970.
    #[must_use]
    pub fn seconds(&self, status_b: u8, century: bool) -> Option<u64> {
        let read = |n: u8| {
            if status_b & BINARY == 0 {
                from_bcd(n)
            } else {
                Some(n)
            }
        };
        let hour = if status_b & HOURS_24 == 0 {
            let twelve = read(self.hours & !PM)?;
            if !(1..=12).contains(&twelve) {
                return None;
            }
            twelve % 12 + if self.hours & PM == 0 { 0 } else { 12 }
        } else {
            read(self.hours)?
        };
        let short = u64::from(read(self.year)?);
        let year = if century {
            u64::from(read(self.century)?) * 100 + short
        } else {
            short + if short >= 70 { 1900 } else { 2000 }
        };
        let (month, day) = (u64::from(read(self.month)?), u64::from(read(self.day)?));
        let (minute, second) = (read(self.minutes)?, read(self.seconds)?);
        let fits = year >= 1970
            && (1..=12).contains(&month)
            && (1..=calendar::days_in(year, month)).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60;
        fits.then(|| {
            calendar::days(year, month, day) * 86_400
                + u64::from(hour) * 3600
                + u64::from(minute) * 60
                + u64::from(second)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The forms status register B gives: BCD or binary, 12 hours or 24.
    const FORMS: [u8; 4] = [0, BINARY, HOURS_24, BINARY | HOURS_24];

    /// 12:34:56 UTC on 9 November 2026.
    const AFTERNOON: u64 = 1_794_227_696;

    fn registers(values: [u8; 7]) -> Registers {
        let [seconds, minutes, hours, day, month, year, century] = values;
        Registers {
            seconds,
            minutes,
            hours,
            day,
            month,
            year,
            century,
        }
    }

    #[test]
    fn a_time_goes_into_the_registers_in_each_form() {
        for (status, values) in [
            (HOURS_24, [0x56, 0x34, 0x12, 0x09, 0x11, 0x26, 0x20]),
            (HOURS_24 | BINARY, [56, 34, 12, 9, 11, 26, 20]),
            (0, [0x56, 0x34, 0x92, 0x09, 0x11, 0x26, 0x20]),
            (BINARY, [56, 34, PM | 0x0c, 9, 11, 26, 20]),
        ] {
            assert_eq!(
                Registers::of(AFTERNOON, status),
                Some(registers(values)),
                "{status:#04x}"
            );
        }
        let hours = |seconds: u64, status| Registers::of(seconds, status).unwrap().hours;
        let midnight = AFTERNOON - (12 * 3600 + 34 * 60 + 56);
        assert_eq!(
            hours(midnight + 300, 0),
            0x12,
            "five past midnight is 12 in the morning"
        );
        assert_eq!(hours(midnight + 3600, 0), 0x01);
        assert_eq!(hours(midnight + 11 * 3600, 0), 0x11);
        assert_eq!(hours(midnight + 13 * 3600, 0), 0x01 | PM);
        assert_eq!(hours(midnight + 23 * 3600, BINARY), PM | 0x0b);
        assert_eq!(hours(midnight + 23 * 3600, HOURS_24), 0x23);
        assert_eq!(
            Registers::of(0, HOURS_24),
            Some(registers([0, 0, 0, 0x01, 0x01, 0x70, 0x19])),
            "1970 began"
        );
        assert_eq!(
            Registers::of(253_402_300_799, HOURS_24),
            Some(registers([0x59, 0x59, 0x23, 0x31, 0x12, 0x99, 0x99])),
            "the last second of 9999"
        );
        assert_eq!(
            Registers::of(253_402_300_800, HOURS_24),
            None,
            "10000 has no century of two digits"
        );
    }

    #[test]
    fn the_registers_read_back_to_the_time() {
        let mut seed = 0x510e_527f_ade6_82d1_u64;
        for _ in 0..2000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let seconds = seed % 4_102_444_800;
            for status in FORMS {
                let set = Registers::of(seconds, status).unwrap();
                assert_eq!(
                    set.seconds(status, true),
                    Some(seconds),
                    "{seconds} {status:#04x}"
                );
                let read = set.seconds(status, false);
                let in_window = seconds < 3_155_760_000;
                assert_eq!(
                    read == Some(seconds),
                    in_window,
                    "{seconds}: without a century, 1970 to 2069"
                );
            }
        }
    }

    #[test]
    fn registers_that_make_no_time_make_none() {
        let good = Registers::of(AFTERNOON, HOURS_24).unwrap();
        assert_eq!(good.seconds(HOURS_24, true), Some(AFTERNOON));
        for (what, broken) in [
            (
                "a digit past 9",
                Registers {
                    minutes: 0x5a,
                    ..good
                },
            ),
            (
                "month 13",
                Registers {
                    month: 0x13,
                    ..good
                },
            ),
            (
                "month 0",
                Registers {
                    month: 0x00,
                    ..good
                },
            ),
            (
                "30 February",
                Registers {
                    month: 0x02,
                    day: 0x30,
                    ..good
                },
            ),
            ("day 0", Registers { day: 0x00, ..good }),
            (
                "hour 24",
                Registers {
                    hours: 0x24,
                    ..good
                },
            ),
            (
                "minute 60",
                Registers {
                    minutes: 0x60,
                    ..good
                },
            ),
            (
                "second 60",
                Registers {
                    seconds: 0x60,
                    ..good
                },
            ),
            (
                "1969",
                Registers {
                    century: 0x19,
                    year: 0x69,
                    ..good
                },
            ),
        ] {
            assert_eq!(broken.seconds(HOURS_24, true), None, "{what}");
        }
    }

    #[test]
    fn twelve_hours_and_a_missing_century_read_as_they_should() {
        let good = Registers::of(AFTERNOON, HOURS_24).unwrap();
        let twelve = Registers::of(AFTERNOON, 0).unwrap();
        assert_eq!(twelve.seconds(0, true), Some(AFTERNOON));
        assert_eq!(
            Registers {
                hours: 0x00,
                ..twelve
            }
            .seconds(0, true),
            None,
            "no hour 0 in 12"
        );
        assert_eq!(
            Registers {
                hours: 0x13 | PM,
                ..twelve
            }
            .seconds(0, true),
            None,
            "nor 13"
        );
        assert_eq!(
            Registers {
                hours: 0x12,
                ..twelve
            }
            .seconds(0, true),
            Some(AFTERNOON - 12 * 3600),
            "12 in the morning"
        );
        assert_eq!(
            Registers {
                year: 0x69,
                century: 0x19,
                ..good
            }
            .seconds(HOURS_24, false),
            Some(3_151_226_096),
            "69 without a century is 2069"
        );
    }

    #[test]
    fn decimal_digits_take_four_bits_each() {
        for n in 0..100u8 {
            assert_eq!(from_bcd(bcd(n)), Some(n), "{n}");
        }
        for b in 0..=255u8 {
            assert_eq!(
                from_bcd(b).is_some(),
                b >> 4 < 10 && b & 0x0f < 10,
                "{b:#04x}"
            );
        }
        assert_eq!(bcd(59), 0x59);
    }

    #[test]
    fn the_registers_hold_the_digits_gnu_s_date_writes() {
        use std::io::Write as _;
        let mut seed = 0x9b05_688c_2b3e_6c1f_u64;
        let moments: Vec<u64> = (0..300)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed % 4_102_444_800
            })
            .collect();
        let mut asked = String::new();
        for m in &moments {
            asked.push('@');
            asked.push_str(&m.to_string());
            asked.push('\n');
        }
        let mut child = std::process::Command::new("date")
            .args(["-u", "-f", "-", "+%S %M %H %d %m %y %C %I %p"])
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
        for (&moment, line) in moments.iter().zip(said.lines()) {
            let fields: Vec<&str> = line.split(' ').collect();
            // Two decimal digits read as hexadecimal are their BCD.
            let digits = |at: usize| u8::from_str_radix(fields[at], 16).unwrap();
            let pm = if fields[8] == "PM" { PM } else { 0 };
            assert_eq!(
                Registers::of(moment, HOURS_24),
                Some(registers([
                    digits(0),
                    digits(1),
                    digits(2),
                    digits(3),
                    digits(4),
                    digits(5),
                    digits(6)
                ])),
                "@{moment}"
            );
            assert_eq!(
                Registers::of(moment, 0).map(|r| r.hours),
                Some(digits(7) | pm),
                "@{moment}"
            );
        }
        assert_eq!(said.lines().count(), moments.len());
    }
}
