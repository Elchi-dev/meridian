//! The LZW coding of POSIX's compress: a header of two magic bytes and a
//! byte with the most bits a code may have, then codes of 9 bits and up,
//! the low bits first. Each run of codes of one width fills whole groups
//! of eight, so a reader can skip to the next group when the width grows
//! or the table starts again. Both ways go a piece at a time, so a file
//! of any size takes little memory.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// The bytes a compressed file begins with.
pub const MAGIC: [u8; 2] = [0x1f, 0x9d];

/// The bit of the header's third byte that says the table starts again
/// at a [`CLEAR`].
const BLOCK: u8 = 0x80;

/// The code that starts the table again.
const CLEAR: u32 = 256;

/// The first code a string gets.
const FIRST: u32 = 257;

/// The fewest and the most bits a code may have.
pub const LEAST_BITS: u8 = 9;
pub const MOST_BITS: u8 = 16;

/// How many input bytes go by between looks at how well the coding
/// goes once the table is full.
const CHECK_GAP: u64 = 10_000;

/// The greatest code a width takes before the next grows: all its bits,
/// and for the most a table has, the first code past it, which no code
/// reaches. A table of 9 bits at most never gets that, so its codes grow
/// to 10 bits once it is full, as compress's and gzip's do.
fn limit(width: u8, most: u8) -> u32 {
    if width == most && width > LEAST_BITS {
        1 << most
    } else {
        (1 << width) - 1
    }
}

/// No code: the end of a trie's list of children.
const NONE: u16 = u16::MAX;

/// Codes packed into bytes, the low bits first, a group of eight codes at
/// a time.
#[derive(Default)]
struct Packer {
    out: Vec<u8>,
    bits: u32,
    held: u32,
    /// The codes of the width at hand so far.
    codes: u32,
}

impl Packer {
    fn put(&mut self, code: u32, width: u8) {
        self.bits |= code << self.held;
        self.held += u32::from(width);
        while self.held >= 8 {
            self.out.push(self.bits.to_le_bytes()[0]);
            self.bits >>= 8;
            self.held -= 8;
        }
        self.codes += 1;
    }

    /// Fills the group at hand with codes of nothing, as a new width or
    /// a new table begins.
    fn end_group(&mut self, width: u8) {
        while !self.codes.is_multiple_of(8) {
            self.put(0, width);
        }
        self.codes = 0;
    }

    /// The bits still held, in a last byte.
    fn finish(&mut self) {
        if self.held > 0 {
            self.out.push(self.bits.to_le_bytes()[0]);
        }
        self.bits = 0;
        self.held = 0;
    }
}

/// Compresses what it is given, a piece at a time.
pub struct Encoder {
    most: u8,
    width: u8,
    /// The greatest code of this width.
    limit: u32,
    /// The next code a string gets.
    next: u32,
    /// The table as a trie: each code's first child, next sibling, and
    /// the byte that leads to it.
    child: Vec<u16>,
    sibling: Vec<u16>,
    byte: Vec<u8>,
    /// The code of the string read so far that is in the table.
    string: Option<u32>,
    packer: Packer,
    read: u64,
    written: u64,
    /// When to look next at how well the coding goes, and how well it
    /// went then.
    look_at: u64,
    ratio: u64,
}

impl Encoder {
    /// An encoder whose codes have at most `most` bits, 9 to 16, and the
    /// header that begins what it writes.
    #[must_use]
    pub fn new(most: u8) -> Encoder {
        let most = most.clamp(LEAST_BITS, MOST_BITS);
        let size = 1usize << most;
        let mut encoder = Encoder {
            most,
            width: LEAST_BITS,
            limit: limit(LEAST_BITS, most),
            next: FIRST,
            child: vec![NONE; size],
            sibling: vec![NONE; size],
            byte: vec![0; size],
            string: None,
            packer: Packer::default(),
            read: 0,
            written: 3,
            look_at: CHECK_GAP,
            ratio: 0,
        };
        encoder.packer.out.extend_from_slice(&MAGIC);
        encoder.packer.out.push(BLOCK | most);
        encoder
    }

    /// The code of the string `prefix` and then `b`, if it is in the
    /// table.
    fn find(&self, prefix: u32, b: u8) -> Option<u32> {
        let mut at = self.child[prefix as usize];
        while at != NONE {
            if self.byte[usize::from(at)] == b {
                return Some(u32::from(at));
            }
            at = self.sibling[usize::from(at)];
        }
        None
    }

    fn put(&mut self, code: u32) {
        self.packer.put(code, self.width);
        // Past its greatest code the width grows, once the group is
        // whole.
        if self.next > self.limit {
            self.packer.end_group(self.width);
            self.width += 1;
            self.limit = limit(self.width, self.most);
        }
    }

    /// Codes `bytes`, and gives what is ready to write.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        for &b in bytes {
            self.read += 1;
            let Some(prefix) = self.string else {
                self.string = Some(u32::from(b));
                continue;
            };
            if let Some(code) = self.find(prefix, b) {
                self.string = Some(code);
                continue;
            }
            self.put(prefix);
            self.string = Some(u32::from(b));
            if self.next < 1 << self.most {
                // The new string goes first among the prefix's children.
                let code = self.next as usize;
                self.byte[code] = b;
                self.sibling[code] = self.child[prefix as usize];
                self.child[prefix as usize] = u16::try_from(self.next).unwrap_or(NONE);
                self.next += 1;
            } else if self.read >= self.look_at {
                self.look();
            }
        }
        self.take()
    }

    /// With the table full, starts it again once coding goes worse than
    /// it did, as compress does.
    fn look(&mut self) {
        self.look_at = self.read + CHECK_GAP;
        let written = self.written + self.packer.out.len() as u64;
        let ratio = (self.read << 8) / written.max(1);
        if ratio > self.ratio {
            self.ratio = ratio;
            return;
        }
        self.ratio = 0;
        self.packer.put(CLEAR, self.width);
        self.packer.end_group(self.width);
        self.width = LEAST_BITS;
        self.limit = limit(LEAST_BITS, self.most);
        self.next = FIRST;
        self.child.fill(NONE);
    }

    fn take(&mut self) -> Vec<u8> {
        let out = core::mem::take(&mut self.packer.out);
        self.written += out.len() as u64;
        out
    }

    /// The rest of what it writes, the last string's code with it.
    pub fn finish(&mut self) -> Vec<u8> {
        if let Some(prefix) = self.string.take() {
            self.packer.put(prefix, self.width);
        }
        self.packer.finish();
        self.take()
    }
}

/// Why compressed bytes cannot be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// They do not begin with [`MAGIC`].
    NotCompressed,
    /// The header asks for codes of more bits than [`MOST_BITS`].
    TooManyBits(u8),
    /// A code names no string the table has.
    Corrupt,
}

/// Uncompresses what it is given, a piece at a time.
pub struct Decoder {
    header: Vec<u8>,
    most: u8,
    block: bool,
    width: u8,
    limit: u32,
    /// The next code a string gets.
    next: u32,
    prefix: Vec<u16>,
    suffix: Vec<u8>,
    /// The code before, and the first byte of its string.
    old: Option<(u32, u8)>,
    bits: u32,
    held: u32,
    /// The bits of the group at hand read so far, which a new width or
    /// a new table passes the rest of.
    in_group: u32,
    /// Bits to pass over before the next code.
    skip: u32,
    /// A string spelled backwards, while it is written out.
    stack: Vec<u8>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    #[must_use]
    pub fn new() -> Decoder {
        Decoder {
            header: Vec::new(),
            most: 0,
            block: false,
            width: LEAST_BITS,
            limit: 0,
            next: FIRST,
            prefix: Vec::new(),
            suffix: Vec::new(),
            old: None,
            bits: 0,
            held: 0,
            in_group: 0,
            skip: 0,
            stack: Vec::new(),
        }
    }

    /// Reads the header from the bytes at hand, if there are enough.
    ///
    /// # Errors
    ///
    /// When it is no compress header.
    fn header(&mut self, bytes: &mut &[u8]) -> Result<bool, Fault> {
        while self.header.len() < 3 {
            let Some((&b, rest)) = bytes.split_first() else {
                return Ok(false);
            };
            self.header.push(b);
            *bytes = rest;
        }
        if self.header[..2] != MAGIC {
            return Err(Fault::NotCompressed);
        }
        self.most = self.header[2] & 0x1f;
        if self.most > MOST_BITS {
            return Err(Fault::TooManyBits(self.most));
        }
        self.block = self.header[2] & BLOCK != 0;
        self.limit = limit(LEAST_BITS, self.most);
        self.next = if self.block { FIRST } else { CLEAR };
        let size = 1usize << self.most.max(LEAST_BITS);
        self.prefix = vec![0; size];
        self.suffix = vec![0; size];
        for code in 0..256u16 {
            self.suffix[usize::from(code)] = code.to_le_bytes()[0];
        }
        Ok(true)
    }

    /// Decodes `bytes`, the next of the compressed text, and gives what
    /// they make.
    ///
    /// # Errors
    ///
    /// When they are no compressed text, or a code names no string.
    pub fn feed(&mut self, mut bytes: &[u8]) -> Result<Vec<u8>, Fault> {
        let mut out = Vec::new();
        if self.prefix.is_empty() && !self.header(&mut bytes)? {
            return Ok(out);
        }
        for &b in bytes {
            if self.skip >= 8 {
                self.skip -= 8;
                continue;
            }
            self.bits |= u32::from(b) << self.held;
            self.held += 8;
            while self.held >= u32::from(self.width) {
                let code = self.bits & ((1 << self.width) - 1);
                self.bits >>= self.width;
                self.held -= u32::from(self.width);
                self.in_group += u32::from(self.width);
                self.code(code, &mut out)?;
                if self.skip > 0 {
                    // The rest of the group goes by, from the bits held.
                    let dropped = self.skip.min(self.held);
                    self.bits = self.bits.checked_shr(dropped).unwrap_or(0);
                    self.held -= dropped;
                    self.skip -= dropped;
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Passes the rest of the group at hand, of codes `width` bits wide.
    fn end_group(&mut self, width: u8) {
        let group = u32::from(width) * 8;
        self.skip = (group - self.in_group % group) % group;
        self.in_group = 0;
    }

    fn code(&mut self, code: u32, out: &mut Vec<u8>) -> Result<(), Fault> {
        if code == CLEAR && self.block {
            self.end_group(self.width);
            self.width = LEAST_BITS;
            self.limit = limit(LEAST_BITS, self.most);
            self.next = FIRST;
            self.old = None;
            return Ok(());
        }
        let Some((old, first)) = self.old else {
            // The first code of a table is a byte.
            let b = u8::try_from(code).map_err(|_| Fault::Corrupt)?;
            out.push(b);
            self.old = Some((code, b));
            return Ok(());
        };
        if code > self.next {
            return Err(Fault::Corrupt);
        }
        // The code not yet in the table is the one before and its first
        // byte again.
        let mut at = code;
        self.stack.clear();
        if code == self.next {
            self.stack.push(first);
            at = old;
        }
        while at >= 256 {
            self.stack.push(self.suffix[at as usize]);
            at = u32::from(self.prefix[at as usize]);
        }
        let start = self.suffix[at as usize];
        self.stack.push(start);
        out.extend(self.stack.iter().rev());
        if self.next < 1 << self.most {
            self.prefix[self.next as usize] = u16::try_from(old).unwrap_or(0);
            self.suffix[self.next as usize] = start;
            self.next += 1;
        }
        self.old = Some((code, start));
        if self.next > self.limit {
            self.end_group(self.width);
            self.width += 1;
            self.limit = limit(self.width, self.most);
        }
        Ok(())
    }

    /// Whether the text ended where a whole one may: the header read.
    #[must_use]
    pub fn whole(&self) -> bool {
        self.header.len() == 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bytes` compressed with codes of at most `most` bits, fed in
    /// pieces of `piece`.
    fn compressed(bytes: &[u8], most: u8, piece: usize) -> Vec<u8> {
        let mut encoder = Encoder::new(most);
        let mut out = Vec::new();
        for chunk in bytes.chunks(piece.max(1)) {
            out.extend(encoder.feed(chunk));
        }
        out.extend(encoder.finish());
        out
    }

    fn uncompressed(bytes: &[u8], piece: usize) -> Result<Vec<u8>, Fault> {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        for chunk in bytes.chunks(piece.max(1)) {
            out.extend(decoder.feed(chunk)?);
        }
        Ok(out)
    }

    /// `codes` packed the low bits first, each as wide as it says.
    fn packed(codes: &[(u32, u8)]) -> Vec<u8> {
        let mut packer = Packer::default();
        for &(code, width) in codes {
            packer.put(code, width);
        }
        packer.finish();
        packer.out
    }

    /// The codes of 9 bits in `bytes`, after the header.
    fn nine_bit_codes(bytes: &[u8]) -> Vec<u32> {
        let mut codes = Vec::new();
        let (mut bits, mut held) = (0u32, 0u32);
        for &b in &bytes[3..] {
            bits |= u32::from(b) << held;
            held += 8;
            while held >= 9 {
                codes.push(bits & 0x1ff);
                bits >>= 9;
                held -= 9;
            }
        }
        codes
    }

    /// What the host's gzip, which reads compress's files, makes of
    /// `bytes`.
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut child = std::process::Command::new("gzip")
            .arg("-dc")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let input = bytes.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&input).unwrap());
        let done = child.wait_with_output().unwrap();
        writer.join().unwrap();
        assert!(done.status.success(), "gzip refused it");
        done.stdout
    }

    /// Bytes from a random generator that needs nothing, from `kinds`
    /// values, so that some texts repeat a lot and some little.
    fn random(len: usize, kinds: u64, mut seed: u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed % kinds).to_le_bytes()[0]
            })
            .collect()
    }

    #[test]
    fn a_header_says_the_bits_and_the_codes_come_low_first() {
        assert_eq!(compressed(b"", 16, 1), [0x1f, 0x9d, 0x90]);
        assert_eq!(compressed(b"a", 16, 1), [0x1f, 0x9d, 0x90, 0x61, 0x00]);
        assert_eq!(compressed(b"", 12, 1)[2], 0x8c);
        assert_eq!(compressed(b"", 5, 1)[2], 0x89, "no fewer than 9");
        assert_eq!(compressed(b"", 30, 1)[2], 0x90, "no more than 16");
        // a, b and then ab, nine bits each: 97, 98 and 257.
        assert_eq!(
            compressed(b"abab", 16, 1)[3..],
            packed(&[(97, 9), (98, 9), (257, 9)])
        );
        assert_eq!(gzip(&compressed(b"abab", 16, 1)), b"abab");
    }

    #[test]
    fn texts_come_back_whatever_the_pieces_and_bits() {
        let texts = [
            Vec::new(),
            b"a".to_vec(),
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec(),
            b"TOBEORNOTTOBEORTOBEORNOT#".to_vec(),
            random(5000, 3, 0x9e37_79b9_7f4a_7c15),
            random(5000, 256, 0x2545_f491_4f6c_dd1d),
        ];
        for text in &texts {
            for most in [9, 10, 12, 16] {
                for piece in [1, 7, 4096] {
                    let packed = compressed(text, most, piece);
                    for back_piece in [1, 3, 5000] {
                        assert_eq!(
                            uncompressed(&packed, back_piece).as_ref(),
                            Ok(text),
                            "{} bytes, {most} bits, pieces {piece} and {back_piece}",
                            text.len()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn gzip_reads_what_it_writes_at_every_width() {
        for (len, kinds, seed) in [
            (1000, 2, 1),
            (20_000, 4, 2),
            (70_000, 256, 3),
            (300_000, 16, 4),
        ] {
            let text = random(len, kinds, seed);
            for most in [9, 11, 13, 16] {
                assert_eq!(
                    gzip(&compressed(&text, most, 8192)),
                    text,
                    "{len} bytes of {kinds}, {most} bits"
                );
            }
        }
    }

    #[test]
    fn a_full_table_starts_again_when_coding_goes_worse() {
        // Text that repeats, then text that does not, past a full table
        // of 9 bits: the table starts again, and both ways keep up.
        let mut text = random(40_000, 2, 5);
        text.extend(random(60_000, 256, 6));
        let packed = compressed(&text, 9, 1000);
        assert_eq!(uncompressed(&packed, 999).as_ref(), Ok(&text));
        assert_eq!(gzip(&packed), text);
        let mut encoder = Encoder::new(9);
        encoder.feed(&text);
        assert!(encoder.ratio > 0 || encoder.next < 1 << 9, "it looked");
    }

    #[test]
    fn the_old_kind_without_clear_codes_reads_too() {
        // No block bit: 256 is a string's code, not CLEAR. a, b, then
        // 256 for ab.
        let mut text = alloc::vec![0x1f, 0x9d, 0x10];
        text.extend(packed(&[(97, 9), (98, 9), (256, 9)]));
        assert_eq!(uncompressed(&text, 1).as_deref(), Ok(&b"abab"[..]));
        // With it, the same codes start the table again after ab.
        text[2] = 0x90;
        assert_eq!(uncompressed(&text, 1).as_deref(), Ok(&b"ab"[..]));
    }

    #[test]
    fn what_is_no_compressed_text_is_said() {
        assert_eq!(uncompressed(b"\x1f\x8b\x08", 1), Err(Fault::NotCompressed));
        assert_eq!(
            uncompressed(b"\x1f\x9d\x91", 1),
            Err(Fault::TooManyBits(17))
        );
        // A first code that is no byte, and one past the table.
        assert_eq!(
            uncompressed(&[0x1f, 0x9d, 0x90, 0x01, 0x01], 1),
            Err(Fault::Corrupt)
        );
        let mut text = alloc::vec![0x1f, 0x9d, 0x90];
        text.extend(packed(&[(97, 9), (300, 9)]));
        assert_eq!(uncompressed(&text, 1), Err(Fault::Corrupt));
        let mut decoder = Decoder::default();
        assert!(!decoder.whole());
        assert_eq!(
            decoder.feed(b"\x1f\x9d"),
            Ok(Vec::new()),
            "a header in pieces"
        );
        assert!(!decoder.whole());
        assert_eq!(decoder.feed(b"\x90"), Ok(Vec::new()));
        assert!(decoder.whole());
    }

    #[test]
    fn the_codes_are_those_lzw_gives_with_257_the_first_string() {
        // As an LZW written apart, in Python, gives them.
        assert_eq!(
            nine_bit_codes(&compressed(b"TOBEORNOTTOBEORTOBEORNOT#", 16, 1)),
            [
                84, 79, 66, 69, 79, 82, 78, 79, 84, 257, 259, 261, 266, 260, 262, 264, 35
            ]
        );
        assert_eq!(
            nine_bit_codes(&compressed(b"aaaaaaaaaa", 16, 1)),
            [97, 257, 258, 259]
        );
        assert_eq!(
            nine_bit_codes(&compressed(b"abababab", 16, 1)),
            [97, 98, 257, 259, 98]
        );
        assert_eq!(
            uncompressed(&compressed(b"aaaaaaaaaa", 16, 1), 1).as_deref(),
            Ok(&b"aaaaaaaaaa"[..]),
            "258 and 259 each name the string not yet in the table"
        );
    }

    #[test]
    fn codes_pack_the_low_bits_first_and_groups_fill_with_nothing() {
        assert_eq!(packed(&[(0x1ff, 9)]), [0xff, 0x01]);
        assert_eq!(packed(&[(1, 9), (1, 9)]), [0x01, 0x02, 0x00]);
        assert_eq!(packed(&[(0xffff, 16)]), [0xff, 0xff]);
        let mut packer = Packer::default();
        packer.put(5, 9);
        packer.put(6, 9);
        packer.end_group(9);
        assert_eq!(packer.codes, 0);
        assert_eq!(packer.out.len(), 9, "eight codes of nine bits");
        assert_eq!(packer.held, 0, "a group ends on a byte");
        assert_eq!(packer.out[..3], [5, 12, 0]);
        packer.end_group(9);
        assert_eq!(packer.out.len(), 9, "an empty group is none");
        packer.put(3, 10);
        packer.finish();
        assert_eq!(packer.out[9..], [3, 0]);
        packer.finish();
        assert_eq!(packer.out.len(), 11, "nothing held, nothing more");
    }

    #[test]
    fn widths_grow_past_their_greatest_code() {
        assert_eq!(limit(9, 16), 511);
        assert_eq!(limit(15, 16), 32_767);
        assert_eq!(limit(16, 16), 65_536, "the most has room for every code");
        assert_eq!(limit(12, 12), 4096);
        assert_eq!(limit(9, 9), 511, "but 9 at most grows on to 10");
        assert_eq!(limit(10, 9), 1023);
        let mut encoder = Encoder::new(16);
        assert_eq!((encoder.width, encoder.limit), (9, 511));
        let text = random(3000, 256, 13);
        encoder.feed(&text);
        assert!(encoder.width > 9, "{}", encoder.width);
        assert_eq!(encoder.limit, limit(encoder.width, 16));
    }

    #[test]
    fn the_first_code_of_ten_bits_is_the_257th() {
        // 256 codes of nine bits fill 32 groups, 288 bytes; the next
        // code is ten bits wide.
        let text = random(3000, 256, 9);
        let packed = compressed(&text, 16, 3000);
        let mut decoder = Decoder::new();
        let mut back = Vec::new();
        for (at, piece) in packed.chunks(1).enumerate() {
            back.extend(decoder.feed(piece).unwrap());
            if at == 3 + 288 - 1 {
                assert_eq!(decoder.width, 10, "after 288 bytes of codes");
            }
            if at == 3 + 287 - 1 {
                assert_eq!(decoder.width, 9, "not before");
            }
        }
        assert_eq!(back, text);
    }

    #[test]
    fn a_reader_passes_the_rest_of_a_group_whatever_its_pieces() {
        // Texts whose codes change width at every place in a group.
        for len in [300u64, 301, 310, 333, 500, 777, 1200, 2600, 5300] {
            let text = random(usize::try_from(len).unwrap(), 200, len);
            let packed = compressed(&text, 16, 64);
            for piece in [1, 2, 9, 10, 11, 64] {
                assert_eq!(
                    uncompressed(&packed, piece).as_ref(),
                    Ok(&text),
                    "{len}, {piece}"
                );
            }
            assert_eq!(gzip(&packed), text, "{len}");
        }
    }

    #[test]
    fn strings_are_found_where_they_were_put() {
        let mut encoder = Encoder::new(12);
        assert_eq!(
            encoder.feed(b"abcab"),
            [0x1f, 0x9d, 0x8c, 0x61, 0xc4, 0x8c],
            "a, b and c, 27 bits: 3 whole bytes"
        );
        // ab 257, bc 258, ca 259; ab is in, abc not yet.
        assert_eq!(encoder.find(u32::from(b'a'), b'b'), Some(257));
        assert_eq!(encoder.find(u32::from(b'b'), b'c'), Some(258));
        assert_eq!(encoder.find(u32::from(b'c'), b'a'), Some(259));
        assert_eq!(encoder.find(257, b'c'), None);
        assert_eq!(encoder.find(u32::from(b'z'), b'a'), None);
        assert_eq!(encoder.string, Some(257), "ab, read and not yet written");
        assert_eq!(encoder.next, 260);
        let all = packed(&[(97, 9), (98, 9), (99, 9), (257, 9)]);
        assert_eq!(encoder.finish(), all[3..], "the rest of the codes");
        assert_eq!(encoder.finish(), Vec::<u8>::new(), "nothing left");
    }

    #[test]
    fn every_most_writes_what_gzip_reads_past_a_full_table() {
        // Enough text to fill tables of 10 to 14 bits and start them
        // again.
        let mut text = random(60_000, 3, 10);
        text.extend(random(60_000, 256, 11));
        text.extend(random(60_000, 2, 12));
        for most in 10..=14 {
            let packed = compressed(&text, most, 4096);
            assert_eq!(gzip(&packed), text, "{most} bits");
            assert_eq!(
                uncompressed(&packed, 4096).as_ref(),
                Ok(&text),
                "{most} bits"
            );
        }
    }

    #[test]
    fn a_full_table_starts_again_only_when_the_ratio_falls() {
        let mut encoder = Encoder::new(9);
        encoder.packer.out.clear();
        encoder.written = 1000;
        encoder.read = 4000;
        encoder.next = 1 << 9;
        encoder.width = 10;
        encoder.child[97] = 300;
        encoder.look();
        assert_eq!(encoder.ratio, 4000 * 256 / 1000, "better than none");
        assert_eq!(encoder.look_at, 4000 + CHECK_GAP);
        assert_eq!(
            (encoder.next, encoder.width),
            (1 << 9, 10),
            "the table stays"
        );
        assert_eq!(
            encoder.packer.out,
            Vec::<u8>::new(),
            "and nothing is written"
        );
        encoder.read = 5000;
        encoder.written = 2000;
        encoder.look();
        assert_eq!(encoder.ratio, 0, "worse: the next look starts afresh");
        assert_eq!((encoder.next, encoder.width), (FIRST, 9));
        assert_eq!(encoder.limit, 511);
        assert_eq!(encoder.child[97], NONE, "the table is empty");
        assert_eq!(encoder.packer.codes, 0, "after a whole group");
        assert_eq!(
            encoder.packer.out.len(),
            10,
            "CLEAR and seven codes of nothing, ten bits each"
        );
        assert_eq!(encoder.packer.out[..2], [0x00, 0x01], "256, ten bits");
    }

    #[test]
    fn the_same_text_comes_out_the_same_whatever_its_pieces() {
        for (len, kinds) in [(10_000, 4), (70_000, 256), (200_000, 7)] {
            let text = random(len, kinds, len as u64);
            let whole = compressed(&text, 16, len);
            for piece in [1, 100, 4096] {
                assert_eq!(compressed(&text, 16, piece), whole, "{len} in {piece}");
            }
            assert!(
                whole.len() < len || kinds == 256,
                "{len} of {kinds} got smaller"
            );
        }
    }

    #[test]
    fn empty_pieces_and_a_text_cut_short_give_what_they_have() {
        let mut encoder = Encoder::new(16);
        assert_eq!(
            encoder.feed(&[]),
            MAGIC.iter().copied().chain([0x90]).collect::<Vec<_>>()
        );
        assert_eq!(encoder.feed(&[]), Vec::<u8>::new());
        assert_eq!(
            encoder.finish(),
            Vec::<u8>::new(),
            "an empty text is its header"
        );
        let text = random(2000, 30, 14);
        let packed = compressed(&text, 16, 2000);
        let mut decoder = Decoder::new();
        assert_eq!(decoder.feed(&[]), Ok(Vec::new()));
        let half = decoder.feed(&packed[..packed.len() / 2]).unwrap();
        assert!(
            !half.is_empty() && text.starts_with(&half),
            "what came is the text's start"
        );
        assert!(decoder.whole());
        let rest = decoder.feed(&packed[packed.len() / 2..]).unwrap();
        assert_eq!([half, rest].concat(), text);
    }

    #[test]
    fn only_the_low_five_bits_of_the_header_say_the_most() {
        let mut text = alloc::vec![0x1f, 0x9d, 0xec];
        text.extend(packed(&[(104, 9), (105, 9)]));
        assert_eq!(
            uncompressed(&text, 1).as_deref(),
            Ok(&b"hi"[..]),
            "12 bits, with others set"
        );
        let mut decoder = Decoder::new();
        decoder.feed(&text).unwrap();
        assert_eq!((decoder.most, decoder.block), (12, true));
        assert_eq!(
            uncompressed(b"\x1f\x9d\x9f", 1),
            Err(Fault::TooManyBits(31))
        );
        assert_eq!(
            uncompressed(b"\x9d\x1f\x90", 1),
            Err(Fault::NotCompressed),
            "the bytes in order"
        );
        assert_eq!(
            uncompressed(b"\x1f\x9d", 1),
            Ok(Vec::new()),
            "a header cut short is no text yet"
        );
    }

    #[test]
    fn what_fills_a_group_is_passed_over_whatever_it_holds() {
        // a, CLEAR, six codes to fill the group, then b: compress fills
        // with what its buffer held, so the six may be anything.
        for filler in [0u32, 0x1ff, 0x0aa, 300] {
            let mut codes = alloc::vec![(97, 9), (256, 9)];
            codes.extend([(filler, 9); 6]);
            codes.push((98, 9));
            let mut text = alloc::vec![0x1f, 0x9d, 0x8a];
            text.extend(packed(&codes));
            for piece in [1, 2, 5] {
                assert_eq!(
                    uncompressed(&text, piece).as_deref(),
                    Ok(&b"ab"[..]),
                    "filled with {filler}, in pieces of {piece}"
                );
            }
        }
    }

    #[test]
    fn a_table_of_nine_bits_at_most_writes_ten_once_full() {
        let text = random(4000, 256, 15);
        let mut encoder = Encoder::new(9);
        encoder.feed(&text);
        assert_eq!(encoder.next, 1 << 9, "full");
        assert_eq!(
            (encoder.width, encoder.limit),
            (10, 1023),
            "as compress has it"
        );
        let packed = compressed(&text, 9, 4000);
        let mut decoder = Decoder::new();
        assert_eq!(decoder.feed(&packed).as_ref(), Ok(&text));
        assert_eq!(
            (decoder.width, decoder.next),
            (10, 1 << 9),
            "and reads it so"
        );
        assert_eq!(gzip(&packed), text, "as gzip does");
    }

    #[test]
    fn a_group_of_sixteen_bit_codes_is_sixteen_bytes() {
        let mut packer = Packer::default();
        packer.put(0xabcd, 16);
        packer.end_group(16);
        assert_eq!(packer.out.len(), 16);
        assert_eq!(packer.out[..2], [0xcd, 0xab]);
        assert!(packer.out[2..].iter().all(|&b| b == 0));
        let mut packer = Packer::default();
        for code in 0..8 {
            packer.put(code, 13);
        }
        packer.end_group(13);
        assert_eq!(packer.out.len(), 13, "a whole group needs nothing more");
    }

    #[test]
    fn decoders_start_alike() {
        let (a, b) = (Decoder::new(), Decoder::default());
        assert_eq!(
            (a.width, a.next, a.most, a.whole()),
            (b.width, b.next, b.most, b.whole())
        );
        assert_eq!((a.width, a.next), (LEAST_BITS, FIRST));
    }

    #[test]
    fn the_old_kind_grows_its_width_as_its_table_fills() {
        // A run of a: a, then each code the next string, a longer run,
        // numbered from 256. The 258th code is the first of ten bits,
        // after the group of nine-bit codes is filled.
        let mut codes = alloc::vec![(97, 9)];
        codes.extend((256..=511).map(|code| (code, 9)));
        codes.extend([(0, 9); 7]);
        codes.push((512, 10));
        let mut text = alloc::vec![0x1f, 0x9d, 0x10];
        text.extend(packed(&codes));
        let run = 258 * 259 / 2;
        for piece in [1, 64, text.len()] {
            let back = uncompressed(&text, piece).unwrap();
            assert_eq!(back.len(), run, "in pieces of {piece}");
            assert!(back.iter().all(|&b| b == b'a'));
        }
        let read = gzip(&text);
        assert_eq!(read.len(), run, "as gzip reads it");
        assert!(read.iter().all(|&b| b == b'a'));
    }
}
