//! What init says about its services. Each service init starts gets a
//! line, a channel to init, as the handle it starts with, and asks over it
//! for the service at a place in init's list: init answers with that
//! service's name, what it is doing and how often it started, or that no
//! service is there. One question, one answer, so the channel never holds
//! more than one message each way.

#![cfg_attr(not(test), no_std)]

/// The longest message: what a channel carries.
pub const MESSAGE: usize = abi::MESSAGE_BYTES;

const ASK: u8 = b'?';
const SERVICE: u8 = b's';
const NONE: u8 = b'.';

/// The bytes before a service's name in an answer: what it is, its state
/// and how often it started.
const HEAD: usize = 6;

/// What a service is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It waits for the services it names in `after`.
    Waiting,
    /// Its program runs.
    Running,
    /// Its program ended, and it waits before it starts it again.
    Resting,
    /// It ran once and will not run again, or init ends its services.
    Ended,
}

impl State {
    fn byte(self) -> u8 {
        match self {
            State::Waiting => b'w',
            State::Running => b'r',
            State::Resting => b'p',
            State::Ended => b'e',
        }
    }

    fn of(byte: u8) -> Option<State> {
        match byte {
            b'w' => Some(State::Waiting),
            b'r' => Some(State::Running),
            b'p' => Some(State::Resting),
            b'e' => Some(State::Ended),
            _ => None,
        }
    }

    /// The word the shell shows for it.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Running => "running",
            State::Resting => "resting",
            State::Ended => "ended",
        }
    }
}

/// A service as init describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct About<'a> {
    pub name: &'a str,
    pub state: State,
    pub starts: u32,
}

/// The question for the service at `place`.
#[must_use]
pub fn ask(place: u8) -> [u8; 2] {
    [ASK, place]
}

/// The place `message` asks about, when it is a question.
#[must_use]
pub fn asked(message: &[u8]) -> Option<u8> {
    match *message {
        [ASK, place] => Some(place),
        _ => None,
    }
}

/// The answer that tells `about` into `room`, or that no service is at the
/// place asked, and its length. A name too long for a message is cut at
/// the last whole character that fits.
pub fn answer(about: Option<About<'_>>, room: &mut [u8; MESSAGE]) -> usize {
    let Some(about) = about else {
        room[0] = NONE;
        return 1;
    };
    room[0] = SERVICE;
    room[1] = about.state.byte();
    room[2..HEAD].copy_from_slice(&about.starts.to_le_bytes());
    let mut fits = about.name.len().min(MESSAGE - HEAD);
    while !about.name.is_char_boundary(fits) {
        fits -= 1;
    }
    room[HEAD..HEAD + fits].copy_from_slice(&about.name.as_bytes()[..fits]);
    HEAD + fits
}

/// A message that is no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoAnswer;

/// What an answer says: a service, or none at the place asked. An error
/// for anything else.
///
/// # Errors
///
/// When `message` is no answer: empty, of another kind, too short, with a
/// state no service has, or a name that is no text.
pub fn read(message: &[u8]) -> Result<Option<About<'_>>, NoAnswer> {
    match message {
        [NONE] => Ok(None),
        [SERVICE, state, a, b, c, d, name @ ..] => Ok(Some(About {
            name: core::str::from_utf8(name).map_err(|_| NoAnswer)?,
            state: State::of(*state).ok_or(NoAnswer)?,
            starts: u32::from_le_bytes([*a, *b, *c, *d]),
        })),
        _ => Err(NoAnswer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(about: Option<About<'_>>) -> (Vec<u8>, Option<About<'static>>) {
        let mut room = [0u8; MESSAGE];
        let len = answer(about, &mut room);
        let bytes = room[..len].to_vec();
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        (bytes, read(leaked).unwrap())
    }

    #[test]
    fn a_question_names_a_place() {
        assert_eq!(ask(3), [b'?', 3]);
        assert_eq!(asked(&ask(0)), Some(0));
        assert_eq!(asked(&ask(255)), Some(255));
        assert_eq!(asked(b"?"), None, "no place");
        assert_eq!(asked(b"?12"), None, "one place");
        assert_eq!(asked(b"s1"), None, "an answer is no question");
        assert_eq!(asked(&[]), None);
    }

    #[test]
    fn an_answer_tells_a_service() {
        let whoami = About {
            name: "whoami",
            state: State::Resting,
            starts: 0x0102_0304,
        };
        let (bytes, back) = round(Some(whoami));
        assert_eq!(bytes, b"sp\x04\x03\x02\x01whoami");
        assert_eq!(back, Some(whoami));
        for state in [State::Waiting, State::Running, State::Resting, State::Ended] {
            let about = About {
                name: "",
                state,
                starts: 0,
            };
            assert_eq!(round(Some(about)).1, Some(about), "{state:?}");
        }
    }

    #[test]
    fn no_service_past_the_end() {
        assert_eq!(round(None), (b".".to_vec(), None));
    }

    #[test]
    fn a_long_name_is_cut_where_a_character_ends() {
        let long = "a".repeat(200);
        let (bytes, back) = round(Some(About {
            name: &long,
            state: State::Running,
            starts: 1,
        }));
        assert_eq!(bytes.len(), MESSAGE);
        assert_eq!(back.unwrap().name, &long[..MESSAGE - HEAD]);
        let umlauts = "\u{e4}".repeat(100);
        let (bytes, back) = round(Some(About {
            name: &umlauts,
            state: State::Running,
            starts: 1,
        }));
        assert_eq!(
            bytes.len(),
            MESSAGE,
            "61 of the two-byte characters fit exactly"
        );
        assert_eq!(back.unwrap().name.chars().count(), (MESSAGE - HEAD) / 2);
        let odd = format!("b{umlauts}");
        let (bytes, back) = round(Some(About {
            name: &odd,
            state: State::Running,
            starts: 1,
        }));
        assert_eq!(
            bytes.len(),
            MESSAGE - 1,
            "the character that would be cut stays out"
        );
        assert_eq!(
            back.unwrap().name.chars().count(),
            1 + (MESSAGE - HEAD - 1) / 2
        );
    }

    #[test]
    fn anything_else_is_no_answer() {
        assert_eq!(read(&[]), Err(NoAnswer));
        assert_eq!(read(b".."), Err(NoAnswer), "none says nothing more");
        assert_eq!(read(b"sr\0\0\0"), Err(NoAnswer), "too short for the count");
        assert_eq!(read(b"sx\0\0\0\0name"), Err(NoAnswer), "no such state");
        assert_eq!(read(b"sr\0\0\0\0\xff"), Err(NoAnswer), "no text");
        assert_eq!(read(b"?\x01"), Err(NoAnswer), "a question");
        assert_eq!(
            read(b"se\x01\0\0\0"),
            Ok(Some(About {
                name: "",
                state: State::Ended,
                starts: 1
            }))
        );
    }

    #[test]
    fn a_name_that_fills_the_message_is_kept_whole() {
        let name = "n".repeat(MESSAGE - HEAD);
        let (bytes, back) = round(Some(About {
            name: &name,
            state: State::Waiting,
            starts: u32::MAX,
        }));
        assert_eq!(bytes.len(), MESSAGE);
        let back = back.unwrap();
        assert_eq!((back.name, back.starts), (name.as_str(), u32::MAX));
    }

    #[test]
    fn every_place_can_be_asked_about() {
        for place in 0..=u8::MAX {
            assert_eq!(asked(&ask(place)), Some(place));
        }
    }

    #[test]
    fn each_state_has_a_byte_of_its_own() {
        let states = [State::Waiting, State::Running, State::Resting, State::Ended];
        for state in states {
            assert_eq!(State::of(state.byte()), Some(state));
        }
        let bytes = states.map(State::byte);
        assert_eq!(bytes, *b"wrpe");
        assert_eq!(State::of(b's'), None);
        assert_eq!(State::of(0), None);
    }

    #[test]
    fn each_state_has_its_word() {
        let words = [State::Waiting, State::Running, State::Resting, State::Ended].map(State::word);
        assert_eq!(words, ["waiting", "running", "resting", "ended"]);
        assert_eq!(MESSAGE, 128);
    }
}
