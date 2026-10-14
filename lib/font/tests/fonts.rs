//! The shipped fonts, checked against values the tests read from the files
//! themselves.

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

use std::time::Instant;

use common::{Raw, Recorder, exact_coverage, expected_byte};
use font::{Error, Font, MAX_PIXEL_SIZE, Placement, Point, Positioned};

const INTER_REGULAR: &[u8] = include_bytes!("../../../third-party/fonts/Inter-Regular.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../../../third-party/fonts/JetBrainsMono-Regular.ttf");

const FONTS: [(&str, &[u8]); 6] = [
    ("Inter Regular", INTER_REGULAR),
    (
        "Inter Medium",
        include_bytes!("../../../third-party/fonts/Inter-Medium.ttf"),
    ),
    (
        "Inter SemiBold",
        include_bytes!("../../../third-party/fonts/Inter-SemiBold.ttf"),
    ),
    (
        "Inter Bold",
        include_bytes!("../../../third-party/fonts/Inter-Bold.ttf"),
    ),
    ("JetBrains Mono Regular", MONO_REGULAR),
    (
        "JetBrains Mono Bold",
        include_bytes!("../../../third-party/fonts/JetBrainsMono-Bold.ttf"),
    ),
];

fn parse(data: &[u8]) -> Font<'_> {
    Font::parse(data).unwrap()
}

fn glyph(font: &Font<'_>, c: char) -> u16 {
    font.glyph_index(c).unwrap()
}

/// A glyph rasterized into buffers of the size it asks for.
fn render(font: &Font<'_>, glyph: u16, px: f32, shift: f32) -> (Placement, Vec<u8>) {
    let p = font.placement(glyph, px, shift).unwrap().unwrap();
    let mut scratch = vec![0.0; p.scratch_len()];
    let mut out = vec![0; p.width * p.height];
    let placed = font.rasterize(glyph, px, shift, &mut scratch, &mut out);
    assert_eq!(placed, Ok(Some(p)));
    (p, out)
}

fn outline(font: &Font<'_>, glyph: u16) -> Recorder {
    let mut r = Recorder::default();
    font.outline(glyph, &mut r).unwrap();
    r
}

#[test]
fn header_values_match_the_files() {
    for (name, data) in FONTS {
        let (font, raw) = (parse(data), Raw { data });
        assert_eq!(font.units_per_em(), raw.units_per_em(), "{name}");
        assert_eq!(font.glyph_count(), raw.glyph_count(), "{name}");
        let want = if name.starts_with("Inter") {
            2048
        } else {
            1000
        };
        assert_eq!(font.units_per_em(), want, "{name}");
        assert_eq!(font.scale(32.0), 32.0 / f32::from(want));
    }
}

#[test]
fn line_metrics_scale_hhea() {
    for (name, data) in FONTS {
        let (font, raw) = (parse(data), Raw { data });
        let [ascent, descent, gap] = raw.line().map(f32::from);
        let s = 20.0 / f32::from(raw.units_per_em());
        let m = font.line_metrics(20.0);
        assert_eq!(m.ascent, ascent * s, "{name}");
        assert_eq!(m.descent, descent * s, "{name}");
        assert_eq!(m.line_gap, gap * s, "{name}");
        assert!(m.ascent > 0.0 && m.descent < 0.0, "{name}");
        assert_eq!(m.line_height(), (ascent - descent + gap) * s, "{name}");
    }
}

#[test]
fn advances_and_bearings_match_hmtx() {
    for (name, data) in FONTS {
        let (font, raw) = (parse(data), Raw { data });
        for g in 0..font.glyph_count() {
            assert_eq!(font.h_metrics(g), Ok(raw.h_metrics(g)), "{name} glyph {g}");
        }
        assert!(font.h_metrics(font.glyph_count()).is_err());
    }
}

#[test]
fn every_mapped_character_finds_its_glyph() {
    for (name, data) in FONTS {
        let (font, raw) = (parse(data), Raw { data });
        let pairs = raw.format12_pairs();
        assert!(pairs.len() > 500, "{name}");
        for &(c, g) in &pairs {
            let Some(c) = char::from_u32(c) else { continue };
            let want = (g != 0).then_some(g);
            assert_eq!(font.glyph_index(c), want, "{name} {c:?}");
        }
        // Characters between the mapped ones have no glyph.
        let mapped: std::collections::HashSet<u32> = pairs.iter().map(|p| p.0).collect();
        let missing = (0..0x3_0000)
            .filter(|c| !mapped.contains(c))
            .filter_map(char::from_u32);
        for c in missing {
            assert_eq!(font.glyph_index(c), None, "{name} {c:?}");
        }
    }
}

#[test]
fn common_characters_are_there() {
    for (name, data) in FONTS {
        let (font, raw) = (parse(data), Raw { data });
        for c in (' '..='~').chain(['ä', 'ö', 'ü', 'Ä', 'ß', '€']) {
            let want = raw.format4(c as u16);
            assert_ne!(want, 0, "{name} {c:?}");
            assert_eq!(font.glyph_index(c), Some(want), "{name} {c:?}");
        }
    }
}

/// The font with its full repertoire subtables relabelled, so the crate
/// falls back to the format 4 subtable.
fn without_format12(data: &[u8]) -> Vec<u8> {
    let raw = Raw { data };
    let mut copy = data.to_vec();
    let (cmap, _) = raw.table(b"cmap").unwrap();
    for i in 0..raw.u16(cmap + 2) as usize {
        let record = cmap + 4 + 8 * i;
        if raw.u16(cmap + raw.u32(record + 4) as usize) == 12 {
            copy[record + 2..record + 4].copy_from_slice(&0xbeef_u16.to_be_bytes());
        }
    }
    copy
}

#[test]
fn format4_agrees_with_format12() {
    for (name, data) in FONTS {
        let copy = without_format12(data);
        let (full, bmp, raw) = (parse(data), parse(&copy), Raw { data });
        let mut found = 0;
        for c in (0..=0xffff).filter_map(char::from_u32) {
            let want = raw.format4(c as u16);
            assert_eq!(
                bmp.glyph_index(c),
                (want != 0).then_some(want),
                "{name} {c:?}"
            );
            assert_eq!(bmp.glyph_index(c), full.glyph_index(c), "{name} {c:?}");
            found += usize::from(want != 0);
        }
        assert!(found > 500, "{name}");
        // Format 4 cannot reach past the first plane.
        let emoji = '\u{1f600}';
        assert_eq!(bmp.glyph_index(emoji), None, "{name}");
    }
}

#[test]
fn coverage_sums_to_the_outline_area() {
    let text = "oIHagäß€8&@Qe";
    for (name, data) in FONTS {
        let font = parse(data);
        for px in [16.0, 40.0] {
            let s = f64::from(font.scale(px));
            for c in text.chars() {
                let g = glyph(&font, c);
                let r = outline(&font, g);
                let area = r.area().abs() * s * s;
                let (_, out) = render(&font, g, px, 0.3);
                let sum: f64 = out.iter().map(|&b| f64::from(b) / 255.0).sum();
                let partial = out.iter().filter(|&&b| b != 0 && b != 255).count();
                // Rounding to 1/255 per partly covered pixel, and what
                // flattening may lose along the curves.
                let tolerance =
                    partial as f64 * 0.5 / 255.0 + 2.0 / 3.0 * 0.05 * r.curve_length * s + 1e-3;
                assert!(
                    (sum - area).abs() <= tolerance,
                    "{name} {c:?} at {px}: sum {sum}, area {area}, tolerance {tolerance}"
                );
            }
        }
    }
}

#[test]
fn straight_glyphs_match_exact_coverage() {
    let font = parse(INTER_REGULAR);
    for c in ['I', 'H', 'L', 'E', 'T', 'l', 'k', '-', '+', '#'] {
        let g = glyph(&font, c);
        let shape = outline(&font, g);
        assert_eq!(shape.curve_length, 0.0, "{c:?} has curves");
        for (px, shift) in [(16.0, 0.0), (23.0, 0.37), (11.0, 0.9)] {
            let (p, out) = render(&font, g, px, shift);
            let scale = f64::from(font.scale(px));
            let map = |q: Point| {
                [
                    f64::from(q.x) * scale + f64::from(shift) - f64::from(p.left),
                    -f64::from(p.top) - f64::from(q.y) * scale,
                ]
            };
            let mut contours = Vec::new();
            for command in &shape.commands {
                match *command {
                    common::Command::Move(q) => contours.push(vec![map(q)]),
                    common::Command::Line(q) => contours.last_mut().unwrap().push(map(q)),
                    common::Command::Quad(..) => unreachable!(),
                    common::Command::Close => {}
                }
            }
            for y in 0..p.height {
                for x in 0..p.width {
                    let want = expected_byte(exact_coverage(&contours, x, y));
                    let got = out[y * p.width + x];
                    assert!(
                        got.abs_diff(want) <= 1,
                        "{c:?} at {px}: ({x}, {y}) {got} != {want}"
                    );
                }
            }
        }
    }
}

/// How far, in font units, the outline is from its mirror image about
/// `axis`: the largest distance from a mirrored point to the nearest point.
fn asymmetry(points: &[Point], axis: f32) -> f32 {
    let nearest = |m: Point| {
        points
            .iter()
            .map(|q| (q.x - m.x).abs().max((q.y - m.y).abs()))
            .fold(f32::INFINITY, f32::min)
    };
    points
        .iter()
        .map(|q| nearest(Point::new(2.0 * axis - q.x, q.y)))
        .fold(0.0, f32::max)
}

#[test]
fn symmetric_glyphs_render_symmetric() {
    for (name, data) in FONTS {
        let font = parse(data);
        for c in ['o', 'I', 'H'] {
            let g = glyph(&font, c);
            let [x_min, _, x_max, _] = font.bounds(g).unwrap().unwrap();
            let middle = f32::midpoint(f32::from(x_min), f32::from(x_max));
            // Designers place points on whole units, so a symmetric glyph
            // may be off by a unit or so.
            let off = asymmetry(&outline(&font, g).points(), middle);
            assert!(off <= 2.0, "{name} {c:?} is {off} units from symmetric");
            for px in [12.0, 16.0, 21.0, 32.0] {
                // Put the glyph's axis on a pixel edge or a pixel's middle,
                // where the grid is symmetric too.
                let axis = middle * font.scale(px);
                let shift = (axis * 2.0).ceil() / 2.0 - axis;
                let (p, out) = render(&font, g, px, shift);
                assert!(p.width > 1 && p.height > 4, "{name} {c:?}");
                // An edge moved by d pixels changes a pixel by at most d,
                // and rounding adds a step.
                let tolerance = 1 + (255.0 * off * font.scale(px)).ceil() as u8;
                for row in out.chunks(p.width) {
                    for x in 0..p.width / 2 {
                        let (l, r) = (row[x], row[p.width - 1 - x]);
                        assert!(
                            l.abs_diff(r) <= tolerance,
                            "{name} {c:?} at {px}: {row:?} is not symmetric"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn outlines_stay_in_their_boxes() {
    // The compiler wrote each glyph's box from its points, components
    // moved and scaled, so a misplaced component shows up here.
    for (name, data) in FONTS {
        let font = parse(data);
        let mut composites = 0;
        for g in 0..font.glyph_count() {
            let r = outline(&font, g);
            let Some([x0, y0, x1, y1]) = font.bounds(g).unwrap() else {
                assert!(r.commands.is_empty(), "{name} glyph {g}");
                continue;
            };
            let (x0, y0, x1, y1) = (f32::from(x0), f32::from(y0), f32::from(x1), f32::from(y1));
            let points = r.points();
            for q in &points {
                let inside =
                    q.x >= x0 - 1.0 && q.x <= x1 + 1.0 && q.y >= y0 - 1.0 && q.y <= y1 + 1.0;
                assert!(
                    inside,
                    "{name} glyph {g}: {q:?} outside {x0} {y0} {x1} {y1}"
                );
            }
            let touches =
                |f: fn(&Point) -> f32, v: f32| points.iter().any(|q| (f(q) - v).abs() <= 1.0);
            assert!(
                touches(|q| q.x, x0) && touches(|q| q.x, x1),
                "{name} glyph {g}"
            );
            assert!(
                touches(|q| q.y, y0) && touches(|q| q.y, y1),
                "{name} glyph {g}"
            );
            let raw = Raw { data };
            let (glyf, _) = raw.table(b"glyf").unwrap();
            let (loca, _) = raw.table(b"loca").unwrap();
            let at = glyf + raw.u32(loca + 4 * g as usize) as usize;
            composites += usize::from(raw.i16(at) < 0);
        }
        assert!(composites > 100, "{name} has {composites} composite glyphs");
    }
}

#[test]
fn every_glyph_rasterizes() {
    for (name, data) in FONTS {
        let font = parse(data);
        let mut drawn = 0;
        for g in 0..font.glyph_count() {
            let Some(p) = font.placement(g, 16.0, 0.5).unwrap() else {
                continue;
            };
            let mut scratch = vec![0.0; p.scratch_len()];
            let mut out = vec![0; p.width * p.height];
            assert_eq!(
                font.rasterize(g, 16.0, 0.5, &mut scratch, &mut out),
                Ok(Some(p)),
                "{name} glyph {g}"
            );
            if outline(&font, g).area() != 0.0 {
                assert!(out.iter().any(|&b| b > 0), "{name} glyph {g} is blank");
                drawn += 1;
            }
        }
        assert!(drawn > 1000, "{name}");
    }
}

#[test]
fn composite_umlaut_is_the_letter_and_its_dots() {
    let font = parse(INTER_REGULAR);
    let raw = Raw {
        data: INTER_REGULAR,
    };
    let g = glyph(&font, 'ä');
    let (glyf, _) = raw.table(b"glyf").unwrap();
    let (loca, _) = raw.table(b"loca").unwrap();
    let at = glyf + raw.u32(loca + 4 * g as usize) as usize;
    assert_eq!(raw.i16(at), -1, "ä is drawn from components");
    let a = outline(&font, glyph(&font, 'a'));
    let umlaut = outline(&font, g);
    assert!((umlaut.area() - a.area()).abs() > 1000.0);
    // The first component is the plain letter, where it stands alone.
    assert_eq!(umlaut.commands[..a.commands.len()], a.commands[..]);
}

#[test]
fn layout_adds_up_advances() {
    for (name, data) in FONTS {
        let font = parse(data);
        let raw = Raw { data };
        let text = "Hallo, Wörld! 1 € = ß\u{10fffd}";
        let mut placed = Vec::new();
        let width = font.layout(text, 18.0, |p| placed.push(p)).unwrap();
        assert_eq!(placed.len(), text.chars().count(), "{name}");
        let s = 18.0 / f32::from(raw.units_per_em());
        let mut pen = 0.0;
        for (p, c) in placed.iter().zip(text.chars()) {
            let g = font.glyph_index(c).unwrap_or(0);
            assert_eq!(*p, Positioned { glyph: g, x: pen }, "{name} {c:?}");
            pen += f32::from(raw.h_metrics(g).0) * s;
        }
        assert_eq!(width, pen, "{name}");
        assert_eq!(placed.last().unwrap().glyph, 0, "{name}: missing glyph box");
        // These fonts kern with GPOS only, which Meridian does not read.
        let (h, a) = (glyph(&font, 'A'), glyph(&font, 'V'));
        assert_eq!(font.kerning(h, a), 0, "{name}");
        assert_eq!(font.layout("", 18.0, |_| unreachable!()), Ok(0.0));
    }
}

#[test]
fn mono_advances_are_all_the_same() {
    for data in [MONO_REGULAR, FONTS[5].1] {
        let font = parse(data);
        let m = font.h_metrics(glyph(&font, 'M')).unwrap().0;
        assert_eq!(m, 600);
        for c in '!'..='~' {
            assert_eq!(font.h_metrics(glyph(&font, c)).unwrap().0, m, "{c:?}");
        }
    }
}

#[test]
fn placement_comes_from_the_glyph_box() {
    let font = parse(INTER_REGULAR);
    let raw = Raw {
        data: INTER_REGULAR,
    };
    let (glyf, _) = raw.table(b"glyf").unwrap();
    let (loca, _) = raw.table(b"loca").unwrap();
    for c in ['g', 'T', '.', 'Ä', 'j'] {
        let g = glyph(&font, c);
        let at = glyf + raw.u32(loca + 4 * g as usize) as usize;
        let b = [2, 4, 6, 8].map(|o| f64::from(raw.i16(at + o)));
        assert_eq!(font.bounds(g), Ok(Some(b.map(|v| v as i16))));
        for (px, shift) in [(16.0, 0.0), (13.0, 0.25), (40.0, 0.99), (1.0, 1.0)] {
            let s = px / 2048.0;
            let left = (b[0] * s + shift).floor() as i32;
            let right = (b[2] * s + shift).ceil() as i32;
            let top = (-b[3] * s).floor() as i32;
            let bottom = (-b[1] * s).ceil() as i32;
            let want = Placement {
                left,
                top,
                width: (right - left) as usize,
                height: (bottom - top) as usize,
            };
            let p = font.placement(g, px as f32, shift as f32).unwrap().unwrap();
            assert_eq!(p, want, "{c:?} at {px}");
            assert_eq!(p.scratch_len(), (p.width + 2) * p.height);
        }
    }
}

#[test]
fn rasterize_checks_sizes_and_buffers() {
    let font = parse(INTER_REGULAR);
    let g = glyph(&font, 'W');
    let p = font.placement(g, 16.0, 0.0).unwrap().unwrap();
    let mut scratch = vec![0.0; p.scratch_len()];
    let mut out = vec![0; p.width * p.height];
    let n = out.len();
    let short = font.rasterize(g, 16.0, 0.0, &mut scratch[1..], &mut out);
    assert_eq!(short, Err(Error::BufferTooSmall));
    let short = font.rasterize(g, 16.0, 0.0, &mut scratch, &mut out[..n - 1]);
    assert_eq!(short, Err(Error::BufferTooSmall));
    assert_eq!(
        font.rasterize(g, 16.0, 0.0, &mut scratch, &mut out),
        Ok(Some(p))
    );
    for (px, shift) in [
        (0.0, 0.0),
        (-3.0, 0.0),
        (f32::NAN, 0.0),
        (f32::INFINITY, 0.0),
        (MAX_PIXEL_SIZE + 1.0, 0.0),
        (16.0, -0.01),
        (16.0, 1.01),
        (16.0, f32::NAN),
    ] {
        assert_eq!(
            font.placement(g, px, shift),
            Err(Error::BadSize),
            "{px} {shift}"
        );
        let r = font.rasterize(g, px, shift, &mut scratch, &mut out);
        assert_eq!(r, Err(Error::BadSize), "{px} {shift}");
    }
    assert!(font.placement(g, MAX_PIXEL_SIZE, 1.0).unwrap().is_some());
    let space = glyph(&font, ' ');
    assert_eq!(font.placement(space, 16.0, 0.0), Ok(None));
    assert_eq!(font.rasterize(space, 16.0, 0.0, &mut [], &mut []), Ok(None));
    assert_eq!(font.bounds(space), Ok(None));
    assert_eq!(outline(&font, space).commands, []);
    assert!(font.placement(font.glyph_count(), 16.0, 0.0).is_err());
}

#[test]
fn glyphs_per_second_at_16_px() {
    let font = parse(INTER_REGULAR);
    let glyphs: Vec<u16> = ('!'..='~').map(|c| glyph(&font, c)).collect();
    let mut scratch = vec![0.0; 4096];
    let mut out = vec![0; 4096];
    let start = Instant::now();
    let mut count = 0u32;
    while start.elapsed().as_millis() < 300 {
        for (i, &g) in glyphs.iter().enumerate() {
            let shift = (i % 4) as f32 / 4.0;
            font.rasterize(g, 16.0, shift, &mut scratch, &mut out)
                .unwrap();
            count += 1;
        }
    }
    let rate = f64::from(count) / start.elapsed().as_secs_f64();
    eprintln!("{rate:.0} glyphs per second at 16 px");
    // Far below what even an unoptimized build manages; it catches work
    // that grows out of all proportion, not small slowdowns.
    assert!(rate > 500.0, "{rate:.0} glyphs per second");
}
