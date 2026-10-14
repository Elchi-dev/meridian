//! The rasterizer against shapes whose coverage is known exactly.

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

use std::f64::consts::PI;

use common::{Rng, exact_coverage, expected_byte, signed_area};
use font::{Error, Point, Rasterizer};

fn point(p: [f64; 2]) -> Point {
    Point::new(p[0] as f32, p[1] as f32)
}

/// Rasterizes polygons given as lists of corners.
fn render(width: usize, height: usize, contours: &[Vec<[f64; 2]>]) -> Vec<u8> {
    let mut acc = vec![0.0; Rasterizer::scratch_len(width, height)];
    let mut raster = Rasterizer::new(&mut acc, width, height).unwrap();
    for c in contours {
        for i in 0..c.len() {
            raster.line(point(c[i]), point(c[(i + 1) % c.len()]));
        }
    }
    let mut out = vec![0; width * height];
    raster.coverage(&mut out).unwrap();
    out
}

/// Checks every pixel against the exact area of the polygons inside it.
/// One step of 255 is allowed for rounding single precision sums.
fn assert_exact(width: usize, height: usize, contours: &[Vec<[f64; 2]>]) {
    let out = render(width, height, contours);
    for y in 0..height {
        for x in 0..width {
            let want = expected_byte(exact_coverage(contours, x, y));
            let got = out[y * width + x];
            assert!(
                got.abs_diff(want) <= 1,
                "pixel ({x}, {y}) is {got}, should be {want}, for {contours:?}"
            );
        }
    }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

#[test]
fn aligned_square_is_fully_on_or_off() {
    let (w, h) = (8, 7);
    for square in [rect(2.0, 1.0, 6.0, 5.0), rect(6.0, 5.0, 2.0, 1.0)] {
        let out = render(w, h, &[square]);
        for y in 0..h {
            for x in 0..w {
                let inside = (2..6).contains(&x) && (1..5).contains(&y);
                assert_eq!(out[y * w + x], if inside { 255 } else { 0 }, "({x}, {y})");
            }
        }
    }
}

#[test]
fn half_pixel_offset_covers_half() {
    let (w, h) = (6, 6);
    let out = render(w, h, &[rect(1.5, 1.5, 4.5, 4.5)]);
    let row = |y: usize| &out[y * w..(y + 1) * w];
    assert_eq!(row(0), [0, 0, 0, 0, 0, 0]);
    assert_eq!(row(1), [0, 64, 128, 128, 64, 0]);
    for y in 2..4 {
        assert_eq!(row(y), [0, 128, 255, 255, 128, 0]);
    }
    assert_eq!(row(4), [0, 64, 128, 128, 64, 0]);
    assert_eq!(row(5), [0, 0, 0, 0, 0, 0]);
}

#[test]
fn edge_inside_one_pixel_covers_the_part_right_of_it() {
    // A quarter of pixel 2 lies left of x = 2.25, and its right edge is
    // past the canvas.
    let out = render(4, 1, &[rect(2.25, 0.0, 9.0, 1.0)]);
    assert_eq!(out, [0, 0, 191, 255]);
    let out = render(4, 1, &[rect(0.0, 0.0, 1.75, 1.0)]);
    assert_eq!(out, [255, 191, 0, 0]);
}

#[test]
fn triangle_coverage_sums_to_its_area() {
    let triangle = vec![[1.3, 0.7], [9.6, 3.2], [4.1, 8.8]];
    let area = signed_area(&triangle).abs();
    let out = render(11, 10, &[triangle]);
    let partial = out.iter().filter(|&&c| c != 0 && c != 255).count();
    let sum: f64 = out.iter().map(|&c| f64::from(c) / 255.0).sum();
    // Every pixel rounds to the nearest 1/255, so each partly covered one
    // may be off by half of that.
    let tolerance = partial as f64 * 0.5 / 255.0 + 1e-3;
    assert!(
        (sum - area).abs() <= tolerance,
        "sum {sum}, area {area}, tolerance {tolerance}"
    );
    assert!(out.contains(&255));
}

#[test]
fn edges_through_one_two_and_many_pixels_per_row() {
    // Steep, shallow and in-between slopes reach the three ways an edge
    // is spread over a row.
    for contour in [
        vec![[2.2, 0.0], [2.7, 6.0], [5.5, 6.0], [5.4, 0.0]],
        vec![[1.25, 0.5], [2.75, 1.5], [1.25, 2.5]],
        vec![[0.5, 1.1], [15.7, 1.9], [0.4, 2.6]],
        vec![[0.3, 0.2], [11.8, 3.9], [3.1, 3.7]],
    ] {
        assert_exact(16, 7, &[contour]);
    }
}

#[test]
fn random_triangles_match_exact_coverage() {
    let mut rng = Rng(0x5eed_f00d_1234_5678);
    for _ in 0..300 {
        let triangle: Vec<[f64; 2]> = (0..3)
            .map(|_| [rng.range(0.0, 12.0), rng.range(0.0, 10.0)])
            .collect();
        assert_exact(12, 10, &[triangle]);
    }
}

#[test]
fn shapes_past_the_canvas_are_clipped() {
    // Left of the canvas counts as on its left edge and right of it as on
    // its right edge, which is exactly clipping for coverage inside it.
    let mut rng = Rng(42);
    for _ in 0..300 {
        let triangle: Vec<[f64; 2]> = (0..3)
            .map(|_| [rng.range(-6.0, 14.0), rng.range(-5.0, 13.0)])
            .collect();
        assert_exact(8, 8, &[triangle]);
    }
    assert_exact(5, 3, &[rect(-3.0, -2.0, 2.5, 9.0)]);
    assert_exact(5, 3, &[rect(2.5, 1.5, 40.0, 2.0)]);
}

#[test]
fn concave_shapes_and_holes() {
    let arrow = vec![
        [1.0, 4.0],
        [6.5, 0.5],
        [6.2, 2.7],
        [12.6, 2.9],
        [12.4, 5.3],
        [6.1, 5.6],
        [6.6, 7.8],
    ];
    assert_exact(14, 9, &[arrow]);
    // A ring: the inner square runs the other way round.
    let outer = rect(1.2, 1.4, 9.7, 8.9);
    let mut inner = rect(3.6, 3.3, 7.1, 6.5);
    inner.reverse();
    assert_exact(11, 10, &[outer, inner]);
}

#[test]
fn overlapping_shapes_count_once() {
    // Two squares on top of each other, wound the same way, still cover a
    // pixel no more than fully.
    let a = rect(1.0, 1.0, 4.0, 4.0);
    let b = rect(2.0, 2.0, 5.0, 5.0);
    let out = render(6, 6, &[a, b]);
    assert_eq!(out[3 * 6 + 3], 255);
    assert_eq!(out[6 + 1], 255);
    assert_eq!(out[6 + 4], 0);
}

#[test]
fn edges_in_either_direction_cancel() {
    let mut acc = vec![0.0; Rasterizer::scratch_len(4, 4)];
    let mut raster = Rasterizer::new(&mut acc, 4, 4).unwrap();
    let (a, b) = (Point::new(0.3, 0.1), Point::new(3.7, 3.9));
    raster.line(a, b);
    raster.line(b, a);
    let mut out = [9; 16];
    raster.coverage(&mut out).unwrap();
    assert_eq!(out, [0; 16]);
}

#[test]
fn horizontal_and_empty_edges_add_nothing() {
    let mut acc = vec![0.0; Rasterizer::scratch_len(4, 4)];
    let mut raster = Rasterizer::new(&mut acc, 4, 4).unwrap();
    raster.line(Point::new(0.5, 1.5), Point::new(3.5, 1.5));
    raster.line(Point::new(2.0, 2.0), Point::new(2.0, 2.0));
    raster.line(Point::new(1.0, -3.0), Point::new(2.0, -1.0));
    raster.line(Point::new(1.0, 4.0), Point::new(2.0, 7.0));
    let mut out = [9; 16];
    raster.coverage(&mut out).unwrap();
    assert_eq!(out, [0; 16]);
}

/// A circle of radius `r` around `(cx, cy)` from `n` quadratic curves,
/// each with its ends on the circle and its control point where the
/// tangents at the ends meet.
fn quad_circle(raster: &mut Rasterizer<'_>, cx: f64, cy: f64, r: f64, n: usize) {
    let step = 2.0 * PI / n as f64;
    let on = |i: usize| {
        let a = i as f64 * step;
        [cx + r * a.cos(), cy + r * a.sin()]
    };
    for i in 0..n {
        let a = (i as f64 + 0.5) * step;
        let reach = r / (step / 2.0).cos();
        let control = [cx + reach * a.cos(), cy + reach * a.sin()];
        raster.quad(point(on(i)), point(control), point(on(i + 1)));
    }
}

#[test]
fn circle_matches_pi_r_squared() {
    let (w, h, r) = (90, 90, 40.5);
    let mut acc = vec![0.0; Rasterizer::scratch_len(w, h)];
    let mut raster = Rasterizer::new(&mut acc, w, h).unwrap();
    let n = 16;
    quad_circle(&mut raster, 45.25, 44.75, r, n);
    let mut out = vec![0; w * h];
    raster.coverage(&mut out).unwrap();
    let sum: f64 = out.iter().map(|&c| f64::from(c) / 255.0).sum();

    // The curves bulge out of the circle a little: each piece adds the
    // triangle from the centre plus two thirds of the triangle its control
    // point spans over the chord, a known amount above pi r squared.
    let half = PI / n as f64;
    let spline =
        n as f64 * r * r * (half.sin() * half.cos() + half.sin().powi(3) / half.cos() * 2.0 / 3.0);
    let circle = PI * r * r;
    assert!(spline > circle && spline - circle < 2.5e-4 * circle);
    // Flattening keeps within 0.05 pixels of each curve, which loses at
    // most two thirds of that times the outline's length, a little over
    // 2 pi r. Every partly covered pixel may be off by half a step of 255.
    let flattening = 2.0 / 3.0 * 0.05 * 2.0 * PI * r * 1.03;
    let partial = out.iter().filter(|&&c| c != 0 && c != 255).count();
    let rounding = partial as f64 * 0.5 / 255.0;
    assert!(
        sum <= spline + rounding && sum >= spline - flattening - rounding,
        "sum {sum}, spline {spline}, circle {circle}"
    );
    assert!(
        (sum - circle).abs() <= (spline - circle) + flattening + rounding,
        "sum {sum}, circle {circle}"
    );
    // The middle is solid and the corners are empty.
    assert_eq!(out[45 * w + 45], 255);
    assert_eq!(out[0], 0);
    assert_eq!(out[w * h - 1], 0);
}

#[test]
fn straight_curve_is_a_line() {
    let triangle = [[1.0, 1.0], [9.0, 2.0], [3.0, 7.0]];
    let (w, h) = (10, 8);
    let lines = render(w, h, &[triangle.to_vec()]);
    let mut acc = vec![0.0; Rasterizer::scratch_len(w, h)];
    let mut raster = Rasterizer::new(&mut acc, w, h).unwrap();
    for i in 0..3 {
        let (a, b) = (triangle[i], triangle[(i + 1) % 3]);
        let mid = [f64::midpoint(a[0], b[0]), f64::midpoint(a[1], b[1])];
        raster.quad(point(a), point(mid), point(b));
    }
    let mut curves = vec![0; w * h];
    raster.coverage(&mut curves).unwrap();
    for (l, c) in lines.iter().zip(&curves) {
        assert!(l.abs_diff(*c) <= 1);
    }
}

#[test]
fn tight_curves_are_followed_closely() {
    // A parabola drawn from one curve, closed by its chord. The area
    // between them is two thirds of the triangle the control point spans.
    let (p0, c, p2) = ([1.0, 30.0], [20.0, -28.0], [39.0, 30.0]);
    let (w, h) = (40, 31);
    let mut acc = vec![0.0; Rasterizer::scratch_len(w, h)];
    let mut raster = Rasterizer::new(&mut acc, w, h).unwrap();
    raster.quad(point(p0), point(c), point(p2));
    raster.line(point(p2), point(p0));
    let mut out = vec![0; w * h];
    raster.coverage(&mut out).unwrap();
    let sum: f64 = out.iter().map(|&c| f64::from(c) / 255.0).sum();
    let area = 2.0 / 3.0 * signed_area(&[p0, c, p2]).abs();
    let partial = out.iter().filter(|&&c| c != 0 && c != 255).count() as f64;
    // The parabola is shorter than its control polygon, 122 pixels, and
    // flattening loses at most two thirds of 0.05 pixels along all of it.
    let tolerance = 2.0 / 3.0 * 0.05 * 122.0 + partial * 0.5 / 255.0;
    assert!(sum <= area + partial * 0.5 / 255.0, "{sum} > {area}");
    assert!(area - sum <= tolerance, "{sum} vs {area}");
    // The tip of the curve is at y = 1 at the middle.
    assert_eq!(out[20], 0);
    assert_eq!(out[2 * w + 20], 255);
}

#[test]
fn starts_empty_whatever_the_buffer_held() {
    let mut acc = vec![7.5; Rasterizer::scratch_len(3, 3) + 5];
    let raster = Rasterizer::new(&mut acc, 3, 3).unwrap();
    assert_eq!((raster.width(), raster.height()), (3, 3));
    let mut out = [1; 9];
    raster.coverage(&mut out).unwrap();
    assert_eq!(out, [0; 9]);
}

#[test]
fn buffers_must_be_large_enough() {
    assert_eq!(Rasterizer::scratch_len(3, 2), 10);
    assert_eq!(Rasterizer::scratch_len(0, 4), 8);
    assert_eq!(Rasterizer::scratch_len(usize::MAX, 2), usize::MAX);
    let mut acc = vec![0.0; 9];
    assert!(matches!(
        Rasterizer::new(&mut acc, 3, 2),
        Err(Error::BufferTooSmall)
    ));
    let mut acc = vec![0.0; 10];
    let raster = Rasterizer::new(&mut acc, 3, 2).unwrap();
    assert_eq!(raster.coverage(&mut [0; 5]), Err(Error::BufferTooSmall));
    assert_eq!(raster.coverage(&mut [0; 6]), Ok(()));
}

#[test]
fn empty_canvas_draws_nothing() {
    let mut acc = vec![0.0; Rasterizer::scratch_len(0, 3)];
    let mut raster = Rasterizer::new(&mut acc, 0, 3).unwrap();
    raster.line(Point::new(-1.0, 0.0), Point::new(2.0, 3.0));
    raster.quad(
        Point::new(0.0, 0.0),
        Point::new(5.0, 1.0),
        Point::new(0.0, 3.0),
    );
    assert_eq!(raster.coverage(&mut []), Ok(()));
}

#[test]
fn broken_coordinates_do_not_panic() {
    let mut acc = vec![0.0; Rasterizer::scratch_len(4, 4)];
    let mut raster = Rasterizer::new(&mut acc, 4, 4).unwrap();
    let odd = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, 2.5];
    for &x0 in &odd {
        for &y0 in &odd {
            for &x1 in &odd {
                raster.line(Point::new(x0, y0), Point::new(x1, 3.0));
                raster.quad(
                    Point::new(x0, 0.5),
                    Point::new(x1, y0),
                    Point::new(1.0, 2.0),
                );
            }
        }
    }
    let mut out = [0; 16];
    raster.coverage(&mut out).unwrap();
}
