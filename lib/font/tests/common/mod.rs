//! Helpers the test files share: exact coverage by clipping, a sink that
//! records outlines, a reader for font files that knows nothing of the
//! crate, and a small random number generator.

// Each test file uses a different part of this module.
#![allow(dead_code)]

pub mod build;

use font::{OutlineSink, Point};

/// The exact area of a set of polygons inside pixel `(x, y)`, found by
/// clipping each polygon to the pixel's square. Polygons wound the other
/// way subtract, as holes do.
pub fn exact_coverage(contours: &[Vec<[f64; 2]>], x: usize, y: usize) -> f64 {
    let (x, y) = (x as f64, y as f64);
    let mut area = 0.0;
    for c in contours {
        let c = clip(c, 0, x, false);
        let c = clip(&c, 0, x + 1.0, true);
        let c = clip(&c, 1, y, false);
        let c = clip(&c, 1, y + 1.0, true);
        area += signed_area(&c);
    }
    area.abs()
}

/// Keeps the part of a polygon on one side of a line `p[axis] = bound`:
/// at or below it when `below`, else at or above it.
fn clip(poly: &[[f64; 2]], axis: usize, bound: f64, below: bool) -> Vec<[f64; 2]> {
    let inside = |p: [f64; 2]| {
        if below {
            p[axis] <= bound
        } else {
            p[axis] >= bound
        }
    };
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if inside(a) {
            out.push(a);
        }
        if inside(a) != inside(b) {
            let t = (bound - a[axis]) / (b[axis] - a[axis]);
            out.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
        }
    }
    out
}

pub fn signed_area(poly: &[[f64; 2]]) -> f64 {
    let mut twice = 0.0;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        twice += a[0] * b[1] - a[1] * b[0];
    }
    twice / 2.0
}

/// The coverage byte a pixel should get, with the rounding the crate
/// documents.
pub fn expected_byte(coverage: f64) -> u8 {
    (coverage.min(1.0) * 255.0).round() as u8
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Close,
}

/// Records every command, and keeps the outline's exact area and length.
#[derive(Default)]
pub struct Recorder {
    pub commands: Vec<Command>,
    start: Point,
    last: Point,
    twice_area: f64,
    /// Length of the straight edges, closing edges included.
    pub line_length: f64,
    /// Length of the curves' control polygons, which is at least the
    /// length of the curves.
    pub curve_length: f64,
}

fn cross(a: Point, b: Point) -> f64 {
    f64::from(a.x) * f64::from(b.y) - f64::from(a.y) * f64::from(b.x)
}

fn distance(a: Point, b: Point) -> f64 {
    f64::from(a.x - b.x).hypot(f64::from(a.y - b.y))
}

impl Recorder {
    /// The signed area the outline encloses, in its own units: exact for
    /// curves too, since the shoelace sum has a closed form for quadratic
    /// pieces.
    pub fn area(&self) -> f64 {
        self.twice_area / 2.0
    }

    pub fn points(&self) -> Vec<Point> {
        let mut out = Vec::new();
        for c in &self.commands {
            match *c {
                Command::Move(p) | Command::Line(p) => out.push(p),
                Command::Quad(c, p) => out.extend([c, p]),
                Command::Close => {}
            }
        }
        out
    }

    fn line(&mut self, p: Point) {
        self.twice_area += cross(self.last, p);
        self.line_length += distance(self.last, p);
        self.last = p;
    }
}

impl OutlineSink for Recorder {
    fn move_to(&mut self, p: Point) {
        self.commands.push(Command::Move(p));
        self.start = p;
        self.last = p;
    }

    fn line_to(&mut self, p: Point) {
        self.commands.push(Command::Line(p));
        self.line(p);
    }

    fn quad_to(&mut self, c: Point, p: Point) {
        self.commands.push(Command::Quad(c, p));
        let p0 = self.last;
        self.twice_area += (2.0 * cross(p0, c) + 2.0 * cross(c, p) + cross(p0, p)) / 3.0;
        self.curve_length += distance(p0, c) + distance(c, p);
        self.last = p;
    }

    fn close(&mut self) {
        self.commands.push(Command::Close);
        self.line(self.start);
    }
}

/// A reader for the parts of a font file the tests check the crate
/// against. It walks tables front to back where the crate searches, so a
/// shared mistake is unlikely.
pub struct Raw<'a> {
    pub data: &'a [u8],
}

impl Raw<'_> {
    pub fn u16(&self, at: usize) -> u16 {
        u16::from_be_bytes([self.data[at], self.data[at + 1]])
    }

    pub fn i16(&self, at: usize) -> i16 {
        self.u16(at) as i16
    }

    pub fn u32(&self, at: usize) -> u32 {
        u32::from_be_bytes(self.data[at..at + 4].try_into().unwrap())
    }

    /// Offset and length of a table.
    pub fn table(&self, tag: &[u8]) -> Option<(usize, usize)> {
        let count = self.u16(4) as usize;
        (0..count).map(|i| 12 + 16 * i).find_map(|r| {
            (&self.data[r..r + 4] == tag)
                .then(|| (self.u32(r + 8) as usize, self.u32(r + 12) as usize))
        })
    }

    fn at(&self, tag: &[u8]) -> usize {
        self.table(tag).unwrap().0
    }

    pub fn units_per_em(&self) -> u16 {
        self.u16(self.at(b"head") + 18)
    }

    pub fn glyph_count(&self) -> u16 {
        self.u16(self.at(b"maxp") + 4)
    }

    /// Ascender, descender and line gap from `hhea`.
    pub fn line(&self) -> [i16; 3] {
        let h = self.at(b"hhea");
        [self.i16(h + 4), self.i16(h + 6), self.i16(h + 8)]
    }

    /// Advance and left side bearing by walking `hmtx` from the start.
    pub fn h_metrics(&self, glyph: u16) -> (u16, i16) {
        let long = self.u16(self.at(b"hhea") + 34);
        let mut at = self.at(b"hmtx");
        let mut advance = 0;
        for g in 0..=glyph {
            if g < long {
                advance = self.u16(at);
                at += 2;
            }
            if g == glyph {
                return (advance, self.i16(at));
            }
            at += 2;
        }
        unreachable!()
    }

    fn subtable(&self, platform: u16, encoding: u16) -> Option<usize> {
        let cmap = self.at(b"cmap");
        let count = self.u16(cmap + 2) as usize;
        (0..count).map(|i| cmap + 4 + 8 * i).find_map(|r| {
            (self.u16(r) == platform && self.u16(r + 2) == encoding)
                .then(|| cmap + self.u32(r + 4) as usize)
        })
    }

    /// Every (character, glyph) pair of the Windows full repertoire
    /// subtable, in order.
    pub fn format12_pairs(&self) -> Vec<(u32, u16)> {
        let t = self.subtable(3, 10).unwrap();
        assert_eq!(self.u16(t), 12);
        let mut out = Vec::new();
        for g in 0..self.u32(t + 12) as usize {
            let at = t + 16 + 12 * g;
            let (first, last, glyph) = (self.u32(at), self.u32(at + 4), self.u32(at + 8));
            for c in first..=last {
                out.push((c, (glyph + c - first) as u16));
            }
        }
        out
    }

    /// Looks a character up in the Windows BMP subtable by trying every
    /// segment in turn.
    pub fn format4(&self, c: u16) -> u16 {
        let t = self.subtable(3, 1).unwrap();
        assert_eq!(self.u16(t), 4);
        let segments = self.u16(t + 6) as usize / 2;
        for s in 0..segments {
            let end = self.u16(t + 14 + 2 * s);
            let start_at = t + 16 + 2 * segments + 2 * s;
            let start = self.u16(start_at);
            if c < start || c > end {
                continue;
            }
            let delta_at = start_at + 2 * segments;
            let delta = self.u16(delta_at);
            let range_at = delta_at + 2 * segments;
            let range = self.u16(range_at) as usize;
            if range == 0 {
                return c.wrapping_add(delta);
            }
            let glyph = self.u16(range_at + range + 2 * (c - start) as usize);
            return if glyph == 0 {
                0
            } else {
                glyph.wrapping_add(delta)
            };
        }
        0
    }
}

/// xorshift64*, enough to pick places to break a font.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// A float from `lo` to `hi`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}
