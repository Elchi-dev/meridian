//! The PS/2 controller, the i8042 a PC's keyboard sits behind, and the
//! keyboard itself, the parts that need no hardware: its ports, commands
//! and configuration, and the scan codes it sends read as keys. The
//! controller translates what the keyboard sends into scan code set 1, so
//! that is the set read here. Keys are named by their USB HID usage, the
//! number a USB keyboard gives the key in the same place, so both kinds of
//! keyboard end up as the same keys and a layout decides later what each
//! one types.

#![cfg_attr(not(test), no_std)]

/// The controller's I/O ports.
pub mod port {
    /// Bytes from the keyboard, and to it.
    pub const DATA: u16 = 0x60;
    /// The status when read, a command for the controller when written.
    pub const COMMAND: u16 = 0x64;
}

/// In the status register.
pub mod status {
    /// A byte waits in [`super::port::DATA`] to be read.
    pub const OUTPUT_FULL: u8 = 1;
    /// The controller has not taken the last byte written yet.
    pub const INPUT_FULL: u8 = 1 << 1;
}

/// Commands for the controller.
pub mod command {
    pub const READ_CONFIG: u8 = 0x20;
    pub const WRITE_CONFIG: u8 = 0x60;
    pub const DISABLE_SECOND: u8 = 0xa7;
    pub const SELF_TEST: u8 = 0xaa;
    pub const DISABLE_FIRST: u8 = 0xad;
    pub const ENABLE_FIRST: u8 = 0xae;
}

/// What the controller answers its self test with when it passed.
pub const TEST_PASSED: u8 = 0x55;

/// In the configuration byte.
pub mod config {
    pub const FIRST_INTERRUPT: u8 = 1;
    pub const SECOND_INTERRUPT: u8 = 1 << 1;
    pub const FIRST_CLOCK_OFF: u8 = 1 << 4;
    pub const SECOND_CLOCK_OFF: u8 = 1 << 5;
    pub const TRANSLATION: u8 = 1 << 6;
}

/// The configuration for the keyboard on the first port: its interrupt on
/// and its clock running, translation to scan code set 1 on, and the
/// second port, for a mouse, left off for now. The other bits stay.
#[must_use]
pub fn keyboard_config(old: u8) -> u8 {
    let on = config::FIRST_INTERRUPT | config::TRANSLATION | config::SECOND_CLOCK_OFF;
    let off = config::FIRST_CLOCK_OFF | config::SECOND_INTERRUPT;
    (old | on) & !off
}

/// A key, by its USB HID usage on the keyboard page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(pub u16);

impl Key {
    pub const A: Key = Key(0x04);
    pub const Z: Key = Key(0x1d);
    pub const ENTER: Key = Key(0x28);
    pub const ESCAPE: Key = Key(0x29);
    pub const SPACE: Key = Key(0x2c);
    pub const F12: Key = Key(0x45);
    pub const PAUSE: Key = Key(0x48);
    pub const UP: Key = Key(0x52);
    pub const LEFT_SHIFT: Key = Key(0xe1);
    pub const RIGHT_ALT: Key = Key(0xe6);
}

/// A key going down or coming up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub key: Key,
    pub pressed: bool,
}

/// Events waiting to be taken, the oldest first, at most `N`; one more is
/// dropped, as a keyboard drops what its own buffer has no room for.
pub struct Keys<const N: usize> {
    events: [Event; N],
    first: usize,
    len: usize,
}

impl<const N: usize> Default for Keys<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Keys<N> {
    #[must_use]
    pub const fn new() -> Self {
        Keys {
            events: [Event {
                key: Key(0),
                pressed: false,
            }; N],
            first: 0,
            len: 0,
        }
    }

    /// Puts `event` at the end, and says whether there was room for it.
    pub fn push(&mut self, event: Event) -> bool {
        if self.len == N {
            return false;
        }
        self.events[(self.first + self.len) % N] = event;
        self.len += 1;
        true
    }

    /// Takes the oldest event.
    pub fn pop(&mut self) -> Option<Event> {
        if self.len == 0 {
            return None;
        }
        let event = self.events[self.first];
        self.first = (self.first + 1) % N;
        self.len -= 1;
        Some(event)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The usage of each scan code of set 1 without a prefix, 0 for none.
const PLAIN: [u8; 0x59] = [
    0x00, 0x29, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, // 0x00: escape, 1 to 6
    0x24, 0x25, 0x26, 0x27, 0x2d, 0x2e, 0x2a, 0x2b, // 7 to 0, - =, backspace, tab
    0x14, 0x1a, 0x08, 0x15, 0x17, 0x1c, 0x18, 0x0c, // 0x10: q w e r t y u i
    0x12, 0x13, 0x2f, 0x30, 0x28, 0xe0, 0x04, 0x16, // o p [ ], enter, left control, a s
    0x07, 0x09, 0x0a, 0x0b, 0x0d, 0x0e, 0x0f, 0x33, // 0x20: d f g h j k l ;
    0x34, 0x35, 0xe1, 0x31, 0x1d, 0x1b, 0x06, 0x19, // ' `, left shift, \, z x c v
    0x05, 0x11, 0x10, 0x36, 0x37, 0x38, 0xe5,
    0x55, // 0x30: b n m , . /, right shift, keypad *
    0xe2, 0x2c, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, // left alt, space, caps lock, f1 to f5
    0x3f, 0x40, 0x41, 0x42, 0x43, 0x53, 0x47,
    0x5f, // 0x40: f6 to f10, num lock, scroll lock, keypad 7
    0x60, 0x61, 0x56, 0x5c, 0x5d, 0x5e, 0x57, 0x59, // keypad 8 9 - 4 5 6 + 1
    0x5a, 0x5b, 0x62, 0x63, 0x00, 0x00, 0x64,
    0x44, // 0x50: keypad 2 3 0 ., the key left of z, f11
    0x45, // f12
];

/// The usage of each scan code that comes after 0xe0.
const EXTENDED: [(u8, u8); 18] = [
    (0x1c, 0x58), // keypad enter
    (0x1d, 0xe4), // right control
    (0x35, 0x54), // keypad /
    (0x37, 0x46), // print screen
    (0x38, 0xe6), // right alt
    (0x47, 0x4a), // home
    (0x48, 0x52), // up
    (0x49, 0x4b), // page up
    (0x4b, 0x50), // left
    (0x4d, 0x4f), // right
    (0x4f, 0x4d), // end
    (0x50, 0x51), // down
    (0x51, 0x4e), // page down
    (0x52, 0x49), // insert
    (0x53, 0x4c), // delete
    (0x5b, 0xe3), // left windows key
    (0x5c, 0xe7), // right windows key
    (0x5d, 0x65), // menu
];

/// Reads the bytes a keyboard sends in scan code set 1 as keys going down
/// and up, one byte at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decoder {
    /// 0xe0 came, so the next code is an extended one.
    extended: bool,
    /// How many bytes of Pause's sequence after its 0xe1 are still to come,
    /// and whether it is the half that presses it.
    pause: u8,
    pause_down: bool,
}

impl Decoder {
    #[must_use]
    pub const fn new() -> Decoder {
        Decoder {
            extended: false,
            pause: 0,
            pause_down: false,
        }
    }

    /// Takes the next byte, and answers the key it ends, if it ends one.
    /// Bytes that are no key, and the shifts some keyboards wrap around
    /// extended keys, are passed over.
    pub fn feed(&mut self, byte: u8) -> Option<Event> {
        if self.pause > 0 {
            self.pause -= 1;
            if self.pause == 1 {
                self.pause_down = byte == 0x1d;
                return None;
            }
            return Some(Event {
                key: Key::PAUSE,
                pressed: self.pause_down,
            });
        }
        match byte {
            0xe0 => {
                self.extended = true;
                return None;
            }
            0xe1 => {
                self.extended = false;
                self.pause = 2;
                return None;
            }
            _ => {}
        }
        let extended = core::mem::replace(&mut self.extended, false);
        let code = byte & 0x7f;
        let usage = if extended {
            EXTENDED
                .iter()
                .find(|&&(c, _)| c == code)
                .map_or(0, |&(_, u)| u)
        } else {
            PLAIN.get(usize::from(code)).copied().unwrap_or(0)
        };
        (usage != 0).then_some(Event {
            key: Key(u16::from(usage)),
            pressed: byte & 0x80 == 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The events `bytes` give, fed one after another.
    fn events(bytes: &[u8]) -> Vec<Event> {
        let mut d = Decoder::new();
        bytes.iter().filter_map(|&b| d.feed(b)).collect()
    }

    fn down(key: Key) -> Event {
        Event { key, pressed: true }
    }

    fn up(key: Key) -> Event {
        Event {
            key,
            pressed: false,
        }
    }

    #[test]
    fn a_key_goes_down_and_comes_up() {
        assert_eq!(events(&[0x1e, 0x9e]), [down(Key::A), up(Key::A)]);
        assert_eq!(events(&[0x2c]), [down(Key::Z)]);
        assert_eq!(
            events(&[0x1c, 0x39, 0x01]),
            [down(Key::ENTER), down(Key::SPACE), down(Key::ESCAPE)]
        );
        assert_eq!(
            events(&[0x2a, 0x1e, 0x9e, 0xaa]),
            [
                down(Key::LEFT_SHIFT),
                down(Key::A),
                up(Key::A),
                up(Key::LEFT_SHIFT)
            ]
        );
    }

    #[test]
    fn the_keys_are_where_usb_puts_them() {
        let keys: Vec<u16> = events(&[0x02, 0x0b, 0x10, 0x19, 0x2b, 0x36, 0x3b, 0x44, 0x57, 0x58])
            .iter()
            .map(|e| e.key.0)
            .collect();
        assert_eq!(
            keys,
            [0x1e, 0x27, 0x14, 0x13, 0x31, 0xe5, 0x3a, 0x43, 0x44, 0x45],
            "1 0 q p \\ right-shift f1 f10 f11 f12"
        );
        assert_eq!(
            events(&[0x56]),
            [down(Key(0x64))],
            "the key left of z on keyboards like the German one"
        );
        assert_eq!(
            events(&[0x52, 0x53, 0x4e])
                .iter()
                .map(|e| e.key.0)
                .collect::<Vec<_>>(),
            [0x62, 0x63, 0x57],
            "the keypad"
        );
    }

    /// Every key of set 1 with its name and the usage USB gives it, as the
    /// two specifications list them, to check the table against.
    const KEYS: [(u8, &str, u16); 83] = [
        (0x01, "escape", 0x29),
        (0x02, "1", 0x1e),
        (0x03, "2", 0x1f),
        (0x04, "3", 0x20),
        (0x05, "4", 0x21),
        (0x06, "5", 0x22),
        (0x07, "6", 0x23),
        (0x08, "7", 0x24),
        (0x09, "8", 0x25),
        (0x0a, "9", 0x26),
        (0x0b, "0", 0x27),
        (0x0c, "-", 0x2d),
        (0x0d, "=", 0x2e),
        (0x0e, "backspace", 0x2a),
        (0x0f, "tab", 0x2b),
        (0x10, "q", 0x14),
        (0x11, "w", 0x1a),
        (0x12, "e", 0x08),
        (0x13, "r", 0x15),
        (0x14, "t", 0x17),
        (0x15, "y", 0x1c),
        (0x16, "u", 0x18),
        (0x17, "i", 0x0c),
        (0x18, "o", 0x12),
        (0x19, "p", 0x13),
        (0x1a, "[", 0x2f),
        (0x1b, "]", 0x30),
        (0x1c, "enter", 0x28),
        (0x1d, "left control", 0xe0),
        (0x1e, "a", 0x04),
        (0x1f, "s", 0x16),
        (0x20, "d", 0x07),
        (0x21, "f", 0x09),
        (0x22, "g", 0x0a),
        (0x23, "h", 0x0b),
        (0x24, "j", 0x0d),
        (0x25, "k", 0x0e),
        (0x26, "l", 0x0f),
        (0x27, ";", 0x33),
        (0x28, "'", 0x34),
        (0x29, "`", 0x35),
        (0x2a, "left shift", 0xe1),
        (0x2b, "\\", 0x31),
        (0x2c, "z", 0x1d),
        (0x2d, "x", 0x1b),
        (0x2e, "c", 0x06),
        (0x2f, "v", 0x19),
        (0x30, "b", 0x05),
        (0x31, "n", 0x11),
        (0x32, "m", 0x10),
        (0x33, ",", 0x36),
        (0x34, ".", 0x37),
        (0x35, "/", 0x38),
        (0x36, "right shift", 0xe5),
        (0x37, "keypad *", 0x55),
        (0x38, "left alt", 0xe2),
        (0x39, "space", 0x2c),
        (0x3a, "caps lock", 0x39),
        (0x3b, "f1", 0x3a),
        (0x3c, "f2", 0x3b),
        (0x3d, "f3", 0x3c),
        (0x3e, "f4", 0x3d),
        (0x3f, "f5", 0x3e),
        (0x40, "f6", 0x3f),
        (0x41, "f7", 0x40),
        (0x42, "f8", 0x41),
        (0x43, "f9", 0x42),
        (0x44, "f10", 0x43),
        (0x45, "num lock", 0x53),
        (0x46, "scroll lock", 0x47),
        (0x47, "keypad 7", 0x5f),
        (0x48, "keypad 8", 0x60),
        (0x49, "keypad 9", 0x61),
        (0x4a, "keypad -", 0x56),
        (0x4b, "keypad 4", 0x5c),
        (0x4c, "keypad 5", 0x5d),
        (0x4d, "keypad 6", 0x5e),
        (0x4e, "keypad +", 0x57),
        (0x4f, "keypad 1", 0x59),
        (0x50, "keypad 2", 0x5a),
        (0x51, "keypad 3", 0x5b),
        (0x52, "keypad 0", 0x62),
        (0x53, "keypad .", 0x63),
    ];

    #[test]
    fn every_plain_key_has_the_usage_usb_gives_it() {
        for (code, name, usage) in KEYS {
            assert_eq!(events(&[code]), [down(Key(usage))], "{name} going down");
            assert_eq!(events(&[code | 0x80]), [up(Key(usage))], "{name} coming up");
        }
        for (code, name, usage) in [
            (0x56, "the key left of z", 0x64),
            (0x57, "f11", 0x44),
            (0x58, "f12", 0x45),
        ] {
            assert_eq!(events(&[code]), [down(Key(usage))], "{name}");
        }
        for code in [0x54, 0x55, 0x59, 0x5a, 0x7f] {
            assert_eq!(events(&[code]), [], "no key has {code:#x}");
        }
    }

    #[test]
    fn every_extended_key_has_the_usage_usb_gives_it() {
        let keys = [
            (0x1c, "keypad enter", 0x58),
            (0x1d, "right control", 0xe4),
            (0x35, "keypad /", 0x54),
            (0x37, "print screen", 0x46),
            (0x38, "right alt", 0xe6),
            (0x47, "home", 0x4a),
            (0x48, "up", 0x52),
            (0x49, "page up", 0x4b),
            (0x4b, "left", 0x50),
            (0x4d, "right", 0x4f),
            (0x4f, "end", 0x4d),
            (0x50, "down", 0x51),
            (0x51, "page down", 0x4e),
            (0x52, "insert", 0x49),
            (0x53, "delete", 0x4c),
            (0x5b, "left windows key", 0xe3),
            (0x5c, "right windows key", 0xe7),
            (0x5d, "menu", 0x65),
        ];
        for (code, name, usage) in keys {
            assert_eq!(
                events(&[0xe0, code]),
                [down(Key(usage))],
                "{name} going down"
            );
            assert_eq!(
                events(&[0xe0, code | 0x80]),
                [up(Key(usage))],
                "{name} coming up"
            );
        }
    }

    #[test]
    fn an_extended_key_comes_after_its_prefix() {
        assert_eq!(
            events(&[0xe0, 0x48, 0xe0, 0xc8]),
            [down(Key::UP), up(Key::UP)]
        );
        assert_eq!(events(&[0xe0, 0x38]), [down(Key::RIGHT_ALT)], "AltGr");
        assert_eq!(events(&[0x48]), [down(Key(0x60))], "without it, keypad 8");
        assert_eq!(
            events(&[0xe0, 0x48, 0x48]),
            [down(Key::UP), down(Key(0x60))],
            "the prefix holds for one code"
        );
        assert_eq!(
            events(&[0xe0, 0x1c, 0xe0, 0x53, 0xe0, 0x5b])
                .iter()
                .map(|e| e.key.0)
                .collect::<Vec<_>>(),
            [0x58, 0x4c, 0xe3]
        );
    }

    #[test]
    fn the_shifts_around_extended_keys_are_passed_over() {
        assert_eq!(
            events(&[0xe0, 0x2a, 0xe0, 0x37, 0xe0, 0xb7, 0xe0, 0xaa]),
            [down(Key(0x46)), up(Key(0x46))],
            "print screen"
        );
        assert_eq!(events(&[0xe0, 0x36, 0xe0, 0xb6]), []);
    }

    #[test]
    fn pause_sends_its_six_bytes_at_once() {
        assert_eq!(
            events(&[0xe1, 0x1d, 0x45, 0xe1, 0x9d, 0xc5]),
            [down(Key::PAUSE), up(Key::PAUSE)]
        );
        assert_eq!(
            events(&[0xe1, 0x1d, 0x45, 0x1e]),
            [down(Key::PAUSE), down(Key::A)],
            "and then keys go on"
        );
        assert_eq!(
            events(&[0xe0, 0xe1, 0x1d, 0x45, 0x48]),
            [down(Key::PAUSE), down(Key(0x60))],
            "a prefix before it is dropped"
        );
    }

    #[test]
    fn bytes_that_are_no_key_give_nothing() {
        assert_eq!(events(&[0xfa, 0xfe, 0x00, 0xff, 0x55, 0x54, 0x80]), []);
        assert_eq!(events(&[0xe0, 0x10]), [], "an extended code no key has");
    }

    #[test]
    fn keys_wait_in_the_order_they_came() {
        let mut keys = Keys::<3>::new();
        assert!(keys.is_empty() && keys.pop().is_none());
        assert!(keys.push(down(Key::A)) && keys.push(up(Key::A)));
        assert_eq!(keys.len(), 2);
        assert_eq!(keys.pop(), Some(down(Key::A)));
        assert!(keys.push(down(Key::Z)) && keys.push(up(Key::Z)));
        assert!(!keys.push(down(Key::UP)), "a fourth finds no room");
        assert_eq!(keys.len(), 3);
        assert_eq!(
            [keys.pop(), keys.pop(), keys.pop(), keys.pop()],
            [Some(up(Key::A)), Some(down(Key::Z)), Some(up(Key::Z)), None],
            "around the end of the room and back"
        );
        assert!(keys.push(down(Key::ESCAPE)) && keys.pop() == Some(down(Key::ESCAPE)));
        assert!(Keys::<1>::default().is_empty());
    }

    #[test]
    fn the_keyboard_is_configured_on_the_first_port_alone() {
        assert_eq!(keyboard_config(0), 0b0110_0001);
        assert_eq!(
            keyboard_config(0xff),
            0b1110_1101,
            "the clock of the first port on, the second's interrupt off"
        );
        assert_eq!(
            keyboard_config(0b1000_0100),
            0b1110_0101,
            "the bits it does not know stay"
        );
    }

    #[test]
    fn the_ports_and_commands_are_the_i8042s() {
        assert_eq!([port::DATA, port::COMMAND], [0x60, 0x64]);
        assert_eq!([status::OUTPUT_FULL, status::INPUT_FULL], [1, 2]);
        assert_eq!(
            [
                command::READ_CONFIG,
                command::WRITE_CONFIG,
                command::SELF_TEST,
                command::DISABLE_FIRST,
                command::ENABLE_FIRST,
                command::DISABLE_SECOND
            ],
            [0x20, 0x60, 0xaa, 0xad, 0xae, 0xa7]
        );
        assert_eq!(TEST_PASSED, 0x55);
    }
}
