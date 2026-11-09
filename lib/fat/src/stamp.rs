//! The times FAT keeps in its entries: a day in 16 bits, from 1980 to
//! 2107, and a time of day in 16, to two seconds, here in UTC (decision
//! record 0029). A stamp holds both, the day in its high half.

/// 1 January 1980, 00:00: the first moment a stamp can say.
pub const FIRST: u32 = 0x0021_0000;

/// The first second of 1980, and the last of 2107, since 1970 began.
const FROM: u64 = 315_532_800;
const TO: u64 = 4_354_819_199;

/// The stamp of `seconds` since 1970 began, cut down to the two seconds
/// before; the first or the last a stamp can say for one before 1980 or
/// past 2107.
#[must_use]
pub fn stamp(seconds: u64) -> u32 {
    let seconds = seconds.clamp(FROM, TO);
    let (year, month, day) = calendar::civil(seconds / 86_400);
    let of_day = seconds % 86_400;
    let date = (year - 1980) << 9 | month << 5 | day;
    let time = (of_day / 3600) << 11 | (of_day / 60 % 60) << 5 | ((of_day % 60) / 2);
    u32::try_from(date << 16 | time).unwrap_or(FIRST)
}

/// The seconds since 1970 began of `stamp`; none for one that is no
/// moment, as a stamp of zeros, which some writers leave, is not.
#[must_use]
pub fn seconds(stamp: u32) -> Option<u64> {
    let (date, time) = (u64::from(stamp >> 16), u64::from(stamp & 0xffff));
    let (year, month, day) = (1980 + (date >> 9), date >> 5 & 0xf, date & 0x1f);
    let (hour, minute, second) = (time >> 11, time >> 5 & 0x3f, (time & 0x1f) * 2);
    let fits = (1..=12).contains(&month)
        && (1..=calendar::days_in(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second < 60;
    fits.then(|| calendar::days(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_become_stamps_and_back() {
        assert_eq!(stamp(FROM), FIRST);
        assert_eq!(seconds(FIRST), Some(FROM));
        // 12:00 UTC on 9 November 2026: 46 years from 1980, month 11,
        // day 9, and 12 hours.
        let noon = 1_794_225_600;
        assert_eq!(stamp(noon), (46 << 9 | 11 << 5 | 9) << 16 | 12 << 11);
        assert_eq!(seconds(stamp(noon)), Some(noon));
        assert_eq!(
            seconds(stamp(noon + 59)),
            Some(noon + 58),
            "two seconds at a time"
        );
        assert_eq!(seconds(stamp(noon + 3599)), Some(noon + 3598));
        assert_eq!(
            seconds(stamp(0)),
            Some(FROM),
            "before 1980, its first moment"
        );
        assert_eq!(
            seconds(stamp(u64::MAX)),
            Some(TO - 1),
            "past 2107, its last"
        );
        assert_eq!(stamp(TO), 0xff9f_bf7d, "31 December 2107, 23:59:58");
    }

    #[test]
    fn stamps_that_are_no_moment_say_none() {
        assert_eq!(seconds(0), None, "zeros, which some writers leave");
        for (date, time, what) in [
            (0x0001, 0, "month 0"),
            (0x01a1, 0, "month 13"),
            (0x0040, 0, "day 0 of February"),
            (0x005e, 0, "30 February"),
            (0x0021, 24 << 11, "hour 24"),
            (0x0021, 60 << 5, "minute 60"),
            (0x0021, 30, "second 60"),
        ] {
            assert_eq!(seconds(date << 16 | time), None, "{what}");
        }
        assert!(
            seconds(0x0021 << 16 | 23 << 11 | 59 << 5 | 0x1d).is_some(),
            "23:59:58"
        );
    }

    #[test]
    fn every_two_seconds_of_some_days_go_there_and_back() {
        for day in [3652, 11_016, 20_766, 50_402] {
            for at in (0..86_400).step_by(2) {
                let moment = day * 86_400 + at;
                assert_eq!(seconds(stamp(moment)), Some(moment), "{moment}");
                assert_eq!(seconds(stamp(moment + 1)), Some(moment), "{moment} and one");
            }
        }
    }
}
