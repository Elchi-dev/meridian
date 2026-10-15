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

/// Bits of CR4, from the Intel SDM, volume 3, section 2.5.
pub mod cr4 {
    /// The kernel faults when it runs code from a page of ring 3.
    pub const SMEP: u64 = 1 << 20;
    /// The kernel faults when it touches a page of ring 3, unless the AC
    /// flag is set.
    pub const SMAP: u64 = 1 << 21;
}

/// What keeps the kernel off the pages of ring 3, as far as this processor
/// has it (Intel SDM, volume 3, section 4.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Protections {
    pub smep: bool,
    pub smap: bool,
}

impl Protections {
    /// Reads them from the highest basic CPUID leaf and EBX of leaf 7.
    #[must_use]
    pub fn from_cpuid(highest: u32, leaf_7_ebx: u32) -> Protections {
        if !has_leaf(highest, 7) {
            return Protections::default();
        }
        Protections {
            smep: leaf_7_ebx & 1 << 7 != 0,
            smap: leaf_7_ebx & 1 << 20 != 0,
        }
    }

    /// Asks this processor.
    #[must_use]
    pub fn detect() -> Protections {
        Protections::from_cpuid(raw::cpuid(0)[0], raw::cpuid(7)[1])
    }

    /// The bits of CR4 that turn them on.
    #[must_use]
    pub fn cr4_bits(self) -> u64 {
        [(self.smep, cr4::SMEP), (self.smap, cr4::SMAP)]
            .iter()
            .filter(|(on, _)| *on)
            .fold(0, |bits, (_, bit)| bits | bit)
    }
}

/// The extended feature enable register and its bits (Intel SDM, volume 3,
/// section 2.2.1).
pub mod efer {
    pub const MSR: u32 = 0xc000_0080;
    /// Makes bit 63 of page table entries forbid running code.
    pub const NO_EXECUTE_ENABLE: u64 = 1 << 11;
}

/// The register that holds a number for this processor, which RDTSCP and
/// RDPID hand back (Intel SDM, volume 4, `IA32_TSC_AUX`).
pub mod tsc_aux {
    pub const MSR: u32 = 0xc000_0103;

    /// Whether the processor has the register, given the highest extended
    /// CPUID leaf and EDX of leaf `0x8000_0001`: it comes with RDTSCP.
    #[must_use]
    pub fn supported(highest_extended: u32, extended_edx: u32) -> bool {
        crate::has_leaf(highest_extended, 0x8000_0001) && extended_edx & 1 << 27 != 0
    }
}

/// The local APIC in x2APIC mode, whose registers are model specific
/// registers from 0x800 on (Intel SDM, volume 3, chapter 11).
pub mod apic {
    /// `IA32_APIC_BASE`, with the bits that turn the APIC and its x2APIC
    /// mode on.
    pub const BASE_MSR: u32 = 0x1b;
    pub const ENABLE: u64 = 1 << 11;
    pub const X2APIC: u64 = 1 << 10;

    pub const ID: u32 = 0x802;
    pub const VERSION: u32 = 0x803;
    pub const END_OF_INTERRUPT: u32 = 0x80b;
    /// The interrupt command register, through which a processor sends
    /// interrupts to others.
    pub const COMMAND: u32 = 0x830;
    pub const SPURIOUS: u32 = 0x80f;
    pub const LVT_TIMER: u32 = 0x832;
    pub const TIMER_INITIAL: u32 = 0x838;
    pub const TIMER_CURRENT: u32 = 0x839;
    pub const TIMER_DIVIDE: u32 = 0x83e;
    /// The timer counts at the APIC's own rate, undivided.
    pub const DIVIDE_BY_1: u64 = 0b1011;
    /// The timer's local vector table entry bits that hold its interrupt
    /// back and that start it again each time it reaches 0.
    pub const MASKED: u64 = 1 << 16;
    pub const PERIODIC: u64 = 1 << 17;

    /// Whether the processor has an x2APIC, given ECX of CPUID leaf 1.
    #[must_use]
    pub fn supported(leaf_1_ecx: u32) -> bool {
        leaf_1_ecx & 1 << 21 != 0
    }

    /// The timer's local vector table entry for interrupt `vector` each
    /// time a period ends.
    #[must_use]
    pub fn periodic(vector: u8) -> u64 {
        u64::from(vector) | PERIODIC
    }

    /// The command that sends INIT to the processor whose APIC id is `id`,
    /// which stops it and has it wait for a start.
    #[must_use]
    pub fn init(id: u32) -> u64 {
        u64::from(id) << 32 | 1 << 14 | 0b101 << 8
    }

    /// The command that starts the processor whose APIC id is `id`, after
    /// an INIT, in real mode at the start of `page` below 1 MiB.
    #[must_use]
    pub fn startup(id: u32, page: u8) -> u64 {
        u64::from(id) << 32 | 1 << 14 | 0b110 << 8 | u64::from(page)
    }

    /// The command that sends interrupt `vector` to the processor whose
    /// APIC id is `id`.
    #[must_use]
    pub fn fixed(id: u32, vector: u8) -> u64 {
        u64::from(id) << 32 | 1 << 14 | u64::from(vector)
    }

    /// The values to write to `IA32_APIC_BASE`, which holds `base` now, one
    /// after the other: the APIC on, then its x2APIC mode, the only order
    /// the SDM allows from an APIC that is off.
    #[must_use]
    pub fn steps_to_x2apic(base: u64) -> [u64; 2] {
        [base | ENABLE, base | ENABLE | X2APIC]
    }

    /// The spurious interrupt register's value: the APIC on, and `vector`
    /// for the interrupts it gives up on.
    #[must_use]
    pub fn spurious(vector: u8) -> u64 {
        u64::from(vector) | 1 << 8
    }

    /// Whether the processor has a local APIC at all, given EDX of CPUID
    /// leaf 1.
    #[must_use]
    pub fn on_chip(leaf_1_edx: u32) -> bool {
        leaf_1_edx & 1 << 9 != 0
    }

    /// The physical address of the APIC's registers in xAPIC mode, from
    /// `IA32_APIC_BASE`: the page in bits 12 to 51.
    #[must_use]
    pub fn base_address(base: u64) -> u64 {
        base & 0x000f_ffff_ffff_f000
    }

    /// Where register `register`, named by its x2APIC number, lies in the
    /// xAPIC's page: 16 bytes apart in the same order.
    #[must_use]
    pub fn offset(register: u32) -> u64 {
        u64::from(register - 0x800) << 4
    }

    /// The two halves an xAPIC takes an interrupt command in, from the
    /// command in x2APIC form: the high one with the destination in its top
    /// byte, written first, and the low one, whose writing sends it.
    #[must_use]
    pub fn split_command(command: u64) -> (u32, u32) {
        let destination = (command >> 32) as u32;
        (
            destination << 24,
            u32::try_from(command & 0xffff_ffff).unwrap_or(0),
        )
    }

    /// The bit of the low command half that is set while an xAPIC is still
    /// delivering the last command.
    pub const PENDING: u32 = 1 << 12;

    /// The APIC id from an xAPIC's id register, which keeps it in the top
    /// byte.
    #[must_use]
    pub fn xapic_id(register: u32) -> u32 {
        register >> 24
    }

    /// The timer's count per period for `hz` interrupts a second, when it
    /// counted `counted` in `ticks` of a clock that does `clock_hz` a
    /// second, or `None` when the numbers make no period.
    #[must_use]
    pub fn period(counted: u64, ticks: u64, clock_hz: u64, hz: u64) -> Option<u32> {
        let rate = u128::from(counted) * u128::from(clock_hz) / u128::from(ticks.max(1));
        let period = rate / u128::from(hz.max(1));
        u32::try_from(period).ok().filter(|&p| p > 0)
    }
}

/// The 64-bit value an instruction hands back in two halves, as rdmsr and
/// rdtsc do in EDX and EAX.
#[must_use]
pub fn join(high: u32, low: u32) -> u64 {
    u64::from(high) << 32 | u64::from(low)
}

/// Whether CPUID has `leaf`, given the highest leaf of its range that
/// leaf `0x8000_0000` or leaf 0 reports.
#[must_use]
pub fn has_leaf(highest: u32, leaf: u32) -> bool {
    highest >= leaf
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
        if !has_leaf(raw::cpuid(0x8000_0000)[0], 0x8000_0001) {
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
    if !has_leaf(raw::cpuid(0x8000_0000)[0], 0x8000_0004) {
        return CpuText([0; 48]);
    }
    let [a, b, c] = [0x8000_0002, 0x8000_0003, 0x8000_0004].map(raw::cpuid);
    CpuText::from_registers([a, b, c].as_flattened())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protections_come_from_leaf_7() {
        let both = (1 << 7) | (1 << 20);
        assert_eq!(
            Protections::from_cpuid(7, both),
            Protections {
                smep: true,
                smap: true
            }
        );
        assert_eq!(
            Protections::from_cpuid(0x1f, 1 << 7),
            Protections {
                smep: true,
                smap: false
            }
        );
        assert_eq!(
            Protections::from_cpuid(0x1f, 1 << 20),
            Protections {
                smep: false,
                smap: true
            }
        );
        assert_eq!(
            Protections::from_cpuid(6, both),
            Protections::default(),
            "a processor without leaf 7 leaves EBX meaning something else"
        );
        assert_eq!(
            Protections::from_cpuid(0x1f, !both),
            Protections::default(),
            "only bits 7 and 20"
        );
    }

    #[test]
    fn each_protection_sets_its_own_bit_of_cr4() {
        let only = |smep, smap| Protections { smep, smap }.cr4_bits();
        assert_eq!(only(false, false), 0);
        assert_eq!(only(true, false), 1 << 20);
        assert_eq!(only(false, true), 1 << 21);
        assert_eq!(only(true, true), cr4::SMEP | cr4::SMAP);
        assert_eq!((cr4::SMEP, cr4::SMAP), (0x10_0000, 0x20_0000));
    }

    #[test]
    fn the_x2apic_bit_is_bit_21_of_leaf_1() {
        assert!(apic::supported(1 << 21));
        assert!(!apic::supported(!(1 << 21)));
        assert!(!apic::supported(0));
    }

    #[test]
    fn the_apic_registers_are_where_the_sdm_puts_them() {
        assert_eq!(
            [
                apic::END_OF_INTERRUPT,
                apic::SPURIOUS,
                apic::LVT_TIMER,
                apic::TIMER_INITIAL,
                apic::TIMER_CURRENT,
                apic::TIMER_DIVIDE
            ],
            [0x80b, 0x80f, 0x832, 0x838, 0x839, 0x83e]
        );
        assert_eq!(
            (apic::BASE_MSR, apic::ENABLE, apic::X2APIC),
            (0x1b, 0x800, 0x400)
        );
        assert_eq!(apic::DIVIDE_BY_1, 0xb);
    }

    #[test]
    fn timer_entries_carry_the_vector_mask_and_mode() {
        assert_eq!((apic::MASKED, apic::PERIODIC), (0x1_0000, 0x2_0000));
        assert_eq!(apic::periodic(0x20), 0x2_0020);
        assert_eq!(apic::periodic(0xff), 0x2_00ff);
        assert_eq!(apic::spurious(0xff), 0x1ff, "bit 8 turns the APIC on");
        assert_eq!(apic::spurious(0x27), 0x127);
    }

    #[test]
    fn commands_name_the_processor_and_what_to_do() {
        assert_eq!((apic::ID, apic::COMMAND), (0x802, 0x830));
        assert_eq!(apic::init(0), 0x4500, "INIT, level asserted");
        assert_eq!(apic::init(3), 0x3_0000_4500);
        assert_eq!(apic::init(u32::MAX), 0xffff_ffff_0000_4500);
        assert_eq!(apic::startup(1, 0x08), 0x1_0000_4608, "the page at 0x8000");
        assert_eq!(apic::startup(0x1_0203, 0x9f), 0x1_0203_0000_469f);
        assert_eq!(apic::fixed(0, 0x21), 0x4021, "fixed delivery, asserted");
        assert_eq!(apic::fixed(3, 0xff), 0x3_0000_40ff);
        assert_eq!(apic::fixed(u32::MAX, 0), 0xffff_ffff_0000_4000);
    }

    #[test]
    fn init_and_startup_differ_only_in_what_they_ask() {
        for id in [0, 1, 7, 0xff, 0x100, 0xdead_beef, u32::MAX] {
            let init = apic::init(id);
            assert_eq!(init >> 32, u64::from(id), "{id}: the destination");
            assert_eq!(init & 0xffff_ffff, 0x4500);
            for page in [0x01, 0x08, 0x9f, 0xff] {
                let startup = apic::startup(id, page);
                assert_eq!(startup >> 32, u64::from(id));
                assert_eq!(startup & 0xff, u64::from(page), "{id}: the page");
                assert_eq!(startup & 0xff00, 0x4600, "{id}: startup, asserted");
                assert_eq!(init ^ startup, 0x0300 | u64::from(page));
            }
        }
    }

    #[test]
    fn an_interrupt_differs_from_init_only_in_mode_and_vector() {
        for id in [0, 1, 7, 0xff, 0x100, 0xdead_beef, u32::MAX] {
            for vector in [0x00, 0x20, 0x21, 0x7f, 0xfe, 0xff] {
                let fixed = apic::fixed(id, vector);
                assert_eq!(fixed >> 32, u64::from(id), "{id}: the destination");
                assert_eq!(fixed & 0xff, u64::from(vector), "{id}: the vector");
                assert_eq!(fixed & 0x700, 0, "{id}: fixed delivery");
                assert_eq!(apic::init(id) ^ fixed, 0x500 | u64::from(vector));
            }
        }
    }

    #[test]
    fn xapic_registers_lie_where_their_x2apic_numbers_say() {
        assert!(apic::on_chip(1 << 9) && !apic::on_chip(!(1 << 9)));
        assert_eq!(
            apic::base_address(0xfee0_0900),
            0xfee0_0000,
            "flags and the BSP bit go"
        );
        assert_eq!(
            apic::base_address(0xfff0_0000_0000_0000 | 0x1_2345_6000),
            0x1_2345_6000
        );
        assert_eq!(apic::offset(apic::ID), 0x20);
        assert_eq!(apic::offset(apic::VERSION), 0x30);
        assert_eq!(apic::offset(apic::END_OF_INTERRUPT), 0xb0);
        assert_eq!(apic::offset(apic::SPURIOUS), 0xf0);
        assert_eq!(apic::offset(apic::COMMAND), 0x300);
        assert_eq!(apic::offset(apic::LVT_TIMER), 0x320);
        assert_eq!(apic::offset(apic::TIMER_INITIAL), 0x380);
        assert_eq!(apic::offset(apic::TIMER_CURRENT), 0x390);
        assert_eq!(apic::offset(apic::TIMER_DIVIDE), 0x3e0);
    }

    #[test]
    fn every_register_has_a_place_of_its_own_in_the_page() {
        let registers = [
            apic::ID,
            apic::VERSION,
            apic::END_OF_INTERRUPT,
            apic::SPURIOUS,
            apic::COMMAND,
            apic::COMMAND + 1,
            apic::LVT_TIMER,
            apic::TIMER_INITIAL,
            apic::TIMER_CURRENT,
            apic::TIMER_DIVIDE,
        ];
        let offsets = registers.map(apic::offset);
        for (i, &at) in offsets.iter().enumerate() {
            assert_eq!(at % 16, 0, "register {:#x}", registers[i]);
            assert!(
                at < 4096,
                "register {:#x} lies outside the page",
                registers[i]
            );
            assert!(
                !offsets[..i].contains(&at),
                "register {:#x} shares its place",
                registers[i]
            );
        }
        assert_eq!(
            apic::offset(apic::COMMAND + 1) - apic::offset(apic::COMMAND),
            0x10
        );
    }

    #[test]
    fn a_command_split_in_two_comes_back_whole() {
        for id in 0..=255 {
            for vector in [0x20, 0x21, 0xfe, 0xff] {
                let command = apic::fixed(id, vector);
                let (high, low) = apic::split_command(command);
                assert_eq!(
                    high & 0x00ff_ffff,
                    0,
                    "{id}: only the top byte names the processor"
                );
                assert_eq!(
                    u64::from(high >> 24) << 32 | u64::from(low),
                    command,
                    "{id}, {vector:#x}"
                );
            }
        }
        // An xAPIC addresses 8 bits of processors; more cannot be sent.
        let (high, _) = apic::split_command(apic::fixed(0x1_02, 0x21));
        assert_eq!(high >> 24, 0x02);
    }

    #[test]
    fn the_base_keeps_only_its_page() {
        for value in [
            0,
            0xfee0_0900,
            0xfee0_0fff,
            u64::MAX,
            0x8000_0000_0000_0d00,
            0x000f_ffff_ffff_f000,
        ] {
            let base = apic::base_address(value);
            assert_eq!(base % 4096, 0, "{value:#x}");
            assert_eq!(
                base >> 52,
                0,
                "{value:#x}: above the physical address width"
            );
            assert_eq!(apic::base_address(base), base, "{value:#x}");
            assert_eq!(base & !value, 0, "{value:#x}: nothing added");
        }
    }

    #[test]
    fn only_bit_9_says_there_is_an_apic() {
        for bit in 0..32 {
            assert_eq!(apic::on_chip(1 << bit), bit == 9, "bit {bit}");
            assert_eq!(apic::on_chip(!(1 << bit)), bit != 9, "all but bit {bit}");
        }
    }

    #[test]
    fn the_xapic_id_is_the_top_byte() {
        for id in 0..=255u32 {
            assert_eq!(apic::xapic_id(id << 24 | 0x00ff_ffff), id);
            assert_eq!(apic::xapic_id(id << 24), id);
        }
    }

    #[test]
    fn an_xapic_takes_commands_in_two_halves() {
        assert_eq!(apic::split_command(apic::init(3)), (0x0300_0000, 0x4500));
        assert_eq!(
            apic::split_command(apic::startup(0xff, 0x08)),
            (0xff00_0000, 0x4608)
        );
        assert_eq!(
            apic::split_command(apic::fixed(1, 0x21)),
            (0x0100_0000, 0x4021)
        );
        assert_eq!(apic::PENDING, 0x1000);
        assert_eq!(apic::xapic_id(0x0300_0000), 3);
        assert_eq!(apic::xapic_id(0xff12_3456), 0xff);
    }

    #[test]
    fn the_apic_is_turned_on_before_its_x2apic_mode() {
        assert_eq!(
            apic::steps_to_x2apic(0xfee0_0000),
            [0xfee0_0800, 0xfee0_0c00]
        );
        assert_eq!(
            apic::steps_to_x2apic(0xfee0_0900),
            [0xfee0_0900, 0xfee0_0d00],
            "on already, and the BSP bit kept"
        );
        assert_eq!(
            apic::steps_to_x2apic(0xfee0_0c00),
            [0xfee0_0c00, 0xfee0_0c00]
        );
    }

    #[test]
    fn the_period_follows_the_measured_rate() {
        // 1 GHz counted over 10 ms of a 3 GHz clock, for 100 a second.
        assert_eq!(
            apic::period(10_000_000, 30_000_000, 3_000_000_000, 100),
            Some(10_000_000)
        );
        assert_eq!(
            apic::period(10_000_000, 30_000_000, 3_000_000_000, 1000),
            Some(1_000_000)
        );
        assert_eq!(
            apic::period(999, 1000, 1000, 1000),
            None,
            "less than one count"
        );
        assert_eq!(apic::period(1, 1, 1 << 40, 100), None, "more than 32 bits");
        assert_eq!(
            apic::period(1000, 0, 1000, 0),
            Some(1_000_000),
            "nothing taken as one"
        );
        assert_eq!(apic::period(u64::MAX, 1, u64::MAX, 1 << 60), None);
    }

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
    fn halves_join_high_first() {
        assert_eq!(join(0, 0), 0);
        assert_eq!(join(1, 0), 1 << 32);
        assert_eq!(join(0, 1), 1);
        assert_eq!(join(0x0123_4567, 0x89ab_cdef), 0x0123_4567_89ab_cdef);
        assert_eq!(join(u32::MAX, u32::MAX), u64::MAX);
    }

    #[test]
    fn a_leaf_is_there_up_to_the_highest() {
        assert!(has_leaf(0x8000_0008, 0x8000_0001));
        assert!(
            has_leaf(0x8000_0004, 0x8000_0004),
            "the highest leaf itself"
        );
        assert!(!has_leaf(0x8000_0003, 0x8000_0004));
        assert!(!has_leaf(0, 0x8000_0001));
    }

    #[test]
    fn the_processor_number_comes_with_rdtscp() {
        let leaf = 0x8000_0001;
        assert!(tsc_aux::supported(leaf, 1 << 27));
        assert!(!tsc_aux::supported(leaf, !(1 << 27)));
        assert!(!tsc_aux::supported(leaf, 0));
        assert!(
            !tsc_aux::supported(leaf - 1, 1 << 27),
            "without the leaf its bits mean nothing"
        );
        assert_eq!(tsc_aux::MSR, 0xc000_0103);
        if let Some(flags) = cpuinfo("flags") {
            let [highest, ..] = raw::cpuid(0x8000_0000);
            let edx = raw::cpuid(0x8000_0001)[3];
            assert_eq!(
                tsc_aux::supported(highest, edx),
                flags.split_whitespace().any(|f| f == "rdtscp"),
                "Linux calls the same bit rdtscp"
            );
        }
    }

    #[test]
    fn the_time_stamp_counter_keeps_time() {
        // Measured twice against the system's clock, the counter runs at
        // the same rate, as an invariant counter does.
        let rate = || {
            let (start, cycles) = (std::time::Instant::now(), raw::tsc());
            std::thread::sleep(std::time::Duration::from_millis(100));
            let counted = u128::from(raw::tsc() - cycles);
            counted * 1_000_000 / start.elapsed().as_micros()
        };
        let (first, second) = (rate(), rate());
        assert!(
            (100_000_000..100_000_000_000).contains(&first),
            "{first} Hz"
        );
        assert!(
            raw::tsc() > u64::from(u32::MAX),
            "the build machine has been on for more than a few seconds, so the high half counts too"
        );
        assert!(
            first.abs_diff(second) * 10 < first,
            "{first} Hz, then {second} Hz"
        );
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
