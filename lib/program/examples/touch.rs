//! touch: sets when the files it names last changed, and makes those not
//! there yet, as POSIX's touch does: to the time of day, to when the file
//! `-r` names changed, or to the time `-t` gives as
//! `[[CC]YY]MMDDhhmm[.SS]` or `-d` as `YYYY-MM-DDThh:mm:SS[.frac][Z]`, in
//! UTC, as Meridian knows no zones yet. `-c` makes no file. Meridian's
//! files keep only when they last changed, so `-a` alone changes no time
//! and `-m` is what touch does anyway. A volume keeps a time to two
//! seconds, from 1980 to 2107 as FAT does, so touch refuses one outside
//! those years before it touches anything. It ends with 0 when every file
//! named is there with its time set, and 1 otherwise.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::RangeInclusive;

use program::abi::file::{OPEN_ALWAYS, OPEN_EXISTING, WRITE};
use program::{Error, Handle, text};

program::main!(touch);

/// The seconds since 1970 began that Meridian's volumes keep, FAT's from
/// the start of 1980 to the end of 2107 (decision record 0030).
const KEPT: RangeInclusive<u64> = 315_532_800..=4_354_819_199;

/// Where the time touch sets comes from.
#[derive(Debug, PartialEq, Eq)]
enum When<'w> {
    /// The seconds since 1970 began.
    At(u64),
    /// When the file at the path last changed, as `-r` asks.
    Like(&'w str),
}

/// What touch's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    /// Whether a file not there is made.
    make: bool,
    /// The time to set, none when `-a` alone asks for no change.
    when: Option<When<'w>>,
    paths: &'w [String],
}

/// What touch's words ask for at `now`, the seconds since 1970 began when
/// the time of day is known, or why they cannot be taken.
fn asked(arguments: &[String], now: Option<u64>) -> Result<Asked<'_>, String> {
    let (mut make, mut access, mut modify) = (true, false, false);
    let mut given = None;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        words = rest;
        for (at, letter) in letters.char_indices() {
            match letter {
                'c' => make = false,
                'a' => access = true,
                'm' => modify = true,
                'r' | 't' | 'd' => {
                    // What the option takes is the rest of its word, or the
                    // next word.
                    let value = match &letters[at + 1..] {
                        "" => {
                            let (value, rest) = words.split_first().ok_or_else(|| {
                                format!("touch: -{letter} wants {}\n", wanted(letter))
                            })?;
                            words = rest;
                            value.as_str()
                        }
                        attached => attached,
                    };
                    if given.is_some() {
                        return Err(String::from("touch: takes one of -r, -t and -d at most\n"));
                    }
                    given = Some(match letter {
                        'r' => When::Like(value),
                        't' => When::At(kept(value, t_time(value, now)?)?),
                        _ => When::At(kept(value, d_time(value)?)?),
                    });
                    break;
                }
                _ => return Err(format!("touch: -{letter} is no option\n")),
            }
        }
    }
    if words.is_empty() {
        return Err(String::from("touch: names a file at least\n"));
    }
    let when = if access && !modify {
        None
    } else if given.is_some() {
        given
    } else {
        let now =
            now.ok_or_else(|| String::from("touch: Meridian does not know the time of day\n"))?;
        Some(When::At(now))
    };
    Ok(Asked {
        make,
        when,
        paths: words,
    })
}

/// What the option `letter` takes, as it is said when it is missing.
fn wanted(letter: char) -> &'static str {
    match letter {
        'r' => "a file",
        't' => "a time",
        _ => "a date and a time",
    }
}

/// `seconds`, the time `text` gives, when a volume keeps it.
fn kept(text: &str, seconds: u64) -> Result<u64, String> {
    if KEPT.contains(&seconds) {
        Ok(seconds)
    } else {
        Err(format!(
            "touch: {text} is outside 1980 to 2107, the years Meridian's volumes keep\n"
        ))
    }
}

/// Whether `text` is one decimal digit or more.
fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The number the two digits of `text` from `at` on make.
fn two(text: &str, at: usize) -> u64 {
    let b = text.as_bytes();
    u64::from(b[at] - b'0') * 10 + u64::from(b[at + 1] - b'0')
}

/// The seconds since 1970 began at the time `-t` gives as
/// `[[CC]YY]MMDDhhmm[.SS]`: in the year of `now` when it names none, and
/// for a YY alone in the 1900s from 69 on and in the 2000s below, as
/// POSIX has.
fn t_time(text: &str, now: Option<u64>) -> Result<u64, String> {
    let wrong = || format!("touch: -t wants [[CC]YY]MMDDhhmm[.SS], not {text}\n");
    let (clock, second) = text.split_once('.').unwrap_or((text, "00"));
    if !(digits(clock) && second.len() == 2 && digits(second)) {
        return Err(wrong());
    }
    let (year, from) = match clock.len() {
        8 => {
            let now = now.ok_or_else(|| {
                String::from(
                    "touch: -t takes the year it is when it names none, and Meridian does not know the time of day\n",
                )
            })?;
            (calendar::civil(now / 86_400).0, 0)
        }
        10 => {
            let short = two(clock, 0);
            (short + if short >= 69 { 1900 } else { 2000 }, 2)
        }
        12 => (two(clock, 0) * 100 + two(clock, 2), 4),
        _ => return Err(wrong()),
    };
    let fields = [0, 2, 4, 6].map(|at| two(clock, from + at));
    moment(year, fields, two(second, 0)).ok_or_else(wrong)
}

/// The seconds since 1970 began at the time `-d` gives as
/// `YYYY-MM-DDThh:mm:SS[.frac][Z]`, with a blank for the `T` and a comma
/// for the dot as POSIX lets it, and with or without the `Z` in UTC; the
/// fraction is finer than a volume keeps.
fn d_time(text: &str) -> Result<u64, String> {
    let wrong = || format!("touch: -d wants YYYY-MM-DDThh:mm:SS[.frac][Z], not {text}\n");
    let bare = text.strip_suffix('Z').unwrap_or(text);
    let (date, clock) = bare.split_once(['T', ' ']).ok_or_else(wrong)?;
    let (clock, fraction) = clock.split_once(['.', ',']).unwrap_or((clock, "0"));
    let date: Vec<&str> = date.split('-').collect();
    let clock: Vec<&str> = clock.split(':').collect();
    let (&[year, month, day], &[hour, minute, second]) = (&date[..], &clock[..]) else {
        return Err(wrong());
    };
    let fields = [month, day, hour, minute, second];
    let fits = year.len() >= 4
        && digits(year)
        && digits(fraction)
        && fields.iter().all(|f| f.len() == 2 && digits(f));
    if !fits {
        return Err(wrong());
    }
    // A year too long for 64 bits is far past any that is kept.
    let year = year.parse().unwrap_or(u64::MAX);
    let [month, day, hour, minute, second] = fields.map(|f| two(f, 0));
    moment(year, [month, day, hour, minute], second).ok_or_else(wrong)
}

/// The seconds since 1970 began at the month, day, hour and minute of
/// `fields` and `second` of `year` in UTC, a second of 60 being the one
/// after 59, as POSIX has for leap seconds; none for what is no moment.
/// One before 1970 counts as its start and one past 9999 as the last
/// second there is, neither a time a volume keeps.
fn moment(year: u64, fields: [u64; 4], second: u64) -> Option<u64> {
    let [month, day, hour, minute] = fields;
    let fits = (1..=12).contains(&month)
        && (1..=calendar::days_in(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second <= 60;
    if !fits {
        return None;
    }
    Some(match year {
        ..1970 => 0,
        10_000.. => u64::MAX,
        _ => calendar::days(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second,
    })
}

/// What touch does with files.
trait Files {
    /// When the file at `path` last changed, as seconds since 1970 began;
    /// none for what keeps no time.
    fn changed(&mut self, path: &str) -> Result<Option<u64>, Error>;

    /// Opens the file at `path` to write, made when `make` says so and it
    /// is not there, and sets when it last changed to `at` when given.
    fn touch(&mut self, path: &str, make: bool, at: Option<u64>) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// Touches the files `asked` names, as far as it asks; whether every one
/// is there with its time set.
fn touch_all(files: &mut impl Files, asked: &Asked) -> bool {
    let at = match asked.when {
        None => None,
        Some(When::At(seconds)) => Some(seconds),
        Some(When::Like(path)) => match files.changed(path) {
            Ok(Some(seconds)) => Some(seconds),
            Ok(None) => {
                files.say(&format!("touch: {path}: keeps no time\n"));
                return false;
            }
            Err(e) => {
                files.say(&text::trouble("touch", path, e));
                return false;
            }
        },
    };
    let mut all = true;
    for path in asked.paths {
        match files.touch(path, asked.make, at) {
            // With -c a file not there is left so, and nothing said.
            Err(Error::NotFound) if !asked.make => {}
            Err(e) => {
                files.say(&text::trouble("touch", path, e));
                all = false;
            }
            Ok(()) => {}
        }
    }
    all
}

/// Meridian's files.
struct System;

impl Files for System {
    fn changed(&mut self, path: &str) -> Result<Option<u64>, Error> {
        let file = program::open(path, program::abi::file::READ)?;
        let changed = program::changed(file);
        let _ = program::close(file);
        changed
    }

    fn touch(&mut self, path: &str, make: bool, at: Option<u64>) -> Result<(), Error> {
        let how = if make { OPEN_ALWAYS } else { OPEN_EXISTING };
        let file = program::open_with(path, WRITE, how)?;
        let set = at.map_or(Ok(()), |seconds| program::set_changed(file, seconds));
        let _ = program::close(file);
        set
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn touch(_: Handle) -> u64 {
    let now = program::now().map(|nanoseconds| nanoseconds / 1_000_000_000);
    match asked(program::arguments(), now) {
        Ok(asked) => u64::from(!touch_all(&mut System, &asked)),
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

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in touch reaches the host's own files.
    fn confined(root: &std::path::Path, path: &str) -> std::path::PathBuf {
        let mut depth = 0usize;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .unwrap_or_else(|| panic!("{path} climbs out of the test's directory"));
                }
                _ => depth += 1,
            }
        }
        root.join(path.trim_start_matches('/'))
    }

    /// Files among the host's temporary ones, gone once dropped, and what
    /// touch said.
    struct Host {
        root: std::path::PathBuf,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-touch-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("d")).unwrap();
            std::fs::write(root.join("kept"), "held").unwrap();
            Host {
                root,
                said: String::new(),
            }
        }

        fn holds(&self, path: &str) -> Option<String> {
            std::fs::read_to_string(self.root.join(path)).ok()
        }

        /// When the file at `path` last changed, as seconds since 1970
        /// began.
        fn when(&self, path: &str) -> u64 {
            let found = std::fs::metadata(self.root.join(path)).unwrap();
            let since = found
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH);
            since.unwrap().as_secs()
        }

        /// Sets when the file at `path` last changed.
        fn set(&self, path: &str, seconds: u64) {
            let file = std::fs::File::open(self.root.join(path)).unwrap();
            let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
            file.set_modified(at).unwrap();
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            _ => Error::Unwritable,
        }
    }

    impl Files for Host {
        fn changed(&mut self, path: &str) -> Result<Option<u64>, Error> {
            let found = std::fs::metadata(confined(&self.root, path)).map_err(|e| error(&e))?;
            let since = found
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH);
            Ok(Some(since.unwrap().as_secs()))
        }

        fn touch(&mut self, path: &str, make: bool, at: Option<u64>) -> Result<(), Error> {
            let at_path = confined(&self.root, path);
            let file = if at_path.exists() {
                std::fs::File::open(&at_path)
            } else if make {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&at_path)
            } else {
                return Err(Error::NotFound);
            };
            let file = file.map_err(|e| error(&e))?;
            if let Some(seconds) = at {
                let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
                file.set_modified(at).map_err(|e| error(&e))?;
            }
            Ok(())
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// 12:00 UTC on 9 November 2026, a Monday.
    const NOON: u64 = 1_794_225_600;

    /// How touch with the words `text` ran on `host` at [`NOON`]: whether
    /// every file is there with its time, and what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String) {
        let given = words(text);
        let taken = asked(&given, Some(NOON)).unwrap();
        let all = touch_all(host, &taken);
        (all, core::mem::take(&mut host.said))
    }

    /// What touch takes the words `text` for at [`NOON`], or at no time
    /// known with `now` none.
    fn taken(text: &str, now: Option<u64>) -> Result<(bool, Option<When<'static>>, usize), String> {
        let given: &'static [String] = Vec::leak(words(text));
        asked(given, now).map(|a| (a.make, a.when, a.paths.len()))
    }

    #[test]
    fn its_words_are_options_then_files() {
        let at = |seconds| Some(When::At(seconds));
        for (line, make, when) in [
            ("a", true, at(NOON)),
            ("-c a", false, at(NOON)),
            ("-am a", true, at(NOON)),
            ("-m -a a", true, at(NOON)),
            ("-a a", true, None),
            ("-a -t 202611091200 a", true, None),
            ("-r ref a", true, Some(When::Like("ref"))),
            ("-rref a", true, Some(When::Like("ref"))),
            ("-cr ref a", false, Some(When::Like("ref"))),
            ("-t 202611091200.30 a", true, at(NOON + 30)),
            ("-ct202611091200 a", false, at(NOON)),
            ("-d 2026-11-09T12:00:01Z a", true, at(NOON + 1)),
        ] {
            assert_eq!(taken(line, Some(NOON)), Ok((make, when, 1)), "{line}");
        }
        let given = words("-- -c");
        assert_eq!(
            asked(&given, Some(NOON)),
            Ok(Asked {
                make: true,
                when: at(NOON),
                paths: &given[1..]
            })
        );
        for (line, why) in [
            ("-x a", "touch: -x is no option\n"),
            ("-c", "touch: names a file at least\n"),
            ("", "touch: names a file at least\n"),
            ("-t", "touch: -t wants a time\n"),
            ("-r", "touch: -r wants a file\n"),
            ("-d", "touch: -d wants a date and a time\n"),
            (
                "-r a -t 202611091200 f",
                "touch: takes one of -r, -t and -d at most\n",
            ),
            (
                "-t 202611091200 -t 202611091200 f",
                "touch: takes one of -r, -t and -d at most\n",
            ),
            ("-t 1 f", "touch: -t wants [[CC]YY]MMDDhhmm[.SS], not 1\n"),
            (
                "-d 1 f",
                "touch: -d wants YYYY-MM-DDThh:mm:SS[.frac][Z], not 1\n",
            ),
        ] {
            assert_eq!(taken(line, Some(NOON)), Err(why.into()), "{line}");
        }
        assert_eq!(
            taken("a", None),
            Err("touch: Meridian does not know the time of day\n".into())
        );
        assert_eq!(
            taken("-a a", None),
            Ok((true, None, 1)),
            "-a alone wants no time"
        );
        assert_eq!(taken("-r x a", None), Ok((true, Some(When::Like("x")), 1)));
        assert_eq!(taken("-t 202611091200 a", None), Ok((true, at(NOON), 1)));
        assert_eq!(
            taken("-t 11091200 a", None),
            Err("touch: -t takes the year it is when it names none, and Meridian does not know the time of day\n".into())
        );
    }

    #[test]
    fn t_takes_the_forms_posix_gives() {
        for (text, seconds) in [
            ("202611091200", NOON),
            ("2611091200", NOON),
            ("11091200", NOON),
            ("8001010000", 315_532_800),
            ("6801010000", 3_092_601_600),
            ("6901010000", 0),
            ("7006010000", 13_046_400),
            ("202611091259.60", NOON + 3600),
            ("210712312359.59", 4_354_819_199),
            ("000101010000", 0),
            ("999912312359.59", 253_402_300_799),
        ] {
            assert_eq!(t_time(text, Some(NOON)), Ok(seconds), "{text}");
        }
        assert_eq!(
            t_time("01010000", Some(315_532_800 + 86_400 * 400)),
            Ok(315_532_800 + 86_400 * 366),
            "the year of the time of day, 1981 here"
        );
        for text in [
            "2026110912",
            "202602291200",
            "202611092400",
            "202611091260",
            "202611091200.61",
            "202611091200.3",
            "202611091200.",
            "20261109120",
            "2026110912000",
            "1109120",
            "x",
            "",
            ".30",
            "+202611091200",
            "2026-1109120",
            "202600091200",
            "202613091200",
            "202611001200",
        ] {
            assert_eq!(
                t_time(text, Some(NOON)),
                Err(format!(
                    "touch: -t wants [[CC]YY]MMDDhhmm[.SS], not {text}\n"
                )),
                "{text}"
            );
        }
    }

    #[test]
    fn d_takes_the_forms_posix_gives() {
        for (text, seconds) in [
            ("2026-11-09T12:00:00", NOON),
            ("2026-11-09 12:00:00", NOON),
            ("2026-11-09T12:00:00Z", NOON),
            ("2026-11-09T12:00:00.75", NOON),
            ("2026-11-09T12:00:00,5Z", NOON),
            ("2026-11-09T12:00:60", NOON + 60),
            ("02026-11-09T12:00:00", NOON),
            ("1979-12-31T23:59:60Z", 315_532_800),
            ("2024-02-29T00:00:00", 1_709_164_800),
            ("1969-12-31T23:59:59", 0),
            ("10000-01-01T00:00:00", u64::MAX),
            ("99999999999999999999999-01-01T00:00:00", u64::MAX),
        ] {
            assert_eq!(d_time(text), Ok(seconds), "{text}");
        }
        for text in [
            "2026-11-09T12:00",
            "26-11-09T12:00:00",
            "2026-1-9T12:00:00",
            "2026-11-09t12:00:00",
            "2026-11-09T12:00:00z",
            "2026-11-09T12:00:00.",
            "2026-11-09T12:00:00.5.5",
            "2026-11-09T12:00:00ZZ",
            "2026-11-09T12:00:00+01:00",
            "2026-11-09T12:00:00:00",
            "2026-11-09-01T12:00:00",
            "2026-11T12:00:00",
            "2026-02-29T00:00:00",
            "2026-11-09T24:00:00",
            "2026-11-09T12:60:00",
            "2026-11-09T12:00:61",
            "2026-11-09",
            "2026-11-09T",
            "-11-09T12:00:00",
        ] {
            assert_eq!(
                d_time(text),
                Err(format!(
                    "touch: -d wants YYYY-MM-DDThh:mm:SS[.frac][Z], not {text}\n"
                )),
                "{text}"
            );
        }
    }

    #[test]
    fn a_time_no_volume_keeps_is_refused() {
        for (line, text) in [
            ("-t 197912312359.59", "197912312359.59"),
            ("-t 210801010000", "210801010000"),
            ("-t 6912312359", "6912312359"),
            ("-d 2108-01-01T00:00:00Z", "2108-01-01T00:00:00Z"),
            ("-d 10000-01-01T00:00:00", "10000-01-01T00:00:00"),
        ] {
            assert_eq!(
                taken(&format!("{line} f"), Some(NOON)),
                Err(format!(
                    "touch: {text} is outside 1980 to 2107, the years Meridian's volumes keep\n"
                )),
                "{line}"
            );
        }
        assert_eq!(
            taken("-t 198001010000 f", Some(NOON)),
            Ok((true, Some(When::At(315_532_800)), 1)),
            "the first second kept"
        );
        assert_eq!(
            taken("-d 2107-12-31T23:59:59 f", Some(NOON)),
            Ok((true, Some(When::At(4_354_819_199)), 1)),
            "and the last"
        );
    }

    #[test]
    fn times_are_those_gnu_s_touch_sets() {
        use core::fmt::Write as _;
        let host = Host::new("gnu");
        let mut seed = 0x243f_6a88_85a3_08d3_u64;
        let mut next = |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        let mut asked = Vec::new();
        for _ in 0..200 {
            let year = 1980 + next(127);
            let month = 1 + next(12);
            let day = 1 + next(calendar::days_in(year, month));
            let (hour, minute, second) = (next(24), next(60), next(60));
            let clock = format!("{month:02}{day:02}{hour:02}{minute:02}");
            let t = match next(3) {
                0 => format!("{year}{clock}.{second:02}"),
                1 if (1980..2069).contains(&year) => {
                    format!("{:02}{clock}.{second:02}", year % 100)
                }
                _ => format!("{year}{clock}"),
            };
            let d = format!(
                "{year}-{month:02}-{day:02}{}{hour:02}:{minute:02}:{second:02}{}",
                if next(2) == 0 { 'T' } else { ' ' },
                ["", "Z", ".25", ",5Z"][usize::try_from(next(4)).unwrap()],
            );
            asked.push(("-t", t));
            asked.push(("-d", d));
        }
        let mut script = String::new();
        for (option, value) in &asked {
            let _ = writeln!(script, "touch {option} '{value}' f && stat -c %Y f");
        }
        let done = std::process::Command::new("sh")
            .args(["-c", &script])
            .current_dir(&host.root)
            .env("TZ", "UTC")
            .output()
            .unwrap();
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
        let theirs = String::from_utf8(done.stdout).unwrap();
        for ((option, value), seconds) in asked.iter().zip(theirs.lines()) {
            let ours = if *option == "-t" {
                t_time(value, Some(NOON))
            } else {
                d_time(value)
            };
            assert_eq!(ours, Ok(seconds.parse().unwrap()), "{option} {value}");
        }
        assert_eq!(theirs.lines().count(), asked.len());
    }

    #[test]
    fn what_t_refuses_gnu_s_touch_refuses_too() {
        let host = Host::new("gnu-refused");
        for text in [
            "2026110912",
            "202602291200",
            "202611092400",
            "202611091260",
            "202611091200.61",
            "202611091200.3",
            "20261109120",
            "202600091200",
        ] {
            let done = std::process::Command::new("touch")
                .args(["-t", text, "f"])
                .current_dir(&host.root)
                .env("TZ", "UTC")
                .output()
                .unwrap();
            assert!(!done.status.success(), "{text}");
            assert!(t_time(text, Some(NOON)).is_err(), "{text}");
        }
    }

    #[test]
    fn files_not_there_are_made_and_every_one_named_gets_the_time() {
        let mut host = Host::new("make");
        host.set("kept", NOON - 1000);
        assert_eq!(ran(&mut host, "new kept d/inner"), (true, String::new()));
        assert_eq!(host.holds("new").as_deref(), Some(""));
        assert_eq!(
            host.holds("kept").as_deref(),
            Some("held"),
            "its bytes stay"
        );
        assert_eq!(host.holds("d/inner").as_deref(), Some(""));
        for path in ["new", "kept", "d/inner"] {
            assert_eq!(host.when(path), NOON, "{path}");
        }
        assert_eq!(
            ran(&mut host, "-t 200109090146.40 d"),
            (true, String::new())
        );
        assert_eq!(host.when("d"), 1_000_000_000, "a directory too");
        assert_eq!(ran(&mut host, "-c other"), (true, String::new()));
        assert_eq!(host.holds("other"), None, "-c makes nothing");
        assert_eq!(
            ran(&mut host, "nowhere/x new2"),
            (false, "touch: nowhere/x: no such file\n".into())
        );
        assert_eq!(
            host.holds("new2").as_deref(),
            Some(""),
            "the rest are made all the same"
        );
    }

    #[test]
    fn r_takes_another_file_s_time_and_a_alone_none() {
        let mut host = Host::new("r");
        host.set("kept", NOON - 1000);
        assert_eq!(ran(&mut host, "-r kept new"), (true, String::new()));
        assert_eq!(host.when("new"), NOON - 1000);
        host.set("new", NOON - 5);
        assert_eq!(
            ran(&mut host, "-r missing new"),
            (false, "touch: missing: no such file\n".into())
        );
        assert_eq!(host.when("new"), NOON - 5, "nothing is touched");
        assert_eq!(ran(&mut host, "-a new fresh"), (true, String::new()));
        assert_eq!(host.when("new"), NOON - 5, "-a alone changes no time");
        assert_eq!(host.holds("fresh").as_deref(), Some(""), "but makes a file");
        assert_eq!(
            ran(&mut host, "-c -d 2001-09-09T01:46:40Z new gone"),
            (true, String::new())
        );
        assert_eq!(host.when("new"), 1_000_000_000);
        assert_eq!(host.holds("gone"), None);
    }
}
