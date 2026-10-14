//! The firmware's boot options (UEFI 2.10, section 3.1.3): `BootOrder`
//! lists the numbers of the `Boot####` variables in the order the firmware
//! tries them, and each of those holds an `EFI_LOAD_OPTION`.

use crate::variable::name;

/// The numbers of the boot options, in the order the firmware tries them.
pub const BOOT_ORDER: [u16; 10] = name("BootOrder");
/// The number of the boot option the firmware started this time.
pub const BOOT_CURRENT: [u16; 12] = name("BootCurrent");

/// The option can be started.
pub const ACTIVE: u32 = 0x1;
/// The option is not shown in the firmware's own menus.
pub const HIDDEN: u32 = 0x8;
/// Which kind of option it is; boot options have category 0, applications
/// such as the firmware's setup other values.
pub const CATEGORY: u32 = 0x1f00;

/// The name of the variable that holds boot option `number`: `Boot` and
/// four upper-case hexadecimal digits, with its closing 0.
#[must_use]
pub fn variable(number: u16) -> [u16; 9] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = [0u16; 9];
    for (slot, b) in out.iter_mut().zip(*b"Boot") {
        *slot = u16::from(b);
    }
    for i in 0..4 {
        let digit = (number >> (12 - 4 * i)) & 0xf;
        out[4 + i] = u16::from(HEX[usize::from(digit)]);
    }
    out
}

/// The option numbers in `BootOrder` or `BootCurrent`: little-endian 16-bit
/// values. `None` for data of odd length.
#[must_use]
pub fn numbers(data: &[u8]) -> Option<impl Iterator<Item = u16> + '_> {
    data.len().is_multiple_of(2).then(|| {
        data.as_chunks::<2>()
            .0
            .iter()
            .map(|&n| u16::from_le_bytes(n))
    })
}

/// One boot option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadOption<'a> {
    pub attributes: u32,
    /// The description as UCS-2 bytes, without its closing 0.
    description: &'a [u8],
    /// The device paths, the first of which is what the option starts.
    pub path: &'a [u8],
    /// Data the started program gets, such as Windows' own boot data.
    pub optional: &'a [u8],
}

impl<'a> LoadOption<'a> {
    /// Reads an `EFI_LOAD_OPTION`: attributes, the length of the device
    /// paths, the description up to its 0, the paths and the rest. `None`
    /// when the parts do not fit the data.
    #[must_use]
    pub fn parse(data: &'a [u8]) -> Option<LoadOption<'a>> {
        let attributes = u32::from_le_bytes(data.get(..4)?.try_into().ok()?);
        let path_len = usize::from(u16::from_le_bytes(data.get(4..6)?.try_into().ok()?));
        let text = data.get(6..)?;
        let end = text.as_chunks::<2>().0.iter().position(|u| *u == [0, 0])? * 2;
        let paths = 6 + end + 2;
        let path = data.get(paths..paths.checked_add(path_len)?)?;
        Some(LoadOption {
            attributes,
            description: &text[..end],
            path,
            optional: &data[paths + path_len..],
        })
    }

    /// The description, such as "Windows Boot Manager".
    pub fn description(&self) -> impl Iterator<Item = char> + 'a {
        let units = self
            .description
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&u| u16::from_le_bytes(u));
        char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// Whether the option is one a boot menu should offer: active, not
    /// hidden, and a boot option rather than an application.
    #[must_use]
    pub fn is_offered(&self) -> bool {
        self.attributes & ACTIVE != 0
            && self.attributes & HIDDEN == 0
            && self.attributes & CATEGORY == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ucs2(s: &str) -> Vec<u8> {
        s.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    /// An option as the firmware stores it.
    fn option(attributes: u32, description: &str, path: &[u8], optional: &[u8]) -> Vec<u8> {
        let mut d = attributes.to_le_bytes().to_vec();
        d.extend(u16::try_from(path.len()).unwrap().to_le_bytes());
        d.extend(ucs2(description));
        d.extend(path);
        d.extend(optional);
        d
    }

    fn text(o: &LoadOption<'_>) -> String {
        o.description().collect()
    }

    #[test]
    fn variable_names_are_four_hex_digits() {
        let as_text = |n| {
            char::decode_utf16(variable(n))
                .map(Result::unwrap)
                .collect::<String>()
        };
        assert_eq!(as_text(0), "Boot0000\0");
        assert_eq!(as_text(0x00ab), "Boot00AB\0");
        assert_eq!(as_text(0x1234), "Boot1234\0");
        assert_eq!(as_text(0xffff), "BootFFFF\0");
        assert!(crate::variable::is_name(&variable(7)));
    }

    #[test]
    fn the_order_is_a_list_of_numbers() {
        let got: Vec<u16> = numbers(&[1, 0, 0, 0, 0x10, 0x20]).unwrap().collect();
        assert_eq!(got, [1, 0, 0x2010]);
        assert_eq!(numbers(&[]).unwrap().count(), 0);
        assert!(numbers(&[1, 0, 2]).is_none(), "odd length");
    }

    #[test]
    fn an_option_splits_into_its_parts() {
        let path = [4, 4, 8, 0, 0x41, 0, 0, 0, 0x7f, 0xff, 4, 0];
        let data = option(ACTIVE, "Windows Boot Manager", &path, b"WINDOWS");
        let o = LoadOption::parse(&data).unwrap();
        assert_eq!(o.attributes, ACTIVE);
        assert_eq!(text(&o), "Windows Boot Manager");
        assert_eq!(o.path, path);
        assert_eq!(o.optional, b"WINDOWS");
        let bare = option(0, "", &path, &[]);
        let o = LoadOption::parse(&bare).unwrap();
        assert_eq!(text(&o), "");
        assert_eq!(o.optional, b"");
    }

    #[test]
    fn descriptions_keep_their_letters() {
        let data = option(ACTIVE, "Arch Linux (linux-zen) ü", &[], &[]);
        assert_eq!(
            text(&LoadOption::parse(&data).unwrap()),
            "Arch Linux (linux-zen) ü"
        );
    }

    #[test]
    fn broken_options_are_refused() {
        let good = option(ACTIVE, "x", &[1, 2, 3, 4], &[]);
        assert!(LoadOption::parse(&good).is_some());
        assert!(LoadOption::parse(&good[..3]).is_none(), "no attributes");
        assert!(LoadOption::parse(&good[..5]).is_none(), "no path length");
        assert!(
            LoadOption::parse(&good[..good.len() - 1]).is_none(),
            "the path is cut"
        );
        let mut no_zero = good.clone();
        no_zero.truncate(8);
        assert!(
            LoadOption::parse(&no_zero).is_none(),
            "the description has no end"
        );
        let odd = [1, 0, 0, 0, 0, 0, 0x41];
        assert!(LoadOption::parse(&odd).is_none());
        let mut huge = good;
        huge[4..6].copy_from_slice(&0xffffu16.to_le_bytes());
        assert!(
            LoadOption::parse(&huge).is_none(),
            "a path longer than the data"
        );
    }

    #[test]
    fn only_active_visible_boot_options_are_offered() {
        let offered = |attributes| {
            let data = option(attributes, "x", &[], &[]);
            LoadOption::parse(&data).unwrap().is_offered()
        };
        assert!(offered(ACTIVE));
        assert!(offered(ACTIVE | 0x2), "force reconnect changes nothing");
        assert!(!offered(0), "inactive");
        assert!(!offered(ACTIVE | HIDDEN));
        assert!(!offered(ACTIVE | 0x100), "an application");
        assert!(!offered(ACTIVE | 0x1000));
    }
}
