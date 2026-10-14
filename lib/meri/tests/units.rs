//! Sizes and durations from quantities, worked out exactly as section 13
//! of the specification asks. The expected values come from exact
//! fractions, not from floating point.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests.
#![allow(clippy::unwrap_used)]

use meri::{Code, Event, Reader, Value};

/// Each quantity, with its size in bytes and its duration in nanoseconds.
type Expected = (&'static str, Result<u64, Code>, Result<i64, Code>);

const QUANTITIES: &[Expected] = &[
    ("1B", Ok(1), Err(Code::WrongType)),
    ("1KB", Ok(1000), Err(Code::WrongType)),
    ("1MB", Ok(1_000_000), Err(Code::WrongType)),
    ("1GB", Ok(1_000_000_000), Err(Code::WrongType)),
    ("1TB", Ok(1_000_000_000_000), Err(Code::WrongType)),
    ("1KiB", Ok(1024), Err(Code::WrongType)),
    ("1MiB", Ok(1_048_576), Err(Code::WrongType)),
    ("1GiB", Ok(1_073_741_824), Err(Code::WrongType)),
    ("1TiB", Ok(1_099_511_627_776), Err(Code::WrongType)),
    ("1.5KiB", Ok(1536), Err(Code::WrongType)),
    ("1.5B", Err(Code::OutOfRange), Err(Code::WrongType)),
    ("0.5KB", Ok(500), Err(Code::WrongType)),
    ("0.001KB", Ok(1), Err(Code::WrongType)),
    ("0.0001KB", Err(Code::OutOfRange), Err(Code::WrongType)),
    ("1e3B", Ok(1000), Err(Code::WrongType)),
    ("1.5e3KB", Ok(1_500_000), Err(Code::WrongType)),
    ("15e-1KiB", Ok(1536), Err(Code::WrongType)),
    ("0.0009765625KiB", Ok(1), Err(Code::WrongType)),
    (
        "0.00048828125KiB",
        Err(Code::OutOfRange),
        Err(Code::WrongType),
    ),
    ("-1B", Err(Code::OutOfRange), Err(Code::WrongType)),
    ("-0.0B", Ok(0), Err(Code::WrongType)),
    ("-0B", Ok(0), Err(Code::WrongType)),
    ("0B", Ok(0), Err(Code::WrongType)),
    (
        "8388607TiB",
        Ok(9_223_370_937_343_148_032),
        Err(Code::WrongType),
    ),
    ("8388608TiB", Err(Code::OutOfRange), Err(Code::WrongType)),
    (
        "9223372036854775807B",
        Ok(9_223_372_036_854_775_807),
        Err(Code::WrongType),
    ),
    (
        "9223372036854775807.0B",
        Ok(9_223_372_036_854_775_807),
        Err(Code::WrongType),
    ),
    (
        "9223372036854775808.0B",
        Err(Code::OutOfRange),
        Err(Code::WrongType),
    ),
    (
        "9223372036854775807.5B",
        Err(Code::OutOfRange),
        Err(Code::WrongType),
    ),
    ("1e30B", Err(Code::OutOfRange), Err(Code::WrongType)),
    ("0e99999999999999999999B", Ok(0), Err(Code::WrongType)),
    (
        "1e-99999999999999999999B",
        Err(Code::OutOfRange),
        Err(Code::WrongType),
    ),
    ("0.000_001MB", Ok(1), Err(Code::WrongType)),
    ("1_000.5KB", Ok(1_000_500), Err(Code::WrongType)),
    (
        "0.0000000000009094947017729282379150390625TiB",
        Ok(1),
        Err(Code::WrongType),
    ),
    (
        "4194304.0000000000009094947017729282379150390625TiB",
        Ok(4_611_686_018_427_387_905),
        Err(Code::WrongType),
    ),
    ("1mib", Err(Code::WrongType), Err(Code::WrongType)),
    ("5%", Err(Code::WrongType), Err(Code::WrongType)),
    ("3s", Err(Code::WrongType), Ok(3_000_000_000)),
    ("0.000000001s", Err(Code::WrongType), Ok(1)),
    ("1.5h", Err(Code::WrongType), Ok(5_400_000_000_000)),
    ("1d", Err(Code::WrongType), Ok(86_400_000_000_000)),
    (
        "106751d",
        Err(Code::WrongType),
        Ok(9_223_286_400_000_000_000),
    ),
    ("106752d", Err(Code::WrongType), Err(Code::OutOfRange)),
    (
        "-9223372036854775808ns",
        Err(Code::WrongType),
        Ok(-9_223_372_036_854_775_808),
    ),
    (
        "-9223372036854775808.0ns",
        Err(Code::WrongType),
        Ok(-9_223_372_036_854_775_808),
    ),
    (
        "9223372036854775808.0ns",
        Err(Code::WrongType),
        Err(Code::OutOfRange),
    ),
    ("0.5ns", Err(Code::WrongType), Err(Code::OutOfRange)),
    ("1.5us", Err(Code::WrongType), Ok(1500)),
    ("1e-9s", Err(Code::WrongType), Ok(1)),
    ("1e-10s", Err(Code::WrongType), Err(Code::OutOfRange)),
    ("2.5min", Err(Code::WrongType), Ok(150_000_000_000)),
    ("-1.5s", Err(Code::WrongType), Ok(-1_500_000_000)),
    ("1e3ms", Err(Code::WrongType), Ok(1_000_000_000)),
    ("1S", Err(Code::WrongType), Err(Code::WrongType)),
    ("1m", Err(Code::WrongType), Err(Code::WrongType)),
    ("1MiB", Ok(1_048_576), Err(Code::WrongType)),
    ("-0.5KiB", Err(Code::OutOfRange), Err(Code::WrongType)),
    ("1E2B", Ok(100), Err(Code::WrongType)),
    ("100e-2B", Ok(1), Err(Code::WrongType)),
    ("0.10e1KB", Ok(1000), Err(Code::WrongType)),
];

#[test]
fn sizes_and_durations_are_exact() {
    let mut failed = Vec::new();
    for (text, size, duration) in QUANTITIES {
        let src = format!("a = {text}");
        let Some(Ok(Event::Entry {
            value: Value::Quantity(q),
            ..
        })) = Reader::new(src.as_bytes()).next()
        else {
            panic!("{text} is not a quantity");
        };
        if q.size() != *size || q.duration() != *duration {
            failed.push(format!(
                "{text}: size {:?} duration {:?}, want {size:?} {duration:?}",
                q.size(),
                q.duration()
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
