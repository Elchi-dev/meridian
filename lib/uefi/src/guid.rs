//! `EFI_GUID` (UEFI 2.10, appendix A) and the GUIDs Meridian uses.

use core::fmt;

/// A GUID in the firmware's in-memory layout: the first three fields are
/// little-endian integers, the last eight bytes are stored as written.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    #[must_use]
    pub const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
        Guid {
            data1,
            data2,
            data3,
            data4,
        }
    }

    /// Parses the registry form used throughout the specification, such as
    /// `8be4df61-93ca-11d2-aa0d-00e098032b8c`. Upper and lower case both work.
    #[must_use]
    pub const fn try_parse(text: &str) -> Option<Guid> {
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        let mut digits = [0u8; 32];
        let (mut at, mut count) = (0, 0);
        while at < bytes.len() {
            let dash = at == 8 || at == 13 || at == 18 || at == 23;
            match (dash, bytes[at], hex(bytes[at])) {
                (true, b'-', _) => {}
                (false, _, Some(value)) => {
                    digits[count] = value;
                    count += 1;
                }
                _ => return None,
            }
            at += 1;
        }
        let mut data4 = [0u8; 8];
        let mut pair = 0;
        while pair < 8 {
            data4[pair] = (digits[16 + 2 * pair] << 4) | digits[17 + 2 * pair];
            pair += 1;
        }
        Some(Guid {
            data1: (nibbles16(&digits, 0) as u32) << 16 | nibbles16(&digits, 4) as u32,
            data2: nibbles16(&digits, 8),
            data3: nibbles16(&digits, 12),
            data4,
        })
    }

    /// Like [`Guid::try_parse`], for constants written in the source.
    ///
    /// # Panics
    ///
    /// Panics when `s` is not a GUID. In a `const` this stops the build.
    #[must_use]
    pub const fn parse(s: &str) -> Guid {
        match Guid::try_parse(s) {
            Some(g) => g,
            None => panic!("not a GUID in registry form"),
        }
    }

    /// The sixteen bytes as the firmware stores them.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        let a = self.data1.to_le_bytes();
        let b = self.data2.to_le_bytes();
        let c = self.data3.to_le_bytes();
        let d = self.data4;
        [
            a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5],
            d[6], d[7],
        ]
    }
}

const fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Four hex digits starting at `from`, as a number.
const fn nibbles16(digits: &[u8; 32], from: usize) -> u16 {
    (digits[from] as u16) << 12
        | (digits[from + 1] as u16) << 8
        | (digits[from + 2] as u16) << 4
        | digits[from + 3] as u16
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.data4;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

pub const GLOBAL_VARIABLE: Guid = Guid::parse("8be4df61-93ca-11d2-aa0d-00e098032b8c");
pub const LOADED_IMAGE_PROTOCOL: Guid = Guid::parse("5b1b31a1-9562-11d2-8e3f-00a0c969723b");
pub const DEVICE_PATH_PROTOCOL: Guid = Guid::parse("09576e91-6d3f-11d2-8e39-00a0c969723b");
pub const SIMPLE_FILE_SYSTEM_PROTOCOL: Guid = Guid::parse("964e5b22-6459-11d2-8e39-00a0c969723b");
pub const FILE_INFO: Guid = Guid::parse("09576e92-6d3f-11d2-8e39-00a0c969723b");
pub const GRAPHICS_OUTPUT_PROTOCOL: Guid = Guid::parse("9042a9de-23dc-4a38-96fb-7aded080516a");
pub const SIMPLE_TEXT_INPUT_EX_PROTOCOL: Guid = Guid::parse("dd9e7534-7762-4698-8c14-f58517a625aa");
pub const ACPI_20_TABLE: Guid = Guid::parse("8868e871-e4f1-11d3-bc22-0080c73c8881");
pub const SMBIOS3_TABLE: Guid = Guid::parse("f2fd1544-9794-4a2c-992e-e5bbcf20e394");
pub const IMAGE_SECURITY_DATABASE: Guid = Guid::parse("d719b2cb-3d3a-4596-a3bc-dad00e67656f");
pub const SHIM_LOCK_PROTOCOL: Guid = Guid::parse("605dab50-e046-4300-abb6-3dd810dd8b23");
/// The vendor of Meridian's own firmware variables, made at random for
/// Meridian.
pub const MERIDIAN_VARIABLE: Guid = Guid::parse("bacddb3d-6355-4764-b93f-9889689a4ce4");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_into_the_firmware_layout() {
        let g = Guid::parse("8be4df61-93ca-11d2-aa0d-00e098032b8c");
        assert_eq!(g.data1, 0x8be4_df61);
        assert_eq!(g.data2, 0x93ca);
        assert_eq!(g.data3, 0x11d2);
        assert_eq!(g.data4, [0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c]);
    }

    #[test]
    fn bytes_are_mixed_endian() {
        let g = Guid::parse("8be4df61-93ca-11d2-aa0d-00e098032b8c");
        assert_eq!(
            g.to_bytes(),
            [
                0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03,
                0x2b, 0x8c
            ]
        );
    }

    #[test]
    fn every_hex_digit_and_both_cases() {
        let g = Guid::parse("01234567-89AB-cdEF-0123-456789abcdef");
        assert_eq!(g.data1, 0x0123_4567);
        assert_eq!(g.data2, 0x89ab);
        assert_eq!(g.data3, 0xcdef);
        assert_eq!(g.data4, [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        let max = Guid::parse("ffffffff-ffff-ffff-ffff-ffffffffffff");
        assert_eq!(
            (max.data1, max.data2, max.data3, max.data4),
            (u32::MAX, u16::MAX, u16::MAX, [0xff; 8])
        );
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "8be4df61-93ca-11d2-aa0d-00e098032b8c",
            "00000000-0000-0000-0000-000000000000",
            "01234567-89ab-cdef-0123-456789abcdef",
        ] {
            assert_eq!(Guid::parse(s).to_string(), s);
            assert_eq!(format!("{:?}", Guid::parse(s)), s);
        }
        assert_eq!(
            Guid::parse("ABCDEF01-2345-6789-ABCD-EF0123456789").to_string(),
            "abcdef01-2345-6789-abcd-ef0123456789"
        );
    }

    #[test]
    fn rejects_anything_else() {
        for bad in [
            "",
            "8be4df61-93ca-11d2-aa0d-00e098032b8",
            "8be4df61-93ca-11d2-aa0d-00e098032b8cc",
            "8be4df61x93ca-11d2-aa0d-00e098032b8c",
            "8be4df61-93cax11d2-aa0d-00e098032b8c",
            "8be4df61-93ca-11d2xaa0d-00e098032b8c",
            "8be4df61-93ca-11d2-aa0dx00e098032b8c",
            "8be4df6g-93ca-11d2-aa0d-00e098032b8c",
            "8be4df61-93ca-11d2-aa0d-00e098032b8 ",
            "{8be4df61-93ca-11d2-aa0d-00e098032b8}",
            "8be4df61-93ca-11d2-aa0d-00e09803/b8c",
            "8be4df61-93ca-11d2-aa0d-00e09803:b8c",
            "8be4df61-93ca-11d2-aa0d-00e09803@b8c",
            "8be4df61-93ca-11d2-aa0d-00e09803`b8c",
            "8be4df61-93ca-11d2-aa0d-00e09803Gb8c",
        ] {
            assert_eq!(Guid::try_parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    #[should_panic(expected = "not a GUID in registry form")]
    fn parse_panics_on_bad_input() {
        let _ = Guid::parse("nope");
    }

    #[test]
    fn layout_is_sixteen_bytes() {
        assert_eq!(core::mem::size_of::<Guid>(), 16);
        assert_eq!(core::mem::align_of::<Guid>(), 4);
    }

    #[test]
    fn known_guids_as_published() {
        let cases = [
            (
                LOADED_IMAGE_PROTOCOL,
                "5b1b31a1-9562-11d2-8e3f-00a0c969723b",
            ),
            (DEVICE_PATH_PROTOCOL, "09576e91-6d3f-11d2-8e39-00a0c969723b"),
            (
                SIMPLE_FILE_SYSTEM_PROTOCOL,
                "964e5b22-6459-11d2-8e39-00a0c969723b",
            ),
            (FILE_INFO, "09576e92-6d3f-11d2-8e39-00a0c969723b"),
            (
                GRAPHICS_OUTPUT_PROTOCOL,
                "9042a9de-23dc-4a38-96fb-7aded080516a",
            ),
            (
                SIMPLE_TEXT_INPUT_EX_PROTOCOL,
                "dd9e7534-7762-4698-8c14-f58517a625aa",
            ),
            (ACPI_20_TABLE, "8868e871-e4f1-11d3-bc22-0080c73c8881"),
            (SMBIOS3_TABLE, "f2fd1544-9794-4a2c-992e-e5bbcf20e394"),
            (
                IMAGE_SECURITY_DATABASE,
                "d719b2cb-3d3a-4596-a3bc-dad00e67656f",
            ),
            (SHIM_LOCK_PROTOCOL, "605dab50-e046-4300-abb6-3dd810dd8b23"),
            (GLOBAL_VARIABLE, "8be4df61-93ca-11d2-aa0d-00e098032b8c"),
            (MERIDIAN_VARIABLE, "bacddb3d-6355-4764-b93f-9889689a4ce4"),
        ];
        for (guid, text) in cases {
            assert_eq!(guid.to_string(), text);
        }
    }
}
