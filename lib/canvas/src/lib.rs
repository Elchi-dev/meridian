//! Drawing into a framebuffer: solid rectangles and anti-aliased text. The
//! loader's boot screen and the kernel's own screens use it, and it never
//! allocates, so it works before there is a heap.

#![cfg_attr(not(test), no_std)]

pub mod palette;

use font::{Error, Font};

/// Text of at most `N` bytes, written with `write!` and no heap. What does
/// not fit is cut off at a character boundary, and the write that hit the
/// end reports an error.
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    #[must_use]
    pub fn new() -> Text<N> {
        Text {
            bytes: [0; N],
            len: 0,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Text::new()
    }
}

impl<const N: usize> core::fmt::Write for Text<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = N - self.len;
        let mut take = s.len().min(room);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        if take < s.len() {
            Err(core::fmt::Error)
        } else {
            Ok(())
        }
    }
}

/// How the four bytes of a pixel are ordered in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Red, green, blue, unused.
    Rgb,
    /// Blue, green, red, unused.
    Bgr,
}

/// A colour as red, green and blue from 0 to 255.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// The colour written as `0xRRGGBB`, the way the palette lists it.
    #[must_use]
    pub const fn hex(rgb: u32) -> Color {
        let [b, g, r, _] = rgb.to_le_bytes();
        Color { r, g, b }
    }

    /// The colour as a pixel in `format`.
    #[must_use]
    pub fn encode(self, format: Format) -> u32 {
        let (first, third) = match format {
            Format::Rgb => (self.r, self.b),
            Format::Bgr => (self.b, self.r),
        };
        u32::from_le_bytes([first, self.g, third, 0])
    }

    /// The colour of a pixel in `format`.
    #[must_use]
    pub fn decode(pixel: u32, format: Format) -> Color {
        let [first, g, third, _] = pixel.to_le_bytes();
        match format {
            Format::Rgb => Color {
                r: first,
                g,
                b: third,
            },
            Format::Bgr => Color {
                r: third,
                g,
                b: first,
            },
        }
    }

    /// `self` moved towards `over` by `coverage` out of 255.
    #[must_use]
    pub fn blend(self, over: Color, coverage: u8) -> Color {
        let mix = |from: u8, to: u8| {
            let (a, b, c) = (i32::from(from), i32::from(to), i32::from(coverage));
            let d = (b - a) * c;
            // Rounds half away from zero, so the result stays between the
            // two.
            let step = (d + 127 * d.signum()) / 255;
            u8::try_from(a + step).unwrap_or(to)
        };
        Color {
            r: mix(self.r, over.r),
            g: mix(self.g, over.g),
            b: mix(self.b, over.b),
        }
    }
}

/// The largest glyph [`Canvas::text`] draws, in pixels across and down,
/// or any other shape of no more area. That is enough for text up to
/// about 44 pixels.
pub const GLYPH_LIMIT: usize = 62;

/// Room to rasterize one glyph of up to [`GLYPH_LIMIT`] pixels each way.
pub struct Scratch {
    acc: [f32; (GLYPH_LIMIT + 2) * GLYPH_LIMIT],
    coverage: [u8; GLYPH_LIMIT * GLYPH_LIMIT],
}

impl Scratch {
    #[must_use]
    pub const fn new() -> Scratch {
        Scratch {
            acc: [0.0; (GLYPH_LIMIT + 2) * GLYPH_LIMIT],
            coverage: [0; GLYPH_LIMIT * GLYPH_LIMIT],
        }
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Scratch::new()
    }
}

/// Pixels in rows of `stride`, of which the first `width` are visible.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
    stride: usize,
    format: Format,
}

impl<'a> Canvas<'a> {
    /// A canvas over `pixels`, or `None` when they do not hold `height`
    /// rows of `width` pixels each, `stride` apart.
    #[must_use]
    pub fn new(
        pixels: &'a mut [u32],
        width: usize,
        height: usize,
        stride: usize,
        format: Format,
    ) -> Option<Canvas<'a>> {
        let needed = height
            .checked_sub(1)?
            .checked_mul(stride)?
            .checked_add(width)?;
        (width > 0 && stride >= width && pixels.len() >= needed).then_some(Canvas {
            pixels,
            width,
            height,
            stride,
            format,
        })
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// The colour at a visible pixel.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> Option<Color> {
        (x < self.width && y < self.height)
            .then(|| Color::decode(self.pixels[y * self.stride + x], self.format))
    }

    /// Fills the part of the rectangle that is on the canvas.
    pub fn fill_rect(&mut self, x: i32, y: i32, width: usize, height: usize, color: Color) {
        let (x0, x1) = clip(x, width, self.width);
        let (y0, y1) = clip(y, height, self.height);
        let value = color.encode(self.format);
        for row in y0..y1 {
            self.pixels[row * self.stride + x0..row * self.stride + x1].fill(value);
        }
    }

    pub fn fill(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    /// Copies the rectangle at `x`, `y` to the same place on `to`, as far
    /// as it lies on both, so a frame drawn off screen reaches the screen
    /// whole. Pixels change order when the two canvases differ in it.
    pub fn copy_to(&self, to: &mut Canvas<'_>, x: i32, y: i32, width: usize, height: usize) {
        let (x0, x1) = clip(x, width, self.width.min(to.width));
        let (y0, y1) = clip(y, height, self.height.min(to.height));
        for row in y0..y1 {
            let from = &self.pixels[row * self.stride + x0..row * self.stride + x1];
            let onto = &mut to.pixels[row * to.stride + x0..row * to.stride + x1];
            if self.format == to.format {
                onto.copy_from_slice(from);
            } else {
                for (pixel, &source) in onto.iter_mut().zip(from) {
                    *pixel = Color::decode(source, self.format).encode(to.format);
                }
            }
        }
    }

    /// Draws one line of text with its baseline at `baseline` and its pen
    /// starting at `x`, blending into what is already there. Returns the
    /// width of the line in pixels, rounded up.
    ///
    /// # Errors
    ///
    /// The font's errors, and [`Error::BufferTooSmall`] for a glyph larger
    /// than [`GLYPH_LIMIT`].
    #[allow(clippy::cast_precision_loss)]
    pub fn text(
        &mut self,
        style: &Style<'_>,
        x: i32,
        baseline: i32,
        text: &str,
        scratch: &mut Scratch,
    ) -> Result<u32, Error> {
        let mut result = Ok(());
        let width = style.font.layout(text, style.px, |glyph| {
            if result.is_ok() {
                let whole = floor(glyph.x);
                let shift = glyph.x - whole as f32;
                let pen = (x + whole, baseline);
                result = self.glyph(style, glyph.glyph, shift, pen, scratch);
            }
        })?;
        result?;
        Ok(ceil(width))
    }

    fn glyph(
        &mut self,
        style: &Style<'_>,
        glyph: u16,
        shift: f32,
        (pen_x, baseline): (i32, i32),
        scratch: &mut Scratch,
    ) -> Result<(), Error> {
        let raster = style.font.rasterize(
            glyph,
            style.px,
            shift,
            &mut scratch.acc,
            &mut scratch.coverage,
        )?;
        if let Some(p) = raster {
            let coverage = &scratch.coverage[..p.width * p.height];
            self.coverage(
                pen_x + p.left,
                baseline + p.top,
                p.width,
                coverage,
                style.color,
            );
        }
        Ok(())
    }

    /// Blends `color` into the rectangle `width` pixels wide whose top left
    /// pixel is at `left`, `top`, row by row, each pixel as far as
    /// `coverage` says, from 0 for none to 255 for all of it. What lies off
    /// the canvas is left out, and so are pixels without coverage.
    pub fn coverage(&mut self, left: i32, top: i32, width: usize, coverage: &[u8], color: Color) {
        if width == 0 {
            return;
        }
        for (gy, row) in coverage.chunks(width).enumerate() {
            let Some(y) = on_canvas(top, gy, self.height) else {
                continue;
            };
            for (gx, &c) in row.iter().enumerate() {
                let Some(x) = on_canvas(left, gx, self.width) else {
                    continue;
                };
                if c > 0 {
                    let at = y * self.stride + x;
                    let under = Color::decode(self.pixels[at], self.format);
                    self.pixels[at] = under.blend(color, c).encode(self.format);
                }
            }
        }
    }
}

/// Rasterizes `c` in `style` with its pen on a whole pixel, as a monospaced
/// grid draws each character in its own cell, and returns where the bitmap
/// goes relative to the pen, as [`Canvas::coverage`] takes it, with the
/// bitmap itself in `scratch`. `None` for a character that draws nothing,
/// such as a space; one the font lacks gets its missing-glyph box.
///
/// # Errors
///
/// The font's errors, and [`Error::BufferTooSmall`] for a glyph larger
/// than [`GLYPH_LIMIT`].
pub fn rasterize<'s>(
    style: &Style<'_>,
    c: char,
    scratch: &'s mut Scratch,
) -> Result<Option<(font::Placement, &'s [u8])>, Error> {
    let glyph = style.font.glyph_index(c).unwrap_or(0);
    let raster = style.font.rasterize(
        glyph,
        style.px,
        0.0,
        &mut scratch.acc,
        &mut scratch.coverage,
    )?;
    Ok(raster.map(|p| (p, &scratch.coverage[..p.width * p.height])))
}

/// What text looks like: its font, its size in pixels per em, its colour.
#[derive(Clone, Copy, Debug)]
pub struct Style<'f> {
    pub font: Font<'f>,
    pub px: f32,
    pub color: Color,
}

/// The visible range of `len` pixels starting at `start`, on an axis of
/// `size` pixels.
fn clip(start: i32, len: usize, size: usize) -> (usize, usize) {
    let start = i64::from(start);
    let end = start.saturating_add(i64::try_from(len).unwrap_or(i64::MAX));
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    let on_axis = |at: i64| usize::try_from(at.clamp(0, size)).unwrap_or(0);
    (on_axis(start), on_axis(end))
}

/// Where the `offset`th pixel after `start` lands, if it is on the canvas.
fn on_canvas(start: i32, offset: usize, size: usize) -> Option<usize> {
    let at = i64::from(start) + i64::try_from(offset).ok()?;
    usize::try_from(at).ok().filter(|&at| at < size)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn floor(v: f32) -> i32 {
    let t = v as i32;
    if (t as f32) > v { t - 1 } else { t }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn ceil(v: f32) -> u32 {
    let t = v as u32;
    if (t as f32) < v { t + 1 } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{NIGHT, SUN, TEXT};

    const INTER: &[u8] = include_bytes!("../../../third-party/fonts/Inter-Regular.ttf");

    fn style(px: f32, color: Color) -> Style<'static> {
        Style {
            font: Font::parse(INTER).unwrap(),
            px,
            color,
        }
    }

    /// A canvas of `w` by `h` with two pixels of padding per row, which
    /// hold a marker no drawing may touch.
    struct Buffer {
        pixels: Vec<u32>,
        w: usize,
        h: usize,
    }

    const PAD: u32 = 0xdead_beef;

    impl Buffer {
        fn new(w: usize, h: usize) -> Buffer {
            Buffer {
                pixels: vec![PAD; (w + 2) * h],
                w,
                h,
            }
        }

        fn canvas(&mut self, format: Format) -> Canvas<'_> {
            let mut c = Canvas::new(&mut self.pixels, self.w, self.h, self.w + 2, format).unwrap();
            c.fill(NIGHT);
            c
        }

        fn padding_untouched(&self) -> bool {
            (0..self.h).all(|y| {
                self.pixels[y * (self.w + 2) + self.w..(y + 1) * (self.w + 2)]
                    .iter()
                    .all(|&p| p == PAD)
            })
        }
    }

    #[test]
    fn text_fills_up_and_cuts_off() {
        use core::fmt::Write;
        let mut t = Text::<8>::new();
        assert!(write!(t, "abc").is_ok());
        assert!(write!(t, "defgh").is_ok());
        assert_eq!(t.as_str(), "abcdefgh");
        assert!(write!(t, "i").is_err());
        assert_eq!(t.as_str(), "abcdefgh");
        let mut cut = Text::<5>::default();
        assert!(
            write!(cut, "xy\u{e9}\u{e9}").is_err(),
            "the second e acute does not fit"
        );
        assert_eq!(cut.as_str(), "xy\u{e9}");
        let mut empty = Text::<0>::new();
        assert!(write!(empty, "a").is_err());
        assert!(write!(empty, "").is_ok());
        assert_eq!(empty.as_str(), "");
    }

    #[test]
    fn text_takes_formatted_pieces_in_turn() {
        use core::fmt::Write;
        let mut t = Text::<32>::new();
        let (free, all) = ("1 MiB", "2 MiB");
        write!(t, "{free} free of {all}").unwrap();
        assert_eq!(t.as_str(), "1 MiB free of 2 MiB");
        write!(t, ", {:>4}", 7).unwrap();
        assert_eq!(t.as_str(), "1 MiB free of 2 MiB,    7");
        assert!(write!(t, " and then some more").is_err());
        assert_eq!(t.as_str().len(), 32, "filled to the last byte");
        assert!(
            t.write_str("").is_ok(),
            "writing nothing into a full buffer is fine"
        );
        assert!(t.as_str().starts_with("1 MiB free of 2 MiB,    7 and t"));
    }

    #[test]
    fn text_never_splits_a_character() {
        use core::fmt::Write;
        for room in 0..8 {
            let mut cut = Text::<8>::new();
            let _ = write!(cut, "{}", "x".repeat(room));
            let _ = write!(cut, "\u{20ac}\u{20ac}\u{20ac}");
            let s = cut.as_str();
            assert!(s.len() <= 8, "{room}: {s:?}");
            assert_eq!(
                s.trim_start_matches('x').len() % 3,
                0,
                "{room}: whole euro signs only, {s:?}"
            );
        }
        assert_eq!(Text::<4>::default().as_str(), "");
        assert_eq!(Text::<4>::new().as_str(), Text::<4>::default().as_str());
    }

    #[test]
    fn colours_encode_per_format() {
        let c = Color::hex(0x12_3456);
        assert_eq!((c.r, c.g, c.b), (0x12, 0x34, 0x56));
        assert_eq!(c.encode(Format::Rgb), 0x0056_3412);
        assert_eq!(c.encode(Format::Bgr), 0x0012_3456);
        for format in [Format::Rgb, Format::Bgr] {
            assert_eq!(Color::decode(c.encode(format), format), c);
            assert_eq!(
                Color::decode(c.encode(format) | 0xff00_0000, format),
                c,
                "the unused byte"
            );
        }
    }

    #[test]
    fn blending_moves_towards_the_new_colour() {
        let (black, white) = (Color::hex(0), Color::hex(0xff_ffff));
        assert_eq!(black.blend(white, 0), black);
        assert_eq!(black.blend(white, 255), white);
        assert_eq!(black.blend(white, 128), Color::hex(0x80_8080));
        assert_eq!(white.blend(black, 128), Color::hex(0x7f_7f7f));
        assert_eq!(SUN.blend(SUN, 77), SUN);
        let a = Color::hex(0x10_f080);
        let b = Color::hex(0xf0_1080);
        let mut last = a;
        for c in 0..=255 {
            let m = a.blend(b, c);
            assert!(m.r >= last.r && m.g <= last.g && m.b == 0x80, "{c}: {m:?}");
            last = m;
        }
        assert_eq!(last, b);
    }

    #[test]
    fn a_canvas_needs_room_for_every_row() {
        let mut px = vec![0u32; 10 * 3 + 8];
        assert!(
            Canvas::new(&mut px, 8, 4, 10, Format::Rgb).is_some(),
            "the last row may be short"
        );
        assert!(Canvas::new(&mut px, 9, 4, 10, Format::Rgb).is_none());
        assert!(
            Canvas::new(&mut px, 11, 3, 10, Format::Rgb).is_none(),
            "stride below width"
        );
        assert!(Canvas::new(&mut px, 0, 3, 10, Format::Rgb).is_none());
        assert!(Canvas::new(&mut px, 8, 0, 10, Format::Rgb).is_none());
        assert!(Canvas::new(&mut px, 8, usize::MAX, 10, Format::Rgb).is_none());
        let c = Canvas::new(&mut px, 8, 4, 10, Format::Rgb).unwrap();
        assert_eq!((c.width(), c.height()), (8, 4));
    }

    /// A canvas of `w` by `h` whose every pixel is its own colour.
    fn numbered(pixels: &mut [u32], w: usize, h: usize, format: Format) -> Canvas<'_> {
        let mut c = Canvas::new(pixels, w, h, w, format).unwrap();
        for y in 0..h {
            for x in 0..w {
                let n = u32::try_from(y * w + x).unwrap();
                let at = |v: usize| i32::try_from(v).unwrap();
                c.fill_rect(at(x), at(y), 1, 1, Color::hex(0x0010_2030 + n * 0x0101));
            }
        }
        c
    }

    #[test]
    fn a_frame_is_copied_whole_into_rows_further_apart() {
        let mut frame = vec![0; 6 * 4];
        let back = numbered(&mut frame, 6, 4, Format::Bgr);
        let mut buf = Buffer::new(6, 4);
        let mut screen = buf.canvas(Format::Bgr);
        back.copy_to(&mut screen, 0, 0, 6, 4);
        for y in 0..4 {
            for x in 0..6 {
                assert_eq!(screen.pixel(x, y), back.pixel(x, y), "({x}, {y})");
            }
        }
        assert!(buf.padding_untouched());
    }

    #[test]
    fn only_the_rectangle_is_copied() {
        let mut frame = vec![0; 6 * 4];
        let back = numbered(&mut frame, 6, 4, Format::Bgr);
        let mut buf = Buffer::new(6, 4);
        let mut screen = buf.canvas(Format::Bgr);
        back.copy_to(&mut screen, 2, 1, 3, 2);
        for y in 0..4 {
            for x in 0..6 {
                let inside = (2..5).contains(&x) && (1..3).contains(&y);
                let want = if inside {
                    back.pixel(x, y)
                } else {
                    Some(NIGHT)
                };
                assert_eq!(screen.pixel(x, y), want, "({x}, {y})");
            }
        }
    }

    #[test]
    fn a_copy_stays_on_both_canvases() {
        let mut frame = vec![0; 5 * 6];
        let back = numbered(&mut frame, 5, 6, Format::Rgb);
        let mut buf = Buffer::new(7, 3);
        let mut screen = buf.canvas(Format::Rgb);
        back.copy_to(&mut screen, -2, -1, 100, 100);
        for y in 0..3 {
            for x in 0..7 {
                let want = if x < 5 { back.pixel(x, y) } else { Some(NIGHT) };
                assert_eq!(screen.pixel(x, y), want, "({x}, {y})");
            }
        }
        back.copy_to(&mut screen, i32::MIN, i32::MAX, usize::MAX, 1);
        assert!(buf.padding_untouched());
    }

    #[test]
    fn a_copy_between_pixel_orders_keeps_the_colours() {
        let mut frame = vec![0; 4 * 2];
        let back = numbered(&mut frame, 4, 2, Format::Rgb);
        let mut buf = Buffer::new(4, 2);
        let mut screen = buf.canvas(Format::Bgr);
        back.copy_to(&mut screen, 0, 0, 4, 2);
        for y in 0..2 {
            for x in 0..4 {
                assert_eq!(screen.pixel(x, y), back.pixel(x, y), "({x}, {y})");
            }
        }
        assert_ne!(
            buf.pixels[1],
            frame_word(&back, 1, 0),
            "the bytes themselves are swapped"
        );
    }

    fn frame_word(c: &Canvas<'_>, x: usize, y: usize) -> u32 {
        c.pixels[y * c.stride + x]
    }

    #[test]
    fn rectangles_are_clipped_to_the_canvas() {
        let mut buf = Buffer::new(20, 10);
        let c = &mut buf.canvas(Format::Bgr);
        c.fill_rect(-5, -5, 8, 7, SUN);
        c.fill_rect(15, 8, 100, 100, TEXT);
        c.fill_rect(i32::MAX - 1, 0, usize::MAX, 3, SUN);
        c.fill_rect(i32::MIN, 5, usize::MAX, 1, TEXT);
        c.fill_rect(4, 4, 0, 3, SUN);
        assert_eq!(c.pixel(0, 0), Some(SUN));
        assert_eq!(c.pixel(2, 1), Some(SUN));
        assert_eq!(c.pixel(3, 1), Some(NIGHT));
        assert_eq!(c.pixel(2, 2), Some(NIGHT));
        assert_eq!(c.pixel(15, 8), Some(TEXT));
        assert_eq!(c.pixel(19, 9), Some(TEXT));
        assert_eq!(c.pixel(14, 8), Some(NIGHT));
        assert_eq!(c.pixel(4, 4), Some(NIGHT), "an empty rectangle");
        assert!(
            (0..20).all(|x| c.pixel(x, 5) == Some(TEXT)),
            "a row from far left to far right"
        );
        assert_eq!(c.pixel(20, 0), None);
        assert_eq!(c.pixel(0, 10), None);
        assert!(buf.padding_untouched());
    }

    #[test]
    fn text_blends_between_background_and_colour() {
        let mut buf = Buffer::new(200, 40);
        let c = &mut buf.canvas(Format::Bgr);
        let s = style(20.0, TEXT);
        let width = c.text(&s, 4, 28, "Meridian", &mut Scratch::new()).unwrap();
        let laid = s.font.layout("Meridian", 20.0, |_| {}).unwrap();
        assert_eq!(width, ceil(laid));
        assert!((80..120).contains(&width), "{width}");

        let mut inked = 0;
        for y in 0..40 {
            for x in 0..200 {
                let p = c.pixel(x, y).unwrap();
                if p == NIGHT {
                    continue;
                }
                inked += 1;
                assert!(p.r >= NIGHT.r && p.r <= TEXT.r, "({x}, {y}) {p:?}");
                assert!(p.b >= NIGHT.b && p.b <= TEXT.b, "({x}, {y}) {p:?}");
                assert!((4..4 + width as usize + 1).contains(&x), "x {x}");
                assert!((28 - 16..28 + 1).contains(&y), "y {y}");
            }
        }
        assert!(inked > 150, "{inked} pixels of ink");
        assert!(buf.padding_untouched());
    }

    #[test]
    fn straight_stems_get_the_full_colour() {
        let mut buf = Buffer::new(40, 50);
        let mut c = buf.canvas(Format::Rgb);
        c.text(&style(40.0, SUN), 10, 40, "I", &mut Scratch::new())
            .unwrap();
        let full = (0..50)
            .flat_map(|y| (0..40).map(move |x| (x, y)))
            .filter(|&(x, y)| c.pixel(x, y) == Some(SUN))
            .count();
        assert!(full >= 3 * 25, "{full} pixels are the colour itself");
    }

    #[test]
    fn text_off_the_edges_is_cut_not_wrapped() {
        let mut buf = Buffer::new(30, 20);
        let c = &mut buf.canvas(Format::Bgr);
        let mut scratch = Scratch::new();
        let s = style(18.0, TEXT);
        c.text(&s, -12, 14, "Wide", &mut scratch).unwrap();
        c.text(&s, 20, 14, "Wide", &mut scratch).unwrap();
        c.text(&s, 0, 2, "Top", &mut scratch).unwrap();
        c.text(&s, 0, 30, "Below", &mut scratch).unwrap();
        c.text(&s, i32::MIN / 2, i32::MAX / 2, "Far", &mut scratch)
            .unwrap();
        assert!(buf.padding_untouched());
    }

    #[test]
    fn nothing_to_draw() {
        let mut buf = Buffer::new(30, 20);
        let mut c = buf.canvas(Format::Bgr);
        assert_eq!(
            c.text(&style(16.0, TEXT), 0, 15, "", &mut Scratch::new()),
            Ok(0)
        );
        assert_eq!(
            c.text(&style(16.0, TEXT), 0, 15, "   ", &mut Scratch::default())
                .map(|w| w > 6),
            Ok(true)
        );
        assert!((0..20).all(|y| (0..30).all(|x| c.pixel(x, y) == Some(NIGHT))));
    }

    #[test]
    fn glyphs_beyond_the_limit_are_refused() {
        let mut buf = Buffer::new(300, 300);
        let mut c = buf.canvas(Format::Bgr);
        let result = c.text(&style(120.0, TEXT), 0, 150, "M", &mut Scratch::new());
        assert_eq!(result, Err(Error::BufferTooSmall));
        assert_eq!(
            c.text(&style(0.0, TEXT), 0, 150, "M", &mut Scratch::new()),
            Err(Error::BadSize)
        );
    }

    /// Fills a canvas whose unused pixel bytes are set, which drawing would
    /// clear, so a pixel that was written at all can be told apart.
    fn marked(pixels: &mut [u32], w: usize, h: usize) -> Canvas<'_> {
        let c = Canvas::new(pixels, w, h, w, Format::Bgr).unwrap();
        let night = NIGHT.encode(Format::Bgr) | 0xff00_0000;
        c.pixels.fill(night);
        c
    }

    #[test]
    fn pixels_without_coverage_are_left_alone() {
        let mut px = vec![0; 60 * 30];
        let c = &mut marked(&mut px, 60, 30);
        c.text(&style(16.0, TEXT), 2, 20, "o.o", &mut Scratch::new())
            .unwrap();
        let (mut written, mut kept) = (0, 0);
        for &p in c.pixels.iter() {
            if p >> 24 == 0xff {
                kept += 1;
                assert_eq!(p & 0xff_ffff, NIGHT.encode(Format::Bgr));
            } else {
                written += 1;
                assert_ne!(p, NIGHT.encode(Format::Bgr), "written without changing");
            }
        }
        assert!(
            written > 40 && kept > 1500,
            "{written} written, {kept} kept"
        );
    }

    /// The second glyph starts between pixels; its bitmap must be the one
    /// rasterized for that fraction, placed at the whole pixel left of it.
    #[test]
    fn glyphs_keep_their_fraction_of_a_pixel() {
        let s = style(15.0, TEXT);
        let glyph = s.font.glyph_index('l').unwrap();
        let mut pens = Vec::new();
        s.font.layout("ll", 15.0, |g| pens.push(g.x)).unwrap();
        let second = pens[1];
        assert!(
            second.fract() > 0.1 && second.fract() < 0.9,
            "pen at {second}"
        );

        let mut acc = vec![0.0; 4096];
        let mut want = vec![0u8; 4096];
        let placed = s
            .font
            .rasterize(glyph, 15.0, second.fract(), &mut acc, &mut want)
            .unwrap()
            .unwrap();
        let mut buf = Buffer::new(40, 30);
        let canvas = &mut buf.canvas(Format::Bgr);
        canvas.text(&s, 0, 20, "ll", &mut Scratch::new()).unwrap();
        let left = floor(second) + placed.left;
        for gy in 0..placed.height {
            for gx in 0..placed.width {
                let x = usize::try_from(left).unwrap() + gx;
                let y = usize::try_from(20 + placed.top).unwrap() + gy;
                let expected = NIGHT.blend(TEXT, want[gy * placed.width + gx]);
                assert_eq!(canvas.pixel(x, y), Some(expected), "({gx}, {gy})");
            }
        }
    }

    #[test]
    fn coverage_blends_by_how_much_of_each_pixel_it_covers() {
        let mut buf = Buffer::new(6, 4);
        let mut c = buf.canvas(Format::Rgb);
        c.coverage(1, 1, 3, &[255, 128, 0, 0, 64, 255], SUN);
        assert_eq!(c.pixel(1, 1), Some(SUN));
        assert_eq!(c.pixel(2, 1), Some(NIGHT.blend(SUN, 128)));
        assert_eq!(c.pixel(3, 1), Some(NIGHT), "no coverage, no change");
        assert_eq!(c.pixel(1, 2), Some(NIGHT));
        assert_eq!(c.pixel(2, 2), Some(NIGHT.blend(SUN, 64)));
        assert_eq!(c.pixel(3, 2), Some(SUN));
        let outside = [(0, 0), (4, 1), (1, 0), (1, 3), (5, 3)];
        assert!(outside.iter().all(|&(x, y)| c.pixel(x, y) == Some(NIGHT)));
        assert!(buf.padding_untouched());
    }

    #[test]
    fn coverage_off_the_canvas_is_left_out() {
        let mut buf = Buffer::new(4, 3);
        let mut c = buf.canvas(Format::Bgr);
        c.coverage(-1, -1, 2, &[255; 4], TEXT);
        assert_eq!(c.pixel(0, 0), Some(TEXT), "the one pixel on the canvas");
        assert_eq!((c.pixel(1, 0), c.pixel(0, 1)), (Some(NIGHT), Some(NIGHT)));
        c.coverage(3, 2, 3, &[255; 6], TEXT);
        assert_eq!(c.pixel(3, 2), Some(TEXT));
        c.coverage(i32::MAX, i32::MIN, 2, &[255; 4], TEXT);
        c.coverage(0, 0, 0, &[255; 4], TEXT);
        assert_eq!(c.pixel(1, 1), Some(NIGHT), "nothing is 0 pixels wide");
        let lit = (0..3).flat_map(|y| (0..4).map(move |x| (x, y)));
        assert_eq!(lit.filter(|&(x, y)| c.pixel(x, y) == Some(TEXT)).count(), 2);
        assert!(buf.padding_untouched());
    }

    #[test]
    fn a_character_rasterized_once_draws_as_text_draws_it() {
        let s = style(18.0, TEXT);
        let mut scratch = Scratch::new();
        let (p, coverage) = rasterize(&s, 'M', &mut scratch).unwrap().unwrap();
        let coverage = coverage.to_vec();
        assert_eq!(coverage.len(), p.width * p.height);
        assert!(p.top < 0 && p.width > 5, "{p:?}");
        let mut from_text = Buffer::new(30, 30);
        let mut from_mask = Buffer::new(30, 30);
        from_text
            .canvas(Format::Bgr)
            .text(&s, 7, 22, "M", &mut scratch)
            .unwrap();
        from_mask
            .canvas(Format::Bgr)
            .coverage(7 + p.left, 22 + p.top, p.width, &coverage, TEXT);
        assert_eq!(from_text.pixels, from_mask.pixels);
        assert_eq!(rasterize(&s, ' ', &mut scratch).unwrap(), None);
        let lacking = rasterize(&s, '\u{10ffff}', &mut scratch).unwrap();
        assert!(
            lacking.is_some_and(|(p, _)| p.width > 0),
            "the missing-glyph box"
        );
        assert_eq!(
            rasterize(&style(120.0, TEXT), 'M', &mut scratch),
            Err(Error::BufferTooSmall)
        );
    }

    #[test]
    fn a_failing_glyph_fails_the_whole_line() {
        let mut buf = Buffer::new(300, 120);
        let c = &mut buf.canvas(Format::Bgr);
        let result = c.text(&style(100.0, TEXT), 0, 100, "M.", &mut Scratch::new());
        assert_eq!(result, Err(Error::BufferTooSmall));
        let dot = c.text(&style(100.0, TEXT), 0, 100, ".", &mut Scratch::new());
        assert!(dot.is_ok(), "the dot alone fits");
    }

    #[test]
    fn both_formats_show_the_same_colours() {
        let mut rgb = Buffer::new(80, 30);
        let mut bgr = Buffer::new(80, 30);
        let (a, b) = (&mut rgb.canvas(Format::Rgb), &mut bgr.canvas(Format::Bgr));
        let s = style(18.0, palette::SKY);
        a.text(&s, 3, 22, "Sky 42", &mut Scratch::new()).unwrap();
        b.text(&s, 3, 22, "Sky 42", &mut Scratch::new()).unwrap();
        for y in 0..30 {
            for x in 0..80 {
                assert_eq!(a.pixel(x, y), b.pixel(x, y), "({x}, {y})");
            }
        }
        assert_ne!(
            rgb.pixels, bgr.pixels,
            "the bytes differ even though the colours agree"
        );
    }

    #[test]
    fn scratch_space_can_be_reused() {
        let mut scratch = Scratch::new();
        let mut once = Buffer::new(120, 60);
        let mut reused = Buffer::new(120, 60);
        once.canvas(Format::Bgr)
            .text(&style(14.0, TEXT), 2, 40, "small", &mut Scratch::new())
            .unwrap();
        let big = &mut Buffer::new(200, 80);
        big.canvas(Format::Bgr)
            .text(&style(40.0, TEXT), 0, 60, "BIG", &mut scratch)
            .unwrap();
        reused
            .canvas(Format::Bgr)
            .text(&style(14.0, TEXT), 2, 40, "small", &mut scratch)
            .unwrap();
        assert_eq!(once.pixels, reused.pixels);
    }

    #[test]
    fn drawing_twice_goes_further_towards_the_colour() {
        let mut buf = Buffer::new(60, 30);
        let c = &mut buf.canvas(Format::Bgr);
        let s = style(16.0, SUN);
        c.text(&s, 2, 20, "a", &mut Scratch::new()).unwrap();
        let first: Vec<_> = (0..30)
            .flat_map(|y| (0..60).map(move |x| (x, y)))
            .map(|(x, y)| c.pixel(x, y).unwrap())
            .collect();
        c.text(&s, 2, 20, "a", &mut Scratch::new()).unwrap();
        let mut closer = 0;
        for (i, before) in first.iter().enumerate() {
            let after = c.pixel(i % 60, i / 60).unwrap();
            assert!(after.g >= before.g, "pixel {i} went back");
            closer += usize::from(after != *before);
        }
        assert!(closer > 10, "{closer}");
    }

    #[test]
    fn characters_the_font_lacks_draw_a_box() {
        let mut buf = Buffer::new(40, 30);
        let c = &mut buf.canvas(Format::Bgr);
        let width = c
            .text(&style(16.0, TEXT), 2, 20, "\u{10fff0}", &mut Scratch::new())
            .unwrap();
        assert!(width > 4);
        let inked = (0..30)
            .flat_map(|y| (0..40).map(move |x| (x, y)))
            .filter(|&(x, y)| c.pixel(x, y) != Some(NIGHT))
            .count();
        assert!(inked > 20, "{inked}");
    }

    #[test]
    fn rounding_helpers() {
        assert_eq!(floor(2.0), 2);
        assert_eq!(floor(2.7), 2);
        assert_eq!(floor(-0.5), -1);
        assert_eq!(floor(-2.0), -2);
        assert_eq!(ceil(0.0), 0);
        assert_eq!(ceil(2.0), 2);
        assert_eq!(ceil(2.1), 3);
    }

    #[test]
    fn clipping_ranges() {
        assert_eq!(clip(0, 5, 10), (0, 5));
        assert_eq!(clip(-3, 5, 10), (0, 2));
        assert_eq!(clip(8, 5, 10), (8, 10));
        assert_eq!(clip(12, 5, 10), (10, 10));
        assert_eq!(clip(-9, 5, 10), (0, 0));
        assert_eq!(clip(i32::MAX, usize::MAX, 10), (10, 10));
        assert_eq!(clip(i32::MIN, usize::MAX, 10), (0, 10));
        assert_eq!(clip(-3, 2, 10), (0, 0));
        assert_eq!(clip(3, 0, 10), (3, 3));
        assert_eq!(on_canvas(-2, 1, 5), None);
        assert_eq!(on_canvas(-2, 2, 5), Some(0));
        assert_eq!(on_canvas(3, 1, 5), Some(4));
        assert_eq!(on_canvas(3, 2, 5), None);
    }

    #[test]
    fn the_palette_has_its_documented_values() {
        assert_eq!(
            SUN,
            Color {
                r: 0xff,
                g: 0xc6,
                b: 0x2b
            }
        );
        assert_eq!(palette::TANGERINE, Color::hex(0xff_9633));
        assert_eq!(palette::INK, Color::hex(0x16_181b));
        assert_ne!(palette::TEXT, palette::MUTED);
    }
}
