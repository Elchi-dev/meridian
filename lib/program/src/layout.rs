//! What keys type on a US keyboard: the character each key gives, with
//! shift held or not. Other layouts come from CLDR's with Horizon.

/// The character the key of USB HID usage `usage` types, shifted or not;
/// none for a key that types none, such as an arrow or shift itself.
#[must_use]
pub fn us(usage: u16, shift: bool) -> Option<char> {
    const DIGITS: &[u8; 10] = b"1234567890";
    const SHIFTED: &[u8; 10] = b"!@#$%^&*()";
    let byte = match usage {
        0x04..=0x1d => {
            let letter = b'a' + u8::try_from(usage - 0x04).ok()?;
            if shift {
                letter.to_ascii_uppercase()
            } else {
                letter
            }
        }
        0x1e..=0x27 => {
            let at = usize::from(usage - 0x1e);
            if shift { SHIFTED[at] } else { DIGITS[at] }
        }
        0x28 => b'\n',
        0x2a => 0x08,
        0x2b => b'\t',
        0x2c => b' ',
        _ => {
            let (plain, shifted) = match usage {
                0x2d => (b'-', b'_'),
                0x2e => (b'=', b'+'),
                0x2f => (b'[', b'{'),
                0x30 => (b']', b'}'),
                0x31 | 0x64 => (b'\\', b'|'),
                0x33 => (b';', b':'),
                0x34 => (b'\'', b'"'),
                0x35 => (b'`', b'~'),
                0x36 => (b',', b'<'),
                0x37 => (b'.', b'>'),
                0x38 => (b'/', b'?'),
                _ => return None,
            };
            if shift { shifted } else { plain }
        }
    };
    Some(char::from(byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_type_themselves_and_shift_makes_capitals() {
        assert_eq!(us(0x04, false), Some('a'));
        assert_eq!(us(0x1d, false), Some('z'));
        assert_eq!(us(0x04, true), Some('A'));
        assert_eq!(us(0x1d, true), Some('Z'));
        let all: String = (0x04..=0x1d).filter_map(|u| us(u, false)).collect();
        assert_eq!(all, "abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn the_row_of_digits_types_its_symbols_shifted() {
        let digits: String = (0x1e..=0x27).filter_map(|u| us(u, false)).collect();
        let symbols: String = (0x1e..=0x27).filter_map(|u| us(u, true)).collect();
        assert_eq!(
            (digits.as_str(), symbols.as_str()),
            ("1234567890", "!@#$%^&*()")
        );
    }

    #[test]
    fn the_other_keys_type_what_a_us_keyboard_shows() {
        let plain: String = [
            0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
        ]
        .iter()
        .filter_map(|&u| us(u, false))
        .collect();
        assert_eq!(plain, r"-=[]\;'`,./");
        let shifted: String = [
            0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
        ]
        .iter()
        .filter_map(|&u| us(u, true))
        .collect();
        assert_eq!(shifted, "_+{}|:\"~<>?");
        assert_eq!(
            us(0x64, false),
            Some('\\'),
            "the key left of z on ISO boards"
        );
        assert_eq!(
            (
                us(0x28, false),
                us(0x2a, false),
                us(0x2b, true),
                us(0x2c, true)
            ),
            (Some('\n'), Some('\u{8}'), Some('\t'), Some(' '))
        );
    }

    #[test]
    fn keys_that_type_nothing_give_none() {
        for usage in [
            0x00, 0x03, 0x29, 0x32, 0x39, 0x3a, 0x45, 0x52, 0xe1, 0xe6, 0xffff,
        ] {
            assert_eq!(
                (us(usage, false), us(usage, true)),
                (None, None),
                "{usage:#x}"
            );
        }
    }
}
