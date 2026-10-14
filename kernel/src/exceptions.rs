//! Words for the processor's exceptions, for the panic screen.

use core::fmt::{self, Write};

/// The name the Intel SDM gives exception `vector`, in lower case.
#[must_use]
pub fn name(vector: u64) -> &'static str {
    match vector {
        0 => "division error",
        1 => "debug exception",
        2 => "non-maskable interrupt",
        3 => "breakpoint",
        4 => "overflow",
        5 => "bound range exceeded",
        6 => "invalid opcode",
        7 => "device not available",
        8 => "double fault",
        9 => "coprocessor segment overrun",
        10 => "invalid task state segment",
        11 => "segment not present",
        12 => "stack segment fault",
        13 => "general protection fault",
        14 => "page fault",
        16 => "x87 floating point error",
        17 => "alignment check",
        18 => "machine check",
        19 => "SIMD floating point error",
        20 => "virtualization exception",
        21 => "control protection exception",
        28 => "hypervisor injection exception",
        29 => "VMM communication exception",
        30 => "security exception",
        _ => "reserved exception",
    }
}

/// What exception `vector` means, with its error code and, for a page
/// fault, the address it was about.
///
/// # Errors
///
/// Fails when the writer does.
pub fn describe(vector: u64, error: u64, address: u64, out: &mut impl Write) -> fmt::Result {
    match vector {
        14 => {
            let action = if error & 0x10 != 0 {
                "ran code at"
            } else if error & 0x2 != 0 {
                "wrote to"
            } else {
                "read"
            };
            let who = if error & 0x4 != 0 {
                "a program"
            } else {
                "the kernel"
            };
            write!(out, "page fault: {who} {action} {address:#x}, ")?;
            if error & 0x8 != 0 {
                out.write_str("and a page table entry there is broken")
            } else if error & 0x1 == 0 {
                out.write_str("which is not mapped")
            } else {
                out.write_str("which it may not")
            }
        }
        8 => out.write_str(
            "double fault: a second exception came while the first was being handled, \
             most likely because the kernel ran out of stack",
        ),
        10 | 11 | 12 | 13 | 17 | 21 | 29 | 30 if error != 0 => {
            write!(out, "{} (error code {error:#x})", name(vector))
        }
        _ => out.write_str(name(vector)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(vector: u64, error: u64, address: u64) -> String {
        let mut out = String::new();
        describe(vector, error, address, &mut out).unwrap();
        out
    }

    #[test]
    fn every_vector_has_a_name() {
        assert_eq!(name(0), "division error");
        assert_eq!(name(6), "invalid opcode");
        assert_eq!(name(13), "general protection fault");
        assert_eq!(name(14), "page fault");
        for reserved in [15, 22, 23, 24, 25, 26, 27, 31, 32, 255] {
            assert_eq!(name(reserved), "reserved exception", "{reserved}");
        }
        let named = (0..32).filter(|&v| name(v) != "reserved exception").count();
        assert_eq!(named, 24);
    }

    #[test]
    fn page_faults_say_what_was_done_where() {
        assert_eq!(
            text(14, 0, 0),
            "page fault: the kernel read 0x0, which is not mapped"
        );
        assert_eq!(
            text(14, 0b11, 0xffff_ffff_8000_5000),
            "page fault: the kernel wrote to 0xffffffff80005000, which it may not"
        );
        assert_eq!(
            text(14, 0b1_0001, 0x1000),
            "page fault: the kernel ran code at 0x1000, which it may not"
        );
        assert_eq!(
            text(14, 0b110, 0x40_0000),
            "page fault: a program wrote to 0x400000, which is not mapped"
        );
        assert_eq!(
            text(14, 0b1001, 0x2000),
            "page fault: the kernel read 0x2000, and a page table entry there is broken"
        );
    }

    #[test]
    fn error_codes_are_shown_where_they_mean_something() {
        assert_eq!(
            text(13, 0x28, 0),
            "general protection fault (error code 0x28)"
        );
        assert_eq!(text(13, 0, 0), "general protection fault");
        assert_eq!(
            text(6, 0, 0xdead),
            "invalid opcode",
            "no address for other exceptions"
        );
        assert!(text(8, 0, 0).starts_with("double fault: "));
        assert!(text(8, 0, 0).ends_with("ran out of stack"));
    }

    #[test]
    fn faults_with_error_codes_name_them_and_others_never_do() {
        for vector in [10, 11, 12, 13, 17, 21, 29, 30] {
            let with = text(vector, 0x18, 0);
            assert_eq!(with, format!("{} (error code 0x18)", name(vector)));
            assert_eq!(
                text(vector, 0, 0),
                name(vector),
                "an error code of 0 says nothing"
            );
        }
        for vector in [0, 1, 3, 4, 5, 6, 7, 16, 18, 19, 20] {
            assert_eq!(
                text(vector, 0x18, 0x1000),
                name(vector),
                "vector {vector} has no error code"
            );
        }
    }

    #[test]
    fn page_fault_bits_combine() {
        assert_eq!(
            text(14, 0b111, 0x5000),
            "page fault: a program wrote to 0x5000, which it may not"
        );
        assert_eq!(
            text(14, 0b1_0101, 0x6000),
            "page fault: a program ran code at 0x6000, which it may not"
        );
        assert_eq!(
            text(14, 0b1_0000, 0x7000),
            "page fault: the kernel ran code at 0x7000, which is not mapped"
        );
    }

    #[test]
    fn a_full_writer_stops_the_description() {
        let mut short = crate::Text::<10>::new();
        assert!(describe(14, 0, 0, &mut short).is_err());
        assert_eq!(short.as_str(), "page fault");
    }
}
