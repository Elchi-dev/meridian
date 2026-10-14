//! Text for the firmware console, which takes UCS-2 and wants `\r\n`.

/// `EFI_INPUT_KEY`: a scan code for keys without a character, or a UCS-2
/// character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

/// Scan codes from UEFI 2.10, table 12.4.
pub mod scan {
    pub const NULL: u16 = 0x00;
    pub const UP: u16 = 0x01;
    pub const DOWN: u16 = 0x02;
    pub const RIGHT: u16 = 0x03;
    pub const LEFT: u16 = 0x04;
    pub const HOME: u16 = 0x05;
    pub const END: u16 = 0x06;
    pub const DELETE: u16 = 0x08;
    pub const ESCAPE: u16 = 0x17;
}

const REPLACEMENT: u16 = 0xfffd;

/// Encodes a string piece by piece into NUL-terminated UCS-2 buffers for
/// `OutputString`. Every `\n` becomes `\r\n`; characters outside the basic
/// plane and NUL, which would end the string early, become U+FFFD.
pub struct Ucs2<'a> {
    chars: core::str::Chars<'a>,
    pending: Option<u16>,
}

impl<'a> Ucs2<'a> {
    #[must_use]
    pub fn new(s: &'a str) -> Self {
        Ucs2 {
            chars: s.chars(),
            pending: None,
        }
    }

    /// Fills `buf` with the next piece and a NUL. Returns how many units
    /// were written before the NUL; 0 means the string is done. A buffer of
    /// one unit only gets the NUL, an empty one is left alone.
    pub fn fill(&mut self, buf: &mut [u16]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        let mut n = 0;
        while n + 1 < buf.len() {
            let unit = match self.pending.take() {
                Some(u) => u,
                None => match self.chars.next() {
                    None => break,
                    Some('\n') => {
                        self.pending = Some(u16::from(b'\n'));
                        u16::from(b'\r')
                    }
                    Some('\0') => REPLACEMENT,
                    Some(c) => u16::try_from(u32::from(c)).unwrap_or(REPLACEMENT),
                },
            };
            buf[n] = unit;
            n += 1;
        }
        buf[n] = 0;
        n
    }
}

/// Decodes a UCS-2 string up to its NUL, as firmware strings such as the
/// vendor name are stored. Unpaired surrogates become U+FFFD.
pub fn decode_ucs2(units: &[u16]) -> impl Iterator<Item = char> + '_ {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    char::decode_utf16(units[..end].iter().copied())
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
}

/// Encodes a path for `EFI_FILE_PROTOCOL.Open`: UCS-2, NUL-terminated,
/// with `/` turned into the `\` UEFI separates directories with. Returns
/// how many units come before the NUL, or `None` when the path does not
/// fit or holds a character a UEFI file name cannot.
pub fn file_path(path: &str, buf: &mut [u16]) -> Option<usize> {
    let mut n = 0;
    for c in path.chars() {
        let unit = match c {
            '/' => u16::from(b'\\'),
            '\0' => return None,
            c => u16::try_from(u32::from(c)).ok()?,
        };
        if n + 1 >= buf.len() {
            return None;
        }
        buf[n] = unit;
        n += 1;
    }
    *buf.get_mut(n)? = 0;
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str, room: usize) -> Option<Vec<u16>> {
        let mut buf = vec![0xffff; room];
        let n = file_path(p, &mut buf)?;
        assert_eq!(buf[n], 0, "the path ends in NUL");
        Some(buf[..n].to_vec())
    }

    #[test]
    fn paths_use_backslashes() {
        let want: Vec<u16> = "\\meridian\\kernel".encode_utf16().collect();
        assert_eq!(path("/meridian/kernel", 64), Some(want.clone()));
        assert_eq!(path("\\meridian\\kernel", 64), Some(want));
        assert_eq!(path("caf\u{e9}", 8), Some(vec![0x63, 0x61, 0x66, 0xe9]));
        assert_eq!(path("", 1), Some(vec![]));
    }

    #[test]
    fn paths_must_fit_with_their_terminator() {
        assert_eq!(path("abcd", 5).map(|p| p.len()), Some(4));
        assert_eq!(path("abcd", 4), None);
        assert_eq!(path("", 0), None);
        assert_eq!(path("a", 1), None);
    }

    #[test]
    fn paths_refuse_what_ucs2_names_cannot_hold() {
        assert_eq!(path("a\0b", 8), None);
        assert_eq!(path("emoji\u{1F600}", 16), None);
        assert_eq!(path("\u{ffff}", 4), Some(vec![0xffff]));
    }

    fn pieces(s: &str, size: usize) -> Vec<Vec<u16>> {
        let mut enc = Ucs2::new(s);
        let mut out = Vec::new();
        loop {
            let mut buf = vec![0xaaaa; size];
            let n = enc.fill(&mut buf);
            if n == 0 {
                return out;
            }
            assert_eq!(buf[n], 0, "piece must end with NUL");
            out.push(buf[..n].to_vec());
        }
    }

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn plain_ascii_fits_in_one_piece() {
        assert_eq!(pieces("Meridian", 64), vec![utf16("Meridian")]);
    }

    #[test]
    fn newlines_become_crlf() {
        assert_eq!(pieces("a\nb\n", 64), vec![utf16("a\r\nb\r\n")]);
    }

    #[test]
    fn long_text_is_split_without_losing_anything() {
        let text = "0123456789".repeat(5);
        let got = pieces(&text, 8);
        assert!(got.iter().all(|p| p.len() == 7 || p == got.last().unwrap()));
        assert_eq!(got.concat(), utf16(&text));
        assert_eq!(got.len(), 8);
    }

    #[test]
    fn a_crlf_pair_may_straddle_two_pieces() {
        let got = pieces("abc\nd", 4);
        assert_eq!(got, vec![utf16("abc"), utf16("\r\nd")]);
        let got = pieces("ab\nd", 4);
        assert_eq!(got, vec![utf16("ab\r"), utf16("\nd")]);
    }

    #[test]
    fn non_bmp_and_nul_are_replaced() {
        assert_eq!(pieces("a\u{1F600}b", 16), vec![vec![0x61, 0xfffd, 0x62]]);
        assert_eq!(pieces("a\0b", 16), vec![vec![0x61, 0xfffd, 0x62]]);
        assert_eq!(pieces("\u{ffff}\u{e9}", 16), vec![vec![0xffff, 0xe9]]);
    }

    #[test]
    fn tiny_buffers() {
        let mut enc = Ucs2::new("ab");
        assert_eq!(enc.fill(&mut []), 0);
        let mut one = [7u16];
        assert_eq!(enc.fill(&mut one), 0);
        assert_eq!(one, [0]);
        let mut two = [9u16; 2];
        assert_eq!(enc.fill(&mut two), 1);
        assert_eq!(two, [0x61, 0]);
        assert_eq!(enc.fill(&mut two), 1);
        assert_eq!(two, [0x62, 0]);
        assert_eq!(enc.fill(&mut two), 0);
        assert_eq!(two, [0, 0]);
    }

    #[test]
    fn empty_string_writes_just_the_terminator() {
        let mut buf = [5u16; 4];
        assert_eq!(Ucs2::new("").fill(&mut buf), 0);
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn decodes_up_to_the_nul() {
        let mut units = utf16("EDK II");
        units.push(0);
        units.extend(utf16("garbage"));
        assert_eq!(decode_ucs2(&units).collect::<String>(), "EDK II");
        assert_eq!(decode_ucs2(&utf16("no nul")).collect::<String>(), "no nul");
        assert_eq!(
            decode_ucs2(&[0xd800, 0x41, 0]).collect::<String>(),
            "\u{fffd}A"
        );
        assert_eq!(decode_ucs2(&[]).count(), 0);
    }

    #[test]
    fn decodes_surrogate_pairs() {
        let units: Vec<u16> = "a\u{1F600}b".encode_utf16().chain([0]).collect();
        assert_eq!(units.len(), 5);
        assert_eq!(decode_ucs2(&units).collect::<String>(), "a\u{1F600}b");
        assert_eq!(decode_ucs2(&[0xdc00, 0]).collect::<String>(), "\u{fffd}");
    }

    #[test]
    fn every_piece_but_the_last_is_full() {
        let text = "x".repeat(300);
        let got = pieces(&text, 128);
        assert_eq!(got.iter().map(Vec::len).collect::<Vec<_>>(), [127, 127, 46]);
    }

    #[test]
    fn a_newline_exactly_at_the_end_of_a_piece() {
        let got = pieces("abc\n", 5);
        assert_eq!(got, vec![utf16("abc\r"), utf16("\n")]);
        let got = pieces("ab\n\n", 5);
        assert_eq!(got, vec![utf16("ab\r\n"), utf16("\r\n")]);
    }

    #[test]
    fn input_key_layout() {
        assert_eq!(core::mem::size_of::<InputKey>(), 4);
        assert_eq!(core::mem::offset_of!(InputKey, unicode_char), 2);
        assert_eq!(
            (scan::UP, scan::DOWN, scan::RIGHT, scan::LEFT, scan::ESCAPE),
            (1, 2, 3, 4, 0x17)
        );
        assert_eq!(
            (scan::NULL, scan::HOME, scan::END, scan::DELETE),
            (0, 5, 6, 8)
        );
    }
}
