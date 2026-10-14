//! Firmware variables (UEFI 2.10, section 8.2), and the global ones the
//! loader reads (section 3.3).

/// What a variable allows and how long it lasts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attributes(pub u32);

impl Attributes {
    /// Kept across a restart.
    pub const NON_VOLATILE: Attributes = Attributes(1);
    pub const BOOTSERVICE_ACCESS: Attributes = Attributes(2);
    pub const RUNTIME_ACCESS: Attributes = Attributes(4);
    /// Written only with a signature over a time stamp, as the Secure
    /// Boot databases are (section 8.2.2).
    pub const TIME_BASED_AUTHENTICATED_WRITE_ACCESS: Attributes = Attributes(0x20);

    #[must_use]
    pub const fn with(self, other: Attributes) -> Attributes {
        Attributes(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, other: Attributes) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A variable name in UCS-2 with its closing 0, made from ASCII text when
/// the program is compiled. `N` is the number of letters plus one.
///
/// # Panics
///
/// When `text` is not ASCII or `N` does not fit it, which fails the build
/// for names made in a constant.
#[must_use]
pub const fn name<const N: usize>(text: &str) -> [u16; N] {
    let bytes = text.as_bytes();
    assert!(bytes.len() + 1 == N, "N is the number of letters plus one");
    let mut out = [0u16; N];
    let mut i = 0;
    while i < bytes.len() {
        assert!(
            bytes[i] != 0 && bytes[i].is_ascii(),
            "names are ASCII without 0"
        );
        out[i] = bytes[i] as u16;
        i += 1;
    }
    out
}

/// Whether `name` ends in its 0 and has no other, as the firmware needs.
#[must_use]
pub fn is_name(name: &[u16]) -> bool {
    matches!(name.split_last(), Some((0, rest)) if !rest.contains(&0))
}

/// What the firmware can do at the next start, when asked through
/// [`OS_INDICATIONS`].
pub const OS_INDICATIONS_SUPPORTED: [u16; 23] = name("OsIndicationsSupported");
/// What the firmware should do at the next start.
pub const OS_INDICATIONS: [u16; 14] = name("OsIndications");
/// 1 when the firmware checks signatures as it starts programs.
pub const SECURE_BOOT: [u16; 11] = name("SecureBoot");
/// 1 while the firmware has no platform key, and so takes any.
pub const SETUP_MODE: [u16; 10] = name("SetupMode");
/// The platform key, which ends Setup Mode once it is written.
pub const PLATFORM_KEY: [u16; 3] = name("PK");
/// The keys that may change the signature databases.
pub const KEY_EXCHANGE_KEY: [u16; 4] = name("KEK");
/// The signatures and keys of the programs allowed to start, under
/// [`crate::guid::IMAGE_SECURITY_DATABASE`].
pub const ALLOWED: [u16; 3] = name("db");

/// The bit of [`OS_INDICATIONS`] that opens the firmware's settings at the
/// next start.
pub const BOOT_TO_FIRMWARE_UI: u64 = 1;

/// The value of a 64-bit variable such as [`OS_INDICATIONS`], which is
/// stored in 8 little-endian bytes.
#[must_use]
pub fn u64_value(data: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(data.try_into().ok()?))
}

/// Whether the firmware, by the data of its [`OS_INDICATIONS_SUPPORTED`],
/// can open its settings at the next start.
#[must_use]
pub fn offers_firmware_ui(supported: &[u8]) -> bool {
    u64_value(supported).is_some_and(|bits| bits & BOOT_TO_FIRMWARE_UI != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(name: &[u16]) -> String {
        char::decode_utf16(name.iter().copied())
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn names_end_in_a_zero() {
        assert_eq!(text(&OS_INDICATIONS), "OsIndications\0");
        assert_eq!(text(&OS_INDICATIONS_SUPPORTED), "OsIndicationsSupported\0");
        assert_eq!(text(&SECURE_BOOT), "SecureBoot\0");
        assert_eq!(text(&SETUP_MODE), "SetupMode\0");
        assert_eq!(text(&PLATFORM_KEY), "PK\0");
        assert_eq!(text(&KEY_EXCHANGE_KEY), "KEK\0");
        assert_eq!(text(&ALLOWED), "db\0");
        assert_eq!(name::<3>("Ab"), [0x41, 0x62, 0]);
        assert_eq!(name::<1>(""), [0]);
    }

    #[test]
    #[should_panic(expected = "letters plus one")]
    fn a_name_needs_room_for_its_zero() {
        let _ = name::<2>("Ab");
    }

    #[test]
    #[should_panic(expected = "letters plus one")]
    fn a_name_fills_its_array() {
        let _ = name::<4>("Ab");
    }

    #[test]
    #[should_panic(expected = "ASCII")]
    fn names_are_ascii() {
        let _ = name::<4>("Aé");
    }

    #[test]
    #[should_panic(expected = "ASCII")]
    fn names_hold_no_zero() {
        let _ = name::<3>("A\0");
    }

    #[test]
    fn attributes_are_the_specifications_bits() {
        let all = Attributes::NON_VOLATILE
            .with(Attributes::BOOTSERVICE_ACCESS)
            .with(Attributes::RUNTIME_ACCESS)
            .with(Attributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS);
        assert_eq!(
            all,
            Attributes(0x27),
            "what the Secure Boot databases carry"
        );
        assert!(all.contains(Attributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS));
        assert!(!Attributes(7).contains(Attributes::TIME_BASED_AUTHENTICATED_WRITE_ACCESS));
    }

    #[test]
    fn only_a_single_closing_zero_makes_a_name() {
        assert!(is_name(&OS_INDICATIONS));
        assert!(is_name(&[0]));
        assert!(!is_name(&[]));
        assert!(!is_name(&[0x41]));
        assert!(!is_name(&[0x41, 0, 0x42, 0]));
        assert!(!is_name(&[0x41, 0, 0]));
    }

    #[test]
    fn attributes_combine() {
        let both = Attributes::BOOTSERVICE_ACCESS.with(Attributes::RUNTIME_ACCESS);
        assert_eq!(both, Attributes(6));
        assert!(both.contains(Attributes::RUNTIME_ACCESS));
        assert!(both.contains(both));
        assert!(!both.contains(Attributes::NON_VOLATILE));
        assert!(!Attributes::NON_VOLATILE.contains(both));
        assert!(both.contains(Attributes::default()));
        assert_eq!(Attributes::NON_VOLATILE.0, 1);
    }

    #[test]
    fn the_firmware_ui_is_offered_by_its_bit() {
        assert!(offers_firmware_ui(&1u64.to_le_bytes()));
        assert!(offers_firmware_ui(&0x41u64.to_le_bytes()));
        assert!(!offers_firmware_ui(&0x40u64.to_le_bytes()));
        assert!(!offers_firmware_ui(&0u64.to_le_bytes()));
        assert!(!offers_firmware_ui(&[1]), "eight bytes or nothing");
        assert!(!offers_firmware_ui(&[]));
    }

    #[test]
    fn sixty_four_bit_values_take_exactly_eight_bytes() {
        assert_eq!(
            u64_value(&[1, 0, 0, 0, 0, 0, 0, 0]),
            Some(BOOT_TO_FIRMWARE_UI)
        );
        assert_eq!(
            u64_value(&0x0102_0304_0506_0708u64.to_le_bytes()),
            Some(0x0102_0304_0506_0708)
        );
        assert_eq!(u64_value(&[1, 0, 0, 0]), None);
        assert_eq!(u64_value(&[0; 9]), None);
        assert_eq!(u64_value(&[]), None);
    }
}
