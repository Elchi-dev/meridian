//! The floating point and vector registers: how a processor turns them on,
//! and how the kernel keeps a thread's while the thread does not run
//! (Intel SDM, volume 1, chapters 10 and 13).

use crate::{cr0, cr4, has_leaf};

/// The parts of the registers XSAVE keeps, as bits of XCR0.
pub mod parts {
    pub const X87: u64 = 1;
    pub const SSE: u64 = 1 << 1;
    /// The upper halves of the 16 YMM registers.
    pub const AVX: u64 = 1 << 2;
    /// AVX-512's mask registers, the upper halves of ZMM0 to ZMM15 and
    /// ZMM16 to ZMM31: XCR0 takes all three or none.
    pub const OPMASK: u64 = 1 << 5;
    pub const ZMM_HI256: u64 = 1 << 6;
    pub const HI16_ZMM: u64 = 1 << 7;
    pub const AVX_512: u64 = OPMASK | ZMM_HI256 | HI16_ZMM;
}

/// How a thread's registers are kept while it does not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saving {
    /// FXSAVE and FXRSTOR, which every x86-64 processor has: the x87 and
    /// SSE registers, in 512 bytes.
    Fx,
    /// XSAVE and XRSTOR, of the parts whose bits these are, which XCR0
    /// gets. The x87 and SSE are always among them.
    X(u64),
}

impl Saving {
    /// Picks from the highest basic CPUID leaf, ECX of leaf 1 and EAX of
    /// leaf 0xD: XSAVE wherever the processor has it, for the x87 and SSE
    /// registers, AVX's where it has them, and AVX-512's where it has every
    /// part of them as well.
    #[must_use]
    pub fn from_cpuid(highest: u32, leaf_1_ecx: u32, leaf_d_eax: u32) -> Saving {
        if !has_leaf(highest, 0xd) || leaf_1_ecx & 1 << 26 == 0 {
            return Saving::Fx;
        }
        let has = u64::from(leaf_d_eax);
        let mut on = parts::X87 | parts::SSE | has & parts::AVX;
        if on & parts::AVX != 0 && has & parts::AVX_512 == parts::AVX_512 {
            on |= parts::AVX_512;
        }
        Saving::X(on)
    }

    /// Asks this processor.
    #[must_use]
    pub fn detect() -> Saving {
        let highest = crate::raw::cpuid(0)[0];
        let leaf_d = if has_leaf(highest, 0xd) {
            crate::raw::cpuid(0xd)[0]
        } else {
            0
        };
        Saving::from_cpuid(highest, crate::raw::cpuid(1)[2], leaf_d)
    }

    /// The bits of CR4 that turn SSE on, and XSAVE with it if it is used.
    #[must_use]
    pub fn cr4_bits(self) -> u64 {
        let xsave = match self {
            Saving::Fx => 0,
            Saving::X(_) => cr4::OSXSAVE,
        };
        cr4::OSFXSR | cr4::OSXMMEXCPT | xsave
    }

    /// Whether the YMM registers' upper halves are kept, and so may be used.
    #[must_use]
    pub fn has_avx(self) -> bool {
        matches!(self, Saving::X(on) if on & parts::AVX != 0)
    }
}

/// CR0 as `old` with the x87 unit on and nothing in the way: no emulation,
/// no fault at the first instruction after a switch, and errors reported
/// as exceptions.
#[must_use]
pub fn cr0_on(old: u64) -> u64 {
    old & !(cr0::EMULATION | cr0::TASK_SWITCHED) | cr0::MONITOR_COPROCESSOR | cr0::NUMERIC_ERROR
}

/// How many bytes [`fresh`] fills: the 512 FXSAVE keeps, and the 64 byte
/// header XSAVE puts after them.
pub const FRESH_BYTES: usize = 576;

/// The x87 control word FNINIT leaves: every exception masked, rounding to
/// nearest, 64-bit precision.
pub const FRESH_CONTROL: u16 = 0x037f;

/// MXCSR as the processor starts with it: every SSE exception masked and
/// rounding to nearest.
pub const FRESH_MXCSR: u32 = 0x1f80;

/// The registers a new thread starts with, as FXRSTOR and XRSTOR read
/// them: the x87 unit as FNINIT leaves it, every register empty, MXCSR as
/// at power on, every vector register zero, and a header that says every
/// other part is as at power on too.
#[must_use]
pub fn fresh() -> [u8; FRESH_BYTES] {
    let mut area = [0; FRESH_BYTES];
    area[..2].copy_from_slice(&FRESH_CONTROL.to_le_bytes());
    area[24..28].copy_from_slice(&FRESH_MXCSR.to_le_bytes());
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    // CPUID as two real processors give it: a Ryzen 5 5600G, with AVX and
    // no AVX-512, and an EPYC 9355P under KVM, with both.
    const RYZEN: (u32, u32, u32) = (0x10, 0x7ef8_320b, 0x207);
    const EPYC: (u32, u32, u32) = (0x10, 0xfffa_3203, 0x2e7);

    fn pick((highest, ecx, eax): (u32, u32, u32)) -> Saving {
        Saving::from_cpuid(highest, ecx, eax)
    }

    #[test]
    fn xsave_keeps_what_the_processor_has_of_the_known_parts() {
        assert_eq!(pick(RYZEN), Saving::X(0b111), "x87, SSE and AVX, no PKRU");
        assert_eq!(
            pick(EPYC),
            Saving::X(0b1110_0111),
            "and AVX-512's three parts"
        );
        assert_eq!(
            pick((0x10, 1 << 26, 0b11)),
            Saving::X(0b11),
            "XSAVE without AVX"
        );
        assert_eq!(
            pick((0x10, 1 << 26, 0b1100_0111)),
            Saving::X(0b111),
            "AVX-512 needs every part"
        );
        assert_eq!(
            pick((0x10, 1 << 26, 0b1110_0011)),
            Saving::X(0b11),
            "and AVX under it"
        );
        assert_eq!(
            pick((0x10, 1 << 26, 0)),
            Saving::X(0b11),
            "the x87 and SSE are always there"
        );
    }

    #[test]
    fn without_xsave_fxsave_keeps_them() {
        assert_eq!(pick((0x10, RYZEN.1 & !(1 << 26), RYZEN.2)), Saving::Fx);
        assert_eq!(pick((0xc, RYZEN.1, RYZEN.2)), Saving::Fx, "no leaf 0xD");
        assert_eq!(pick((0xd, RYZEN.1, RYZEN.2)), Saving::X(0b111));
    }

    #[test]
    fn cr4_turns_sse_on_and_xsave_where_it_is_used() {
        assert_eq!(Saving::Fx.cr4_bits(), 0x600);
        assert_eq!(Saving::X(0b11).cr4_bits(), 0x4_0600);
        assert_eq!(pick(EPYC).cr4_bits(), 0x4_0600);
        assert_eq!(
            (cr4::OSFXSR, cr4::OSXMMEXCPT, cr4::OSXSAVE),
            (0x200, 0x400, 0x4_0000)
        );
    }

    #[test]
    fn avx_may_be_used_only_where_it_is_kept() {
        assert!(pick(RYZEN).has_avx());
        assert!(pick(EPYC).has_avx());
        assert!(!Saving::X(0b11).has_avx());
        assert!(!Saving::Fx.has_avx());
        assert!(!Saving::X(parts::AVX_512 | 0b11).has_avx());
    }

    #[test]
    fn cr0_has_the_x87_unit_on_and_leaves_the_rest() {
        // As OVMF leaves CR0: paging, protection, write protect, MP, ET, NE.
        assert_eq!(cr0_on(0x8001_0033), 0x8001_0033);
        assert_eq!(cr0_on(0x8001_003f), 0x8001_0033, "EM and TS cleared");
        assert_eq!(cr0_on(0x8000_0011), 0x8000_0033, "MP and NE set");
        assert_eq!(cr0_on(0), 0x22);
        assert_eq!(cr0_on(u64::MAX), !0b1100);
    }

    #[test]
    fn a_fresh_thread_starts_with_the_registers_power_on_gives() {
        let area = fresh();
        assert_eq!(&area[..2], [0x7f, 0x03], "the control word");
        assert_eq!(&area[24..28], [0x80, 0x1f, 0, 0], "MXCSR");
        assert!(
            area[2..24].iter().all(|&b| b == 0),
            "no flags, no x87 register in use"
        );
        assert!(
            area[28..].iter().all(|&b| b == 0),
            "zero registers, an empty header"
        );
        assert_eq!(area.len(), 512 + 64);
        assert_eq!((FRESH_CONTROL, FRESH_MXCSR), (0x37f, 0x1f80));
    }

    #[test]
    fn this_processor_is_read_as_the_standard_library_reads_it() {
        let saving = Saving::detect();
        let kept = |wanted: u64| matches!(saving, Saving::X(on) if on & wanted == wanted);
        if std::is_x86_feature_detected!("xsave") {
            assert!(kept(parts::X87 | parts::SSE), "{saving:?}");
        }
        if std::is_x86_feature_detected!("avx") {
            assert!(saving.has_avx(), "{saving:?}");
        }
        if std::is_x86_feature_detected!("avx512f") {
            assert!(kept(parts::AVX_512), "{saving:?}");
        }
    }

    #[test]
    fn the_x87_units_bits_of_cr0() {
        assert_eq!(
            (
                cr0::MONITOR_COPROCESSOR,
                cr0::EMULATION,
                cr0::TASK_SWITCHED,
                cr0::NUMERIC_ERROR
            ),
            (0x2, 0x4, 0x8, 0x20)
        );
    }

    #[test]
    fn avx_512_is_its_three_parts() {
        assert_eq!(parts::AVX_512, 0xe0);
        assert_eq!((parts::X87, parts::SSE, parts::AVX), (1, 2, 4));
        assert_eq!(
            (parts::OPMASK, parts::ZMM_HI256, parts::HI16_ZMM),
            (0x20, 0x40, 0x80)
        );
    }
}
