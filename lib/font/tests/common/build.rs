//! Builds small TrueType fonts byte by byte, written from the
//! specification without looking at the crate's reader.

pub fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn i16_bits(v: i16) -> u16 {
    u16::from_be_bytes(v.to_be_bytes())
}

/// A point of a simple glyph: x, y, on the curve or not.
pub type P = (i16, i16, bool);

pub fn on(x: i16, y: i16) -> P {
    (x, y, true)
}

pub fn off(x: i16, y: i16) -> P {
    (x, y, false)
}

/// A simple glyph from its contours, its box taken from all points.
/// Coordinates use the shortest form each delta allows, and runs of equal
/// flags are packed with the repeat flag.
pub fn simple(contours: &[&[P]], instructions: &[u8]) -> Vec<u8> {
    let points: Vec<P> = contours.iter().flat_map(|c| c.iter().copied()).collect();
    let mut out = Vec::new();
    be16(&mut out, contours.len() as u16);
    let xs = points.iter().map(|p| p.0);
    let ys = points.iter().map(|p| p.1);
    for v in [
        xs.clone().min().unwrap_or(0),
        ys.clone().min().unwrap_or(0),
        xs.max().unwrap_or(0),
        ys.max().unwrap_or(0),
    ] {
        be16(&mut out, i16_bits(v));
    }
    let mut end = 0;
    for c in contours {
        end += c.len();
        be16(&mut out, (end - 1) as u16);
    }
    be16(&mut out, instructions.len() as u16);
    out.extend_from_slice(instructions);

    let (mut flags, mut xb, mut yb) = (Vec::new(), Vec::new(), Vec::new());
    let (mut x, mut y) = (0i32, 0i32);
    for &(px, py, is_on) in &points {
        let mut flag = u8::from(is_on);
        flag |= coordinate(i32::from(px) - x, 0x02, 0x10, &mut xb);
        flag |= coordinate(i32::from(py) - y, 0x04, 0x20, &mut yb);
        (x, y) = (i32::from(px), i32::from(py));
        flags.push(flag);
    }
    let mut i = 0;
    while i < flags.len() {
        let mut run = 1;
        while i + run < flags.len() && flags[i + run] == flags[i] && run < 256 {
            run += 1;
        }
        if run > 1 {
            out.extend_from_slice(&[flags[i] | 0x08, (run - 1) as u8]);
        } else {
            out.push(flags[i]);
        }
        i += run;
    }
    out.extend_from_slice(&xb);
    out.extend_from_slice(&yb);
    out
}

/// Writes one coordinate delta and returns the flag bits for it.
fn coordinate(delta: i32, short: u8, same: u8, out: &mut Vec<u8>) -> u8 {
    if delta == 0 {
        same
    } else if delta.abs() < 256 {
        out.push(delta.unsigned_abs() as u8);
        if delta > 0 { short | same } else { short }
    } else {
        be16(out, i16_bits(delta as i16));
        0
    }
}

/// How a component is scaled.
#[derive(Clone, Copy)]
pub enum Scale {
    None,
    Uniform(f32),
    Xy(f32, f32),
    Matrix(f32, f32, f32, f32),
}

#[derive(Clone, Copy)]
pub struct Component {
    pub glyph: u16,
    pub dx: i16,
    pub dy: i16,
    pub scale: Scale,
    /// Flags to set beyond the ones the other fields imply.
    pub flags: u16,
}

pub fn component(glyph: u16, dx: i16, dy: i16) -> Component {
    Component {
        glyph,
        dx,
        dy,
        scale: Scale::None,
        flags: 0,
    }
}

fn f2dot14(out: &mut Vec<u8>, v: f32) {
    be16(out, i16_bits((v * 16384.0).round() as i16));
}

/// A composite glyph. Offsets that fit a byte are stored as bytes.
pub fn composite(components: &[Component], bounds: [i16; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    be16(&mut out, 0xffff);
    for b in bounds {
        be16(&mut out, i16_bits(b));
    }
    for (i, c) in components.iter().enumerate() {
        let words = i8::try_from(c.dx).is_err() || i8::try_from(c.dy).is_err();
        let mut flags = 0x0002 | c.flags;
        if words {
            flags |= 0x0001;
        }
        if i + 1 < components.len() {
            flags |= 0x0020;
        }
        flags |= match c.scale {
            Scale::None => 0,
            Scale::Uniform(_) => 0x0008,
            Scale::Xy(..) => 0x0040,
            Scale::Matrix(..) => 0x0080,
        };
        be16(&mut out, flags);
        be16(&mut out, c.glyph);
        if words {
            be16(&mut out, i16_bits(c.dx));
            be16(&mut out, i16_bits(c.dy));
        } else {
            out.extend_from_slice(&[c.dx as i8 as u8, c.dy as i8 as u8]);
        }
        match c.scale {
            Scale::None => {}
            Scale::Uniform(s) => f2dot14(&mut out, s),
            Scale::Xy(x, y) => {
                f2dot14(&mut out, x);
                f2dot14(&mut out, y);
            }
            Scale::Matrix(a, b, c, d) => {
                for v in [a, b, c, d] {
                    f2dot14(&mut out, v);
                }
            }
        }
    }
    out
}

/// A format 4 segment: characters `start..=end`, mapped by adding `delta`
/// or, when `glyphs` is given, through that list.
pub struct Segment {
    pub start: u16,
    pub end: u16,
    pub delta: i16,
    pub glyphs: Option<Vec<u16>>,
}

pub fn format4(segments: &[Segment]) -> Vec<u8> {
    let n = segments.len();
    let mut out = Vec::new();
    be16(&mut out, 4);
    be16(&mut out, 0);
    be16(&mut out, 0);
    be16(&mut out, (2 * n) as u16);
    // Search hints the reader does not need.
    for _ in 0..3 {
        be16(&mut out, 0);
    }
    for s in segments {
        be16(&mut out, s.end);
    }
    be16(&mut out, 0);
    for s in segments {
        be16(&mut out, s.start);
    }
    for s in segments {
        be16(&mut out, i16_bits(s.delta));
    }
    let mut array: Vec<u16> = Vec::new();
    for (i, s) in segments.iter().enumerate() {
        match &s.glyphs {
            Some(g) => {
                be16(&mut out, (2 * (n - i) + 2 * array.len()) as u16);
                array.extend(g);
            }
            None => be16(&mut out, 0),
        }
    }
    for g in array {
        be16(&mut out, g);
    }
    let len = out.len() as u16;
    out[2..4].copy_from_slice(&len.to_be_bytes());
    out
}

/// A format 12 subtable from (first, last, first glyph) groups.
pub fn format12(groups: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    be16(&mut out, 12);
    be16(&mut out, 0);
    be32(&mut out, (16 + 12 * groups.len()) as u32);
    be32(&mut out, 0);
    be32(&mut out, groups.len() as u32);
    for &(first, last, glyph) in groups {
        be32(&mut out, first);
        be32(&mut out, last);
        be32(&mut out, glyph);
    }
    out
}

/// A `cmap` table from (platform, encoding, subtable) records.
pub fn cmap(subtables: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    be16(&mut out, 0);
    be16(&mut out, subtables.len() as u16);
    let mut offset = 4 + 8 * subtables.len();
    for (platform, encoding, sub) in subtables {
        be16(&mut out, *platform);
        be16(&mut out, *encoding);
        be32(&mut out, offset as u32);
        offset += sub.len();
    }
    for (_, _, sub) in subtables {
        out.extend_from_slice(sub);
    }
    out
}

/// A `kern` table with one format 0 subtable. Pairs are sorted here.
pub fn kern(pairs: &[(u16, u16, i16)], coverage: u16) -> Vec<u8> {
    let mut pairs = pairs.to_vec();
    pairs.sort_by_key(|p| (p.0, p.1));
    let mut out = Vec::new();
    be16(&mut out, 0);
    be16(&mut out, 1);
    be16(&mut out, 0);
    be16(&mut out, (14 + 6 * pairs.len()) as u16);
    be16(&mut out, coverage);
    be16(&mut out, pairs.len() as u16);
    for _ in 0..3 {
        be16(&mut out, 0);
    }
    for (l, r, v) in pairs {
        be16(&mut out, l);
        be16(&mut out, r);
        be16(&mut out, i16_bits(v));
    }
    out
}

pub struct Glyph {
    pub advance: u16,
    pub lsb: i16,
    pub data: Vec<u8>,
}

pub struct FontBuilder {
    pub version: u32,
    pub units_per_em: u16,
    pub long_loca: bool,
    /// Full metric records; the remaining glyphs get only side bearings.
    pub h_metrics: u16,
    pub ascent: i16,
    pub descent: i16,
    pub line_gap: i16,
    pub glyphs: Vec<Glyph>,
    pub cmap: Vec<u8>,
    pub kern: Option<Vec<u8>>,
}

impl FontBuilder {
    pub fn new(glyphs: Vec<Glyph>, cmap: Vec<u8>) -> FontBuilder {
        FontBuilder {
            version: 0x0001_0000,
            units_per_em: 1000,
            long_loca: false,
            h_metrics: glyphs.len() as u16,
            ascent: 800,
            descent: -200,
            line_gap: 90,
            glyphs,
            cmap,
            kern: None,
        }
    }

    /// The tables in the order a font file lists them, by tag.
    pub fn tables(&self) -> Vec<([u8; 4], Vec<u8>)> {
        let mut head = Vec::new();
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0);
        be32(&mut head, 0x5f0f_3cf5);
        be16(&mut head, 0);
        be16(&mut head, self.units_per_em);
        head.extend_from_slice(&[0; 16]);
        head.extend_from_slice(&[0; 8]);
        be16(&mut head, 0);
        be16(&mut head, 8);
        be16(&mut head, 2);
        be16(&mut head, u16::from(self.long_loca));
        be16(&mut head, 0);

        let mut maxp = Vec::new();
        be32(&mut maxp, 0x0000_5000);
        be16(&mut maxp, self.glyphs.len() as u16);

        let mut hhea = Vec::new();
        be32(&mut hhea, 0x0001_0000);
        for v in [self.ascent, self.descent, self.line_gap] {
            be16(&mut hhea, i16_bits(v));
        }
        hhea.extend_from_slice(&[0; 24]);
        be16(&mut hhea, self.h_metrics);

        let mut hmtx = Vec::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            if i < usize::from(self.h_metrics) {
                be16(&mut hmtx, g.advance);
            }
            be16(&mut hmtx, i16_bits(g.lsb));
        }

        let (mut glyf, mut loca) = (Vec::new(), Vec::new());
        let entry = |loca: &mut Vec<u8>, at: usize| {
            if self.long_loca {
                be32(loca, at as u32);
            } else {
                be16(loca, (at / 2) as u16);
            }
        };
        for g in &self.glyphs {
            entry(&mut loca, glyf.len());
            glyf.extend_from_slice(&g.data);
            // The short format counts in pairs of bytes.
            if !self.long_loca && glyf.len() % 2 != 0 {
                glyf.push(0);
            }
        }
        entry(&mut loca, glyf.len());

        let mut tables = vec![
            (*b"cmap", self.cmap.clone()),
            (*b"glyf", glyf),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
            (*b"maxp", maxp),
        ];
        if let Some(k) = &self.kern {
            tables.push((*b"kern", k.clone()));
        }
        tables
    }

    pub fn build(&self) -> Vec<u8> {
        assemble(self.version, &self.tables())
    }
}

/// A font file from its tables, each starting on four bytes.
pub fn assemble(version: u32, tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    be32(&mut out, version);
    be16(&mut out, tables.len() as u16);
    for _ in 0..3 {
        be16(&mut out, 0);
    }
    let mut offset = 12 + 16 * tables.len();
    for (tag, data) in tables {
        out.extend_from_slice(tag);
        be32(&mut out, 0);
        be32(&mut out, offset as u32);
        be32(&mut out, data.len() as u32);
        offset += data.len().next_multiple_of(4);
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}
