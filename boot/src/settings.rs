//! The loader's settings, which a person can change in
//! `\meridian\boot.meri` on the EFI partition:
//!
//! ```text
//! boot {
//!     timeout = 3s
//!     default = meridian
//!     verbose = false
//! }
//! ```

use core::fmt;

use meri::{Code, Document, Error, Str};

/// Where the settings are, from the root of the partition.
pub const PATH: &str = "/meridian/boot.meri";

/// The most bytes of the file the loader reads.
pub const LIMIT: usize = 4096;

/// Whether a file of `size` bytes is short enough to be read.
#[must_use]
pub fn fits(size: u64) -> bool {
    size <= LIMIT as u64
}

/// The longest countdown, in seconds. A longer one is cut to this, so a
/// slip of the finger cannot keep a machine waiting for days.
pub const MAX_TIMEOUT: u32 = 600;

/// Which system starts when the countdown ends.
#[derive(Clone, Copy, Debug)]
pub enum Start<'a> {
    /// The first card, Meridian's.
    First,
    /// The card with this name, whatever its case.
    Named(Str<'a>),
    /// The card that was started last time.
    Last,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings<'a> {
    /// Seconds until the default system starts. 0 waits for a key.
    pub timeout: u32,
    pub start: Start<'a>,
    /// Whether the boot screen opens with the log instead of the cards.
    pub verbose: bool,
}

impl Default for Settings<'_> {
    fn default() -> Self {
        Settings {
            timeout: 3,
            start: Start::First,
            verbose: false,
        }
    }
}

/// The firmware variable, of Meridian's own vendor, that holds the name
/// of the system started last, for `default = last`.
pub const LAST_SYSTEM: [u16; 11] = uefi::variable::name("LastSystem");

impl fmt::Display for Settings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "timeout {} s, default ", self.timeout)?;
        match self.start {
            Start::First => f.write_str("Meridian")?,
            Start::Named(name) => write!(f, "{name}")?,
            Start::Last => f.write_str("the system started last")?,
        }
        f.write_str(if self.verbose { ", verbose" } else { ", quiet" })
    }
}

/// Whole seconds in a duration, counting a part of a second as one.
fn seconds(nanoseconds: i64) -> u32 {
    let whole = nanoseconds.unsigned_abs().div_ceil(1_000_000_000);
    u32::try_from(whole).unwrap_or(u32::MAX).min(MAX_TIMEOUT)
}

/// Reads the settings from a `.meri` document. A setting that is missing
/// or wrong keeps its default; the first problem comes back with where it
/// is, so the log can say so.
#[must_use]
pub fn read(src: &[u8]) -> (Settings<'_>, Option<Error>) {
    let mut settings = Settings::default();
    let doc = match Document::parse(src) {
        Ok(doc) => doc,
        Err(e) => return (settings, Some(e)),
    };
    let mut problem = None;
    match (doc.duration("boot.timeout"), doc.value("boot.timeout")) {
        (Ok(Some(ns)), _) if ns >= 0 => settings.timeout = seconds(ns),
        (Ok(Some(_)), Ok(Some((_, at)))) => {
            problem = Some(Error {
                code: Code::OutOfRange,
                at,
            });
        }
        (Err(e), _) => problem = Some(e),
        _ => {}
    }
    match doc.text("boot.default") {
        Ok(Some(name)) if name == "last" => settings.start = Start::Last,
        Ok(Some(name)) => settings.start = Start::Named(name),
        Ok(None) => {}
        Err(e) => {
            problem.get_or_insert(e);
        }
    }
    match doc.bool("boot.verbose") {
        Ok(Some(verbose)) => settings.verbose = verbose,
        Ok(None) => {}
        Err(e) => {
            problem.get_or_insert(e);
        }
    }
    (settings, problem)
}

/// Whether a name from the settings is the name on a card, ignoring the
/// case of ASCII letters.
#[must_use]
pub fn is_named(name: Str<'_>, card: &str) -> bool {
    name.chars()
        .map(|c| c.to_ascii_lowercase())
        .eq(card.chars().map(|c| c.to_ascii_lowercase()))
}

/// The index of the card to choose first: the one `start` names, or the
/// first card when none matches. `last` is the name of the card started
/// last time, if the loader remembers one.
pub fn chosen<'n>(
    start: Start<'_>,
    names: impl Iterator<Item = &'n str>,
    last: Option<&str>,
) -> usize {
    let mut names = names;
    let found = match start {
        Start::First => None,
        Start::Named(name) => names.position(|card| is_named(name, card)),
        Start::Last => last.and_then(|last| names.position(|card| card == last)),
    };
    found.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: [&str; 3] = ["Meridian", "Windows", "Arch Linux"];

    fn start_of(src: &str) -> Start<'_> {
        let (settings, problem) = read(src.as_bytes());
        assert!(problem.is_none(), "{src}: {problem:?}");
        settings.start
    }

    fn index(src: &str, last: Option<&str>) -> usize {
        chosen(start_of(src), CARDS.into_iter(), last)
    }

    #[test]
    fn an_empty_file_keeps_every_default() {
        for src in [
            "",
            "# nothing set\n",
            "boot {\n}\n",
            "other {\n    timeout = 9s\n}\n",
        ] {
            let (s, problem) = read(src.as_bytes());
            assert!(problem.is_none(), "{src}");
            assert_eq!(s.timeout, 3);
            assert!(matches!(s.start, Start::First));
            assert!(!s.verbose);
        }
    }

    #[test]
    fn every_setting_is_read() {
        let src = "boot {\n    timeout = 10s\n    default = windows\n    verbose = true\n}\n";
        let (s, problem) = read(src.as_bytes());
        assert!(problem.is_none());
        assert_eq!(s.timeout, 10);
        assert!(matches!(s.start, Start::Named(name) if name == "windows"));
        assert!(s.verbose);
    }

    #[test]
    fn timeouts_count_started_seconds() {
        let cases = [
            ("0s", 0),
            ("1ns", 1),
            ("2500ms", 3),
            ("3s", 3),
            ("3001ms", 4),
            ("1min", 60),
            ("10min", MAX_TIMEOUT),
            ("2d", MAX_TIMEOUT),
        ];
        for (written, want) in cases {
            let src = format!("boot {{\n    timeout = {written}\n}}\n");
            let (s, problem) = read(src.as_bytes());
            assert!(problem.is_none(), "{written}");
            assert_eq!(s.timeout, want, "{written}");
        }
        assert_eq!(seconds(i64::MAX), MAX_TIMEOUT);
    }

    #[test]
    fn wrong_settings_keep_their_default_and_say_where() {
        let (s, problem) = read(b"boot {\n    timeout = -1s\n    verbose = true\n}\n");
        assert_eq!(s.timeout, 3, "a negative countdown is refused");
        assert!(s.verbose, "the other settings still count");
        let problem = problem.unwrap();
        assert_eq!(
            (problem.code, problem.at.line, problem.at.col),
            (Code::OutOfRange, 2, 15)
        );

        let (s, problem) =
            read(b"boot {\n    timeout = 3\n    default = 7\n    verbose = yes\n}\n");
        assert_eq!(s.timeout, 3);
        assert!(matches!(s.start, Start::First));
        assert!(!s.verbose);
        let problem = problem.unwrap();
        assert_eq!(
            (problem.code, problem.at.line),
            (Code::WrongType, 2),
            "the first problem is the one reported"
        );

        let (_, problem) = read(b"boot {\n    default = 7\n}\n");
        assert_eq!(
            problem.map(|p| (p.code, p.at.line)),
            Some((Code::WrongType, 2))
        );
        let (_, problem) = read(b"boot {\n    verbose = 1\n}\n");
        assert_eq!(
            problem.map(|p| (p.code, p.at.line)),
            Some((Code::WrongType, 2))
        );
    }

    #[test]
    fn a_broken_document_keeps_every_default() {
        let (s, problem) = read(b"boot {\n    timeout = 9s\n");
        assert_eq!(s.timeout, 3);
        assert_eq!(problem.map(|p| p.code), Some(Code::UnclosedBlock));
        let (_, problem) = read(b"\xff");
        assert_eq!(problem.map(|p| p.code), Some(Code::InvalidUtf8));
    }

    #[test]
    fn the_default_names_a_card_in_any_case() {
        assert_eq!(index("boot {\n    default = windows\n}\n", None), 1);
        assert_eq!(index("boot {\n    default = WINDOWS\n}\n", None), 1);
        assert_eq!(index("boot {\n    default = \"arch linux\"\n}\n", None), 2);
        assert_eq!(
            index("boot {\n    default = \"Arch \\u{4c}inux\"\n}\n", None),
            2,
            "escapes count as their characters"
        );
        assert_eq!(
            index("boot {\n    default = haiku\n}\n", None),
            0,
            "an unknown system falls back to the first"
        );
        assert_eq!(
            index("boot {\n    default = arch\n}\n", None),
            0,
            "the whole name has to match"
        );
        assert_eq!(index("", Some("Windows")), 0);
    }

    #[test]
    fn last_starts_what_started_before() {
        let last = "boot {\n    default = last\n}\n";
        assert!(matches!(start_of(last), Start::Last));
        assert_eq!(index(last, Some("Arch Linux")), 2);
        assert_eq!(index(last, Some("Meridian")), 0);
        assert_eq!(index(last, None), 0, "nothing remembered yet");
        assert_eq!(index(last, Some("Gone")), 0, "a system no longer there");
        assert_eq!(
            index(last, Some("windows")),
            0,
            "the remembered name is exact"
        );
    }

    #[test]
    fn names_compare_letter_by_letter() {
        let (s, _) = read("boot {\n    default = \"Ä b\"\n}\n".as_bytes());
        let Start::Named(name) = s.start else {
            panic!("a name")
        };
        assert!(is_named(name, "Ä B"));
        assert!(!is_named(name, "ä b"), "only ASCII letters ignore case");
        assert!(!is_named(name, "Ä"));
        assert!(!is_named(name, "Ä b c"));
    }

    #[test]
    fn settings_are_said_in_one_line() {
        assert_eq!(
            Settings::default().to_string(),
            "timeout 3 s, default Meridian, quiet"
        );
        let (named, _) = read(
            b"boot {\n    timeout = 0s\n    default = \"Arch Linux\"\n    verbose = true\n}\n",
        );
        assert_eq!(
            named.to_string(),
            "timeout 0 s, default Arch Linux, verbose"
        );
        let (last, _) = read(b"boot {\n    default = last\n}\n");
        assert_eq!(
            last.to_string(),
            "timeout 3 s, default the system started last, quiet"
        );
    }

    #[test]
    fn the_last_system_has_its_own_variable() {
        let name: String = char::decode_utf16(LAST_SYSTEM.iter().copied())
            .map(Result::unwrap)
            .collect();
        assert_eq!(name, "LastSystem\0");
    }

    #[test]
    fn files_up_to_the_limit_are_read() {
        assert!(fits(0));
        assert!(fits(4096));
        assert!(!fits(4097));
        assert!(!fits(u64::MAX));
        assert_eq!(LIMIT, 4096);
    }
}
