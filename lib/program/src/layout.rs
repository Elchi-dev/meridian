//! What keys type on a US keyboard: the character each key gives, with
//! shift held or not, and what a key does while shift or control is held.
//! Other layouts come from CLDR's with Horizon.

use crate::KeyEvent;

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

/// What a key did to what is being typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typed {
    Char(char),
    /// Control and D: what is typed ends there.
    End,
    Nothing,
}

/// Which shift and control keys are held, a bit each: left shift, right
/// shift, left control, right control. As a byte it fits an atomic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held(pub u8);

impl Held {
    /// What `key` types with what is held, after taking in a shift or
    /// control key going down or coming up.
    pub fn feed(&mut self, key: KeyEvent) -> Typed {
        let bit = match key.usage {
            0xe1 => 0b0001,
            0xe5 => 0b0010,
            0xe0 => 0b0100,
            0xe4 => 0b1000,
            _ if key.pressed => 0,
            _ => return Typed::Nothing,
        };
        if bit != 0 {
            self.0 = if key.pressed {
                self.0 | bit
            } else {
                self.0 & !bit
            };
            return Typed::Nothing;
        }
        if self.0 & 0b1100 != 0 {
            return if key.usage == 0x07 {
                Typed::End
            } else {
                Typed::Nothing
            };
        }
        us(key.usage, self.0 & 0b0011 != 0).map_or(Typed::Nothing, Typed::Char)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(usage: u16) -> KeyEvent {
        KeyEvent {
            usage,
            pressed: true,
        }
    }

    fn up(usage: u16) -> KeyEvent {
        KeyEvent {
            usage,
            pressed: false,
        }
    }

    /// What pressing and letting go of `keys`, one after another, types.
    fn typed(keys: &[KeyEvent]) -> Vec<Typed> {
        let mut held = Held::default();
        keys.iter().map(|&k| held.feed(k)).collect()
    }

    #[test]
    fn either_shift_types_capitals_while_it_is_held() {
        assert_eq!(
            typed(&[
                down(0x04),
                up(0x04),
                down(0xe1),
                down(0x04),
                up(0xe1),
                down(0x04)
            ]),
            [
                Typed::Char('a'),
                Typed::Nothing,
                Typed::Nothing,
                Typed::Char('A'),
                Typed::Nothing,
                Typed::Char('a')
            ]
        );
        assert_eq!(
            typed(&[
                down(0xe5),
                down(0x1e),
                down(0xe1),
                up(0xe5),
                down(0x1e),
                up(0xe1)
            ])[4],
            Typed::Char('!'),
            "the left one is still held"
        );
        assert_eq!(
            typed(&[down(0xe5), up(0xe5), down(0x04)])[2],
            Typed::Char('a')
        );
    }

    #[test]
    fn control_and_d_end_what_is_typed_and_control_types_nothing_else() {
        for control in [0xe0, 0xe4] {
            assert_eq!(
                typed(&[
                    down(control),
                    down(0x07),
                    down(0x04),
                    up(control),
                    down(0x07)
                ]),
                [
                    Typed::Nothing,
                    Typed::End,
                    Typed::Nothing,
                    Typed::Nothing,
                    Typed::Char('d')
                ],
                "{control:#x}"
            );
        }
        assert_eq!(
            typed(&[down(0xe0), down(0xe4), up(0xe0), down(0x07)])[3],
            Typed::End,
            "the right one is still held"
        );
        assert_eq!(
            typed(&[down(0xe1), down(0xe0), down(0x07)])[2],
            Typed::End,
            "with shift too"
        );
    }

    #[test]
    fn keys_coming_up_and_keys_that_type_nothing_do_nothing() {
        assert_eq!(
            typed(&[up(0x04), down(0x52), up(0x28), down(0x29)]),
            [Typed::Nothing; 4]
        );
        assert_eq!(
            typed(&[down(0x28), down(0x2a)]),
            [Typed::Char('\n'), Typed::Char('\u{8}')]
        );
        let mut held = Held(0);
        held.feed(down(0xe1));
        held.feed(down(0xe4));
        assert_eq!(held, Held(0b1001), "a bit for each");
        held.feed(up(0xe1));
        assert_eq!(held, Held(0b1000));
    }

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
