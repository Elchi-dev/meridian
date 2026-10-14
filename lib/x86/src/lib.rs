//! The `x86_64` instructions and devices both the loader and the kernel
//! use: the serial port, CPUID, control and model specific registers, and
//! stopping the processor.

#![cfg_attr(not(test), no_std)]

#[cfg(feature = "qemu-test")]
pub mod qemu_test;
#[allow(unsafe_code)]
pub mod raw;
pub mod serial;
pub mod tables;

/// Bits of CR0, from the Intel SDM, volume 3, section 2.5.
pub mod cr0 {
    /// Read-only pages stay read-only for the kernel too.
    pub const WRITE_PROTECT: u64 = 1 << 16;
}

/// The extended feature enable register and its bits (Intel SDM, volume 3,
/// section 2.2.1).
pub mod efer {
    pub const MSR: u32 = 0xc000_0080;
    /// Makes bit 63 of page table entries forbid running code.
    pub const NO_EXECUTE_ENABLE: u64 = 1 << 11;
}

/// What the processor can do, as far as the loader and the kernel care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    /// Page table entries can forbid running code.
    pub no_execute: bool,
    /// Level 3 entries can map 1 GiB pages.
    pub pages_1g: bool,
}

impl Features {
    /// Reads the bits from EDX of CPUID leaf `0x8000_0001`.
    #[must_use]
    pub fn from_extended_edx(edx: u32) -> Features {
        Features {
            no_execute: edx & 1 << 20 != 0,
            pages_1g: edx & 1 << 26 != 0,
        }
    }

    /// Asks this processor.
    #[must_use]
    pub fn detect() -> Features {
        if raw::cpuid(0x8000_0000)[0] < 0x8000_0001 {
            return Features::default();
        }
        Features::from_extended_edx(raw::cpuid(0x8000_0001)[3])
    }
}

/// Text the processor reports about itself in CPUID registers: the vendor
/// in 12 bytes, the brand in 48. Unused bytes are NUL or spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuText<const N: usize>([u8; N]);

impl<const N: usize> CpuText<N> {
    /// Joins the registers' bytes in the order given.
    #[must_use]
    pub fn from_registers(registers: &[u32]) -> CpuText<N> {
        let mut bytes = [0u8; N];
        for (chunk, register) in bytes.chunks_mut(4).zip(registers) {
            chunk.copy_from_slice(&register.to_le_bytes()[..chunk.len()]);
        }
        CpuText(bytes)
    }

    /// The text without padding, or an empty string if it is not ASCII.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(N);
        let text = core::str::from_utf8(&self.0[..end]).unwrap_or("");
        if text.is_ascii() { text.trim() } else { "" }
    }
}

/// The vendor, such as `AuthenticAMD` or `GenuineIntel`.
#[must_use]
pub fn vendor() -> CpuText<12> {
    let [_, b, c, d] = raw::cpuid(0);
    CpuText::from_registers(&[b, d, c])
}

/// The marketing name, such as `AMD Ryzen 5 5600G with Radeon Graphics`,
/// or an empty text on a processor that has none.
#[must_use]
pub fn brand() -> CpuText<48> {
    if raw::cpuid(0x8000_0000)[0] < 0x8000_0004 {
        return CpuText([0; 48]);
    }
    let [a, b, c] = [0x8000_0002, 0x8000_0003, 0x8000_0004].map(raw::cpuid);
    CpuText::from_registers([a, b, c].as_flattened())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value of the first `name : value` line in /proc/cpuinfo, when
    /// the build machine runs Linux.
    fn cpuinfo(name: &str) -> Option<String> {
        let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        info.lines().find_map(|l| {
            let (key, value) = l.split_once(':')?;
            (key.trim() == name).then(|| value.trim().to_string())
        })
    }

    #[test]
    fn text_joins_registers_in_order() {
        let regs = [
            u32::from_le_bytes(*b"Genu"),
            u32::from_le_bytes(*b"ineI"),
            u32::from_le_bytes(*b"ntel"),
        ];
        assert_eq!(
            CpuText::<12>::from_registers(&regs).as_str(),
            "GenuineIntel"
        );
        let backwards = [regs[2], regs[1], regs[0]];
        assert_eq!(
            CpuText::<12>::from_registers(&backwards).as_str(),
            "ntelineIGenu"
        );
    }

    #[test]
    fn text_drops_padding_and_refuses_garbage() {
        let padded = [
            u32::from_le_bytes(*b"  QE"),
            u32::from_le_bytes(*b"MU  "),
            u32::from_le_bytes(*b"\0xyz"),
        ];
        assert_eq!(CpuText::<12>::from_registers(&padded).as_str(), "QEMU");
        let high = [u32::from_le_bytes([b'a', 0xff, b'b', b'c']), 0, 0];
        assert_eq!(CpuText::<12>::from_registers(&high).as_str(), "");
        assert_eq!(CpuText::<12>::from_registers(&[]).as_str(), "");
        let full = [u32::from_le_bytes(*b"abcd"); 3];
        assert_eq!(
            CpuText::<12>::from_registers(&full).as_str(),
            "abcdabcdabcd"
        );
        assert_eq!(
            CpuText::<12>::from_registers(&[full[0]; 5]).as_str(),
            "abcdabcdabcd"
        );
    }

    #[test]
    fn brand_text_loses_its_padding() {
        let mut text = *b"       Intel(R) Core(TM) i7-10510U CPU @ 1.80GHz";
        let regs: Vec<u32> = text
            .chunks(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let brand = CpuText::<48>::from_registers(&regs);
        assert_eq!(brand.as_str(), "Intel(R) Core(TM) i7-10510U CPU @ 1.80GHz");
        text[47] = 0;
        let cut = CpuText::<48>::from_registers(
            &text
                .chunks(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect::<Vec<_>>(),
        );
        assert_eq!(cut.as_str(), "Intel(R) Core(TM) i7-10510U CPU @ 1.80GH");
    }

    /// Leaf 1 gives family and model in EAX, with the extended fields
    /// added in the way both vendors document. Linux shows the result.
    #[test]
    fn signature_agrees_with_linux() {
        let (Some(family), Some(model)) = (cpuinfo("cpu family"), cpuinfo("model")) else {
            return;
        };
        let eax = raw::cpuid(1)[0];
        let base_family = eax >> 8 & 0xf;
        let base_model = eax >> 4 & 0xf;
        let (mut f, mut m) = (base_family, base_model);
        if base_family == 0xf {
            f += eax >> 20 & 0xff;
        }
        if base_family == 0xf || base_family == 0x6 {
            m |= (eax >> 16 & 0xf) << 4;
        }
        assert_eq!(f.to_string(), family);
        assert_eq!(m.to_string(), model);
    }

    #[test]
    fn a_blank_brand_is_empty() {
        let spaces = [u32::from_le_bytes(*b"    "); 12];
        assert_eq!(CpuText::<48>::from_registers(&spaces).as_str(), "");
        let nothing = [0u32; 12];
        assert_eq!(
            CpuText::<48>::from_registers(&nothing).as_str(),
            "",
            "a processor without one"
        );
    }

    #[test]
    fn vendor_agrees_with_linux() {
        let Some(want) = cpuinfo("vendor_id") else {
            return;
        };
        assert_eq!(vendor().as_str(), want);
    }

    #[test]
    fn brand_agrees_with_linux() {
        let Some(want) = cpuinfo("model name") else {
            return;
        };
        assert_eq!(brand().as_str(), want);
        assert!(brand().as_str().len() > 4);
    }

    #[test]
    fn extended_feature_bits() {
        assert_eq!(Features::from_extended_edx(0), Features::default());
        let nx = Features::from_extended_edx(1 << 20);
        assert_eq!((nx.no_execute, nx.pages_1g), (true, false));
        let huge = Features::from_extended_edx(1 << 26);
        assert_eq!((huge.no_execute, huge.pages_1g), (false, true));
        let neighbours = Features::from_extended_edx(!(1 << 20 | 1 << 26));
        assert_eq!(neighbours, Features::default());
    }

    /// Linux lists the same CPUID bits as "nx" and "pdpe1gb".
    #[test]
    fn detect_agrees_with_linux() {
        let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") else {
            return;
        };
        let flags: Vec<&str> = info
            .lines()
            .find_map(|l| l.strip_prefix("flags"))
            .and_then(|l| l.split_once(':'))
            .map(|(_, f)| f.split_whitespace().collect())
            .unwrap_or_default();
        assert!(!flags.is_empty(), "no flags line in /proc/cpuinfo");
        let found = Features::detect();
        assert_eq!(found.no_execute, flags.contains(&"nx"));
        assert_eq!(found.pages_1g, flags.contains(&"pdpe1gb"));
    }

    #[test]
    fn register_bits() {
        assert_eq!(cr0::WRITE_PROTECT, 0x1_0000);
        assert_eq!(efer::MSR, 0xc000_0080);
        assert_eq!(efer::NO_EXECUTE_ENABLE, 0x800);
    }
}
