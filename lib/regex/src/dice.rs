//! Dice for tests that check the engines against each other on many
//! expressions: the same rolls every run, so a failure comes back.

use alloc::format;
use alloc::string::String;

pub struct Dice(pub u64);

impl Dice {
    pub fn roll(&mut self, sides: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % 1024).unwrap() % sides
    }
}

/// An extended expression over a and b, `depth` deep at most.
pub fn pattern(dice: &mut Dice, depth: u32) -> String {
    const LEAVES: [&str; 8] = ["a", "b", ".", "[ab]", "[^a]", "^", "$", "()"];
    if depth == 0 || dice.roll(4) == 0 {
        return String::from(LEAVES[dice.roll(LEAVES.len())]);
    }
    let inside = pattern(dice, depth - 1);
    match dice.roll(8) {
        0 => format!("({inside})*"),
        1 => format!("({inside})+"),
        2 => format!("({inside})?"),
        3 => {
            let least = dice.roll(3);
            format!("({inside}){{{least},{}}}", least + dice.roll(3))
        }
        4 => format!("({inside}|{})", pattern(dice, depth - 1)),
        _ => format!("{inside}{}", pattern(dice, depth - 1)),
    }
}

/// A text of up to 7 characters, mostly a and b.
pub fn text(dice: &mut Dice) -> String {
    (0..dice.roll(8))
        .map(|_| ['a', 'b', 'a', 'b', 'c'][dice.roll(5)])
        .collect()
}
