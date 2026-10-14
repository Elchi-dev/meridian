//! The boot menu's state: which system is chosen, and the countdown that
//! starts it when nobody presses a key.

/// A key the menu reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Enter,
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
}

/// The menu over `count` systems. Time is counted in tenths of a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Menu {
    count: usize,
    selected: usize,
    /// Tenths of a second until the chosen system starts, or `None` once
    /// a key has stopped the countdown.
    left: Option<u32>,
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
        }
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

    pub fn key(&mut self, key: Key) -> Action {
        let counting = self.left.take().is_some();
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
            Key::Other if counting => Action::Redraw,
            Key::Other => Action::Wait,
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
    }

    #[test]
    fn huge_countdowns_do_not_overflow() {
        let m = Menu::new(1, 0, u32::MAX);
        assert_eq!(m.seconds_left(), Some(u32::MAX / 10 + 1));
    }
}
