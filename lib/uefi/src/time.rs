//! The time of day as the firmware's real-time clock gives it: UEFI's
//! `EFI_TIME`, and the seconds since 1970 it stands for.

/// A time as `GetTime` gives it, laid out as UEFI's `EFI_TIME`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub pad1: u8,
    pub nanosecond: u32,
    /// Minutes from UTC, or [`Time::NO_ZONE`]. Meridian takes the fields
    /// as UTC whatever this says (decision record 0028).
    pub time_zone: i16,
    pub daylight: u8,
    pub pad2: u8,
}

impl Time {
    /// The zone of a clock that does not say which it keeps.
    pub const NO_ZONE: i16 = 2047;

    /// The seconds since 1970 began and the nanoseconds past them, its
    /// fields taken as UTC; none for fields that make no time, or one
    /// before 1970.
    #[must_use]
    pub fn unix(&self) -> Option<(u64, u32)> {
        let (year, month, day) = (
            u64::from(self.year),
            u64::from(self.month),
            u64::from(self.day),
        );
        let fits = (1970..=9999).contains(&year)
            && (1..=12).contains(&month)
            && (1..=calendar::days_in(year, month)).contains(&day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
            && self.nanosecond < 1_000_000_000;
        if !fits {
            return None;
        }
        let seconds = calendar::days(year, month, day) * 86_400
            + u64::from(self.hour) * 3600
            + u64::from(self.minute) * 60
            + u64::from(self.second);
        Some((seconds, self.nanosecond))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    fn at(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Time {
        Time {
            year,
            month,
            day,
            hour,
            minute,
            second,
            time_zone: Time::NO_ZONE,
            ..Time::default()
        }
    }

    #[test]
    fn it_is_laid_out_as_efi_time() {
        assert_eq!(size_of::<Time>(), 16);
        assert_eq!(
            [
                offset_of!(Time, year),
                offset_of!(Time, month),
                offset_of!(Time, day),
                offset_of!(Time, hour),
                offset_of!(Time, minute),
                offset_of!(Time, second),
                offset_of!(Time, pad1),
                offset_of!(Time, nanosecond),
                offset_of!(Time, time_zone),
                offset_of!(Time, daylight),
                offset_of!(Time, pad2),
            ],
            [0, 2, 3, 4, 5, 6, 7, 8, 12, 14, 15]
        );
    }

    #[test]
    fn times_become_the_seconds_since_1970() {
        assert_eq!(at(1970, 1, 1, 0, 0, 0).unix(), Some((0, 0)));
        assert_eq!(
            at(2023, 11, 14, 22, 13, 20).unix(),
            Some((1_700_000_000, 0))
        );
        assert_eq!(at(2000, 2, 29, 0, 0, 0).unix(), Some((951_782_400, 0)));
        assert_eq!(at(2000, 3, 1, 0, 0, 0).unix(), Some((951_868_800, 0)));
        assert_eq!(
            at(9999, 12, 31, 23, 59, 59).unix(),
            Some((253_402_300_799, 0))
        );
        let mut late = at(2026, 11, 9, 12, 0, 0);
        late.nanosecond = 999_999_999;
        late.time_zone = -60;
        late.daylight = 1;
        assert_eq!(
            late.unix(),
            Some((1_794_225_600, 999_999_999)),
            "the zone named changes nothing"
        );
    }

    #[test]
    fn fields_that_make_no_time_make_none() {
        for (time, what) in [
            (at(1969, 12, 31, 23, 59, 59), "before 1970"),
            (at(10000, 1, 1, 0, 0, 0), "past 9999"),
            (at(2026, 0, 1, 0, 0, 0), "month 0"),
            (at(2026, 13, 1, 0, 0, 0), "month 13"),
            (at(2026, 1, 0, 0, 0, 0), "day 0"),
            (at(2026, 1, 32, 0, 0, 0), "32 January"),
            (at(2026, 4, 31, 0, 0, 0), "31 April"),
            (at(2026, 2, 29, 0, 0, 0), "29 February in a common year"),
            (at(2100, 2, 29, 0, 0, 0), "29 February of 2100"),
            (at(2026, 1, 1, 24, 0, 0), "hour 24"),
            (at(2026, 1, 1, 0, 60, 0), "minute 60"),
            (at(2026, 1, 1, 0, 0, 60), "second 60"),
        ] {
            assert_eq!(time.unix(), None, "{what}");
        }
        let mut past = at(2026, 1, 1, 0, 0, 0);
        past.nanosecond = 1_000_000_000;
        assert_eq!(past.unix(), None, "a whole second of nanoseconds");
        for (year, month, days) in [(2024, 2, 29), (2000, 2, 29), (1900, 2, 28), (2026, 6, 30)] {
            assert_eq!(calendar::days_in(year, month), days, "{year}-{month}");
        }
    }

    #[test]
    fn random_times_are_the_seconds_date_gives() {
        use std::fmt::Write as _;
        use std::io::Write as _;
        let mut seed = 0x6a09_e667_f3bc_c908_u64;
        let mut next = |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        let times: Vec<Time> = (0..500)
            .map(|_| {
                let year = 1970 + next(8030);
                let month = 1 + next(12);
                let day = 1 + next(calendar::days_in(year, month));
                at(
                    u16::try_from(year).unwrap(),
                    u8::try_from(month).unwrap(),
                    u8::try_from(day).unwrap(),
                    u8::try_from(next(24)).unwrap(),
                    u8::try_from(next(60)).unwrap(),
                    u8::try_from(next(60)).unwrap(),
                )
            })
            .collect();
        let mut asked = String::new();
        for t in &times {
            let _ = writeln!(
                asked,
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                t.year, t.month, t.day, t.hour, t.minute, t.second
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
        let seconds = String::from_utf8(done.stdout).unwrap();
        for (time, said) in times.iter().zip(seconds.lines()) {
            assert_eq!(time.unix(), Some((said.parse().unwrap(), 0)), "{time:?}");
        }
    }
}
