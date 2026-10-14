//! QR codes in byte mode, following ISO/IEC 18004: the smallest version
//! from 1 to 40 that holds the data at the chosen error correction level,
//! Reed-Solomon codewords over GF(256), the eight masks scored by their
//! penalty, and the format and version information. The kernel's panic
//! screen uses it, so it never allocates and never panics on any input.

#![cfg_attr(not(test), no_std)]

mod tables;

use tables::{BLOCKS, ECC_PER_BLOCK};

/// Modules on a side of the largest code, version 40.
pub const MAX_SIZE: usize = 177;
const MAX_MODULES: usize = MAX_SIZE * MAX_SIZE;
const WORDS: usize = MAX_MODULES.div_ceil(64);
/// Codewords of version 40, data and error correction together.
const MAX_CODEWORDS: usize = 3706;

/// How much of the code can be damaged and still read: about 7, 15, 25
/// and 30 percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ecc {
    Low,
    Medium,
    Quartile,
    High,
}

impl Ecc {
    fn index(self) -> usize {
        self as usize
    }

    /// The two bits the format information uses for the level.
    fn format_bits(self) -> u32 {
        match self {
            Ecc::Low => 1,
            Ecc::Medium => 0,
            Ecc::Quartile => 3,
            Ecc::High => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data does not fit even version 40 at this level.
    TooLong,
}

/// A finished code of `size` by `size` modules.
pub struct Code {
    size: usize,
    version: u8,
    ecc: Ecc,
    mask: u8,
    dark: [u64; WORDS],
    function: [u64; WORDS],
}

impl Code {
    /// An empty code to encode into. It is large, so callers keep one
    /// around rather than make one per use.
    #[must_use]
    pub const fn new() -> Code {
        Code {
            size: 0,
            version: 0,
            ecc: Ecc::Low,
            mask: 0,
            dark: [0; WORDS],
            function: [0; WORDS],
        }
    }

    /// Modules on a side, 21 for version 1 up to 177 for version 40.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    #[must_use]
    pub fn version(&self) -> u8 {
        self.version
    }

    #[must_use]
    pub fn ecc(&self) -> Ecc {
        self.ecc
    }

    #[must_use]
    pub fn mask(&self) -> u8 {
        self.mask
    }

    /// Whether the module in column `x` of row `y` is dark. Modules
    /// outside the code are light, like the quiet zone around it.
    #[must_use]
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && get(&self.dark, y * self.size + x)
    }

    fn is_function(&self, x: usize, y: usize) -> bool {
        get(&self.function, y * self.size + x)
    }

    fn set(&mut self, x: usize, y: usize, dark: bool) {
        put(&mut self.dark, y * self.size + x, dark);
    }

    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        self.set(x, y, dark);
        put(&mut self.function, y * self.size + x, true);
    }
}

impl Default for Code {
    fn default() -> Self {
        Code::new()
    }
}

fn get(bits: &[u64], at: usize) -> bool {
    bits[at / 64] >> (at % 64) & 1 != 0
}

fn put(bits: &mut [u64], at: usize, on: bool) {
    let bit = 1 << (at % 64);
    if on {
        bits[at / 64] |= bit;
    } else {
        bits[at / 64] &= !bit;
    }
}

/// Encodes `data` in byte mode into `code`, in the smallest version that
/// holds it at `ecc`, with the mask of the lowest penalty.
///
/// # Errors
///
/// [`Error::TooLong`] when no version holds the data.
pub fn encode(data: &[u8], ecc: Ecc, code: &mut Code) -> Result<(), Error> {
    let version = (1..=40)
        .find(|&v| bits_needed(data.len(), v).div_ceil(8) <= data_codewords(v, ecc))
        .ok_or(Error::TooLong)?;
    let mut words = [0u8; MAX_CODEWORDS];
    let data_len = data_codewords(version, ecc);
    fill_data(data, version, &mut words[..data_len]);
    let mut all = [0u8; MAX_CODEWORDS];
    let total = raw_codewords(version);
    add_error_correction(&words[..data_len], version, ecc, &mut all[..total]);

    code.size = usize::from(version) * 4 + 17;
    code.version = version;
    code.ecc = ecc;
    code.dark = [0; WORDS];
    code.function = [0; WORDS];
    draw_function_patterns(code);
    place(code, &all[..total]);

    let mut best = (u32::MAX, 0);
    for mask in 0..8 {
        apply_mask(code, mask);
        draw_format_bits(code, mask);
        let score = penalty(code);
        if score < best.0 {
            best = (score, mask);
        }
        apply_mask(code, mask);
    }
    code.mask = best.1;
    apply_mask(code, best.1);
    draw_format_bits(code, best.1);
    Ok(())
}

/// Bits the data takes in byte mode: the mode, the length and the bytes.
fn bits_needed(len: usize, version: u8) -> usize {
    let count_bits = if version <= 9 { 8 } else { 16 };
    if len >= 1 << count_bits {
        return usize::MAX;
    }
    4 + count_bits + 8 * len
}

/// Modules that hold codewords, with the remainder bits.
fn raw_modules(version: u8) -> usize {
    let v = usize::from(version);
    let mut modules = (16 * v + 128) * v + 64;
    if v >= 2 {
        let align = v / 7 + 2;
        modules -= (25 * align - 10) * align - 55;
        if v >= 7 {
            modules -= 36;
        }
    }
    modules
}

fn raw_codewords(version: u8) -> usize {
    raw_modules(version) / 8
}

fn data_codewords(version: u8, ecc: Ecc) -> usize {
    let v = usize::from(version) - 1;
    raw_codewords(version)
        - usize::from(ECC_PER_BLOCK[ecc.index()][v]) * usize::from(BLOCKS[ecc.index()][v])
}

/// The data codewords: mode, length, bytes, terminator, then padding.
fn fill_data(data: &[u8], version: u8, out: &mut [u8]) {
    let mut bits = Bits { out, len: 0 };
    bits.push(0b0100, 4);
    let count_bits = if version <= 9 { 8 } else { 16 };
    bits.push(u32::try_from(data.len()).unwrap_or(0), count_bits);
    for &b in data {
        bits.push(u32::from(b), 8);
    }
    let capacity = bits.out.len() * 8;
    // In byte mode the bits so far always end 4 short of a whole byte, so
    // the 4 bit terminator also fills it up.
    bits.push(0, 4);
    for pad in [0xec, 0x11].into_iter().cycle() {
        if bits.len == capacity {
            break;
        }
        bits.push(pad, 8);
    }
}

struct Bits<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Bits<'_> {
    fn push(&mut self, value: u32, count: usize) {
        for i in (0..count).rev() {
            if value >> i & 1 != 0 {
                self.out[self.len / 8] |= 0x80 >> (self.len % 8);
            }
            self.len += 1;
        }
    }
}

/// Multiplies in GF(256) with the field polynomial x^8 + x^4 + x^3 + x^2 + 1.
fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        let carry = a & 0x80 != 0;
        a <<= 1;
        if carry {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    product
}

/// The generator polynomial of degree `degree`, the product of (x - a^i)
/// for i below it, highest power first; its leading 1 is left out.
fn generator(degree: usize) -> [u8; 30] {
    let mut g = [0u8; 31];
    g[0] = 1;
    let mut root = 1u8;
    for d in 0..degree {
        for k in (1..=d + 1).rev() {
            g[k] ^= mul(root, g[k - 1]);
        }
        root = mul(root, 2);
    }
    let mut out = [0u8; 30];
    out[..degree].copy_from_slice(&g[1..=degree]);
    out
}

/// The remainder of the data times x^degree divided by the generator.
fn remainder(data: &[u8], generator: &[u8], out: &mut [u8]) {
    out.fill(0);
    let n = out.len();
    for &d in data {
        let factor = d ^ out[0];
        out.copy_within(1..n, 0);
        out[n - 1] = 0;
        for (r, &g) in out.iter_mut().zip(generator) {
            *r ^= mul(g, factor);
        }
    }
}

/// Splits the data into blocks, adds each block's error correction and
/// interleaves them: the first codeword of every block, then the second,
/// and so on, data first, then error correction.
fn add_error_correction(data: &[u8], version: u8, ecc: Ecc, out: &mut [u8]) {
    let v = usize::from(version) - 1;
    let blocks = usize::from(BLOCKS[ecc.index()][v]);
    let ecc_len = usize::from(ECC_PER_BLOCK[ecc.index()][v]);
    let total = out.len();
    let short_blocks = blocks - total % blocks;
    let short_data = total / blocks - ecc_len;
    let block_data = |b: usize| short_data + usize::from(b >= short_blocks);
    let block_start = |b: usize| b * short_data + b.saturating_sub(short_blocks);

    let mut at = 0;
    for i in 0..=short_data {
        for b in 0..blocks {
            if i < block_data(b) {
                out[at] = data[block_start(b) + i];
                at += 1;
            }
        }
    }
    let g = generator(ecc_len);
    let mut ecc_words = [[0u8; 30]; 81];
    for (b, words) in ecc_words.iter_mut().enumerate().take(blocks) {
        let start = block_start(b);
        remainder(
            &data[start..start + block_data(b)],
            &g[..ecc_len],
            &mut words[..ecc_len],
        );
    }
    for i in 0..ecc_len {
        for words in &ecc_words[..blocks] {
            out[at] = words[i];
            at += 1;
        }
    }
}

/// Where the centres of the alignment patterns go, along both axes.
fn alignment_positions(version: u8, out: &mut [usize; 7]) -> usize {
    if version == 1 {
        return 0;
    }
    let v = usize::from(version);
    let count = v / 7 + 2;
    let size = v * 4 + 17;
    let step = if v == 32 {
        26
    } else {
        (v * 4 + count * 2 + 1) / (count * 2 - 2) * 2
    };
    out[0] = 6;
    for i in 1..count {
        out[count - i] = size - 7 - (i - 1) * step;
    }
    count
}

fn draw_function_patterns(code: &mut Code) {
    let size = code.size;
    for i in 0..size {
        code.set_function(6, i, i % 2 == 0);
        code.set_function(i, 6, i % 2 == 0);
    }
    for (cx, cy) in [(3, 3), (size - 4, 3), (3, size - 4)] {
        for y in cy.saturating_sub(4)..=(cy + 4).min(size - 1) {
            for x in cx.saturating_sub(4)..=(cx + 4).min(size - 1) {
                let ring = x.abs_diff(cx).max(y.abs_diff(cy));
                code.set_function(x, y, ring != 2 && ring != 4);
            }
        }
    }
    let mut positions = [0usize; 7];
    let count = alignment_positions(code.version, &mut positions);
    for (i, &px) in positions[..count].iter().enumerate() {
        for (j, &py) in positions[..count].iter().enumerate() {
            // The three corners where finders are get none.
            let last = count - 1;
            if (i == 0 && (j == 0 || j == last)) || (i == last && j == 0) {
                continue;
            }
            for y in py - 2..=py + 2 {
                for x in px - 2..=px + 2 {
                    code.set_function(x, y, x.abs_diff(px).max(y.abs_diff(py)) != 1);
                }
            }
        }
    }
    // Reserve the format information; draw_format_bits fills it in.
    draw_format_bits(code, 0);
    if code.version >= 7 {
        let v = u32::from(code.version);
        let mut rem = v;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1f25);
        }
        let bits = v << 12 | rem;
        for i in 0..18 {
            let dark = bits >> i & 1 != 0;
            let (a, b) = (size - 11 + i % 3, i / 3);
            code.set_function(a, b, dark);
            code.set_function(b, a, dark);
        }
    }
}

/// The 15 format bits for a level and mask, with their BCH code and XOR
/// pattern.
fn format_bits(ecc: Ecc, mask: u8) -> u32 {
    let data = ecc.format_bits() << 3 | u32::from(mask);
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    (data << 10 | rem) ^ 0x5412
}

fn draw_format_bits(code: &mut Code, mask: u8) {
    let bits = format_bits(code.ecc, mask);
    let bit = |i: usize| bits >> i & 1 != 0;
    let size = code.size;
    for i in 0..=5 {
        code.set_function(8, i, bit(i));
    }
    code.set_function(8, 7, bit(6));
    code.set_function(8, 8, bit(7));
    code.set_function(7, 8, bit(8));
    for i in 9..15 {
        code.set_function(14 - i, 8, bit(i));
    }
    for i in 0..8 {
        code.set_function(size - 1 - i, 8, bit(i));
    }
    for i in 8..15 {
        code.set_function(8, size - 15 + i, bit(i));
    }
    code.set_function(8, size - 8, true);
}

/// Puts the codewords' bits into the modules that are not function
/// patterns, two columns at a time, up and down in turn from the bottom
/// right. Modules left over stay light.
fn place(code: &mut Code, words: &[u8]) {
    let size = code.size;
    let total_bits = words.len() * 8;
    let mut i = 0;
    let mut right = size - 1;
    loop {
        if right == 6 {
            right = 5;
        }
        let upward = (right + 1) & 2 == 0;
        for vert in 0..size {
            let y = if upward { size - 1 - vert } else { vert };
            for x in [right, right - 1] {
                if !code.is_function(x, y) && i < total_bits {
                    code.set(x, y, words[i / 8] >> (7 - i % 8) & 1 != 0);
                    i += 1;
                }
            }
        }
        if right == 1 {
            break;
        }
        right -= 2;
    }
}

fn mask_hits(mask: u8, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y).is_multiple_of(2),
        1 => y.is_multiple_of(2),
        2 => x.is_multiple_of(3),
        3 => (x + y).is_multiple_of(3),
        4 => (x / 3 + y / 2).is_multiple_of(2),
        5 => x * y % 2 + x * y % 3 == 0,
        6 => (x * y % 2 + x * y % 3).is_multiple_of(2),
        _ => ((x + y) % 2 + x * y % 3).is_multiple_of(2),
    }
}

/// Flips every data module the mask hits. Doing it twice undoes it.
fn apply_mask(code: &mut Code, mask: u8) {
    for y in 0..code.size {
        for x in 0..code.size {
            if !code.is_function(x, y) && mask_hits(mask, x, y) {
                let dark = code.is_dark(x, y);
                code.set(x, y, !dark);
            }
        }
    }
}

/// The penalty of the code as it stands, by the four rules of the
/// standard: long runs, 2 by 2 blocks, patterns that look like finders,
/// and an uneven share of dark modules.
fn penalty(code: &Code) -> u32 {
    let size = code.size;
    let mut score = 0;
    for line in 0..size {
        for across in [true, false] {
            let at = |i: usize| {
                if across {
                    code.is_dark(i, line)
                } else {
                    code.is_dark(line, i)
                }
            };
            let mut run = 0;
            for i in 0..size {
                if i > 0 && at(i) == at(i - 1) {
                    run += 1;
                } else {
                    run = 1;
                }
                match run {
                    5 => score += 3,
                    6.. => score += 1,
                    _ => {}
                }
                if i + 11 <= size {
                    let window = (0..11).fold(0u32, |acc, k| acc << 1 | u32::from(at(i + k)));
                    if window == 0b101_1101_0000 || window == 0b000_0101_1101 {
                        score += 40;
                    }
                }
            }
        }
    }
    let mut dark = 0;
    for y in 0..size {
        for x in 0..size {
            let d = code.is_dark(x, y);
            dark += usize::from(d);
            if x + 1 < size && y + 1 < size {
                let same = code.is_dark(x + 1, y) == d
                    && code.is_dark(x, y + 1) == d
                    && code.is_dark(x + 1, y + 1) == d;
                if same {
                    score += 3;
                }
            }
        }
    }
    score + balance(dark, size * size)
}

/// Rule 4: ten points for every full five percent the share of dark
/// modules is away from half.
fn balance(dark: usize, total: usize) -> u32 {
    if total == 0 {
        return 0;
    }
    let off = (dark * 100 / total).abs_diff(50) / 5;
    10 * u32::try_from(off).unwrap_or(10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const LEVELS: [Ecc; 4] = [Ecc::Low, Ecc::Medium, Ecc::Quartile, Ecc::High];

    fn power(mut exp: usize) -> u8 {
        let mut x = 1;
        while exp > 0 {
            x = mul(x, 2);
            exp -= 1;
        }
        x
    }

    /// The value of a polynomial, highest power first, at `x`.
    fn eval(poly: &[u8], x: u8) -> u8 {
        poly.iter().fold(0, |acc, &c| mul(acc, x) ^ c)
    }

    #[test]
    fn field_multiplication() {
        for a in 0..=255u8 {
            assert_eq!(mul(a, 0), 0);
            assert_eq!(mul(a, 1), a);
            assert_eq!(mul(a, 3), mul(a, 2) ^ a, "distributes over addition");
        }
        assert_eq!(mul(0x80, 2), 0x1d, "x^8 wraps to x^4 + x^3 + x^2 + 1");
        assert_eq!(mul(0x53, 0xca), mul(0xca, 0x53));
        assert_eq!(power(255), 1, "2 generates the multiplicative group");
        assert!((1..255).all(|e| power(e) != 1));
    }

    #[test]
    fn generators_vanish_at_their_roots() {
        for degree in [7, 10, 13, 17, 22, 30] {
            let g = generator(degree);
            let mut full = vec![1];
            full.extend_from_slice(&g[..degree]);
            for i in 0..degree {
                assert_eq!(eval(&full, power(i)), 0, "degree {degree}, root {i}");
            }
            assert_ne!(
                eval(&full, power(degree)),
                0,
                "degree {degree} has no more roots"
            );
        }
    }

    #[test]
    fn codewords_are_multiples_of_the_generator() {
        let data: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37) ^ 0x5a).collect();
        for degree in [7, 18, 30] {
            let g = generator(degree);
            let mut rem = vec![0; degree];
            remainder(&data, &g[..degree], &mut rem);
            let mut word = data.clone();
            word.extend_from_slice(&rem);
            for i in 0..degree {
                assert_eq!(eval(&word, power(i)), 0, "degree {degree}, root {i}");
            }
        }
    }

    #[test]
    fn format_bits_match_the_standard() {
        assert_eq!(format_bits(Ecc::Medium, 0), 0x5412, "all zeros masked");
        assert_eq!(format_bits(Ecc::Low, 0), 0b111_0111_1100_0100);
        for ecc in LEVELS {
            for mask in 0..8 {
                let raw = format_bits(ecc, mask) ^ 0x5412;
                let mut rem = raw;
                for shift in (10..15).rev() {
                    if rem >> shift & 1 != 0 {
                        rem ^= 0x537 << (shift - 10);
                    }
                }
                assert_eq!(rem, 0, "{ecc:?} {mask}: the BCH code divides");
                assert_eq!(raw >> 10, ecc.format_bits() << 3 | u32::from(mask));
            }
        }
    }

    #[test]
    fn sizes_and_capacities_match_the_standard() {
        assert_eq!([1, 2, 7, 40].map(raw_codewords), [26, 44, 196, 3706]);
        assert_eq!(LEVELS.map(|e| data_codewords(1, e)), [19, 16, 13, 9]);
        assert_eq!(
            LEVELS.map(|e| data_codewords(40, e)),
            [2956, 2334, 1666, 1276]
        );
        assert_eq!(raw_modules(1) % 8, 0);
        assert_eq!(raw_modules(2) % 8, 7, "seven remainder bits");
    }

    #[test]
    fn alignment_patterns_sit_where_the_standard_puts_them() {
        let cases: [(u8, &[usize]); 6] = [
            (2, &[6, 18]),
            (7, &[6, 22, 38]),
            (14, &[6, 26, 46, 66]),
            (15, &[6, 26, 48, 70]),
            (32, &[6, 34, 60, 86, 112, 138]),
            (40, &[6, 30, 58, 86, 114, 142, 170]),
        ];
        for (version, want) in cases {
            let mut got = [0; 7];
            let n = alignment_positions(version, &mut got);
            assert_eq!(&got[..n], want, "version {version}");
        }
        assert_eq!(alignment_positions(1, &mut [0; 7]), 0);
    }

    /// The most bytes version `v` holds at `ecc`.
    fn capacity(v: u8, ecc: Ecc) -> usize {
        let count_bits = if v <= 9 { 8 } else { 16 };
        (data_codewords(v, ecc) * 8 - 4 - count_bits) / 8
    }

    #[test]
    fn the_smallest_version_that_fits_is_chosen() {
        let mut code = Code::new();
        assert_eq!(LEVELS.map(|e| capacity(1, e)), [17, 14, 11, 7]);
        for ecc in LEVELS {
            for v in [1u8, 2, 9, 10, 26, 39] {
                encode(&vec![b'a'; capacity(v, ecc)], ecc, &mut code).unwrap();
                assert_eq!(
                    (code.version(), code.size()),
                    (v, usize::from(v) * 4 + 17),
                    "{ecc:?}"
                );
                encode(&vec![b'a'; capacity(v, ecc) + 1], ecc, &mut code).unwrap();
                assert_eq!(code.version(), v + 1, "{ecc:?}, one byte more");
            }
        }
        assert_eq!(capacity(40, Ecc::Low), 2953);
        assert_eq!(encode(&[0; 2954], Ecc::Low, &mut code), Err(Error::TooLong));
        assert_eq!(
            encode(&vec![0; 70_000], Ecc::Low, &mut code),
            Err(Error::TooLong)
        );
        encode(&[], Ecc::High, &mut code).unwrap();
        assert_eq!(code.version(), 1);
    }

    #[test]
    fn the_fixed_patterns_are_in_place() {
        let mut code = Code::new();
        encode(b"Meridian", Ecc::Medium, &mut code).unwrap();
        let s = code.size();
        for (cx, cy) in [(3, 3), (s - 4, 3), (3, s - 4)] {
            assert!(code.is_dark(cx, cy), "finder centre");
            assert!(!code.is_dark(cx + 2, cy), "the light ring");
            assert!(code.is_dark(cx + 3, cy), "the dark ring");
        }
        for i in 8..s - 8 {
            assert_eq!(code.is_dark(i, 6), i % 2 == 0, "timing row at {i}");
            assert_eq!(code.is_dark(6, i), i % 2 == 0, "timing column at {i}");
        }
        assert!(code.is_dark(8, s - 8), "the dark module");
        assert!(
            !code.is_dark(s, 0) && !code.is_dark(0, s),
            "outside is light"
        );
    }

    #[test]
    fn version_information_from_version_7_on() {
        let mut code = Code::new();
        encode(&vec![b'v'; capacity(7, Ecc::Low)], Ecc::Low, &mut code).unwrap();
        let s = code.size();
        let mut bits = 0u32;
        for i in 0..18 {
            let (a, b) = (s - 11 + i % 3, i / 3);
            assert_eq!(
                code.is_dark(a, b),
                code.is_dark(b, a),
                "both copies, bit {i}"
            );
            bits |= u32::from(code.is_dark(a, b)) << i;
        }
        assert_eq!(bits, 0x07c94, "version 7 as the standard spells it");
    }

    #[test]
    fn the_chosen_mask_has_the_lowest_penalty() {
        let mut code = Code::new();
        encode(
            b"https://github.com/Elchi-dev/meridian",
            Ecc::Quartile,
            &mut code,
        )
        .unwrap();
        let chosen = code.mask();
        apply_mask(&mut code, chosen);
        let mut scores = [0; 8];
        for (mask, score) in (0u8..).zip(&mut scores) {
            apply_mask(&mut code, mask);
            draw_format_bits(&mut code, mask);
            *score = penalty(&code);
            apply_mask(&mut code, mask);
        }
        let low = *scores.iter().min().unwrap();
        assert_eq!(scores[usize::from(chosen)], low, "{scores:?}");
        assert_eq!(
            scores.iter().position(|&s| s == low),
            Some(usize::from(chosen)),
            "the first of equals"
        );
    }

    #[test]
    fn penalty_rules() {
        let mut code = Code::new();
        code.size = 21;
        // All light: runs along every line and column, 2 by 2 blocks
        // everywhere, and no dark modules at all.
        let runs = 2 * 21 * (3 + 16);
        let blocks = 3 * 20 * 20;
        assert_eq!(penalty(&code), runs + blocks + 10 * 10);
        for y in 0..21 {
            for x in 0..21 {
                code.set(x, y, (x + y) % 2 == 0);
            }
        }
        assert_eq!(penalty(&code), 0, "a checkerboard is perfect");
        for (x, &d) in [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0].iter().enumerate() {
            code.set(x + 3, 10, d == 1);
        }
        assert!(penalty(&code) >= 40, "a finder lookalike costs 40");
    }

    #[test]
    fn a_finder_lookalike_counts_at_either_end_of_a_line() {
        let lookalike = [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0];
        let score = |at: &dyn Fn(usize) -> (usize, usize)| {
            let mut code = Code::new();
            code.size = 21;
            for y in 0..21 {
                for x in 0..21 {
                    code.set(x, y, (x + y) % 2 == 0);
                }
            }
            for (i, &d) in lookalike.iter().enumerate() {
                let (x, y) = at(i);
                code.set(x, y, d == 1);
            }
            penalty(&code)
        };
        // The checkerboard looks the same mirrored or turned, so each of
        // these is the first one seen from another side.
        let first = score(&|i| (i, 10));
        assert!(first >= 40);
        assert_eq!(score(&|i| (20 - i, 10)), first, "at the end of a row");
        assert_eq!(score(&|i| (10, i)), first, "at the top of a column");
        assert_eq!(score(&|i| (10, 20 - i)), first, "at the bottom of a column");
    }

    #[test]
    fn nothing_is_drawn_outside_the_code() {
        let mut code = Code::new();
        for len in [0, 100, 1000, 2953] {
            encode(&vec![0x5a; len], Ecc::Low, &mut code).unwrap();
            let inside = code.size * code.size;
            for at in inside..WORDS * 64 {
                assert!(
                    !get(&code.dark, at) && !get(&code.function, at),
                    "version {}: bit {at} past the last module",
                    code.version
                );
            }
        }
    }

    fn require_tool(name: &str) -> bool {
        if Command::new(name).arg("--version").output().is_ok() {
            return true;
        }
        assert!(
            std::env::var("SEXTANT_REQUIRE_TOOLS").as_deref() != Ok("1"),
            "{name} is required but missing"
        );
        false
    }

    /// Reads the code with zbar, as a phone would read it off the screen.
    fn zbar_reads(code: &Code, file: &str) -> Vec<u8> {
        let (scale, quiet) = (4, 4);
        let side = (code.size() + 2 * quiet) * scale;
        let mut pgm = format!("P5\n{side} {side}\n255\n").into_bytes();
        for y in 0..side {
            for x in 0..side {
                let (mx, my) = (
                    (x / scale).wrapping_sub(quiet),
                    (y / scale).wrapping_sub(quiet),
                );
                pgm.push(if code.is_dark(mx, my) { 0 } else { 255 });
            }
        }
        let path =
            std::env::temp_dir().join(format!("meridian-qr-{}-{file}.pgm", std::process::id()));
        std::fs::write(&path, pgm).unwrap();
        let out = Command::new("zbarimg")
            .args(["-q", "--raw", "-Sbinary"])
            .arg(&path)
            .output()
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        out.stdout
    }

    #[test]
    fn zbar_reads_every_version_at_every_level() {
        if !require_tool("zbarimg") {
            return;
        }
        let mut code = Code::new();
        for ecc in LEVELS {
            for v in (1..=40u8).step_by(3).chain([40]) {
                let data: Vec<u8> = (0..capacity(v, ecc))
                    .map(|i| b"Meridian panic 0123456789"[i % 25])
                    .collect();
                encode(&data, ecc, &mut code).unwrap();
                assert_eq!(code.version(), v);
                let read = zbar_reads(&code, &format!("{v}-{ecc:?}"));
                assert_eq!(read, data, "version {v} at {ecc:?}");
            }
        }
    }

    #[test]
    fn zbar_reads_every_mask() {
        if !require_tool("zbarimg") {
            return;
        }
        let mut code = Code::new();
        let mut seen = [false; 8];
        for n in 0..200u32 {
            let text = format!("crash {n} at 0x{:x}", n * 7919);
            encode(text.as_bytes(), Ecc::Medium, &mut code).unwrap();
            let mask = usize::from(code.mask());
            if !seen[mask] {
                seen[mask] = true;
                assert_eq!(
                    zbar_reads(&code, &format!("mask{mask}")),
                    text.as_bytes(),
                    "mask {mask}"
                );
            }
        }
        assert!(seen.iter().filter(|&&s| s).count() >= 6, "{seen:?}");
    }

    /// The matrix qrencode makes for the same bytes in byte mode, read
    /// from its text output, where each module is two characters.
    fn qrencode(data: &[u8], ecc: Ecc) -> Vec<Vec<bool>> {
        use std::io::Write;
        let level = ["L", "M", "Q", "H"][ecc.index()];
        let mut child = Command::new("qrencode")
            .args(["-t", "ASCII", "-m", "0", "-8", "-l", level])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(data).unwrap();
        let out = child.wait_with_output().unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.as_bytes().chunks(2).map(|m| m[0] == b'#').collect())
            .collect()
    }

    fn matrix(code: &Code) -> Vec<Vec<bool>> {
        (0..code.size())
            .map(|y| (0..code.size()).map(|x| code.is_dark(x, y)).collect())
            .collect()
    }

    /// Puts another mask on a finished code.
    fn remask(code: &mut Code, mask: u8) {
        let old = code.mask;
        apply_mask(code, old);
        apply_mask(code, mask);
        draw_format_bits(code, mask);
        code.mask = mask;
    }

    /// Every module matches qrencode once both use the same mask, which
    /// checks the data layout, the error correction and every pattern
    /// against a separate implementation. Masks are chosen by penalty, and
    /// the two may weigh a borderline case differently, so the mask is
    /// taken from qrencode's matrix.
    #[test]
    fn every_module_matches_qrencode() {
        if !require_tool("qrencode") {
            return;
        }
        let mut code = Code::new();
        for ecc in LEVELS {
            for v in 1..=40u8 {
                for len in [capacity(v, ecc), capacity(v, ecc) * 2 / 3] {
                    let data: Vec<u8> = (0..len)
                        .map(|i| u8::try_from(i * 31 % 95).unwrap() + 0x20 + v % 2)
                        .collect();
                    encode(&data, ecc, &mut code).unwrap();
                    let theirs = qrencode(&data, ecc);
                    assert_eq!(
                        theirs.len(),
                        code.size(),
                        "{ecc:?} version {v}, {len} bytes"
                    );
                    let matches = (0..8).any(|m| {
                        remask(&mut code, m);
                        matrix(&code) == theirs
                    });
                    assert!(matches, "{ecc:?} version {v}, {len} bytes: no mask matches");
                }
            }
        }
    }

    fn data_words(data: &[u8], version: u8, ecc: Ecc) -> Vec<u8> {
        let mut out = vec![0; data_codewords(version, ecc)];
        fill_data(data, version, &mut out);
        out
    }

    #[test]
    fn data_starts_with_mode_and_length() {
        // 0100, then 2 in 8 bits, then 'A' and 'B', then a 4 bit terminator.
        let words = data_words(b"AB", 1, Ecc::Medium);
        assert_eq!(words.len(), 16);
        assert_eq!(&words[..4], [0x40, 0x24, 0x14, 0x20]);
        assert_eq!(&words[4..8], [0xec, 0x11, 0xec, 0x11], "padding alternates");
        assert_eq!(words[15], 0x11);
        assert_eq!(&data_words(b"", 1, Ecc::High)[..3], [0x40, 0x00, 0xec]);
    }

    #[test]
    fn a_full_code_ends_without_padding() {
        // Seven bytes fill version 1 at the high level: 4 + 8 + 56 bits and
        // the terminator make exactly 9 codewords.
        let words = data_words(b"1234567", 1, Ecc::High);
        assert_eq!(words.len(), 9);
        assert_eq!(words[0], 0x40);
        assert_eq!(
            words[1] & 0xf0,
            0x70,
            "the length, 7, straddles the first two words"
        );
        assert_eq!(words[8] & 0x0f, 0, "the terminator");
        assert_ne!(words[8], 0xec, "no padding");
    }

    #[test]
    fn long_codes_count_in_16_bits() {
        let data = vec![0xff; 200];
        let words = data_words(&data, 10, Ecc::Low);
        // 0100, then 200 = 0x00c8 in 16 bits, then the bytes.
        assert_eq!(&words[..4], [0x40, 0x0c, 0x8f, 0xff]);
        assert_eq!(
            bits_needed(255, 9),
            4 + 8 + 8 * 255,
            "versions up to 9 count in 8 bits"
        );
        assert_eq!(
            bits_needed(300, 10),
            4 + 16 + 8 * 300,
            "from version 10 in 16"
        );
        assert_eq!(
            bits_needed(256, 9),
            usize::MAX,
            "256 does not fit an 8 bit count"
        );
        assert_eq!(bits_needed(65_536, 10), usize::MAX);
    }

    #[test]
    fn bits_fill_bytes_from_the_top() {
        let mut out = [0u8; 3];
        let mut bits = Bits {
            out: &mut out,
            len: 0,
        };
        bits.push(0b101, 3);
        bits.push(0b1, 1);
        bits.push(0xff, 6);
        bits.push(0, 2);
        bits.push(0b11, 2);
        assert_eq!(bits.len, 14);
        assert_eq!(out, [0b1011_1111, 0b1100_1100, 0]);
    }

    /// Version 5 at the quartile level has two blocks of 15 data codewords
    /// and two of 16, each with 18 error correction codewords.
    #[test]
    fn blocks_are_interleaved_short_ones_first() {
        assert_eq!(
            (BLOCKS[2][4], ECC_PER_BLOCK[2][4], raw_codewords(5)),
            (4, 18, 134)
        );
        let data: Vec<u8> = (0..62).collect();
        let mut out = vec![0; 134];
        add_error_correction(&data, 5, Ecc::Quartile, &mut out);
        assert_eq!(&out[..8], [0, 15, 30, 46, 1, 16, 31, 47]);
        assert_eq!(
            &out[56..62],
            [14, 29, 44, 60, 45, 61],
            "the long blocks' last codewords come alone"
        );
        let g = generator(18);
        for (block, range) in [(0, 0..15), (1, 15..30), (2, 30..46), (3, 46..62)] {
            let mut ecc = [0; 18];
            remainder(&data[range], &g[..18], &mut ecc);
            for (i, &e) in ecc.iter().enumerate() {
                assert_eq!(out[62 + i * 4 + block], e, "block {block}, codeword {i}");
            }
        }
    }

    #[test]
    fn a_single_block_is_data_then_error_correction() {
        let data: Vec<u8> = (100..119).collect();
        let mut out = vec![0; 26];
        add_error_correction(&data, 1, Ecc::Low, &mut out);
        assert_eq!(&out[..19], data.as_slice());
        let mut ecc = [0; 7];
        remainder(&data, &generator(7)[..7], &mut ecc);
        assert_eq!(&out[19..], ecc);
    }

    /// A version 1 code with its patterns and every data module light.
    fn blank(version: u8) -> Code {
        let mut code = Code::new();
        code.size = usize::from(version) * 4 + 17;
        code.version = version;
        draw_function_patterns(&mut code);
        code
    }

    #[test]
    fn bits_go_up_and_down_in_pairs_of_columns() {
        let order = [
            (20, 20),
            (19, 20),
            (20, 19),
            (19, 19),
            (20, 9),
            (19, 9),
            (18, 9),
            (17, 9),
            (18, 20),
            (14, 9),
        ];
        let positions = [0, 1, 2, 3, 22, 23, 24, 25, 46, 72];
        for (&bit, &(x, y)) in positions.iter().zip(&order) {
            let mut code = blank(1);
            let mut words = [0u8; 26];
            words[bit / 8] = 0x80 >> (bit % 8);
            place(&mut code, &words);
            let dark: Vec<_> = (0..21)
                .flat_map(|y| (0..21).map(move |x| (x, y)))
                .filter(|&(x, y)| !code.is_function(x, y) && code.is_dark(x, y))
                .collect();
            assert_eq!(dark, [(x, y)], "bit {bit}");
        }
    }

    #[test]
    fn patterns_are_never_written_over() {
        let mut code = blank(1);
        let patterns = matrix(&code);
        place(&mut code, &[0xff; 26]);
        let mut data = 0;
        for (y, row) in patterns.iter().enumerate() {
            for (x, &was) in row.iter().enumerate() {
                if code.is_function(x, y) {
                    assert_eq!(code.is_dark(x, y), was, "({x}, {y})");
                } else {
                    assert!(code.is_dark(x, y), "({x}, {y}) got no bit");
                    data += 1;
                }
            }
        }
        assert_eq!(data, 26 * 8, "version 1 has no leftover modules");
        assert!(
            (0..21).all(|y| code.is_function(6, y)),
            "column 6 is all pattern"
        );
    }

    #[test]
    fn leftover_modules_stay_light() {
        // Version 2 has 359 data modules for 44 codewords: 7 are left over.
        assert_eq!(raw_modules(2), 44 * 8 + 7);
        let mut code = blank(2);
        place(&mut code, &[0xff; 44]);
        let light = (0..25)
            .flat_map(|y| (0..25).map(move |x| (x, y)))
            .filter(|&(x, y)| !code.is_function(x, y) && !code.is_dark(x, y))
            .count();
        assert_eq!(light, 7);
    }

    #[test]
    fn masks_repeat_with_their_periods() {
        // How many modules of a 12 by 12 tile each mask flips; every mask
        // repeats after 12 modules both ways.
        let counts = [72, 72, 48, 48, 72, 60, 92, 72];
        for mask in 0..8u8 {
            let tile = (0..12).flat_map(|y| (0..12).map(move |x| (x, y)));
            let hits = tile.filter(|&(x, y)| mask_hits(mask, x, y)).count();
            assert_eq!(hits, counts[usize::from(mask)], "mask {mask}");
            for y in 0..12 {
                for x in 0..12 {
                    assert_eq!(
                        mask_hits(mask, x, y),
                        mask_hits(mask, x + 12, y + 12),
                        "mask {mask}"
                    );
                }
            }
        }
        assert!(mask_hits(0, 0, 0) && !mask_hits(0, 1, 0));
        assert!(mask_hits(1, 5, 0) && !mask_hits(1, 0, 1));
        assert!(mask_hits(2, 3, 1) && !mask_hits(2, 1, 3));
        assert!(mask_hits(3, 1, 2) && !mask_hits(3, 1, 1));
        assert!(mask_hits(4, 2, 1) && !mask_hits(4, 3, 0));
        assert!(mask_hits(5, 3, 2) && !mask_hits(5, 1, 1));
    }

    #[test]
    fn masking_twice_restores_and_spares_the_patterns() {
        let mut code = Code::new();
        encode(b"mask me twice", Ecc::Quartile, &mut code).unwrap();
        let before = matrix(&code);
        for mask in 0..8 {
            apply_mask(&mut code, mask);
            for (y, row) in before.iter().enumerate() {
                for (x, &was) in row.iter().enumerate() {
                    if code.is_function(x, y) {
                        assert_eq!(code.is_dark(x, y), was, "mask {mask} touched a pattern");
                    }
                }
            }
            apply_mask(&mut code, mask);
            assert_eq!(matrix(&code), before, "mask {mask}");
        }
    }

    /// A 21 by 21 checkerboard, whose penalty is 0, with some modules set.
    fn board(dark: &[(usize, usize)], light: &[(usize, usize)]) -> u32 {
        let mut code = Code::new();
        code.size = 21;
        for y in 0..21 {
            for x in 0..21 {
                code.set(x, y, (x + y) % 2 == 0);
            }
        }
        for &(x, y) in dark {
            code.set(x, y, true);
        }
        for &(x, y) in light {
            code.set(x, y, false);
        }
        penalty(&code)
    }

    #[test]
    fn each_penalty_rule_on_its_own() {
        assert_eq!(board(&[], &[]), 0);
        // Row 0 is dark at even columns; filling 1 and 3 makes a run of 5.
        assert_eq!(board(&[(1, 0), (3, 0)], &[]), 3, "a run of five");
        assert_eq!(
            board(&[(1, 0), (3, 0), (5, 0)], &[]),
            3 + 2,
            "a run of seven"
        );
        // Darkening (1, 0) and (0, 1) makes the top left 2 by 2 all dark.
        assert_eq!(board(&[(1, 0), (0, 1)], &[]), 3, "one block");
        assert_eq!(
            board(&[(1, 0), (0, 1), (11, 10), (10, 11)], &[]),
            3 + 3,
            "two blocks"
        );
        let column: Vec<_> = (2..7).map(|y| (10, y)).collect();
        assert_eq!(board(&column, &[]), 3, "runs count down columns too");
    }

    /// The modules of `pattern` in row 4 from column 2, on a checkerboard.
    fn row_pattern(pattern: &[u8]) -> u32 {
        let (mut dark, mut light) = (Vec::new(), Vec::new());
        for (i, &p) in pattern.iter().enumerate() {
            if p == 1 {
                dark.push((2 + i, 4));
            } else {
                light.push((2 + i, 4));
            }
        }
        board(&dark, &light)
    }

    #[test]
    fn a_finder_lookalike_costs_forty() {
        let lookalike = row_pattern(&[1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0]);
        let mirrored = row_pattern(&[0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1]);
        let near_miss = row_pattern(&[1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 1]);
        assert!(lookalike >= 40, "{lookalike}");
        assert_eq!(lookalike, mirrored, "the mirror image costs the same");
        assert!(
            near_miss < 40,
            "three light modules are not enough: {near_miss}"
        );
        assert!(
            lookalike - near_miss >= 40,
            "the lookalike alone costs 40 more"
        );
    }

    #[test]
    fn an_uneven_share_of_dark_costs_ten_per_five_percent() {
        let cases = [
            (50, 0),
            (54, 0),
            (55, 10),
            (60, 20),
            (46, 0),
            (45, 10),
            (44, 10),
            (0, 100),
            (100, 100),
        ];
        for (dark, cost) in cases {
            assert_eq!(balance(dark, 100), cost, "{dark} of 100 dark");
        }
        assert_eq!(balance(221, 441), 0, "a 21 by 21 checkerboard");
        assert_eq!(balance(0, 0), 0, "nothing to weigh");
    }

    fn distance(a: u32, b: u32) -> u32 {
        (a ^ b).count_ones()
    }

    #[test]
    fn format_codes_are_far_apart() {
        let codes: Vec<u32> = LEVELS
            .iter()
            .flat_map(|&e| (0..8).map(move |m| format_bits(e, m)))
            .collect();
        for (i, &a) in codes.iter().enumerate() {
            for &b in &codes[i + 1..] {
                assert!(distance(a, b) >= 7, "{a:015b} and {b:015b}");
            }
        }
    }

    /// The 18 version bits as the code draws them.
    fn version_bits(version: u8) -> u32 {
        let code = blank(version);
        let s = code.size();
        (0..18).fold(0, |acc, i| {
            acc | u32::from(code.is_dark(s - 11 + i % 3, i / 3)) << i
        })
    }

    #[test]
    fn version_codes_divide_and_are_far_apart() {
        let codes: Vec<u32> = (7..=40).map(version_bits).collect();
        for (v, &code) in (7u32..).zip(&codes) {
            assert_eq!(code >> 12, v);
            let mut rem = code;
            for shift in (12..18).rev() {
                if rem >> shift & 1 != 0 {
                    rem ^= 0x1f25 << (shift - 12);
                }
            }
            assert_eq!(rem, 0, "version {v}");
        }
        for (i, &a) in codes.iter().enumerate() {
            for &b in &codes[i + 1..] {
                assert!(distance(a, b) >= 8, "{a:018b} and {b:018b}");
            }
        }
        assert_eq!(
            version_bits(40),
            0x28c69,
            "version 40 as the standard spells it"
        );
    }

    #[test]
    fn the_seven_codeword_generator_from_the_standard() {
        let exponents = [87, 229, 146, 149, 238, 102, 21];
        assert_eq!(generator(7)[..7], exponents.map(power));
    }

    #[test]
    fn zbar_reads_every_byte_value() {
        if !require_tool("zbarimg") {
            return;
        }
        let data: Vec<u8> = (0..=255).collect();
        let mut code = Code::new();
        encode(&data, Ecc::Medium, &mut code).unwrap();
        assert_eq!(zbar_reads(&code, "bytes"), data);
    }

    #[test]
    fn accessors_report_the_encoding() {
        let mut code = Code::default();
        assert_eq!((code.size(), code.version()), (0, 0));
        assert!(!code.is_dark(0, 0));
        encode(&[b'x'; 40], Ecc::High, &mut code).unwrap();
        assert_eq!(
            (code.version(), code.ecc(), code.size()),
            (5, Ecc::High, 37)
        );
        assert!(code.mask() < 8);
    }

    #[test]
    fn a_used_code_is_cleared_before_the_next() {
        let mut used = Code::new();
        encode(&[b'z'; 900], Ecc::Low, &mut used).unwrap();
        encode(b"small", Ecc::Low, &mut used).unwrap();
        let mut fresh = Code::new();
        encode(b"small", Ecc::Low, &mut fresh).unwrap();
        assert_eq!(matrix(&used), matrix(&fresh));
        assert_eq!(
            (used.version(), used.mask()),
            (fresh.version(), fresh.mask())
        );
        assert!(!used.is_dark(30, 30), "nothing is left of the larger code");
    }

    /// Bytes above 127 and below 32 travel in byte mode like any other.
    #[test]
    fn binary_data_matches_qrencode() {
        if !require_tool("qrencode") {
            return;
        }
        let mut code = Code::new();
        for (seed, ecc) in [
            (7u32, Ecc::Low),
            (11, Ecc::Medium),
            (13, Ecc::Quartile),
            (17, Ecc::High),
        ] {
            let mut x = seed;
            let data: Vec<u8> = (0..150)
                .map(|_| {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    x.to_le_bytes()[2]
                })
                .filter(|&b| b != 0)
                .collect();
            encode(&data, ecc, &mut code).unwrap();
            let theirs = qrencode(&data, ecc);
            let matches = (0..8).any(|m| {
                remask(&mut code, m);
                matrix(&code) == theirs
            });
            assert!(matches, "{ecc:?}: no mask matches");
        }
    }

    #[test]
    fn an_empty_message_has_only_padding_behind_its_header() {
        let words = data_words(b"", 1, Ecc::Low);
        assert_eq!(&words[..2], [0x40, 0x00]);
        let pads: Vec<u8> = words[2..].to_vec();
        assert!(
            pads.chunks(2)
                .all(|p| p[0] == 0xec && p.get(1).is_none_or(|&b| b == 0x11)),
            "{pads:x?}"
        );
        let mut rem = [0u8; 7];
        remainder(&[], &generator(7)[..7], &mut rem);
        assert_eq!(rem, [0; 7], "nothing divides to nothing");
    }

    /// When two masks score the same, the first of them is used, so the
    /// same data always gives the same code.
    #[test]
    fn ties_go_to_the_lower_mask() {
        let mut code = Code::new();
        let mut ties = 0;
        for n in 0..400u32 {
            let text = format!("tie {n}");
            encode(text.as_bytes(), Ecc::Low, &mut code).unwrap();
            let chosen = code.mask();
            let mut scores = [0; 8];
            for (mask, score) in (0u8..).zip(&mut scores) {
                remask(&mut code, mask);
                *score = penalty(&code);
            }
            let low = *scores.iter().min().unwrap();
            if scores.iter().filter(|&&s| s == low).count() > 1 {
                ties += 1;
                assert_eq!(
                    scores.iter().position(|&s| s == low),
                    Some(usize::from(chosen)),
                    "{text}: {scores:?}"
                );
            }
        }
        assert!(
            ties > 0,
            "no two masks tied in 400 tries, so this test showed nothing"
        );
    }
}
