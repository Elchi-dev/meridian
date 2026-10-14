//! Small fonts built byte by byte, for the cases the shipped fonts do not
//! reach: every kind of composite, both `loca` formats, short `hmtx`,
//! format 4 glyph lists, `kern` and broken tables.

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

use common::build::{
    Component, FontBuilder, Glyph, Scale, Segment, assemble, cmap, component, composite, format4,
    format12, kern, off, on, simple,
};
use common::{Command, Recorder};
use font::{Error, Font, MAX_COMPONENT_DEPTH, MAX_COMPONENTS, Point, Positioned};

fn pt(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

fn glyph(advance: u16, lsb: i16, data: Vec<u8>) -> Glyph {
    Glyph { advance, lsb, data }
}

fn square(x: i16, y: i16, w: i16, h: i16) -> Vec<u8> {
    simple(
        &[&[on(x, y), on(x, y + h), on(x + w, y + h), on(x + w, y)]],
        &[],
    )
}

const SQUARE: [(f32, f32); 4] = [(0.0, 0.0), (0.0, 700.0), (500.0, 700.0), (500.0, 0.0)];

/// The contours of glyph 2: one for each way a contour can start.
fn curves() -> Vec<u8> {
    simple(
        &[
            // Off-curve first, on-curve last: starts at the last point.
            &[off(0, 0), on(100, 0), on(100, 100)],
            // Off-curve all round: starts between the last and first.
            &[off(0, 0), off(100, 0), off(100, 100), off(0, 100)],
            // Two off-curve points in a row imply one between them.
            &[on(0, 0), off(50, 100), off(150, 100), on(200, 0)],
            // On-curve first, off-curve last: closes with a curve.
            &[on(0, 0), on(100, 0), off(100, 100)],
            // A single point.
            &[on(7, 9)],
        ],
        &[],
    )
}

fn curve_commands() -> Vec<Command> {
    use Command::{Close, Line, Move, Quad};
    vec![
        Move(pt(100.0, 100.0)),
        Quad(pt(0.0, 0.0), pt(100.0, 0.0)),
        Close,
        Move(pt(0.0, 50.0)),
        Quad(pt(0.0, 0.0), pt(50.0, 0.0)),
        Quad(pt(100.0, 0.0), pt(100.0, 50.0)),
        Quad(pt(100.0, 100.0), pt(50.0, 100.0)),
        Quad(pt(0.0, 100.0), pt(0.0, 50.0)),
        Close,
        Move(pt(0.0, 0.0)),
        Quad(pt(50.0, 100.0), pt(100.0, 100.0)),
        Quad(pt(150.0, 100.0), pt(200.0, 0.0)),
        Close,
        Move(pt(0.0, 0.0)),
        Line(pt(100.0, 0.0)),
        Quad(pt(100.0, 100.0), pt(0.0, 0.0)),
        Close,
        Move(pt(7.0, 9.0)),
        Close,
    ]
}

/// A zig-zag of 300 points with equal flags, so the flags repeat across
/// more than one run, then two long jumps, behind some instructions.
fn staircase() -> (Vec<u8>, Vec<(f32, f32)>) {
    let mut points = vec![on(0, 0)];
    points.extend((1..300).map(|k| on(2 * k, k)));
    points.extend([on(2000, 300), on(2000, -300), on(-255, -300)]);
    let data = simple(&[&points], &[0xb0, 1, 0x2c, 0x10, 0x01]);
    let coords = points.iter().map(|p| (f32::from(p.0), f32::from(p.1)));
    (data, coords.collect())
}

fn mapping() -> Vec<u8> {
    let segments = [
        Segment {
            start: 0x20,
            end: 0x20,
            delta: 3 - 0x20,
            glyphs: None,
        },
        Segment {
            start: 0x41,
            end: 0x42,
            delta: -0x40,
            glyphs: None,
        },
        Segment {
            start: 0xe4,
            end: 0xe4,
            delta: 0,
            glyphs: Some(vec![4]),
        },
        Segment {
            start: 0x100,
            end: 0x102,
            delta: 0,
            glyphs: Some(vec![5, 0, 6]),
        },
        Segment {
            start: 0x2000,
            end: 0x2002,
            delta: 1,
            glyphs: Some(vec![6, 7, 40]),
        },
        Segment {
            start: 0x3000,
            end: 0x3001,
            delta: 8 - 0x3000,
            glyphs: None,
        },
        Segment {
            start: 0xffff,
            end: 0xffff,
            delta: 1,
            glyphs: None,
        },
    ];
    cmap(&[(3, 1, format4(&segments))])
}

fn composites() -> Vec<Glyph> {
    let rotate = Scale::Matrix(0.0, 1.0, -1.0, 0.0);
    vec![
        glyph(
            600,
            100,
            composite(&[component(1, 100, -50)], [100, -50, 600, 650]),
        ),
        glyph(
            600,
            0,
            composite(
                &[Component {
                    scale: Scale::Uniform(0.5),
                    ..component(1, 1000, 2000)
                }],
                [1000, 2000, 1250, 2350],
            ),
        ),
        glyph(
            600,
            0,
            composite(
                &[Component {
                    scale: Scale::Xy(1.5, 0.5),
                    ..component(1, 0, 0)
                }],
                [0, 0, 750, 350],
            ),
        ),
        glyph(
            600,
            0,
            composite(
                &[
                    Component {
                        scale: rotate,
                        flags: 0x0800,
                        ..component(1, 10, 20)
                    },
                    Component {
                        scale: rotate,
                        ..component(1, 10, 20)
                    },
                ],
                [-720, 10, 10, 520],
            ),
        ),
        glyph(
            900,
            0,
            composite(
                &[component(4, 0, 0), component(2, -100, 0)],
                [-100, -50, 600, 650],
            ),
        ),
    ]
}

/// Glyphs: 0 a box, 1 a square, 2 curves, 3 a space, 4 to 8 composites,
/// 9 the staircase.
fn builder() -> FontBuilder {
    let mut glyphs = vec![
        glyph(500, 0, square(50, 0, 400, 600)),
        glyph(600, 0, square(0, 0, 500, 700)),
        glyph(700, 0, curves()),
        glyph(250, 0, Vec::new()),
    ];
    glyphs.extend(composites());
    glyphs.push(glyph(2100, -255, staircase().0));
    FontBuilder::new(glyphs, mapping())
}

fn record(font: &Font<'_>, glyph: u16) -> Result<Recorder, Error> {
    let mut r = Recorder::default();
    font.outline(glyph, &mut r)?;
    Ok(r)
}

fn polygon(points: &[(f32, f32)]) -> Vec<Command> {
    let mut out = vec![Command::Move(pt(points[0].0, points[0].1))];
    out.extend(points[1..].iter().map(|&(x, y)| Command::Line(pt(x, y))));
    out.push(Command::Close);
    out
}

fn moved(points: &[(f32, f32)], f: impl Fn(f32, f32) -> (f32, f32)) -> Vec<(f32, f32)> {
    points.iter().map(|&(x, y)| f(x, y)).collect()
}

#[test]
fn simple_glyphs_decode_every_point() {
    for long_loca in [false, true] {
        let mut b = builder();
        b.long_loca = long_loca;
        let data = b.build();
        let font = Font::parse(&data).unwrap();
        assert_eq!(font.glyph_count(), 10);
        assert_eq!(record(&font, 1).unwrap().commands, polygon(&SQUARE));
        assert_eq!(record(&font, 2).unwrap().commands, curve_commands());
        let (_, stairs) = staircase();
        assert_eq!(record(&font, 9).unwrap().commands, polygon(&stairs));
        assert_eq!(record(&font, 3).unwrap().commands, []);
        assert_eq!(font.bounds(9), Ok(Some([-255, -300, 2000, 300])));
        assert_eq!(font.bounds(2), Ok(Some([0, 0, 200, 100])));
    }
}

#[test]
fn glyph_without_contours_is_empty() {
    let mut b = builder();
    b.glyphs[3].data = simple(&[], &[]);
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    assert_eq!(record(&font, 3).unwrap().commands, []);
    assert_eq!(font.bounds(3), Ok(Some([0, 0, 0, 0])));
}

#[test]
fn composites_move_scale_and_turn_their_parts() {
    let data = builder().build();
    let font = Font::parse(&data).unwrap();
    let cases: [(u16, Vec<(f32, f32)>); 3] = [
        (4, moved(&SQUARE, |x, y| (x + 100.0, y - 50.0))),
        (
            5,
            moved(&SQUARE, |x, y| (x * 0.5 + 1000.0, y * 0.5 + 2000.0)),
        ),
        (6, moved(&SQUARE, |x, y| (x * 1.5, y * 0.5))),
    ];
    for (g, points) in cases {
        assert_eq!(
            record(&font, g).unwrap().commands,
            polygon(&points),
            "glyph {g}"
        );
    }
    // A quarter turn, once with the offset turned as well and once without.
    let mut turned = polygon(&moved(&SQUARE, |x, y| (-y - 20.0, x + 10.0)));
    turned.extend(polygon(&moved(&SQUARE, |x, y| (-y + 10.0, x + 20.0))));
    assert_eq!(record(&font, 7).unwrap().commands, turned);
    // Nested: a composite inside a composite, then a simple glyph.
    let mut nested = polygon(&moved(&SQUARE, |x, y| (x + 100.0, y - 50.0)));
    nested.extend(curve_commands().into_iter().map(|c| match c {
        Command::Move(p) => Command::Move(pt(p.x - 100.0, p.y)),
        Command::Line(p) => Command::Line(pt(p.x - 100.0, p.y)),
        Command::Quad(c, p) => Command::Quad(pt(c.x - 100.0, c.y), pt(p.x - 100.0, p.y)),
        Command::Close => Command::Close,
    }));
    assert_eq!(record(&font, 8).unwrap().commands, nested);
}

#[test]
fn composites_have_limits() {
    // Glyph 1 holding itself never ends by itself.
    let mut b = builder();
    b.glyphs[1].data = composite(&[component(1, 0, 0)], [0, 0, 500, 700]);
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    assert_eq!(record(&font, 1).err(), Some(Error::TooDeep));
    let r = font.rasterize(1, 16.0, 0.0, &mut [0.0; 512], &mut [0; 256]);
    assert_eq!(r, Err(Error::TooDeep));

    // A component placed by matching points: the flag that says its
    // arguments are offsets is cleared.
    let mut bytes = composite(&[component(0, 0, 0)], [0, 0, 1, 1]);
    bytes[11] &= !0x02;
    b.glyphs[1].data = bytes;
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    assert_eq!(record(&font, 1).err(), Some(Error::PointMatching));
}

/// A chain of composites `levels` deep on top of the square, glyph 1.
fn chain(levels: u32) -> (Vec<u8>, u16) {
    let mut b = builder();
    let first = b.glyphs.len() as u16;
    for i in 0..levels {
        let inner = if i == 0 { 1 } else { first + i as u16 - 1 };
        b.glyphs.push(glyph(
            600,
            0,
            composite(&[component(inner, 1, 0)], [0, 0, 500, 700]),
        ));
    }
    (b.build(), first + levels as u16 - 1)
}

#[test]
fn composites_nest_as_deep_as_allowed() {
    let (data, top) = chain(MAX_COMPONENT_DEPTH);
    let font = Font::parse(&data).unwrap();
    let shift = MAX_COMPONENT_DEPTH as f32;
    let want = polygon(&moved(&SQUARE, |x, y| (x + shift, y)));
    assert_eq!(record(&font, top).unwrap().commands, want);

    let (data, top) = chain(MAX_COMPONENT_DEPTH + 1);
    let font = Font::parse(&data).unwrap();
    assert_eq!(record(&font, top).err(), Some(Error::TooDeep));
    assert_eq!(record(&font, top - 1).map(|r| r.commands.len()), Ok(5));
}

#[test]
fn composites_have_a_component_budget() {
    let many = |n: u32| -> Vec<u8> {
        let parts: Vec<_> = (0..n).map(|i| component(2, i as i16, 0)).collect();
        let mut b = builder();
        b.glyphs[4] = glyph(600, 0, composite(&parts, [0, 0, 500, 700]));
        b.build()
    };
    let data = many(MAX_COMPONENTS);
    let font = Font::parse(&data).unwrap();
    let closes = |r: &Recorder| r.commands.iter().filter(|c| **c == Command::Close).count();
    assert_eq!(
        record(&font, 4).map(|r| closes(&r)),
        Ok(5 * MAX_COMPONENTS as usize)
    );
    let data = many(MAX_COMPONENTS + 1);
    let font = Font::parse(&data).unwrap();
    assert_eq!(record(&font, 4).err(), Some(Error::TooDeep));
    // Nested components count too: glyph 8 holds glyph 4, which holds
    // all of its own.
    let data = many(MAX_COMPONENTS - 1);
    let font = Font::parse(&data).unwrap();
    assert_eq!(record(&font, 8).err(), Some(Error::TooDeep));
    let data = many(MAX_COMPONENTS - 2);
    let font = Font::parse(&data).unwrap();
    assert!(record(&font, 8).is_ok());
}

#[test]
fn composites_cut_short_are_errors() {
    for (i, g) in composites().into_iter().enumerate() {
        let id = 4 + i as u16;
        for len in 10..g.data.len() {
            let mut b = builder();
            // Long offsets need no padding, so the glyph ends where cut.
            b.long_loca = true;
            b.glyphs[usize::from(id)].data = g.data[..len].to_vec();
            let data = b.build();
            let font = Font::parse(&data).unwrap();
            let r = record(&font, id).err();
            assert_eq!(
                r,
                Some(Error::Malformed(*b"glyf")),
                "glyph {id} cut at {len}"
            );
        }
    }
}

#[test]
fn rasterizes_at_the_placement() {
    let data = builder().build();
    let font = Font::parse(&data).unwrap();
    let mut scratch = [0.0; 128];
    let mut out = [0; 64];
    let p = font
        .rasterize(1, 10.0, 0.0, &mut scratch, &mut out)
        .unwrap()
        .unwrap();
    assert_eq!((p.left, p.top, p.width, p.height), (0, -7, 5, 7));
    assert!(out[..35].iter().all(|&b| b == 255));

    let p = font
        .rasterize(1, 10.0, 0.5, &mut scratch, &mut out)
        .unwrap()
        .unwrap();
    assert_eq!((p.left, p.top, p.width, p.height), (0, -7, 6, 7));
    for row in out[..42].chunks(6) {
        assert_eq!(row, [128, 255, 255, 255, 255, 128]);
    }

    // The composite sits half a pixel lower and one to the right.
    let p = font
        .rasterize(4, 10.0, 0.0, &mut scratch, &mut out)
        .unwrap()
        .unwrap();
    assert_eq!((p.left, p.top, p.width, p.height), (1, -7, 5, 8));
    assert_eq!(&out[..5], [128; 5]);
    assert_eq!(&out[5..10], [255; 5]);
    assert_eq!(&out[35..40], [128; 5]);
}

#[test]
fn line_metrics_follow_hhea() {
    let data = builder().build();
    let font = Font::parse(&data).unwrap();
    let m = font.line_metrics(10.0);
    assert_eq!((m.ascent, m.descent, m.line_gap), (8.0, -2.0, 0.9));
    assert!((m.line_height() - 10.9).abs() < 1e-5);
    assert_eq!(font.units_per_em(), 1000);
    assert_eq!(font.scale(10.0), 0.01);
}

#[test]
fn short_hmtx_repeats_the_last_advance() {
    let mut b = builder();
    b.h_metrics = 3;
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    assert_eq!(font.h_metrics(0), Ok((500, 0)));
    assert_eq!(font.h_metrics(2), Ok((700, 0)));
    assert_eq!(font.h_metrics(3), Ok((700, 0)));
    assert_eq!(font.h_metrics(4), Ok((700, 100)));
    assert_eq!(font.h_metrics(9), Ok((700, -255)));
    assert_eq!(font.h_metrics(10), Err(Error::Malformed(*b"hmtx")));
    b.h_metrics = 1;
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    assert_eq!(font.h_metrics(9), Ok((500, -255)));
    assert_eq!(font.h_metrics(4), Ok((500, 100)));
}

#[test]
fn format4_maps_by_delta_and_by_list() {
    let data = builder().build();
    let font = Font::parse(&data).unwrap();
    let cases = [
        (' ', Some(3)),
        ('A', Some(1)),
        ('B', Some(2)),
        ('C', None),
        ('@', None),
        ('ä', Some(4)),
        ('ã', None),
        ('å', None),
        ('\u{100}', Some(5)),
        ('\u{101}', None),
        ('\u{102}', Some(6)),
        ('\u{1fff}', None),
        ('\u{2000}', Some(7)),
        ('\u{2001}', Some(8)),
        // Past the glyphs the font has.
        ('\u{2002}', None),
        ('\u{3000}', Some(8)),
        ('\u{3001}', Some(9)),
        ('\u{3002}', None),
        ('\u{ffff}', None),
        ('\u{10000}', None),
        ('\u{1f600}', None),
    ];
    for (c, want) in cases {
        assert_eq!(font.glyph_index(c), want, "{c:?}");
    }
}

#[test]
fn format12_maps_every_plane() {
    let groups = [
        (0x41, 0x42, 1),
        (0x3000, 0x3002, 7),
        (0x1f600, 0x1f600, 9),
        (0x2_0000, 0x2_0000, 70000),
    ];
    let mut b = builder();
    b.cmap = cmap(&[(3, 10, format12(&groups))]);
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    let cases = [
        ('@', None),
        ('A', Some(1)),
        ('B', Some(2)),
        ('C', None),
        ('\u{2fff}', None),
        ('\u{3000}', Some(7)),
        ('\u{3002}', Some(9)),
        ('\u{3003}', None),
        ('\u{1f600}', Some(9)),
        ('\u{1f601}', None),
        // A glyph number past 65535 cannot be.
        ('\u{20000}', None),
        ('\u{10ffff}', None),
    ];
    for (c, want) in cases {
        assert_eq!(font.glyph_index(c), want, "{c:?}");
    }
}

#[test]
fn cmap_prefers_the_full_repertoire() {
    let bmp = format4(&[Segment {
        start: 0x41,
        end: 0x41,
        delta: 1 - 0x41,
        glyphs: None,
    }]);
    let full = format12(&[(0x41, 0x41, 2)]);
    let pick = |subtables: &[(u16, u16, Vec<u8>)]| {
        let mut b = builder();
        b.cmap = cmap(subtables);
        let data = b.build();
        Font::parse(&data).map(|f| f.glyph_index('A'))
    };
    assert_eq!(
        pick(&[(3, 1, bmp.clone()), (3, 10, full.clone())]),
        Ok(Some(2))
    );
    assert_eq!(
        pick(&[(3, 10, full.clone()), (3, 1, bmp.clone())]),
        Ok(Some(2))
    );
    assert_eq!(
        pick(&[(0, 3, bmp.clone()), (0, 4, full.clone())]),
        Ok(Some(2))
    );
    assert_eq!(pick(&[(0, 3, bmp.clone())]), Ok(Some(1)));
    assert_eq!(
        pick(&[(1, 0, bmp.clone()), (3, 1, bmp.clone())]),
        Ok(Some(1))
    );
    assert_eq!(pick(&[(0, 4, full.clone())]), Ok(Some(2)));
    // Subtables whose format does not fit their encoding, or that are for
    // other platforms, do not count.
    let missing = Err(Error::MissingTable(*b"cmap"));
    assert_eq!(pick(&[(3, 10, bmp.clone()), (3, 1, full.clone())]), missing);
    assert_eq!(pick(&[(1, 0, bmp.clone()), (0, 6, full.clone())]), missing);
    assert_eq!(pick(&[]), missing);
}

#[test]
fn broken_cmap_is_an_error() {
    let mut b = builder();
    // A record pointing past the table.
    let mut table = mapping();
    table[8..12].copy_from_slice(&0xffff_u32.to_be_bytes());
    b.cmap = table;
    let data = b.build();
    assert_eq!(Font::parse(&data).err(), Some(Error::Malformed(*b"cmap")));
    // Fewer records than it says.
    b.cmap = vec![0, 0, 0, 3, 0, 3, 0, 1];
    let data = b.build();
    assert_eq!(Font::parse(&data).err(), Some(Error::Malformed(*b"cmap")));
    b.cmap = vec![0, 0];
    let data = b.build();
    assert_eq!(Font::parse(&data).err(), Some(Error::Malformed(*b"cmap")));
    // A subtable cut short only fails its lookups.
    let mut table = mapping();
    table.truncate(table.len() - 8);
    b.cmap = table;
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    assert_eq!(font.glyph_index('A'), Some(1));
    assert_eq!(font.glyph_index('\u{2001}'), None);
}

fn kerned(coverage: u16) -> Vec<u8> {
    let mut b = builder();
    b.kern = Some(kern(
        &[(2, 1, 30), (1, 2, -50), (3, 1, -10), (1, 1, 7)],
        coverage,
    ));
    b.build()
}

#[test]
fn kerning_moves_pairs() {
    let data = kerned(0x0001);
    let font = Font::parse(&data).unwrap();
    assert_eq!(font.kerning(1, 2), -50);
    assert_eq!(font.kerning(2, 1), 30);
    assert_eq!(font.kerning(3, 1), -10);
    assert_eq!(font.kerning(1, 1), 7);
    assert_eq!(font.kerning(1, 3), 0);
    assert_eq!(font.kerning(0, 0), 0);
    assert_eq!(font.kerning(9, 9), 0);
    let mut placed = Vec::new();
    let width = font.layout("AB A", 10.0, |p| placed.push(p)).unwrap();
    let want = [(1, 0.0), (2, 5.5), (3, 12.5), (1, 14.9)];
    for (p, (g, x)) in placed.iter().zip(want) {
        assert_eq!(p.glyph, g);
        assert!((p.x - x).abs() < 1e-4, "{p:?}");
    }
    assert!((width - 20.9).abs() < 1e-4, "{width}");
    // The override flag does not change a single table.
    assert_eq!(Font::parse(&kerned(0x0009)).unwrap().kerning(1, 2), -50);
}

#[test]
fn other_kerning_is_ignored() {
    // Vertical, minimum values, across the line, another format.
    for coverage in [0x0000, 0x0003, 0x0005, 0x0101, 0x0201] {
        let data = kerned(coverage);
        let font = Font::parse(&data).unwrap();
        assert_eq!(font.kerning(1, 2), 0, "{coverage:#x}");
    }
    let mut b = builder();
    let mut table = kern(&[(1, 2, -50)], 1);
    table[1] = 1;
    b.kern = Some(table);
    assert_eq!(Font::parse(&b.build()).unwrap().kerning(1, 2), 0);
    let mut table = kern(&[(1, 2, -50)], 1);
    table[3] = 0;
    b.kern = Some(table);
    assert_eq!(Font::parse(&b.build()).unwrap().kerning(1, 2), 0);
    // Pairs cut short.
    let mut table = kern(&[(1, 2, -50), (3, 4, 9)], 1);
    table.truncate(table.len() - 1);
    b.kern = Some(table);
    let data = b.build();
    let font = Font::parse(&data).unwrap();
    assert_eq!(font.kerning(1, 2), -50);
    assert_eq!(font.kerning(3, 4), 0);
    b.kern = Some(vec![0, 0, 0, 1]);
    assert_eq!(Font::parse(&b.build()).unwrap().kerning(1, 2), 0);
}

#[test]
fn layout_uses_the_missing_glyph_box() {
    let data = builder().build();
    let font = Font::parse(&data).unwrap();
    let mut placed = Vec::new();
    let width = font.layout("A?B", 100.0, |p| placed.push(p)).unwrap();
    assert_eq!(
        placed,
        [
            Positioned { glyph: 1, x: 0.0 },
            Positioned { glyph: 0, x: 60.0 },
            Positioned { glyph: 2, x: 110.0 }
        ]
    );
    assert_eq!(width, 180.0);
}

#[test]
fn parse_accepts_both_versions() {
    let mut b = builder();
    b.version = u32::from_be_bytes(*b"true");
    assert!(Font::parse(&b.build()).is_ok());
    for version in [u32::from_be_bytes(*b"OTTO"), 0x0002_0000, 0] {
        b.version = version;
        assert_eq!(Font::parse(&b.build()).err(), Some(Error::NotTrueType));
    }
    assert_eq!(Font::parse(&[]).err(), Some(Error::NotTrueType));
    assert_eq!(
        Font::parse(&[0, 1, 0, 0, 0]).err(),
        Some(Error::NotTrueType)
    );
}

#[test]
fn every_table_is_needed() {
    let b = builder();
    for (tag, _) in b.tables() {
        let tables: Vec<_> = b.tables().into_iter().filter(|t| t.0 != tag).collect();
        let data = assemble(0x0001_0000, &tables);
        assert_eq!(
            Font::parse(&data).err(),
            Some(Error::MissingTable(tag)),
            "{tag:?}"
        );
    }
}

#[test]
fn directory_cut_short_is_no_font() {
    let data = builder().build();
    let count = 7;
    for len in 6..12 + 16 * count {
        let r = Font::parse(&data[..len]);
        assert!(r.is_err(), "{len}");
    }
    // A table reaching past the file.
    let mut copy = data.clone();
    copy[12 + 12..12 + 16].copy_from_slice(&0x00ff_ffff_u32.to_be_bytes());
    assert_eq!(Font::parse(&copy).err(), Some(Error::Malformed(*b"cmap")));
    let mut copy = data.clone();
    copy[12 + 8..12 + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(Font::parse(&copy).err(), Some(Error::Malformed(*b"cmap")));
}

/// The font with one table's bytes changed.
fn patched(tag: [u8; 4], f: impl Fn(&mut Vec<u8>)) -> Vec<u8> {
    let mut tables = builder().tables();
    for (t, data) in &mut tables {
        if *t == tag {
            f(data);
        }
    }
    assemble(0x0001_0000, &tables)
}

fn malformed(tag: [u8; 4], f: impl Fn(&mut Vec<u8>)) -> Option<Error> {
    Font::parse(&patched(tag, f)).err()
}

#[test]
fn header_values_are_checked() {
    let head = Some(Error::Malformed(*b"head"));
    assert_eq!(malformed(*b"head", |h| h[12] = 0), head);
    let upem = |v: u16| move |h: &mut Vec<u8>| h[18..20].copy_from_slice(&v.to_be_bytes());
    assert_eq!(malformed(*b"head", upem(15)), head);
    assert_eq!(malformed(*b"head", upem(16)), None);
    assert_eq!(malformed(*b"head", upem(16384)), None);
    assert_eq!(malformed(*b"head", upem(16385)), head);
    assert_eq!(malformed(*b"head", |h| h[51] = 2), head);
    assert_eq!(malformed(*b"head", |h| h[50] = 1), head);
    assert_eq!(malformed(*b"head", |h| h.truncate(51)), head);
    assert_eq!(malformed(*b"head", |h| h.truncate(19)), head);
    assert_eq!(malformed(*b"head", |h| h.truncate(15)), head);
    assert_eq!(
        malformed(*b"maxp", |m| m.truncate(5)),
        Some(Error::Malformed(*b"maxp"))
    );
    let hhea = Some(Error::Malformed(*b"hhea"));
    assert_eq!(malformed(*b"hhea", |h| h[35] = 0), hhea);
    assert_eq!(malformed(*b"hhea", |h| h[35] = 11), hhea);
    assert_eq!(malformed(*b"hhea", |h| h[35] = 1), None);
    assert_eq!(malformed(*b"hhea", |h| h.truncate(35)), hhea);
}

#[test]
fn metric_and_location_tables_must_be_long_enough() {
    let hmtx = Some(Error::Malformed(*b"hmtx"));
    assert_eq!(malformed(*b"hmtx", |h| h.truncate(39)), hmtx);
    assert_eq!(malformed(*b"hmtx", |h| h.truncate(40)), None);
    let short_hmtx = |h: &mut Vec<u8>| h[35] = 4;
    let data = patched(*b"hhea", short_hmtx);
    // Four full records and six side bearings take 28 bytes.
    let mut tables = builder().tables();
    for (t, d) in &mut tables {
        if t == b"hhea" {
            short_hmtx(d);
        }
        if t == b"hmtx" {
            d.truncate(27);
        }
    }
    assert!(Font::parse(&data).is_ok());
    assert_eq!(Font::parse(&assemble(0x0001_0000, &tables)).err(), hmtx);

    let loca = Some(Error::Malformed(*b"loca"));
    assert_eq!(malformed(*b"loca", |l| l.truncate(21)), loca);
    assert_eq!(malformed(*b"loca", |l| l.truncate(22)), None);
}

#[test]
fn broken_glyph_locations_are_errors() {
    // Glyph 1 ending before it starts, glyph 2 reaching past `glyf`.
    let data = patched(*b"loca", |l| {
        l[2..4].copy_from_slice(&0x0100_u16.to_be_bytes());
        l[6..8].copy_from_slice(&0xfff0_u16.to_be_bytes());
    });
    let font = Font::parse(&data).unwrap();
    let (loca, glyf) = (Error::Malformed(*b"loca"), Error::Malformed(*b"glyf"));
    assert!(record(&font, 0).is_ok());
    assert_eq!(record(&font, 1).err(), Some(loca));
    assert_eq!(font.bounds(1), Err(loca));
    assert_eq!(record(&font, 2).err(), Some(glyf));
    assert_eq!(record(&font, 3).err(), Some(loca));
    assert_eq!(record(&font, 10).err(), Some(loca));
    assert_eq!(font.bounds(10), Err(loca));
    assert_eq!(font.placement(10, 16.0, 0.0), Err(loca));
}

/// The font with glyph 2 replaced, stored without padding.
fn with_glyph2(data: Vec<u8>) -> Vec<u8> {
    let mut b = builder();
    b.long_loca = true;
    b.glyphs[2].data = data;
    b.build()
}

#[test]
fn broken_simple_glyphs_are_errors() {
    let glyf = Some(Error::Malformed(*b"glyf"));
    let fails = |data: Vec<u8>| -> Option<Error> {
        let built = with_glyph2(data);
        let font = Font::parse(&built).unwrap();
        record(&font, 2).err()
    };
    let good = curves();
    assert_eq!(fails(good.clone()), None);
    // Every cut through the points.
    for len in 0..good.len() {
        let want = if len == 0 { None } else { glyf };
        assert_eq!(fails(good[..len].to_vec()), want, "cut at {len}");
    }
    // Contour ends that go backwards.
    let mut back = good.clone();
    back[12..14].copy_from_slice(&1u16.to_be_bytes());
    assert_eq!(fails(back), glyf);
    // A last contour ending past the points is caught by the walk over
    // the flags.
    let mut long = good.clone();
    long[18..20].copy_from_slice(&900u16.to_be_bytes());
    assert_eq!(fails(long), glyf);

    // A box upside down, and one cut short.
    let mut upside = good.clone();
    upside[4..6].copy_from_slice(&500u16.to_be_bytes());
    let data = with_glyph2(upside);
    let font = Font::parse(&data).unwrap();
    assert_eq!(font.bounds(2), Err(Error::Malformed(*b"glyf")));
    assert_eq!(
        font.placement(2, 16.0, 0.0),
        Err(Error::Malformed(*b"glyf"))
    );
    let data = with_glyph2(vec![0, 1, 0, 0, 0, 0, 0, 0, 0]);
    let font = Font::parse(&data).unwrap();
    assert_eq!(font.bounds(2), Err(Error::Malformed(*b"glyf")));
}
