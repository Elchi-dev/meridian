//! The boot menu's state: which system is chosen, and the countdown that
//! starts it when nobody presses a key.

/// A key the menu reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Enter,
    /// V, for the log.
    Log,
    /// F, for the firmware's settings.
    FirmwareSettings,
    /// Any other key, which only stops the countdown.
    Other,
}

impl Key {
    /// The menu's key for a firmware key stroke.
    #[must_use]
    pub fn from_uefi(key: uefi::text::InputKey) -> Key {
        use uefi::text::scan;
        match (key.scan_code, key.unicode_char) {
            (scan::LEFT, _) => Key::Left,
            (scan::RIGHT, _) => Key::Right,
            (_, 0x0d) => Key::Enter,
            (_, 0x56 | 0x76) => Key::Log,
            (_, 0x46 | 0x66) => Key::FirmwareSettings,
            _ => Key::Other,
        }
    }
}

/// What the loader does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing changed on the screen.
    Wait,
    /// Draw the menu again.
    Redraw,
    /// Start the system with this index.
    Start(usize),
    /// Show the log until the next key.
    ShowLog,
    /// Restart into the firmware's settings.
    OpenFirmwareSettings,
}

/// The menu over `count` systems. Time is counted in tenths of a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Menu {
    count: usize,
    selected: usize,
    /// Tenths of a second until the chosen system starts, or `None` once
    /// a key has stopped the countdown.
    left: Option<u32>,
    /// Whether the firmware can be asked to open its settings.
    firmware_settings: bool,
    /// Whether the log is on the screen instead of the cards.
    log_open: bool,
}

impl Menu {
    /// A menu with `default` chosen that starts it after `seconds`, or
    /// waits for a key when `seconds` is 0.
    #[must_use]
    pub fn new(count: usize, default: usize, seconds: u32) -> Menu {
        Menu {
            count,
            selected: if default < count { default } else { 0 },
            left: (seconds > 0).then_some(seconds.saturating_mul(10)),
            firmware_settings: false,
            log_open: false,
        }
    }

    /// The same menu, with F opening the firmware's settings when `can`.
    #[must_use]
    pub fn with_firmware_settings(self, can: bool) -> Menu {
        Menu {
            firmware_settings: can,
            ..self
        }
    }

    #[must_use]
    pub fn firmware_settings(&self) -> bool {
        self.firmware_settings
    }

    #[must_use]
    pub fn log_open(&self) -> bool {
        self.log_open
    }

    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whole seconds until the start, rounded up, while counting down.
    #[must_use]
    pub fn seconds_left(&self) -> Option<u32> {
        self.left.map(|tenths| tenths.div_ceil(10))
    }

    /// What a key does. While the log is open, any key closes it.
    pub fn key(&mut self, key: Key) -> Action {
        if self.log_open {
            self.log_open = false;
            return Action::Redraw;
        }
        let counting = self.left.take().is_some();
        match key {
            Key::Log => {
                self.log_open = true;
                return Action::ShowLog;
            }
            Key::FirmwareSettings if self.firmware_settings => {
                return Action::OpenFirmwareSettings;
            }
            _ => {}
        }
        if self.count == 0 {
            return Action::Wait;
        }
        match key {
            Key::Enter => Action::Start(self.selected),
            Key::Left => {
                self.selected = (self.selected + self.count - 1) % self.count;
                Action::Redraw
            }
            Key::Right => {
                self.selected = (self.selected + 1) % self.count;
                Action::Redraw
            }
            _ if counting => Action::Redraw,
            _ => Action::Wait,
        }
    }

    /// Lets `tenths` tenths of a second pass.
    pub fn tick(&mut self, tenths: u32) -> Action {
        let Some(left) = self.left else {
            return Action::Wait;
        };
        if self.count == 0 {
            return Action::Wait;
        }
        let now = left.saturating_sub(tenths);
        self.left = Some(now);
        if now == 0 {
            self.left = None;
            Action::Start(self.selected)
        } else if now.div_ceil(10) != left.div_ceil(10) {
            Action::Redraw
        } else {
            Action::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_starts_when_the_time_is_up() {
        let mut m = Menu::new(3, 1, 2);
        assert_eq!((m.selected(), m.seconds_left()), (1, Some(2)));
        assert_eq!(m.tick(3), Action::Wait, "still 2 seconds, rounded up");
        assert_eq!(m.tick(7), Action::Redraw, "now 1 second");
        assert_eq!(m.seconds_left(), Some(1));
        assert_eq!(m.tick(9), Action::Wait);
        assert_eq!(m.tick(1), Action::Start(1));
        assert_eq!(m.seconds_left(), None);
        assert_eq!(m.tick(1), Action::Wait, "starting happens once");
    }

    #[test]
    fn a_long_tick_starts_at_once() {
        let mut m = Menu::new(2, 0, 5);
        assert_eq!(m.tick(1000), Action::Start(0));
    }

    #[test]
    fn arrows_choose_and_wrap_around() {
        let mut m = Menu::new(3, 0, 5);
        assert_eq!(m.key(Key::Left), Action::Redraw);
        assert_eq!(m.selected(), 2, "left of the first is the last");
        assert_eq!(m.key(Key::Right), Action::Redraw);
        assert_eq!(m.selected(), 0, "right of the last is the first");
        assert_eq!(m.key(Key::Right), Action::Redraw);
        assert_eq!(m.key(Key::Enter), Action::Start(1));
    }

    #[test]
    fn any_key_stops_the_countdown() {
        let mut m = Menu::new(2, 1, 3);
        assert_eq!(
            m.key(Key::Other),
            Action::Redraw,
            "the countdown text goes away"
        );
        assert_eq!(m.seconds_left(), None);
        assert_eq!(m.tick(100), Action::Wait, "and nothing starts by itself");
        assert_eq!(
            m.key(Key::Other),
            Action::Wait,
            "a second press changes nothing"
        );
        assert_eq!(m.selected(), 1);
        let mut moved = Menu::new(2, 0, 3);
        moved.key(Key::Right);
        assert_eq!(moved.tick(100), Action::Wait, "choosing stops it too");
    }

    #[test]
    fn no_countdown_waits_for_a_key() {
        let mut m = Menu::new(2, 0, 0);
        assert_eq!(m.seconds_left(), None);
        assert_eq!(m.tick(1000), Action::Wait);
        assert_eq!(m.key(Key::Enter), Action::Start(0));
    }

    #[test]
    fn one_card_stays_chosen_whatever_the_arrows_say() {
        let mut m = Menu::new(1, 0, 0);
        assert_eq!(m.key(Key::Left), Action::Redraw);
        assert_eq!(m.key(Key::Right), Action::Redraw);
        assert_eq!(m.selected(), 0);
        assert_eq!(m.key(Key::Enter), Action::Start(0));
    }

    #[test]
    fn a_default_past_the_end_falls_back_to_the_first() {
        assert_eq!(Menu::new(2, 5, 1).selected(), 0);
        assert_eq!(Menu::new(2, 1, 1).selected(), 1);
    }

    #[test]
    fn an_empty_menu_never_starts_anything() {
        let mut m = Menu::new(0, 0, 1);
        assert_eq!(m.key(Key::Enter), Action::Wait);
        assert_eq!(m.key(Key::Right), Action::Wait);
        let mut counting = Menu::new(0, 0, 1);
        assert_eq!(counting.tick(100), Action::Wait);
    }

    #[test]
    fn firmware_keys_become_menu_keys() {
        use uefi::text::{InputKey, scan};
        let key = |scan_code, unicode_char| {
            Key::from_uefi(InputKey {
                scan_code,
                unicode_char,
            })
        };
        assert_eq!(key(scan::LEFT, 0), Key::Left);
        assert_eq!(key(scan::RIGHT, 0), Key::Right);
        assert_eq!(key(scan::NULL, 0x0d), Key::Enter);
        assert_eq!(key(scan::NULL, u16::from(b'a')), Key::Other);
        assert_eq!(
            key(scan::NULL, 0x0a),
            Key::Other,
            "a line feed is not Enter"
        );
        assert_eq!(key(scan::UP, 0), Key::Other);
        assert_eq!(key(scan::ESCAPE, 0), Key::Other);
        assert_eq!(key(scan::NULL, u16::from(b'v')), Key::Log);
        assert_eq!(key(scan::NULL, u16::from(b'V')), Key::Log);
        assert_eq!(key(scan::NULL, u16::from(b'f')), Key::FirmwareSettings);
        assert_eq!(key(scan::NULL, u16::from(b'F')), Key::FirmwareSettings);
        assert_eq!(key(scan::NULL, u16::from(b'w')), Key::Other);
        assert_eq!(key(scan::NULL, u16::from(b'g')), Key::Other);
    }

    #[test]
    fn v_shows_the_log_and_stops_the_countdown() {
        let mut m = Menu::new(2, 1, 3);
        assert_eq!(m.key(Key::Log), Action::ShowLog);
        assert_eq!(m.seconds_left(), None);
        assert_eq!(m.selected(), 1, "the choice stays");
        assert_eq!(m.tick(100), Action::Wait);
        assert_eq!(
            Menu::new(0, 0, 0).key(Key::Log),
            Action::ShowLog,
            "even with nothing to choose"
        );
    }

    #[test]
    fn any_key_closes_the_log_and_does_nothing_else() {
        for key in [
            Key::Enter,
            Key::Right,
            Key::Left,
            Key::Log,
            Key::FirmwareSettings,
            Key::Other,
        ] {
            let mut m = Menu::new(3, 1, 5).with_firmware_settings(true);
            assert_eq!(m.key(Key::Log), Action::ShowLog);
            assert!(m.log_open());
            assert_eq!(m.key(key), Action::Redraw, "{key:?} closes the log");
            assert!(!m.log_open());
            assert_eq!(
                m.selected(),
                1,
                "{key:?} chose nothing while closing the log"
            );
            assert_eq!(m.seconds_left(), None, "the countdown stays stopped");
        }
        let mut m = Menu::new(3, 1, 5);
        m.key(Key::Log);
        m.key(Key::Other);
        assert_eq!(m.key(Key::Right), Action::Redraw);
        assert_eq!(m.selected(), 2, "once closed, keys work again");
        assert_eq!(m.key(Key::Enter), Action::Start(2));
    }

    #[test]
    fn the_log_starts_closed_and_time_does_not_open_it() {
        let mut m = Menu::new(2, 0, 1);
        assert!(!m.log_open());
        assert_eq!(m.tick(5), Action::Wait);
        assert!(!m.log_open());
        m.key(Key::Log);
        assert_eq!(
            m.tick(100),
            Action::Wait,
            "no countdown while the log is open"
        );
        assert!(m.log_open());
    }

    #[test]
    fn f_opens_the_firmware_settings_only_where_it_can() {
        let mut cannot = Menu::new(2, 0, 3);
        assert!(!cannot.firmware_settings());
        assert_eq!(
            cannot.key(Key::FirmwareSettings),
            Action::Redraw,
            "like any other key"
        );
        assert_eq!(cannot.key(Key::FirmwareSettings), Action::Wait);
        let mut can = Menu::new(2, 0, 3).with_firmware_settings(true);
        assert!(can.firmware_settings());
        assert!(!can.with_firmware_settings(false).firmware_settings());
        assert_eq!(can.key(Key::FirmwareSettings), Action::OpenFirmwareSettings);
        assert_eq!(can.seconds_left(), None);
        assert_eq!(can.selected(), 0);
        assert_eq!(
            Menu::new(2, 1, 3).with_firmware_settings(true),
            Menu {
                firmware_settings: true,
                ..Menu::new(2, 1, 3)
            }
        );
    }

    #[test]
    fn huge_countdowns_do_not_overflow() {
        let m = Menu::new(1, 0, u32::MAX);
        assert_eq!(m.seconds_left(), Some(u32::MAX / 10 + 1));
    }
}
