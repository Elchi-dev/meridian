//! Broken fonts: cut short and with bits flipped. Whatever the bytes say,
//! the crate must answer with values or errors, never panic, and finish.

// clippy.toml allows unwrapping in tests, but not in the helpers of
// integration tests; and test code converts between number types freely,
// where a wrong conversion fails the test.
#![allow(
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod common;

use std::time::{Duration, Instant};

use common::Rng;
use common::build::{
    Component, FontBuilder, Glyph, Scale, Segment, cmap, component, composite, format4, format12,
    kern, off, on, simple,
};
use font::{Font, OutlineSink, Point};

const INTER_REGULAR: &[u8] = include_bytes!("../../../third-party/fonts/Inter-Regular.ttf");

/// Counts commands without keeping them, so a hostile outline costs no
/// memory.
struct Count(usize);

impl OutlineSink for Count {
    fn move_to(&mut self, _: Point) {
        self.0 += 1;
    }
    fn line_to(&mut self, _: Point) {
        self.0 += 1;
    }
    fn quad_to(&mut self, _: Point, _: Point) {
        self.0 += 1;
    }
    fn close(&mut self) {
        self.0 += 1;
    }
}

struct Buffers {
    scratch: Vec<f32>,
    out: Vec<u8>,
    slowest: Duration,
    /// Glyphs that still rasterized, to show the broken fonts got past
    /// parsing often enough to test the rest.
    drawn: usize,
}

impl Buffers {
    fn new() -> Buffers {
        Buffers {
            scratch: vec![0.0; 1 << 16],
            out: vec![0; 1 << 16],
            slowest: Duration::ZERO,
            drawn: 0,
        }
    }

    /// Runs everything the crate offers over a font, ignoring the answers.
    fn exercise(&mut self, data: &[u8], text: &str) {
        let start = Instant::now();
        if let Ok(font) = Font::parse(data) {
            let _ = (font.units_per_em(), font.line_metrics(16.0));
            let mut glyphs: Vec<u16> = text.chars().filter_map(|c| font.glyph_index(c)).collect();
            glyphs.extend(0..font.glyph_count().min(4));
            for g in glyphs {
                let _ = font.h_metrics(g);
                let _ = font.bounds(g);
                let _ = font.kerning(g, g.wrapping_add(1));
                let _ = font.outline(g, &mut Count(0));
                for (px, shift) in [(16.0, 0.25), (5.0, 1.0)] {
                    let r = font.rasterize(g, px, shift, &mut self.scratch, &mut self.out);
                    self.drawn += usize::from(matches!(r, Ok(Some(_))));
                }
            }
            let _ = font.layout(text, 16.0, |_| {});
        }
        self.slowest = self.slowest.max(start.elapsed());
    }

    fn assert_quick_and_drawn(&self, at_least: usize) {
        // Far more than any of these fonts needs; a loop that does not end
        // would never get here, and one that runs away shows up.
        assert!(self.slowest < Duration::from_secs(2), "{:?}", self.slowest);
        assert!(self.drawn >= at_least, "only {} glyphs drawn", self.drawn);
    }
}

/// A small font with every feature the crate reads.
fn small_font() -> Vec<u8> {
    let square = simple(&[&[on(0, 0), on(0, 700), on(500, 700), on(500, 0)]], &[]);
    let curves = simple(
        &[
            &[off(0, 0), on(100, 0), on(100, 100)],
            &[off(0, 0), off(100, 0), off(100, 100), off(0, 100)],
            &[
                on(0, 0),
                off(50, 300),
                off(450, 300),
                on(500, 0),
                on(800, -400),
            ],
        ],
        &[1, 2, 3],
    );
    let glyphs = vec![
        Glyph {
            advance: 500,
            lsb: 0,
            data: square.clone(),
        },
        Glyph {
            advance: 600,
            lsb: 0,
            data: curves,
        },
        Glyph {
            advance: 250,
            lsb: 0,
            data: Vec::new(),
        },
        Glyph {
            advance: 600,
            lsb: 0,
            data: composite(
                &[
                    component(1, 100, -50),
                    Component {
                        scale: Scale::Matrix(0.5, 0.25, -0.25, 0.5),
                        flags: 0x0800,
                        ..component(0, 1000, 20)
                    },
                ],
                [0, -50, 1500, 700],
            ),
        },
        Glyph {
            advance: 700,
            lsb: 0,
            data: composite(
                &[component(3, 0, 0), component(1, -300, 0)],
                [-300, -50, 1500, 700],
            ),
        },
    ];
    let bmp = format4(&[
        Segment {
            start: 0x20,
            end: 0x20,
            delta: 2 - 0x20,
            glyphs: None,
        },
        Segment {
            start: 0x41,
            end: 0x44,
            delta: 0,
            glyphs: Some(vec![0, 1, 3, 4]),
        },
        Segment {
            start: 0xffff,
            end: 0xffff,
            delta: 1,
            glyphs: None,
        },
    ]);
    let full = format12(&[(0x41, 0x44, 1), (0x1f600, 0x1f600, 4)]);
    let mut b = FontBuilder::new(glyphs, cmap(&[(3, 1, bmp), (3, 10, full)]));
    b.h_metrics = 3;
    b.kern = Some(kern(&[(1, 3, -40), (3, 4, 25)], 1));
    b.build()
}

const SMALL_TEXT: &str = "ABCD A\u{1f600}";

#[test]
fn small_font_works_whole() {
    let data = small_font();
    let font = Font::parse(&data).unwrap();
    for g in 0..font.glyph_count() {
        font.outline(g, &mut Count(0)).unwrap();
    }
    assert_eq!(font.glyph_index('D'), Some(4));
    assert_eq!(font.kerning(1, 3), -40);
}

#[test]
fn small_font_cut_anywhere() {
    let data = small_font();
    let mut b = Buffers::new();
    for len in 0..data.len() {
        b.exercise(&data[..len], SMALL_TEXT);
    }
    b.assert_quick_and_drawn(16);
}

#[test]
fn small_font_with_any_bit_flipped() {
    let mut data = small_font();
    let mut b = Buffers::new();
    for byte in 0..data.len() {
        for bit in 0..8 {
            data[byte] ^= 1 << bit;
            b.exercise(&data, SMALL_TEXT);
            data[byte] ^= 1 << bit;
        }
    }
    b.assert_quick_and_drawn(20_000);
}

#[test]
fn small_font_with_random_bytes() {
    let mut data = small_font();
    let clean = data.clone();
    let mut b = Buffers::new();
    let mut rng = Rng(0x0dd_ba11);
    for _ in 0..5000 {
        for _ in 0..=rng.below(6) {
            let at = rng.below(data.len());
            data[at] = rng.next() as u8;
        }
        b.exercise(&data, SMALL_TEXT);
        data.copy_from_slice(&clean);
    }
    b.assert_quick_and_drawn(10_000);
}

const INTER_TEXT: &str = "Hä€o";

#[test]
fn real_font_cut_short() {
    let mut b = Buffers::new();
    let lengths = (0..2048).chain((2048..INTER_REGULAR.len()).step_by(4099));
    for len in lengths {
        b.exercise(&INTER_REGULAR[..len], INTER_TEXT);
    }
    b.assert_quick_and_drawn(50);
}

/// The parts of the real font the exercise reads: its directory, the
/// small tables, the start of `cmap`, and the glyphs it draws.
fn hot_regions(data: &[u8]) -> Vec<(usize, usize)> {
    let raw = common::Raw { data };
    let font = Font::parse(data).unwrap();
    let mut regions = vec![(0, 12 + 16 * raw.u16(4) as usize)];
    for tag in [b"head", b"maxp", b"hhea"] {
        regions.push(raw.table(tag).unwrap());
    }
    let (cmap, len) = raw.table(b"cmap").unwrap();
    regions.push((cmap, len.min(1024)));
    let (loca, _) = raw.table(b"loca").unwrap();
    let (glyf, _) = raw.table(b"glyf").unwrap();
    for c in INTER_TEXT.chars().chain(['a']) {
        let g = font.glyph_index(c).unwrap() as usize;
        regions.push((loca + 4 * g, 8));
        let start = raw.u32(loca + 4 * g) as usize;
        let end = raw.u32(loca + 4 * g + 4) as usize;
        regions.push((glyf + start, end - start));
    }
    regions
}

#[test]
fn real_font_with_bits_flipped() {
    let mut data = INTER_REGULAR.to_vec();
    let regions = hot_regions(INTER_REGULAR);
    let mut b = Buffers::new();
    let mut rng = Rng(0xfeed_beef);
    for round in 0..4000 {
        let (start, len) = regions[rng.below(regions.len())];
        let flips: Vec<(usize, u8)> = (0..=round % 3)
            .map(|_| (start + rng.below(len), 1 << rng.below(8)))
            .collect();
        for &(at, bit) in &flips {
            data[at] ^= bit;
        }
        b.exercise(&data, INTER_TEXT);
        for &(at, bit) in &flips {
            data[at] ^= bit;
        }
    }
    assert_eq!(data, INTER_REGULAR);
    b.assert_quick_and_drawn(20_000);
}
