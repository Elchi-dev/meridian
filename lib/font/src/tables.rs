//! Reading the TrueType tables Meridian uses.

use core::cmp::Ordering;

use crate::math::{ceil, floor};
use crate::raster::PixelSink;
use crate::read::{f2dot14_at, i8_at, i16_at, slice, u8_at, u16_at, u32_at};
use crate::{
    Error, LineMetrics, MAX_COMPONENT_DEPTH, MAX_COMPONENTS, MAX_PIXEL_SIZE, OutlineSink,
    Placement, Point, Positioned, Rasterizer,
};

const HEAD: [u8; 4] = *b"head";
const MAXP: [u8; 4] = *b"maxp";
const HHEA: [u8; 4] = *b"hhea";
const HMTX: [u8; 4] = *b"hmtx";
const CMAP: [u8; 4] = *b"cmap";
const LOCA: [u8; 4] = *b"loca";
const GLYF: [u8; 4] = *b"glyf";
const KERN: [u8; 4] = *b"kern";

/// A TrueType font borrowed from its file's bytes.
#[derive(Clone, Copy, Debug)]
pub struct Font<'a> {
    hmtx: &'a [u8],
    loca: &'a [u8],
    glyf: &'a [u8],
    kern: Option<&'a [u8]>,
    cmap: Cmap<'a>,
    units_per_em: u16,
    glyphs: u16,
    long_loca: bool,
    h_metrics: u16,
    ascent: i16,
    descent: i16,
    line_gap: i16,
}

#[derive(Clone, Copy, Debug)]
enum Cmap<'a> {
    Format4(&'a [u8]),
    Format12(&'a [u8]),
}

fn table(data: &[u8], tag: [u8; 4]) -> Result<Option<&[u8]>, Error> {
    let count = u16_at(data, 4).ok_or(Error::NotTrueType)?;
    for i in 0..usize::from(count) {
        let record = 12 + 16 * i;
        if slice(data, record, 4).ok_or(Error::NotTrueType)? != tag {
            continue;
        }
        let offset = u32_at(data, record + 8).ok_or(Error::NotTrueType)?;
        let len = u32_at(data, record + 12).ok_or(Error::NotTrueType)?;
        let start = usize::try_from(offset).map_err(|_| Error::Malformed(tag))?;
        let len = usize::try_from(len).map_err(|_| Error::Malformed(tag))?;
        return slice(data, start, len)
            .map(Some)
            .ok_or(Error::Malformed(tag));
    }
    Ok(None)
}

fn required(data: &[u8], tag: [u8; 4]) -> Result<&[u8], Error> {
    table(data, tag)?.ok_or(Error::MissingTable(tag))
}

fn bad(tag: [u8; 4]) -> impl Fn() -> Error {
    move || Error::Malformed(tag)
}

impl<'a> Font<'a> {
    /// Reads the table directory and the tables every glyph needs.
    ///
    /// # Errors
    ///
    /// [`Error::NotTrueType`] for data that is no TrueType font,
    /// [`Error::MissingTable`] and [`Error::Malformed`] for missing and
    /// broken tables.
    pub fn parse(data: &'a [u8]) -> Result<Font<'a>, Error> {
        if !matches!(u32_at(data, 0), Some(0x0001_0000 | 0x7472_7565)) {
            return Err(Error::NotTrueType);
        }
        let head = required(data, HEAD)?;
        if u32_at(head, 12) != Some(0x5f0f_3cf5) {
            return Err(Error::Malformed(HEAD));
        }
        let units_per_em = u16_at(head, 18).ok_or_else(bad(HEAD))?;
        if !(16..=16384).contains(&units_per_em) {
            return Err(Error::Malformed(HEAD));
        }
        let long_loca = match i16_at(head, 50) {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(Error::Malformed(HEAD)),
        };
        let glyphs = u16_at(required(data, MAXP)?, 4).ok_or_else(bad(MAXP))?;
        let hhea = required(data, HHEA)?;
        let h_metrics = u16_at(hhea, 34).ok_or_else(bad(HHEA))?;
        if h_metrics == 0 || h_metrics > glyphs {
            return Err(Error::Malformed(HHEA));
        }
        let hmtx = required(data, HMTX)?;
        if hmtx.len() < 2 * usize::from(h_metrics) + 2 * usize::from(glyphs) {
            return Err(Error::Malformed(HMTX));
        }
        let loca = required(data, LOCA)?;
        let entry = if long_loca { 4 } else { 2 };
        if loca.len() < entry * (usize::from(glyphs) + 1) {
            return Err(Error::Malformed(LOCA));
        }
        Ok(Font {
            hmtx,
            loca,
            glyf: required(data, GLYF)?,
            kern: table(data, KERN)?,
            cmap: pick_cmap(required(data, CMAP)?)?,
            units_per_em,
            glyphs,
            long_loca,
            h_metrics,
            ascent: i16_at(hhea, 4).ok_or_else(bad(HHEA))?,
            descent: i16_at(hhea, 6).ok_or_else(bad(HHEA))?,
            line_gap: i16_at(hhea, 8).ok_or_else(bad(HHEA))?,
        })
    }

    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    #[must_use]
    pub fn glyph_count(&self) -> u16 {
        self.glyphs
    }

    /// The scale from font units to pixels at a pixel size, which is the
    /// height of one em.
    #[must_use]
    pub fn scale(&self, px: f32) -> f32 {
        px / f32::from(self.units_per_em)
    }

    #[must_use]
    pub fn line_metrics(&self, px: f32) -> LineMetrics {
        let s = self.scale(px);
        LineMetrics {
            ascent: f32::from(self.ascent) * s,
            descent: f32::from(self.descent) * s,
            line_gap: f32::from(self.line_gap) * s,
        }
    }

    /// The glyph for a character, or `None` when the font has none and
    /// would show its missing-glyph box.
    #[must_use]
    pub fn glyph_index(&self, c: char) -> Option<u16> {
        let glyph = match self.cmap {
            Cmap::Format4(t) => format4(t, u32::from(c)),
            Cmap::Format12(t) => format12(t, u32::from(c)),
        }?;
        (glyph != 0 && glyph < self.glyphs).then_some(glyph)
    }

    /// Advance width and left side bearing in font units.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for a glyph the font does not have.
    pub fn h_metrics(&self, glyph: u16) -> Result<(u16, i16), Error> {
        if glyph >= self.glyphs {
            return Err(Error::Malformed(HMTX));
        }
        // Glyphs past the last full record share its advance, and their
        // side bearings follow the records as a plain list.
        let long = usize::from(self.h_metrics);
        let g = usize::from(glyph);
        let advance = u16_at(self.hmtx, 4 * g.min(long - 1)).ok_or_else(bad(HMTX))?;
        let lsb_at = if g < long {
            4 * g + 2
        } else {
            2 * long + 2 * g
        };
        let lsb = i16_at(self.hmtx, lsb_at).ok_or_else(bad(HMTX))?;
        Ok((advance, lsb))
    }

    /// The `kern` table's adjustment between two glyphs in font units, or
    /// 0 when the font has no such pair or no `kern` table.
    #[must_use]
    pub fn kerning(&self, left: u16, right: u16) -> i16 {
        self.kern
            .and_then(|k| kern_pair(k, left, right))
            .unwrap_or(0)
    }

    fn glyph_data(&self, glyph: u16) -> Result<&'a [u8], Error> {
        if glyph >= self.glyphs {
            return Err(Error::Malformed(LOCA));
        }
        let g = usize::from(glyph);
        let (start, end) = if self.long_loca {
            (u32_at(self.loca, 4 * g), u32_at(self.loca, 4 * g + 4))
        } else {
            let half = |at| u16_at(self.loca, at).map(|v| u32::from(v) * 2);
            (half(2 * g), half(2 * g + 2))
        };
        let (start, end) = (start.ok_or_else(bad(LOCA))?, end.ok_or_else(bad(LOCA))?);
        let len = end.checked_sub(start).ok_or_else(bad(LOCA))?;
        let start = usize::try_from(start).map_err(|_| Error::Malformed(LOCA))?;
        let len = usize::try_from(len).map_err(|_| Error::Malformed(LOCA))?;
        slice(self.glyf, start, len).ok_or(Error::Malformed(GLYF))
    }

    /// The glyph's bounding box in font units from its `glyf` header, as
    /// (x min, y min, x max, y max), or `None` for a glyph without outline.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] when the glyph's data is broken.
    pub fn bounds(&self, glyph: u16) -> Result<Option<[i16; 4]>, Error> {
        let data = self.glyph_data(glyph)?;
        if data.is_empty() {
            return Ok(None);
        }
        let get = |at| i16_at(data, at).ok_or(Error::Malformed(GLYF));
        let b = [get(2)?, get(4)?, get(6)?, get(8)?];
        if b[0] > b[2] || b[1] > b[3] {
            return Err(Error::Malformed(GLYF));
        }
        Ok(Some(b))
    }

    /// Sends the glyph's outline to `sink`, composite glyphs included.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for broken glyph data, [`Error::TooDeep`] and
    /// [`Error::PointMatching`] for composites Meridian cannot draw.
    pub fn outline(&self, glyph: u16, sink: &mut impl OutlineSink) -> Result<(), Error> {
        let mut budget = MAX_COMPONENTS;
        self.outline_at(glyph, sink, Transform::IDENTITY, 0, &mut budget)
    }

    fn outline_at(
        &self,
        glyph: u16,
        sink: &mut impl OutlineSink,
        t: Transform,
        depth: u32,
        budget: &mut u32,
    ) -> Result<(), Error> {
        let data = self.glyph_data(glyph)?;
        if data.is_empty() {
            return Ok(());
        }
        let contours = i16_at(data, 0).ok_or(Error::Malformed(GLYF))?;
        match u16::try_from(contours) {
            Ok(n) => simple(data, n, sink, t),
            Err(_) => self.composite(data, sink, t, depth, budget),
        }
    }

    fn composite(
        &self,
        data: &[u8],
        sink: &mut impl OutlineSink,
        parent: Transform,
        depth: u32,
        budget: &mut u32,
    ) -> Result<(), Error> {
        const WORDS: u16 = 0x0001;
        const XY_VALUES: u16 = 0x0002;
        const SCALE: u16 = 0x0008;
        const MORE: u16 = 0x0020;
        const XY_SCALE: u16 = 0x0040;
        const TWO_BY_TWO: u16 = 0x0080;
        const SCALED_OFFSET: u16 = 0x0800;
        let err = || Error::Malformed(GLYF);
        if depth >= MAX_COMPONENT_DEPTH {
            return Err(Error::TooDeep);
        }
        let f2 = |at| f2dot14_at(data, at).ok_or_else(err);
        let mut at = 10;
        loop {
            *budget = budget.checked_sub(1).ok_or(Error::TooDeep)?;
            let flags = u16_at(data, at).ok_or_else(err)?;
            let glyph = u16_at(data, at + 2).ok_or_else(err)?;
            if flags & XY_VALUES == 0 {
                return Err(Error::PointMatching);
            }
            let (dx, dy) = if flags & WORDS == 0 {
                at += 6;
                let x = i8_at(data, at - 2).map(f32::from);
                (x, i8_at(data, at - 1).map(f32::from))
            } else {
                at += 8;
                let x = i16_at(data, at - 4).map(f32::from);
                (x, i16_at(data, at - 2).map(f32::from))
            };
            let (dx, dy) = (dx.ok_or_else(err)?, dy.ok_or_else(err)?);
            let mut m = Transform::IDENTITY;
            if flags & SCALE != 0 {
                (m.a, m.d) = (f2(at)?, f2(at)?);
                at += 2;
            } else if flags & XY_SCALE != 0 {
                (m.a, m.d) = (f2(at)?, f2(at + 2)?);
                at += 4;
            } else if flags & TWO_BY_TWO != 0 {
                (m.a, m.b, m.c, m.d) = (f2(at)?, f2(at + 2)?, f2(at + 4)?, f2(at + 6)?);
                at += 8;
            }
            (m.e, m.f) = if flags & SCALED_OFFSET == 0 {
                (dx, dy)
            } else {
                (m.a * dx + m.c * dy, m.b * dx + m.d * dy)
            };
            self.outline_at(glyph, sink, parent.then(m), depth + 1, budget)?;
            if flags & MORE == 0 {
                return Ok(());
            }
        }
    }

    /// Where the glyph's bitmap goes at a pixel size, with the pen moved
    /// right by `shift` pixels (0 to 1) for placement between pixels. `None`
    /// for a glyph without outline, such as a space.
    ///
    /// # Errors
    ///
    /// [`Error::BadSize`] for a size or shift out of range,
    /// [`Error::Malformed`] when the glyph's data is broken.
    #[allow(clippy::cast_sign_loss)]
    pub fn placement(&self, glyph: u16, px: f32, shift: f32) -> Result<Option<Placement>, Error> {
        if !(f32::MIN_POSITIVE..=MAX_PIXEL_SIZE).contains(&px) || !(0.0..=1.0).contains(&shift) {
            return Err(Error::BadSize);
        }
        let Some([x_min, y_min, x_max, y_max]) = self.bounds(glyph)? else {
            return Ok(None);
        };
        let s = self.scale(px);
        let left = floor(f32::from(x_min) * s + shift);
        let right = ceil(f32::from(x_max) * s + shift);
        let top = floor(-f32::from(y_max) * s);
        let bottom = ceil(-f32::from(y_min) * s);
        Ok(Some(Placement {
            left,
            top,
            width: (right - left) as usize,
            height: (bottom - top) as usize,
        }))
    }

    /// Rasterizes the glyph at a pixel size into `out`, row by row, using
    /// `scratch` for accumulation. Returns where the bitmap goes, or `None`
    /// for a glyph without outline.
    ///
    /// # Errors
    ///
    /// [`Error::BufferTooSmall`] when `out` is shorter than the placement's
    /// width times height or `scratch` shorter than its
    /// [`Placement::scratch_len`], and the errors of [`Font::placement`]
    /// and [`Font::outline`].
    #[allow(clippy::cast_precision_loss)]
    pub fn rasterize(
        &self,
        glyph: u16,
        px: f32,
        shift: f32,
        scratch: &mut [f32],
        out: &mut [u8],
    ) -> Result<Option<Placement>, Error> {
        let Some(p) = self.placement(glyph, px, shift)? else {
            return Ok(None);
        };
        let mut raster = Rasterizer::new(scratch, p.width, p.height)?;
        let mut sink = PixelSink {
            raster: &mut raster,
            scale: self.scale(px),
            dx: shift - p.left as f32,
            dy: -p.top as f32,
            start: Point::default(),
            last: Point::default(),
        };
        self.outline(glyph, &mut sink)?;
        raster.coverage(out)?;
        Ok(Some(p))
    }

    /// Lays out one line of text at a pixel size from the left: each glyph
    /// with its pen position goes to `place`. Returns the line's width.
    /// Characters the font lacks get its missing-glyph box, glyph 0.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] when a glyph's metrics are broken.
    pub fn layout(
        &self,
        text: &str,
        px: f32,
        mut place: impl FnMut(Positioned),
    ) -> Result<f32, Error> {
        let s = self.scale(px);
        let mut pen = 0.0;
        let mut previous = None;
        for c in text.chars() {
            let glyph = self.glyph_index(c).unwrap_or(0);
            if let Some(left) = previous {
                pen += f32::from(self.kerning(left, glyph)) * s;
            }
            place(Positioned { glyph, x: pen });
            pen += f32::from(self.h_metrics(glyph)?.0) * s;
            previous = Some(glyph);
        }
        Ok(pen)
    }
}

/// Picks the best Unicode subtable: format 12 covers every plane, format 4
/// only the first. Their lengths are not trusted; reads stop at the end of
/// the `cmap` table instead.
fn pick_cmap(cmap: &[u8]) -> Result<Cmap<'_>, Error> {
    let err = || Error::Malformed(CMAP);
    let count = u16_at(cmap, 2).ok_or_else(err)?;
    let mut best = None;
    for i in 0..usize::from(count) {
        let record = 4 + 8 * i;
        let platform = u16_at(cmap, record).ok_or_else(err)?;
        let encoding = u16_at(cmap, record + 2).ok_or_else(err)?;
        let offset = u32_at(cmap, record + 4).ok_or_else(err)?;
        let sub = usize::try_from(offset)
            .ok()
            .and_then(|o| cmap.get(o..))
            .ok_or_else(err)?;
        match (platform, encoding, u16_at(sub, 0)) {
            (3, 10, Some(12)) | (0, 4, Some(12)) => return Ok(Cmap::Format12(sub)),
            (3, 1, Some(4)) | (0, 3, Some(4)) => best = Some(Cmap::Format4(sub)),
            _ => {}
        }
    }
    best.ok_or(Error::MissingTable(CMAP))
}

/// Looks a character up in a format 4 subtable: segments of characters with
/// either a fixed difference to their glyphs or a list of glyphs.
fn format4(t: &[u8], c: u32) -> Option<u16> {
    let c = u16::try_from(c).ok()?;
    let segments = usize::from(u16_at(t, 6)? / 2);
    let ends = 14;
    let starts = ends + 2 * segments + 2;
    let deltas = starts + 2 * segments;
    let ranges = deltas + 2 * segments;
    let (mut lo, mut hi) = (0, segments);
    while lo < hi {
        let mid = usize::midpoint(lo, hi);
        if u16_at(t, ends + 2 * mid)? < c {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == segments {
        return None;
    }
    let offset = c.checked_sub(u16_at(t, starts + 2 * lo)?)?;
    let delta = u16_at(t, deltas + 2 * lo)?;
    let range = usize::from(u16_at(t, ranges + 2 * lo)?);
    if range == 0 {
        return Some(c.wrapping_add(delta));
    }
    // The offset counts from where it is stored to a list of glyphs.
    let glyph = u16_at(t, ranges + 2 * lo + range + 2 * usize::from(offset))?;
    (glyph != 0).then(|| glyph.wrapping_add(delta))
}

/// Looks a character up in a format 12 subtable: sorted groups of
/// characters that map to consecutive glyphs.
fn format12(t: &[u8], c: u32) -> Option<u16> {
    let groups = usize::try_from(u32_at(t, 12)?).ok()?;
    let (mut lo, mut hi) = (0, groups);
    while lo < hi {
        let mid = usize::midpoint(lo, hi);
        let at = 16 + 12 * mid;
        let first = u32_at(t, at)?;
        if c < first {
            hi = mid;
        } else if c > u32_at(t, at + 4)? {
            lo = mid + 1;
        } else {
            let glyph = u32_at(t, at + 8)?.checked_add(c - first)?;
            return u16::try_from(glyph).ok();
        }
    }
    None
}

/// Finds a pair in the first subtable of a `kern` table, if that is a
/// format 0 subtable of plain horizontal kerning.
fn kern_pair(kern: &[u8], left: u16, right: u16) -> Option<i16> {
    if u16_at(kern, 0)? != 0 || u16_at(kern, 2)? == 0 || u16_at(kern, 8)? & 0xff07 != 1 {
        return None;
    }
    let key = (u32::from(left) << 16) | u32::from(right);
    let (mut lo, mut hi) = (0, usize::from(u16_at(kern, 10)?));
    while lo < hi {
        let mid = usize::midpoint(lo, hi);
        let at = 18 + 6 * mid;
        match u32_at(kern, at)?.cmp(&key) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return i16_at(kern, at + 4),
        }
    }
    None
}

/// An affine map: x' = a x + c y + e, y' = b x + d y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Transform {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Transform {
    const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn apply(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// Applies `inner` first, then `self`.
    fn then(&self, inner: Transform) -> Transform {
        Transform {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            e: self.a * inner.e + self.c * inner.f + self.e,
            f: self.b * inner.e + self.d * inner.f + self.f,
        }
    }
}

const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
const REPEAT: u8 = 0x08;
const X_SAME: u8 = 0x10;
const Y_SAME: u8 = 0x20;

/// Reads the points of a simple glyph one by one. Flags, x deltas and y
/// deltas lie in three separate runs, read in step.
#[derive(Clone, Copy)]
struct Points<'a> {
    data: &'a [u8],
    flag_at: usize,
    repeat: u8,
    flag: u8,
    x_at: usize,
    y_at: usize,
    x: i32,
    y: i32,
}

/// Bytes one coordinate takes: one when short, none when it repeats the
/// one before, else two.
fn coordinate_len(flag: u8, short: u8, same: u8) -> usize {
    if flag & short != 0 {
        1
    } else if flag & same != 0 {
        0
    } else {
        2
    }
}

/// Reads one coordinate delta and moves `at` past it. A short delta is a
/// magnitude, with the same flag giving its sign.
fn delta(data: &[u8], at: &mut usize, flag: u8, short: u8, same: u8) -> Option<i32> {
    let len = coordinate_len(flag, short, same);
    let v = match len {
        0 => 0,
        1 => {
            let magnitude = i32::from(u8_at(data, *at)?);
            if flag & same == 0 {
                -magnitude
            } else {
                magnitude
            }
        }
        _ => i32::from(i16_at(data, *at)?),
    };
    *at += len;
    Some(v)
}

impl<'a> Points<'a> {
    /// Finds where the x and y runs start by walking the flags once.
    fn new(data: &'a [u8], flags_at: usize, count: usize) -> Option<Points<'a>> {
        let (mut at, mut seen, mut x_len) = (flags_at, 0, 0);
        while seen < count {
            let flag = u8_at(data, at)?;
            at += 1;
            let mut times = 1;
            if flag & REPEAT != 0 {
                times += usize::from(u8_at(data, at)?);
                at += 1;
            }
            x_len += times * coordinate_len(flag, X_SHORT, X_SAME);
            seen += times;
        }
        Some(Points {
            data,
            flag_at: flags_at,
            repeat: 0,
            flag: 0,
            x_at: at,
            y_at: at + x_len,
            x: 0,
            y: 0,
        })
    }

    #[allow(clippy::cast_precision_loss)]
    fn next(&mut self) -> Option<(Point, bool)> {
        if self.repeat > 0 {
            self.repeat -= 1;
        } else {
            self.flag = u8_at(self.data, self.flag_at)?;
            self.flag_at += 1;
            if self.flag & REPEAT != 0 {
                self.repeat = u8_at(self.data, self.flag_at)?;
                self.flag_at += 1;
            }
        }
        let dx = delta(self.data, &mut self.x_at, self.flag, X_SHORT, X_SAME)?;
        let dy = delta(self.data, &mut self.y_at, self.flag, Y_SHORT, Y_SAME)?;
        self.x = self.x.wrapping_add(dx);
        self.y = self.y.wrapping_add(dy);
        let p = Point::new(self.x as f32, self.y as f32);
        Some((p, self.flag & ON_CURVE != 0))
    }
}

fn simple(
    data: &[u8],
    contours: u16,
    sink: &mut impl OutlineSink,
    t: Transform,
) -> Result<(), Error> {
    let err = || Error::Malformed(GLYF);
    if contours == 0 {
        return Ok(());
    }
    let n = usize::from(contours);
    let end_at = |i| u16_at(data, 10 + 2 * i).map(usize::from).ok_or_else(err);
    let count = end_at(n - 1)? + 1;
    let instructions = usize::from(u16_at(data, 10 + 2 * n).ok_or_else(err)?);
    let mut points = Points::new(data, 12 + 2 * n + instructions, count).ok_or_else(err)?;
    let mut first = 0;
    for i in 0..n {
        let end = end_at(i)?;
        if end < first {
            return Err(err());
        }
        contour(&mut points, end - first + 1, sink, t).ok_or_else(err)?;
        first = end + 1;
    }
    Ok(())
}

/// Sends one contour of `len` points: read once to find where it starts,
/// then again to draw it. Two off-curve points in a row imply an on-curve
/// point halfway between them.
fn contour(
    points: &mut Points<'_>,
    len: usize,
    sink: &mut impl OutlineSink,
    t: Transform,
) -> Option<()> {
    let mut scan = *points;
    let (first, first_on) = scan.next()?;
    let (mut last, mut last_on) = (first, first_on);
    for _ in 1..len {
        (last, last_on) = scan.next()?;
    }
    let (start, skip) = if first_on {
        (first, Some(0))
    } else if last_on {
        (last, Some(len - 1))
    } else {
        (first.mid(last), None)
    };
    sink.move_to(t.apply(start));
    let mut control: Option<Point> = None;
    for i in 0..len {
        let (p, on) = points.next()?;
        if skip == Some(i) {
            continue;
        }
        match (on, control) {
            (true, Some(c)) => {
                sink.quad_to(t.apply(c), t.apply(p));
                control = None;
            }
            (true, None) => sink.line_to(t.apply(p)),
            (false, Some(c)) => {
                sink.quad_to(t.apply(c), t.apply(c.mid(p)));
                control = Some(p);
            }
            (false, None) => control = Some(p),
        }
    }
    if let Some(c) = control {
        sink.quad_to(t.apply(c), t.apply(start));
    }
    sink.close();
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_composed_transform_applies_the_inner_one_first() {
        let outer = Transform {
            a: 2.0,
            b: 0.5,
            c: -1.0,
            d: 3.0,
            e: 10.0,
            f: -4.0,
        };
        let inner = Transform {
            a: 0.5,
            b: -2.0,
            c: 1.5,
            d: 4.0,
            e: -3.0,
            f: 7.0,
        };
        let both = outer.then(inner);
        for p in [
            Point::new(0.0, 0.0),
            Point::new(1.0, 2.0),
            Point::new(-3.0, 0.25),
        ] {
            assert_eq!(both.apply(p), outer.apply(inner.apply(p)), "{p:?}");
        }
        assert_eq!(Transform::IDENTITY.then(inner), inner);
        assert_eq!(inner.then(Transform::IDENTITY), inner);
    }
}
