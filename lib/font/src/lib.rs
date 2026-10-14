//! TrueType fonts for Meridian: reading a font file, laying out a line of
//! text and rasterizing glyphs with grayscale anti-aliasing and no hinting.
//!
//! Nothing here allocates. A [`Font`] borrows the file's bytes, outlines go
//! to an [`OutlineSink`], and rasterizing writes into buffers the caller
//! owns, so the bootloader and the kernel can draw text before they have a
//! heap. Every offset read from the file is checked: a broken or hostile
//! font gives an [`Error`], never a panic.
//!
//! Supported: `glyf` outlines including composite glyphs, `cmap` formats 4
//! and 12, horizontal metrics, and `kern` format 0. Not yet: hinting, CFF
//! outlines, `GPOS` kerning, shaping, colour glyphs.

#![cfg_attr(not(test), no_std)]

mod math;
mod raster;
mod read;
mod tables;

pub use raster::Rasterizer;
pub use tables::Font;

/// A point in whatever units the context uses: font units for outlines,
/// pixels for the rasterizer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn mid(self, other: Point) -> Point {
        Point::new(
            f32::midpoint(self.x, other.x),
            f32::midpoint(self.y, other.y),
        )
    }
}

/// Receives a glyph outline as closed contours of lines and quadratic
/// curves, in font units with y pointing up.
pub trait OutlineSink {
    fn move_to(&mut self, p: Point);
    fn line_to(&mut self, p: Point);
    fn quad_to(&mut self, control: Point, p: Point);
    /// Ends the contour with a straight line back to where it started,
    /// unless it is already there.
    fn close(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data does not start like a TrueType font.
    NotTrueType,
    /// A table the font needs is not there.
    MissingTable([u8; 4]),
    /// A table points outside itself or the file, or holds impossible values.
    Malformed([u8; 4]),
    /// Composite glyphs nest deeper than [`MAX_COMPONENT_DEPTH`] or hold
    /// more than [`MAX_COMPONENTS`] components in all, which also stops a
    /// glyph that contains itself.
    TooDeep,
    /// A composite glyph places a component by matching points, which
    /// Meridian does not support yet.
    PointMatching,
    /// The caller's buffers are smaller than the glyph needs.
    BufferTooSmall,
    /// A pixel size that is not above 0 and at most [`MAX_PIXEL_SIZE`], or
    /// a shift outside 0 to 1.
    BadSize,
}

/// How deeply composite glyphs may nest. Real fonts use one or two levels.
pub const MAX_COMPONENT_DEPTH: u32 = 8;

/// How many components one glyph may draw, counting nested ones. Real
/// fonts use a handful; the limit keeps a hostile font from nesting wide
/// composites into millions of outlines.
pub const MAX_COMPONENTS: u32 = 64;

/// The largest pixel size glyphs rasterize at.
pub const MAX_PIXEL_SIZE: f32 = 1024.0;

/// Line metrics at a pixel size. Ascent is above the baseline and positive,
/// descent below it and negative, as in the font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
}

impl LineMetrics {
    /// The distance from one baseline to the next.
    #[must_use]
    pub fn line_height(&self) -> f32 {
        self.ascent - self.descent + self.line_gap
    }
}

/// Where a rasterized glyph goes, relative to the pen on the baseline:
/// its top left pixel is `left` pixels right of the pen and `top` pixels
/// below the baseline (negative, since glyphs mostly sit above it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub left: i32,
    pub top: i32,
    pub width: usize,
    pub height: usize,
}

impl Placement {
    /// Accumulation cells [`Font::rasterize`] needs for this placement.
    #[must_use]
    pub fn scratch_len(&self) -> usize {
        Rasterizer::scratch_len(self.width, self.height)
    }
}

/// A glyph placed on a line by [`Font::layout`]: its pen position in pixels
/// from the start of the line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Positioned {
    pub glyph: u16,
    pub x: f32,
}
