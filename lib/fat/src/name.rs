//! The names entries get, as fatgen103 makes them: a short name of eight
//! letters and three in capitals, and the long name in entries of 13
//! UTF-16 units each before it whenever the short one does not say it.

use crate::{UNITS, checksum};

/// The most UTF-16 units a long name holds.
pub const LONGEST: usize = 255;

/// Whether `name` may be a long name: something is left once its leading
/// periods and spaces go, it ends in neither, it fits [`LONGEST`] units,
/// and it holds no control character and none of `"*/:<>?\|`.
#[must_use]
pub fn valid(name: &str) -> bool {
    let forbidden = |c: char| c < ' ' || "\"*/:<>?\\|".contains(c);
    !name.trim_start_matches(['.', ' ']).is_empty()
        && !name.ends_with(['.', ' '])
        && name.encode_utf16().count() <= LONGEST
        && !name.chars().any(forbidden)
}

/// Whether `c` may stand in a short name as it is.
fn allowed(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || b"$%'-_@~`!(){}^#&".contains(&c)
}

/// The short name `name` gets in a directory where `taken` says which
/// short names are in use already, and whether its long name has to go
/// with it: none when it gets no number from 1 to 999999 that is free,
/// or is only periods and spaces.
///
/// # Errors
///
/// What `taken` answers when it cannot say, which ends the search.
pub fn short_for<E>(
    name: &str,
    mut taken: impl FnMut(&[u8; 11]) -> Result<bool, E>,
) -> Result<Option<([u8; 11], bool)>, E> {
    let trimmed = name.trim_start_matches(['.', ' ']);
    if trimmed.is_empty() {
        return Ok(None);
    }
    // The basis takes what comes before the first period, the extension
    // what comes after the last.
    let (first, last) = (trimmed.find('.'), trimmed.rfind('.'));
    let base = &trimmed[..first.unwrap_or(trimmed.len())];
    let ext = last.map_or("", |dot| &trimmed[dot + 1..]);
    let mut lossy = trimmed.len() != name.len() || first != last;
    let mut fold = |part: &str, room: &mut [u8]| {
        let mut len = 0;
        for c in part.chars().filter(|&c| c != ' ' && c != '.') {
            let upper = c.to_ascii_uppercase();
            let byte = u8::try_from(upper).ok().filter(|&b| allowed(b));
            lossy |= byte.is_none() || upper != c;
            if len < room.len() {
                room[len] = byte.unwrap_or(b'_');
            } else {
                lossy = true;
            }
            len += 1;
        }
        len.min(room.len())
    };
    let mut short = [b' '; 11];
    let base_len = fold(base, &mut short[..8]);
    fold(ext, &mut short[8..]);
    lossy |= base.contains(' ') || ext.contains(' ');
    if !lossy && !taken(&short)? {
        return Ok(Some((short, false)));
    }
    for n in 1u32..=999_999 {
        let mut tail = [0u8; 7];
        let digits = write_number(n, &mut tail);
        let keep = base_len.min(7 - digits.len());
        let mut candidate = short;
        candidate[keep] = b'~';
        candidate[keep + 1..keep + 1 + digits.len()].copy_from_slice(digits);
        candidate[keep + 1 + digits.len()..8].fill(b' ');
        if !taken(&candidate)? {
            return Ok(Some((candidate, true)));
        }
    }
    Ok(None)
}

/// `n` in decimal digits, written into `room`.
fn write_number(mut n: u32, room: &mut [u8; 7]) -> &[u8] {
    let mut at = room.len();
    loop {
        at -= 1;
        room[at] = b'0' + u8::try_from(n % 10).unwrap_or(0);
        n /= 10;
        if n == 0 {
            return &room[at..];
        }
    }
}

/// The entries of the long name `name` for the short entry `short`, in
/// the order they lie before it, written into `into`, and how many there
/// are; none for a name of more than [`LONGEST`] units or none at all.
pub fn long_entries(name: &str, short: &[u8; 11], into: &mut [[u8; 32]; 20]) -> Option<usize> {
    let units = name.encode_utf16().count();
    if units == 0 || units > LONGEST {
        return None;
    }
    let parts = units.div_ceil(13);
    let sum = checksum(short);
    let mut all = [0xffffu16; 20 * 13];
    for (room, unit) in all.iter_mut().zip(name.encode_utf16()) {
        *room = unit;
    }
    // A zero ends the name. When the name fills its last entry, the zero
    // lands past it, where no entry reads.
    all[units] = 0;
    for (slot, part) in into.iter_mut().zip((1..=parts).rev()) {
        let mut e = [0u8; 32];
        e[0] = u8::try_from(part).unwrap_or(0) | if part == parts { 0x40 } else { 0 };
        e[11] = 0x0f;
        e[13] = sum;
        for (i, &offset) in UNITS.iter().enumerate() {
            e[offset..offset + 2].copy_from_slice(&all[(part - 1) * 13 + i].to_le_bytes());
        }
        *slot = e;
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none_taken(_: &[u8; 11]) -> bool {
        false
    }

    /// The short name `name` gets where `taken` always knows.
    fn short(name: &str, taken: impl Fn(&[u8; 11]) -> bool) -> Option<([u8; 11], bool)> {
        short_for(name, |s| Ok::<_, ()>(taken(s))).unwrap()
    }

    #[test]
    fn a_name_that_cannot_be_looked_up_ends_the_search() {
        let mut asked = 0;
        let answer = short_for("x.tar.gz", |_| {
            asked += 1;
            if asked == 3 {
                Err("unreadable")
            } else {
                Ok(true)
            }
        });
        assert_eq!((answer, asked), (Err("unreadable"), 3));
    }

    #[test]
    fn long_names_may_hold_most_but_not_everything() {
        let fine = [
            "a",
            ".bashrc",
            "My File.txt",
            "a+b,c;d=e[f]",
            "\u{1f600}",
            "tab\u{7f}",
        ];
        for name in fine
            .iter()
            .map(ToString::to_string)
            .chain(["z".repeat(255)])
        {
            assert!(valid(&name), "{name}");
        }
        let bad = [
            "", ".", "..", "...", " . ", "a.", "a ", "a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b",
            "a<b", "a>b", "a|b", "a\u{1}b", "a\u{1f}",
        ];
        for name in bad.iter().map(ToString::to_string).chain(["z".repeat(256)]) {
            assert!(!valid(&name), "{name:?}");
        }
        assert!(valid(" lead"), "leading spaces go, as leading periods do");
    }

    #[test]
    fn a_name_of_only_periods_and_spaces_has_no_short_name() {
        for name in ["", "...", " . "] {
            assert_eq!(short(name, none_taken), None, "{name:?}");
        }
    }

    #[test]
    fn what_a_short_name_cannot_say_takes_a_number() {
        let cases = [
            (".bashrc", *b"BASHRC~1   "),
            ("A.B.C", *b"A~1     C  "),
            ("MY FILE.TXT", *b"MYFILE~1TXT"),
            ("FILE.T T", *b"FILE~1  TT "),
        ];
        for (name, wanted) in cases {
            assert_eq!(short(name, none_taken), Some((wanted, true)), "{name}");
        }
    }

    #[test]
    fn a_name_that_fits_eight_and_three_in_capitals_is_its_own_short_name() {
        assert_eq!(
            short("README.TXT", none_taken),
            Some((*b"README  TXT", false))
        );
        assert_eq!(short("KERNEL", none_taken), Some((*b"KERNEL     ", false)));
        assert_eq!(short("A1$_~.X", none_taken), Some((*b"A1$_~   X  ", false)));
    }

    #[test]
    fn other_names_get_a_number_and_their_long_name() {
        assert_eq!(
            short("readme.txt", none_taken),
            Some((*b"README~1TXT", true)),
            "small letters"
        );
        assert_eq!(
            short("services.meri", none_taken),
            Some((*b"SERVIC~1MER", true))
        );
        assert_eq!(
            short("a b+c.txt", none_taken),
            Some((*b"AB_C~1  TXT", true)),
            "a space left out, a plus not allowed"
        );
        assert_eq!(
            short(".profile", none_taken),
            Some((*b"PROFIL~1   ", true)),
            "leading dots go"
        );
        assert_eq!(
            short("x.tar.gz", none_taken),
            Some((*b"X~1     GZ ", true)),
            "the basis up to the first period, the extension after the last"
        );
        assert_eq!(
            short("\u{e4}pfel", none_taken),
            Some((*b"_PFEL~1    ", true))
        );
    }

    #[test]
    fn a_number_taken_already_makes_way_for_the_next() {
        let taken = |s: &[u8; 11]| s == b"SERVIC~1MER" || s == b"SERVIC~2MER";
        assert_eq!(short("services.meri", taken), Some((*b"SERVIC~3MER", true)));
        let own = |s: &[u8; 11]| s == b"README  TXT";
        assert_eq!(
            short("README.TXT", own),
            Some((*b"README~1TXT", true)),
            "a short name in use is no one's twice"
        );
        let ten = |s: &[u8; 11]| (1..=9).any(|n| s[..8] == *format!("SERVIC~{n}").as_bytes());
        assert_eq!(
            short("services.meri", ten),
            Some((*b"SERVI~10MER", true)),
            "more digits, fewer letters"
        );
        assert_eq!(short("x", |_| true), None, "every number taken");
    }

    #[test]
    fn numbers_are_written_in_decimal() {
        let mut room = [0u8; 7];
        assert_eq!(write_number(0, &mut room), b"0");
        assert_eq!(write_number(7, &mut room), b"7");
        assert_eq!(write_number(999_999, &mut room), b"999999");
    }

    #[test]
    fn a_long_name_goes_into_entries_last_part_first() {
        let mut into = [[0u8; 32]; 20];
        let short = *b"SERVIC~1MER";
        assert_eq!(long_entries("services.meri", &short, &mut into), Some(1));
        let e = into[0];
        assert_eq!((e[0], e[11], e[13]), (0x41, 0x0f, checksum(&short)));
        let units: Vec<u16> = UNITS
            .iter()
            .map(|&at| u16::from_le_bytes([e[at], e[at + 1]]))
            .collect();
        let mut want: Vec<u16> = "services.meri".encode_utf16().collect();
        assert_eq!(units, want, "thirteen units fill it, with no end mark");
        assert_eq!(long_entries("services.meri.x", &short, &mut into), Some(2));
        assert_eq!((into[0][0], into[1][0]), (0x42, 0x01));
        want = "services.meri.x".encode_utf16().collect();
        want.push(0);
        let second: Vec<u16> = UNITS
            .iter()
            .map(|&at| u16::from_le_bytes([into[0][at], into[0][at + 1]]))
            .collect();
        assert_eq!(second[..3], want[13..], "the rest, ended by 0");
        assert!(
            second[3..].iter().all(|&u| u == 0xffff),
            "and the room after it full of ones"
        );
    }

    #[test]
    fn a_long_name_is_one_of_255_units_at_most() {
        let mut into = [[0u8; 32]; 20];
        let short = *b"X~1        ";
        assert_eq!(long_entries(&"x".repeat(255), &short, &mut into), Some(20));
        assert_eq!(into[0][0], 0x40 | 0x14, "the last part, number 20");
        assert_eq!(long_entries(&"x".repeat(256), &short, &mut into), None);
        assert_eq!(long_entries("", &short, &mut into), None);
        assert_eq!(LONGEST, 255);
    }
}
