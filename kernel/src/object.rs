//! What a handle stands for, as the number a handle table keeps: the kind
//! of object in the top byte, and below it where to find the object.

use channel::End;

/// The kinds of kernel objects a handle can stand for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    /// One end of the channel in slot `slot`.
    ChannelEnd { slot: usize, end: End },
    /// The process in slot `slot`.
    Process { slot: usize },
}

/// The top byte of each kind's number.
const CHANNEL_END: u64 = 1;
const PROCESS: u64 = 2;

impl Object {
    /// The number a handle table keeps for the object.
    #[must_use]
    pub fn number(self) -> u64 {
        match self {
            Object::ChannelEnd { slot, end } => {
                let side = match end {
                    End::A => 0,
                    End::B => 1,
                };
                CHANNEL_END << 56 | (slot as u64) << 1 | side
            }
            Object::Process { slot } => PROCESS << 56 | slot as u64,
        }
    }

    /// The object `number` stands for, or `None` for a number no object
    /// has.
    #[must_use]
    pub fn from_number(number: u64) -> Option<Object> {
        match number >> 56 {
            CHANNEL_END => {
                let slot = usize::try_from(number >> 1 & 0x7f_ffff_ffff_ffff).ok()?;
                let end = if number & 1 == 0 { End::A } else { End::B };
                Some(Object::ChannelEnd { slot, end })
            }
            PROCESS => Some(Object::Process {
                slot: usize::try_from(number & 0xff_ffff_ffff_ffff).ok()?,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_ends_keep_their_slot_and_side() {
        for slot in [0, 1, 255, 0x12_3456, (1 << 55) - 1] {
            for end in [End::A, End::B] {
                let o = Object::ChannelEnd { slot, end };
                assert_eq!(Object::from_number(o.number()), Some(o), "{slot}, {end:?}");
                assert_eq!(o.number() >> 56, 1, "the kind sits in the top byte");
            }
        }
        assert_eq!(
            Object::ChannelEnd {
                slot: 3,
                end: End::B
            }
            .number(),
            1 << 56 | 7
        );
        assert_eq!(
            Object::ChannelEnd {
                slot: 0,
                end: End::A
            }
            .number(),
            1 << 56
        );
    }

    #[test]
    fn processes_keep_their_slot() {
        for slot in [0, 1, 255, (1 << 56) - 1] {
            let o = Object::Process { slot };
            assert_eq!(Object::from_number(o.number()), Some(o), "{slot}");
            assert_eq!(o.number() >> 56, 2);
        }
        assert_eq!(Object::Process { slot: 5 }.number(), 2 << 56 | 5);
        assert_ne!(
            Object::Process { slot: 3 }.number(),
            Object::ChannelEnd {
                slot: 1,
                end: End::B
            }
            .number(),
            "kinds never share a number"
        );
    }

    #[test]
    fn numbers_of_no_kind_stand_for_nothing() {
        for number in [0, 7, 3 << 56, u64::MAX, 0xff << 56 | 1] {
            assert_eq!(Object::from_number(number), None, "{number:#x}");
        }
    }
}
