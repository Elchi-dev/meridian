//! `EFI_STATUS` values (UEFI 2.10, appendix D).

use core::fmt;

/// The result of every firmware call. Errors have the highest bit set,
/// warnings are other non-zero values.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Status(pub usize);

const ERROR: usize = 1 << (usize::BITS - 1);

const fn error(code: usize) -> Status {
    Status(ERROR | code)
}

/// Defines each status once: its constant, its name in the specification
/// (the constant with `EFI_` in front) and what it means.
macro_rules! statuses {
    ($($name:ident = $value:expr, $meaning:literal;)*) => {
        impl Status {
            $(pub const $name: Status = $value;)*
        }

        const NAMES: &[(Status, &str, &str)] = &[
            $((Status::$name, concat!("EFI_", stringify!($name)), $meaning),)*
        ];
    };
}

statuses! {
    SUCCESS = Status(0), "the operation finished";
    WARN_UNKNOWN_GLYPH = Status(1), "a character could not be shown";
    WARN_DELETE_FAILURE = Status(2), "a file was closed but could not be deleted";
    WARN_WRITE_FAILURE = Status(3), "a file was closed but its data could not be written";
    WARN_BUFFER_TOO_SMALL = Status(4), "the result was cut short to fit the buffer";
    WARN_STALE_DATA = Status(5), "the data may be out of date";
    WARN_FILE_SYSTEM = Status(6), "the buffer holds a file system image";
    WARN_RESET_REQUIRED = Status(7), "the change takes effect after a restart";
    LOAD_ERROR = error(1), "the image could not be loaded";
    INVALID_PARAMETER = error(2), "a value passed to the firmware was not valid";
    UNSUPPORTED = error(3), "the firmware does not support this";
    BAD_BUFFER_SIZE = error(4), "the buffer has the wrong size for this request";
    BUFFER_TOO_SMALL = error(5), "the buffer is too small for the result";
    NOT_READY = error(6), "there is no data yet";
    DEVICE_ERROR = error(7), "the device reported an error";
    WRITE_PROTECTED = error(8), "the device cannot be written to";
    OUT_OF_RESOURCES = error(9), "the firmware ran out of memory or other resources";
    VOLUME_CORRUPTED = error(10), "the file system is damaged";
    VOLUME_FULL = error(11), "there is no space left on the volume";
    NO_MEDIA = error(12), "there is no medium in the device";
    MEDIA_CHANGED = error(13), "the medium changed since it was last read";
    NOT_FOUND = error(14), "the item was not found";
    ACCESS_DENIED = error(15), "access was denied";
    NO_RESPONSE = error(16), "the other side did not answer";
    NO_MAPPING = error(17), "there is no mapping to the device";
    TIMEOUT = error(18), "the operation timed out";
    NOT_STARTED = error(19), "the protocol has not been started";
    ALREADY_STARTED = error(20), "the protocol is already running";
    ABORTED = error(21), "the operation was aborted";
    ICMP_ERROR = error(22), "a network error was reported over ICMP";
    TFTP_ERROR = error(23), "a TFTP transfer failed";
    PROTOCOL_ERROR = error(24), "a network protocol error occurred";
    INCOMPATIBLE_VERSION = error(25), "the version is not compatible";
    SECURITY_VIOLATION = error(26), "Secure Boot refused it";
    CRC_ERROR = error(27), "a checksum did not match";
    END_OF_MEDIA = error(28), "the end of the medium was reached";
    END_OF_FILE = error(31), "the end of the file was reached";
    INVALID_LANGUAGE = error(32), "the language is not supported";
    COMPROMISED_DATA = error(33), "the data is not trustworthy";
    IP_ADDRESS_CONFLICT = error(34), "the IP address is already in use";
    HTTP_ERROR = error(35), "an HTTP error occurred";
}

impl Status {
    #[must_use]
    pub const fn is_error(self) -> bool {
        self.0 & ERROR != 0
    }

    #[must_use]
    pub const fn is_warning(self) -> bool {
        self.0 != 0 && !self.is_error()
    }

    /// Turns the status into a `Result`. Warnings count as success: the
    /// call did its work and only reports something worth knowing.
    ///
    /// # Errors
    ///
    /// Returns the status itself when it is an error.
    pub const fn ok(self) -> Result<(), Status> {
        if self.is_error() { Err(self) } else { Ok(()) }
    }

    /// The name the specification uses, such as `EFI_NOT_FOUND`.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.entry().0
    }

    /// What the status means, in a sentence a person can act on.
    #[must_use]
    pub fn meaning(self) -> &'static str {
        self.entry().1
    }

    fn entry(self) -> (&'static str, &'static str) {
        for &(status, name, meaning) in NAMES {
            if status == self {
                return (name, meaning);
            }
        }
        if self.is_error() {
            (
                "EFI_UNKNOWN_ERROR",
                "the firmware reported an error this version does not know",
            )
        } else {
            (
                "EFI_UNKNOWN_WARNING",
                "the firmware reported a warning this version does not know",
            )
        }
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:#x})", self.name(), self.0)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.meaning(), self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_have_the_top_bit_set() {
        assert_eq!(Status::LOAD_ERROR.0, 0x8000_0000_0000_0001);
        assert_eq!(Status::NOT_FOUND.0, 0x8000_0000_0000_000e);
        assert_eq!(Status::SECURITY_VIOLATION.0, 0x8000_0000_0000_001a);
        assert_eq!(Status::END_OF_FILE.0, 0x8000_0000_0000_001f);
        assert_eq!(Status::HTTP_ERROR.0, 0x8000_0000_0000_0023);
    }

    #[test]
    fn appendix_d_values() {
        let expected = [
            (Status::INVALID_PARAMETER, 2),
            (Status::UNSUPPORTED, 3),
            (Status::BAD_BUFFER_SIZE, 4),
            (Status::BUFFER_TOO_SMALL, 5),
            (Status::NOT_READY, 6),
            (Status::DEVICE_ERROR, 7),
            (Status::WRITE_PROTECTED, 8),
            (Status::OUT_OF_RESOURCES, 9),
            (Status::VOLUME_CORRUPTED, 10),
            (Status::VOLUME_FULL, 11),
            (Status::NO_MEDIA, 12),
            (Status::MEDIA_CHANGED, 13),
            (Status::ACCESS_DENIED, 15),
            (Status::NO_RESPONSE, 16),
            (Status::NO_MAPPING, 17),
            (Status::TIMEOUT, 18),
            (Status::NOT_STARTED, 19),
            (Status::ALREADY_STARTED, 20),
            (Status::ABORTED, 21),
            (Status::ICMP_ERROR, 22),
            (Status::TFTP_ERROR, 23),
            (Status::PROTOCOL_ERROR, 24),
            (Status::INCOMPATIBLE_VERSION, 25),
            (Status::CRC_ERROR, 27),
            (Status::END_OF_MEDIA, 28),
            (Status::INVALID_LANGUAGE, 32),
            (Status::COMPROMISED_DATA, 33),
            (Status::IP_ADDRESS_CONFLICT, 34),
        ];
        for (status, code) in expected {
            assert_eq!(status.0, ERROR | code, "{}", status.name());
        }
        let warnings = [
            (Status::WARN_UNKNOWN_GLYPH, 1),
            (Status::WARN_DELETE_FAILURE, 2),
            (Status::WARN_WRITE_FAILURE, 3),
            (Status::WARN_BUFFER_TOO_SMALL, 4),
            (Status::WARN_STALE_DATA, 5),
            (Status::WARN_FILE_SYSTEM, 6),
            (Status::WARN_RESET_REQUIRED, 7),
        ];
        for (status, code) in warnings {
            assert_eq!(status.0, code, "{}", status.name());
        }
    }

    #[test]
    fn success_warning_and_error_are_told_apart() {
        assert!(!Status::SUCCESS.is_error() && !Status::SUCCESS.is_warning());
        assert!(Status::WARN_STALE_DATA.is_warning() && !Status::WARN_STALE_DATA.is_error());
        assert!(Status::NOT_FOUND.is_error() && !Status::NOT_FOUND.is_warning());
        assert!(Status(ERROR).is_error());
        assert!(Status(ERROR - 1).is_warning());
    }

    #[test]
    fn ok_treats_warnings_as_success() {
        assert_eq!(Status::SUCCESS.ok(), Ok(()));
        assert_eq!(Status::WARN_UNKNOWN_GLYPH.ok(), Ok(()));
        assert_eq!(Status::DEVICE_ERROR.ok(), Err(Status::DEVICE_ERROR));
    }

    #[test]
    fn every_listed_status_has_its_own_name() {
        let all = [
            Status::SUCCESS,
            Status::WARN_UNKNOWN_GLYPH,
            Status::WARN_DELETE_FAILURE,
            Status::WARN_WRITE_FAILURE,
            Status::WARN_BUFFER_TOO_SMALL,
            Status::WARN_STALE_DATA,
            Status::WARN_FILE_SYSTEM,
            Status::WARN_RESET_REQUIRED,
            Status::LOAD_ERROR,
            Status::INVALID_PARAMETER,
            Status::UNSUPPORTED,
            Status::BAD_BUFFER_SIZE,
            Status::BUFFER_TOO_SMALL,
            Status::NOT_READY,
            Status::DEVICE_ERROR,
            Status::WRITE_PROTECTED,
            Status::OUT_OF_RESOURCES,
            Status::VOLUME_CORRUPTED,
            Status::VOLUME_FULL,
            Status::NO_MEDIA,
            Status::MEDIA_CHANGED,
            Status::NOT_FOUND,
            Status::ACCESS_DENIED,
            Status::NO_RESPONSE,
            Status::NO_MAPPING,
            Status::TIMEOUT,
            Status::NOT_STARTED,
            Status::ALREADY_STARTED,
            Status::ABORTED,
            Status::ICMP_ERROR,
            Status::TFTP_ERROR,
            Status::PROTOCOL_ERROR,
            Status::INCOMPATIBLE_VERSION,
            Status::SECURITY_VIOLATION,
            Status::CRC_ERROR,
            Status::END_OF_MEDIA,
            Status::END_OF_FILE,
            Status::INVALID_LANGUAGE,
            Status::COMPROMISED_DATA,
            Status::IP_ADDRESS_CONFLICT,
            Status::HTTP_ERROR,
        ];
        let mut names: Vec<&str> = all.iter().map(|s| s.name()).collect();
        for s in all {
            assert!(s.name().starts_with("EFI_"), "{s:?}");
            assert!(
                !s.meaning().is_empty() && !s.meaning().ends_with('.'),
                "{s:?}"
            );
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
        let suffix = |s: Status| s.name().trim_start_matches("EFI_").to_owned();
        assert_eq!(suffix(Status::SECURITY_VIOLATION), "SECURITY_VIOLATION");
        assert_eq!(suffix(Status::WARN_STALE_DATA), "WARN_STALE_DATA");
    }

    #[test]
    fn the_table_holds_every_status_once() {
        assert_eq!(NAMES.len(), 41);
        for (i, (a, ..)) in NAMES.iter().enumerate() {
            for (b, ..) in &NAMES[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn unknown_codes_still_say_what_kind_they_are() {
        assert_eq!(Status(ERROR | 0x63).name(), "EFI_UNKNOWN_ERROR");
        assert_eq!(Status(99).name(), "EFI_UNKNOWN_WARNING");
        assert!(Status(ERROR | 0x63).meaning().contains("error"));
        assert!(Status(99).meaning().contains("warning"));
    }

    #[test]
    fn unknown_codes_format_with_their_value() {
        assert_eq!(
            format!("{:?}", Status(ERROR | 0x63)),
            "EFI_UNKNOWN_ERROR (0x8000000000000063)"
        );
        assert_eq!(
            format!("{}", Status(0x63)),
            "the firmware reported a warning this version does not know (EFI_UNKNOWN_WARNING)"
        );
        assert_eq!(error(29).name(), "EFI_UNKNOWN_ERROR");
        assert_eq!(error(30).name(), "EFI_UNKNOWN_ERROR");
        assert_eq!(Status(8).name(), "EFI_UNKNOWN_WARNING");
    }

    #[test]
    fn formatting() {
        assert_eq!(
            format!("{:?}", Status::NOT_FOUND),
            "EFI_NOT_FOUND (0x800000000000000e)"
        );
        assert_eq!(
            format!("{}", Status::SECURITY_VIOLATION),
            "Secure Boot refused it (EFI_SECURITY_VIOLATION)"
        );
        assert_eq!(format!("{:?}", Status::SUCCESS), "EFI_SUCCESS (0x0)");
    }
}
