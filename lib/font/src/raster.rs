//! Anti-aliased rasterizing by accumulating signed area.
//!
//! Every edge adds, to each pixel row it crosses, how much of each pixel
//! lies to its right, weighted by how far the edge runs down that row and
//! by its direction. Stored as differences between neighbouring pixels,
//! this takes a few additions per pixel the edge touches, and a running sum
//! along each row turns the differences into exact coverage. Up and down
//! edges cancel, so holes in glyphs come out empty.

use crate::math::{ceil, floor};
use crate::{Error, OutlineSink, Point};

/// How far a flattened curve may stray from the true one, in pixels.
const TOLERANCE: f32 = 0.05;

/// The most line segments one quadratic curve becomes.
const MAX_SEGMENTS: u32 = 64;

/// Accumulates edges into a caller's buffer and turns them into coverage.
/// Edges may reach past the canvas: parts left or right of it are pressed
/// onto its side, which keeps the coverage inside exact, and parts above
/// or below it are dropped.
pub struct Rasterizer<'a> {
    acc: &'a mut [f32],
    width: usize,
    height: usize,
}

impl<'a> Rasterizer<'a> {
    /// Cells of scratch space a canvas of this size needs: two more per
    /// row than pixels, for differences that land just past the right edge.
    #[must_use]
    pub fn scratch_len(width: usize, height: usize) -> usize {
        width.saturating_add(2).saturating_mul(height)
    }

    /// Starts an empty canvas in `acc`.
    ///
    /// # Errors
    ///
    /// [`Error::BufferTooSmall`] when `acc` is shorter than
    /// [`Rasterizer::scratch_len`].
    pub fn new(acc: &'a mut [f32], width: usize, height: usize) -> Result<Self, Error> {
        let acc = acc
            .get_mut(..Self::scratch_len(width, height))
            .ok_or(Error::BufferTooSmall)?;
        acc.fill(0.0);
        Ok(Rasterizer { acc, width, height })
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Adds a straight edge, in pixels with y pointing down.
    #[allow(clippy::cast_precision_loss)]
    pub fn line(&mut self, p0: Point, p1: Point) {
        // An edge that crosses the left or right side is cut there, so that
        // each part lies on one side of it and can be pressed onto it.
        for side in [0.0, self.width as f32] {
            if (p0.x < side && side < p1.x) || (p1.x < side && side < p0.x) {
                let cut = Point::new(side, p0.y + (side - p0.x) / (p1.x - p0.x) * (p1.y - p0.y));
                self.line(p0, cut);
                self.line(cut, p1);
                return;
            }
        }
        self.edge(p0, p1);
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    fn edge(&mut self, p0: Point, p1: Point) {
        let (dir, top, bottom) = if p0.y < p1.y {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let h = self.height as f32;
        let y_start = top.y.clamp(0.0, h);
        let y_end = bottom.y.clamp(0.0, h);
        if y_start >= y_end {
            return;
        }
        let dxdy = (bottom.x - top.x) / (bottom.y - top.y);
        let mut x = top.x + (y_start - top.y) * dxdy;
        for row in floor(y_start) as usize..ceil(y_end) as usize {
            let y0 = (row as f32).max(y_start);
            let y1 = ((row + 1) as f32).min(y_end);
            let dy = y1 - y0;
            let x_next = x + dxdy * dy;
            self.accumulate(row, x, x_next, dir * dy);
            x = x_next;
        }
    }

    /// Adds the part of an edge that runs through one row: from `xa` at the
    /// top of that part to `xb` at its bottom, with signed height `d`.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::similar_names
    )]
    fn accumulate(&mut self, row: usize, xa: f32, xb: f32, d: f32) {
        let w = self.width as f32;
        let (xa, xb) = (xa.clamp(0.0, w), xb.clamp(0.0, w));
        let (x0, x1) = (xa.min(xb), xa.max(xb));
        let base = row * (self.width + 2);
        let first = floor(x0) as usize;
        let last = ceil(x1) as usize;
        let acc = &mut self.acc[base..base + self.width + 2];
        if last <= first + 1 {
            // Within one pixel: the part of it right of the edge is one
            // minus how far the edge's middle sits into the pixel.
            let into = f32::midpoint(x0, x1) - first as f32;
            acc[first] += d * (1.0 - into);
            acc[first + 1] += d * into;
            return;
        }
        // Across several pixels the edge's height spreads evenly over its
        // width, so each pixel gets the integral of the part right of it.
        let s = 1.0 / (x1 - x0);
        let f0 = x0 - first as f32;
        let a0 = 0.5 * s * (1.0 - f0) * (1.0 - f0);
        let f1 = x1 - (last - 1) as f32;
        let am = 0.5 * s * f1 * f1;
        acc[first] += d * a0;
        if last == first + 2 {
            acc[first + 1] += d * (1.0 - a0 - am);
        } else {
            let a1 = s * (1.5 - f0);
            acc[first + 1] += d * (a1 - a0);
            for cell in &mut acc[first + 2..last - 1] {
                *cell += d * s;
            }
            let a2 = a1 + (last - first - 3) as f32 * s;
            acc[last - 1] += d * (1.0 - a2 - am);
        }
        acc[last] += d * am;
    }

    /// Adds a quadratic curve, flattened into as many lines as keep it
    /// within [`TOLERANCE`] of the true curve.
    #[allow(clippy::cast_precision_loss)]
    pub fn quad(&mut self, p0: Point, control: Point, p2: Point) {
        let n = segments(p0, control, p2);
        let mut prev = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let p = Point::new(
                u * u * p0.x + 2.0 * u * t * control.x + t * t * p2.x,
                u * u * p0.y + 2.0 * u * t * control.y + t * t * p2.y,
            );
            self.line(prev, p);
            prev = p;
        }
    }

    /// Writes coverage, 0 to 255, row by row into `out`.
    ///
    /// # Errors
    ///
    /// [`Error::BufferTooSmall`] when `out` holds fewer than width times
    /// height bytes.
    pub fn coverage(&self, out: &mut [u8]) -> Result<(), Error> {
        let out = out
            .get_mut(..self.width * self.height)
            .ok_or(Error::BufferTooSmall)?;
        for (row, pixels) in out.chunks_exact_mut(self.width.max(1)).enumerate() {
            let cells = &self.acc[row * (self.width + 2)..];
            let mut sum = 0.0;
            for (pixel, cell) in pixels.iter_mut().zip(cells) {
                sum += cell;
                *pixel = to_byte(sum);
            }
        }
        Ok(())
    }
}

/// Coverage as a byte: the amount of the pixel the shape covers, rounded
/// to the nearest of 256 steps. The cast stops at 255, so parts that
/// overlap count once.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_byte(sum: f32) -> u8 {
    (sum.abs() * 255.0 + 0.5) as u8
}

/// How many lines a curve needs. One segment strays from the curve by at
/// most a quarter of `p0 - 2c + p2`, and n segments by that over n squared.
#[allow(clippy::cast_precision_loss)]
fn segments(p0: Point, c: Point, p2: Point) -> u32 {
    let dx = (p0.x - 2.0 * c.x + p2.x).abs();
    let dy = (p0.y - 2.0 * c.y + p2.y).abs();
    let spread = dx.max(dy);
    let mut n = 1;
    while n < MAX_SEGMENTS && spread > 4.0 * TOLERANCE * (n * n) as f32 {
        n += 1;
    }
    n
}

/// Draws an outline in font units onto a rasterizer: scaled, moved so the
/// glyph's top left lands at the canvas origin, and turned so y points down.
pub(crate) struct PixelSink<'r, 'a> {
    pub(crate) raster: &'r mut Rasterizer<'a>,
    pub(crate) scale: f32,
    pub(crate) dx: f32,
    pub(crate) dy: f32,
    pub(crate) start: Point,
    pub(crate) last: Point,
}

impl PixelSink<'_, '_> {
    fn map(&self, p: Point) -> Point {
        Point::new(p.x * self.scale + self.dx, self.dy - p.y * self.scale)
    }
}

impl OutlineSink for PixelSink<'_, '_> {
    fn move_to(&mut self, p: Point) {
        self.start = self.map(p);
        self.last = self.start;
    }

    fn line_to(&mut self, p: Point) {
        let p = self.map(p);
        self.raster.line(self.last, p);
        self.last = p;
    }

    fn quad_to(&mut self, control: Point, p: Point) {
        let (c, p) = (self.map(control), self.map(p));
        self.raster.quad(self.last, c, p);
        self.last = p;
    }

    fn close(&mut self) {
        self.raster.line(self.last, self.start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spread(x: f32, y: f32) -> u32 {
        // A curve whose p0 - 2c + p2 is (x, y).
        segments(
            Point::new(0.0, 0.0),
            Point::new(-x / 2.0, -y / 2.0),
            Point::new(0.0, 0.0),
        )
    }

    #[test]
    fn curves_get_enough_segments() {
        // n segments stray by spread / (4 n^2), which must stay within
        // 0.05 pixels.
        assert_eq!(spread(0.0, 0.0), 1);
        assert_eq!(spread(0.19, 0.0), 1);
        assert_eq!(spread(0.0, 0.21), 2);
        assert_eq!(spread(-0.79, 0.1), 2);
        assert_eq!(spread(0.1, -0.81), 3);
        assert_eq!(spread(-4.9, 0.0), 5);
        assert_eq!(spread(0.0, 5.1), 6);
        assert_eq!(spread(4.0 * TOLERANCE, 0.0), 1, "just within");
        assert_eq!(spread(0.0, 16.0 * TOLERANCE), 2, "just within for two");
        assert_eq!(spread(900.0, 0.0), 64);
        assert_eq!(spread(1e9, 1e9), 64);
        assert_eq!(spread(f32::NAN, 0.0), 1);
    }

    #[test]
    fn coverage_rounds_to_the_nearest_step() {
        assert_eq!(to_byte(0.0), 0);
        assert_eq!(to_byte(0.5), 128);
        assert_eq!(to_byte(-0.5), 128);
        assert_eq!(to_byte(1.0 / 255.0 * 0.49), 0);
        assert_eq!(to_byte(1.0 / 255.0 * 0.51), 1);
        assert_eq!(to_byte(1.0), 255);
        assert_eq!(to_byte(-1.0), 255);
        assert_eq!(to_byte(2.0), 255);
        assert_eq!(to_byte(f32::NAN), 0);
    }
}
