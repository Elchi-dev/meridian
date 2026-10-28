//! The functions of a double awk has, which `core` leaves to the
//! platform's library: `sqrt`, `exp`, `log`, `sin`, `cos` and `atan2`.
//! Each reduces its argument to a small one, sums a short series there
//! and adds back what the reduction took off last, carrying the little
//! a double misses along, so that each lands within a double of the
//! true value; `sqrt` takes the root of the digits as an integer, so it
//! is exact.

use core::f64::consts::{FRAC_PI_4, LN_2, SQRT_2};

/// ln 2 in two parts, the first of 32 bits so that a whole multiple of
/// it is exact, worked out from its first 80 digits.
const LN2_HIGH: f64 = f64::from_bits(0x3fe6_2e42_fee0_0000);
const LN2_LOW: f64 = f64::from_bits(0x3dea_39ef_3579_3c76);

/// 2/3 as the nearest double and what it misses.
const TWO_THIRDS: [f64; 2] = [
    f64::from_bits(0x3fe5_5555_5555_5555),
    f64::from_bits(0x3c85_5555_5555_5555),
];

/// The angles atan2 measures from, as the nearest double and the little
/// it misses, worked out from π to 1500 bits: 0, the arctangent of 1/2
/// and π/4, then the same seen from π/2, from π/2 the other way, and
/// from π.
const ANGLES: [[[f64; 2]; 3]; 4] = [
    [
        [0.0, 0.0],
        [
            f64::from_bits(0x3fdd_ac67_0561_bb4f),
            f64::from_bits(0x3c7a_2b7f_222f_65e2),
        ],
        [
            f64::from_bits(0x3fe9_21fb_5444_2d18),
            f64::from_bits(0x3c81_a626_3314_5c07),
        ],
    ],
    [
        [
            f64::from_bits(0x3ff9_21fb_5444_2d18),
            f64::from_bits(0x3c91_a626_3314_5c07),
        ],
        [
            f64::from_bits(0x3ff1_b6e1_92eb_be44),
            f64::from_bits(0x3c9b_1b46_6a88_828e),
        ],
        [
            f64::from_bits(0x3fe9_21fb_5444_2d18),
            f64::from_bits(0x3c81_a626_3314_5c07),
        ],
    ],
    [
        [
            f64::from_bits(0x3ff9_21fb_5444_2d18),
            f64::from_bits(0x3c91_a626_3314_5c07),
        ],
        [
            f64::from_bits(0x4000_468a_8ace_4df6),
            f64::from_bits(0x3c80_620b_f740_6aff),
        ],
        [
            f64::from_bits(0x4002_d97c_7f33_21d2),
            f64::from_bits(0x3c9a_7939_4c9e_8a0a),
        ],
    ],
    [
        [
            f64::from_bits(0x4009_21fb_5444_2d18),
            f64::from_bits(0x3ca1_a626_3314_5c07),
        ],
        [
            f64::from_bits(0x4005_6c6e_7397_f5ae),
            f64::from_bits(0x3ca6_60b6_4ece_6f4b),
        ],
        [
            f64::from_bits(0x4002_d97c_7f33_21d2),
            f64::from_bits(0x3c9a_7939_4c9e_8a0a),
        ],
    ],
];
const PI: f64 = ANGLES[3][0][0];
const HALF_PI: f64 = ANGLES[1][0][0];
const QUARTER_PI: f64 = ANGLES[0][2][0];
const THREE_QUARTERS_PI: f64 = ANGLES[2][2][0];

/// The bits of 2/π after its point, 64 to a word, from π worked out to
/// 1500 bits: as many as the greatest double needs to find how far it
/// is past a multiple of π/2.
const TWO_OVER_PI: [u64; 20] = [
    0xa2f9_836e_4e44_1529,
    0xfc27_57d1_f534_ddc0,
    0xdb62_9599_3c43_9041,
    0xfe51_63ab_debb_c561,
    0xb724_6e3a_424d_d2e0,
    0x0649_2eea_09d1_921c,
    0xfe1d_eb1c_b129_a73e,
    0xe882_35f5_2ebb_4484,
    0xe99c_7026_b45f_7e41,
    0x3991_d639_8353_39f4,
    0x9c84_5f8b_bdf9_283b,
    0x1ff8_97ff_de05_980f,
    0xef2f_118b_5a0a_6d1f,
    0x6d36_7ecf_27cb_09b7,
    0x4f46_3f66_9e5f_ea2d,
    0x7527_bac7_ebe5_f17b,
    0x3d07_39f7_8a52_92ea,
    0x6bfb_5fb1_1f8d_5d08,
    0x5603_3046_fc7b_6bab,
    0xf0cf_bc20_9af4_361d,
];

/// π/2 in whole 2^-63rds, from the same π, which is as near as the
/// angles left after a reduction need.
const HALF_PI_FIXED: u64 = 0xc90f_daa2_2168_c235;

/// `x` times two to the power `n`, as far as a double reaches.
fn scaled(x: f64, n: i32) -> f64 {
    let mut x = x;
    let mut n = n;
    // A power past what one double holds is taken in steps.
    while n > 1023 {
        x *= f64::from_bits(0x7fe0_0000_0000_0000);
        n -= 1023;
        if x.is_infinite() {
            return x;
        }
    }
    while n < -1022 {
        x *= f64::from_bits(0x0010_0000_0000_0000);
        n += 1022;
        if x == 0.0 {
            return x;
        }
    }
    let bits = u64::try_from(1023 + n).unwrap_or(0) << 52;
    x * f64::from_bits(bits)
}

/// The whole number nearest `x`, which is well inside what an `i32`
/// holds.
#[allow(clippy::cast_possible_truncation)]
fn nearest(x: f64) -> i32 {
    // A cast goes toward zero; half a step first makes it the nearest.
    (if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i32
}

/// `a` + `b` as the double nearest it and what that double misses,
/// which is a double too.
fn added(a: f64, b: f64) -> (f64, f64) {
    let sum = a + b;
    let from_b = sum - a;
    let from_a = sum - from_b;
    (sum, (a - from_a) + (b - from_b))
}

/// The square root of `x`, rounded as IEEE 754 says: the root of its
/// significand taken as a whole number, to as many digits as a double
/// has and one more, which says the way to round.
#[must_use]
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    let bits = x.to_bits();
    let raw = i32::try_from(bits >> 52).unwrap_or(0);
    let (mut significand, mut power) = if raw == 0 {
        (bits & ((1 << 52) - 1), -1074)
    } else {
        ((bits & ((1 << 52) - 1)) | (1 << 52), raw - 1075)
    };
    // x = significand * 2^power, with the power even.
    if power % 2 != 0 {
        significand <<= 1;
        power -= 1;
    }
    // Shifted so that its root has 55 bits or more.
    let shift = 110 - (64 - significand.leading_zeros());
    let shift = shift + shift % 2;
    let wide = u128::from(significand) << shift;
    let mut root = isqrt(wide);
    let exact = root * root == wide;
    let mut half_power = (power - i32::try_from(shift).unwrap_or(0)) / 2;
    // Down to 53 bits, rounding to the nearest and to even on a tie.
    let extra = 128 - root.leading_zeros() - 53;
    let dropped = root & ((1 << extra) - 1);
    let half = 1u128 << (extra - 1);
    root >>= extra;
    half_power += i32::try_from(extra).unwrap_or(0);
    if dropped > half || (dropped == half && (!exact || root & 1 == 1)) {
        root += 1;
    }
    #[allow(clippy::cast_precision_loss)]
    let root = root as f64;
    scaled(root, half_power)
}

/// The whole square root of `n`, rounded down.
fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut root = 1u128 << (128 - n.leading_zeros()).div_ceil(2);
    loop {
        let next = u128::midpoint(root, n / root);
        if next >= root {
            return root;
        }
        root = next;
    }
}

/// e to the power `x`: `x` is k ln 2 and a remainder r no greater than
/// half of ln 2 either way, and e^r is 1 + r + r²/2(1 + r/3(1 + r/4(...))),
/// with 1 + r kept exactly in two doubles so that the series only adds a
/// little to them.
#[must_use]
pub fn exp(x: f64) -> f64 {
    exp_of(x, 0.0)
}

/// e to the power `x` + `more`, where `more` is far below `x`'s last
/// place.
fn exp_of(x: f64, more: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.8 {
        return f64::INFINITY;
    }
    if x < -745.2 {
        return 0.0;
    }
    let k = nearest(x / LN_2);
    let whole = f64::from(k);
    let (r, missed) = added(x - whole * LN2_HIGH, -(whole * LN2_LOW));
    let missed = missed + more;
    let mut tail = 1.0;
    for n in (3..=16).rev() {
        tail = 1.0 + r * tail / f64::from(n);
    }
    let (most, lost) = added(1.0, r);
    let small = lost + 0.5 * r * r * tail;
    // e^(r + missed) is e^r (1 + missed), near enough.
    scaled(most + (small + (most + small) * missed), k)
}

/// The natural logarithm of `x`: `x` is (1 + f) 2^k with 1 + f between
/// √½ and √2, and ln(1 + f) is 2 atanh s for s = f/(2 + f), which is
/// f - f²/2 + s(f²/2 + R) with R = 2s²/3 + 2s⁴/5 + ..., so the part
/// of it that a series gives is small.
#[must_use]
pub fn log(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return x;
    }
    let (high, low) = log_of(x);
    high + low
}

/// The natural logarithm of `x`, finite and above zero, as the double
/// nearest it and what that misses, near enough.
fn log_of(x: f64) -> (f64, f64) {
    let (mut part, mut power) = parts(x);
    if part > SQRT_2 {
        part /= 2.0;
        power += 1;
    }
    let f = part - 1.0;
    // s = f/(2 + f), and what the double of it misses.
    let (two, two_lost) = added(2.0, f);
    let s = f / two;
    let (back, back_lost) = multiplied(s, two);
    let s_lost = (((f - back) - back_lost) - s * two_lost) / two;
    // R = 2s²/3 + s⁴(2/5 + 2s²/7 + ...), its first term carried.
    let (square, square_lost) = multiplied(s, s);
    let square_lost = square_lost + 2.0 * s * s_lost;
    let mut rest = 0.0;
    for j in (2..=12).rev() {
        rest = square * (2.0 / f64::from(2 * j + 1) + rest);
    }
    let (first, first_lost) = multiplied(square, TWO_THIRDS[0]);
    let first_lost = first_lost + square * TWO_THIRDS[1] + square_lost * TWO_THIRDS[0];
    let (f_square, f_square_lost) = multiplied(f, f);
    let (half, half_lost) = (0.5 * f_square, 0.5 * f_square_lost);
    // t = s(f²/2 + R).
    let (inner, inner_lost) = added(half, first);
    let (inner, more_lost) = added(inner, square * rest);
    let inner_lost = inner_lost + more_lost + half_lost + first_lost;
    let (t, t_lost) = multiplied(s, inner);
    let t_lost = t_lost + s * inner_lost + s_lost * inner;
    // ln x = k ln 2 + f - (f²/2 - t).
    let whole = f64::from(power);
    let (less, less_lost) = added(half, -t);
    let (most, most_lost) = added(whole * LN2_HIGH, f);
    let (nearly, nearly_lost) = added(most, -less);
    let (high, high_lost) = added(nearly, whole * LN2_LOW);
    let low = high_lost + nearly_lost + most_lost - (less_lost + half_lost) + t_lost;
    (high, low)
}

/// Whether `x` is a whole number.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]
fn whole_number(x: f64) -> bool {
    // From 2^52 up a double has no fraction; below, a cast drops it.
    x.abs() >= 4_503_599_627_370_496.0 || (x as i64) as f64 == x
}

/// Whether `x` is an odd whole number.
#[allow(clippy::cast_possible_truncation)]
fn odd(x: f64) -> bool {
    whole_number(x) && x.abs() < 9_007_199_254_740_992.0 && (x as i64) % 2 != 0
}

/// `x` to the power `y`, as C's pow has it for zeros, infinities and
/// the value that is no number: a whole power by products of squares
/// when each is exact, else e to the power y ln x with the little that
/// a double of ln x misses carried along.
#[must_use]
#[allow(clippy::float_cmp)]
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if y.is_infinite() {
        if x.abs() == 1.0 {
            return 1.0;
        }
        return if (x.abs() > 1.0) == (y > 0.0) {
            f64::INFINITY
        } else {
            0.0
        };
    }
    let odd = odd(y);
    if x == 0.0 || x.is_infinite() {
        let magnitude = if (x == 0.0) == (y < 0.0) {
            f64::INFINITY
        } else {
            0.0
        };
        return if x.is_sign_negative() && odd {
            -magnitude
        } else {
            magnitude
        };
    }
    if x < 0.0 && !whole_number(y) {
        return f64::NAN;
    }
    let magnitude = power_of(x.abs(), y);
    if x < 0.0 && odd {
        -magnitude
    } else {
        magnitude
    }
}

/// `a`, finite and above zero, to the power `y`, finite.
fn power_of(a: f64, y: f64) -> f64 {
    if whole_number(y)
        && y.abs() <= 1024.0
        && let Some(power) = exact_power(a, y)
    {
        return power;
    }
    let (high, low) = log_of(a);
    let rough = y * high;
    // Far past the doubles' range the parts below change nothing.
    if !rough.is_finite() || rough.abs() > 1000.0 {
        return exp(rough);
    }
    let (product, lost) = multiplied(y, high);
    exp_of(product, lost + y * low)
}

/// `a` to the whole power `y`, when each product of squares on the way
/// is exact: then the power is too, or a single division from it when
/// `y` is below zero.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn exact_power(a: f64, y: f64) -> Option<f64> {
    // Within these bounds a product and what it misses are found exactly.
    let fits = |x: f64| {
        (f64::from_bits(0x20b0_0000_0000_0000)..f64::from_bits(0x5f30_0000_0000_0000)).contains(&x)
    };
    let exact = |a: f64, b: f64| {
        let (product, lost) = multiplied(a, b);
        (lost == 0.0 && fits(product)).then_some(product)
    };
    let mut left = y.abs() as u64;
    let mut base = a;
    let mut power = 1.0;
    if !fits(base) {
        return None;
    }
    while left > 0 {
        if left & 1 == 1 {
            power = exact(power, base)?;
        }
        left >>= 1;
        if left > 0 {
            base = exact(base, base)?;
        }
    }
    Some(if y < 0.0 { 1.0 / power } else { power })
}

/// The remainder of `x` / `y`, with the sign of `x`, as C's fmod has
/// it: exact, by long division of the two significands.
#[must_use]
pub fn fmod(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() || x.is_infinite() || y == 0.0 {
        return f64::NAN;
    }
    if y.is_infinite() || x.abs() < y.abs() {
        return x;
    }
    // Each as a whole significand times a power of two; the greater has
    // the greater power, or the same.
    let split = |v: f64| {
        let bits = v.abs().to_bits();
        let raw = i32::try_from(bits >> 52).unwrap_or(0);
        let fraction = bits & ((1 << 52) - 1);
        if raw == 0 {
            (fraction, -1074)
        } else {
            (fraction | (1 << 52), raw - 1075)
        }
    };
    let (top, top_power) = split(x);
    let (bottom, bottom_power) = split(y);
    let mut rest = top % bottom;
    for _ in bottom_power..top_power {
        rest = (rest << 1) % bottom;
    }
    #[allow(clippy::cast_precision_loss)]
    let rest = scaled(rest as f64, bottom_power);
    if x < 0.0 { -rest } else { rest }
}

/// `x`, which is finite and above zero, as m 2^k with m in [1, 2).
fn parts(x: f64) -> (f64, i32) {
    let (x, shift) = if x < f64::MIN_POSITIVE {
        (x * f64::from_bits(0x4350_0000_0000_0000), 54)
    } else {
        (x, 0)
    };
    let bits = x.to_bits();
    let raw = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
    let m = f64::from_bits((bits & ((1 << 52) - 1)) | (1023 << 52));
    (m, raw - 1023 - shift)
}

/// The 64 bits of 2/π in its word `n`; before its point it has none.
fn word(n: i32) -> u128 {
    let found = usize::try_from(n).ok().and_then(|n| TWO_OVER_PI.get(n));
    u128::from(found.copied().unwrap_or(0))
}

/// `x`, finite and above π/4, less the multiple of π/2 nearest it: what
/// is left, within π/4 either way, as a double and the little it
/// misses, and which multiple it was, counted round by four.
///
/// `x` is m 2^e for a whole m, so the bits of 2/π down to 2^(2 - e)
/// make whole multiples of four quarter turns, which change nothing;
/// the 192 after them make the quarter turns and how far past one `x`
/// is, to 128 bits.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn reduced(x: f64) -> (f64, f64, u32) {
    const LOW: u128 = u64::MAX as u128;
    let bits = x.to_bits();
    let m = u128::from((bits & ((1 << 52) - 1)) | (1 << 52));
    let at = i32::try_from(bits >> 52).unwrap_or(0) - 1077;
    let (first, shift) = (at.div_euclid(64), at.rem_euclid(64));
    let [high, middle, low] = [0, 1, 2].map(|n| {
        let pair = (word(first + n) << 64) | word(first + n + 1);
        m * ((pair >> (64 - shift)) & LOW)
    });
    // m times the 192 bits, of which the lowest 192 count.
    let carried = (low >> 64) + (middle & LOW);
    let top = ((carried >> 64) + (middle >> 64) + high) & LOW;
    let mut quadrant = (top >> 62) as u32;
    let mut past = (top << 66) | ((carried & LOW) << 2) | ((low & LOW) >> 62);
    // Past half a quarter turn the next multiple is nearer, and x is
    // short of it.
    let short = past >> 127 == 1;
    if short {
        quadrant += 1;
        past = past.wrapping_neg();
    }
    // In 2^-127ths of a radian.
    let half_pi = u128::from(HALF_PI_FIXED);
    let left = (past >> 64) * half_pi + (((past & LOW) * half_pi) >> 64);
    let near = left as f64;
    let missed = (left.cast_signed() - (near as u128).cast_signed()) as f64;
    let (near, missed) = (scaled(near, -127), scaled(missed, -127));
    if short {
        (-near, -missed, quadrant % 4)
    } else {
        (near, missed, quadrant % 4)
    }
}

/// The sine of `near` + `missed`, where `near` lies within π/4 either
/// way and `missed` is below its last place: sin r is r(1 - r²/6(1 -
/// r²/20(1 - ...))), and the missed part adds missed cos r.
fn sine(near: f64, missed: f64) -> f64 {
    let square = near * near;
    let mut tail = 1.0;
    for n in (2..=10).rev() {
        tail = 1.0 - square * tail / f64::from(2 * n * (2 * n + 1));
    }
    near + (missed * (1.0 - 0.5 * square) - near * square * tail / 6.0)
}

/// The cosine of `near` + `missed`, as `sine` has them: cos r is 1 -
/// r²/2(1 - r²/12(1 - r²/30(...))), and the missed part takes off
/// missed sin r. What the double nearest 1 - r²/2 misses is carried.
fn cosine(near: f64, missed: f64) -> f64 {
    let square = near * near;
    let mut tail = 1.0;
    for n in (3..=10).rev() {
        tail = 1.0 - square * tail / f64::from((2 * n - 1) * 2 * n);
    }
    let half = 0.5 * square;
    let most = 1.0 - half;
    most + (((1.0 - most) - half) + (square * square / 24.0 * tail - near * missed))
}

/// The sine of `x`, in radians.
#[must_use]
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x.abs() <= FRAC_PI_4 {
        // sin -0 is -0, which the series would lose.
        return if x == 0.0 { x } else { sine(x, 0.0) };
    }
    let (near, missed, quadrant) = reduced(x.abs());
    let turned = match quadrant {
        0 => sine(near, missed),
        1 => cosine(near, missed),
        2 => -sine(near, missed),
        _ => -cosine(near, missed),
    };
    if x < 0.0 { -turned } else { turned }
}

/// The cosine of `x`, in radians.
#[must_use]
pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x.abs() <= FRAC_PI_4 {
        return cosine(x, 0.0);
    }
    let (near, missed, quadrant) = reduced(x.abs());
    match quadrant {
        0 => cosine(near, missed),
        1 => -sine(near, missed),
        2 => -cosine(near, missed),
        _ => sine(near, missed),
    }
}

/// `a` times `b` as the double nearest it and what that double misses,
/// for `a` and `b` well inside the doubles' range: each is cut in two
/// halves of 26 bits, whose products are exact.
fn multiplied(a: f64, b: f64) -> (f64, f64) {
    let halves = |x: f64| {
        let lifted = x * 134_217_729.0;
        let high = lifted - (lifted - x);
        (high, x - high)
    };
    let product = a * b;
    let ((a1, a0), (b1, b0)) = (halves(a), halves(b));
    (product, ((a1 * b1 - product) + a1 * b0 + a0 * b1) + a0 * b0)
}

/// `a` / `b`, for `a` from 0 to `b`, as the double nearest it and what
/// that double misses; far below 1 what it misses changes no arctangent.
fn quotient(a: f64, b: f64) -> (f64, f64) {
    let t = a / b;
    if t < f64::from_bits(0x3e40_0000_0000_0000) {
        return (t, 0.0);
    }
    // Both brought near 1 alike, so that the products stay in range.
    let (near_one, k) = parts(b);
    let (product, lost) = multiplied(t, near_one);
    (t, ((scaled(a, -k) - product) - lost) / near_one)
}

/// The arctangent of `u` + `more`, where `u` is within 7/16 either way
/// and `more` is far below it, from its series: u(1 - u²/3 + u⁴/5 - ...).
fn arc(u: f64, more: f64) -> f64 {
    let square = u * u;
    let mut sum = 0.0;
    for n in (1..=24).rev() {
        let sign = if n % 2 == 0 { 1.0 } else { -1.0 };
        sum = square * (sign / f64::from(2 * n + 1) + sum);
    }
    u + (u * sum + more)
}

/// The arctangent of `t` + `missed`, for `t` from 0 to 1 and `missed`
/// below its last place: which of 0, the arctangent of 1/2 and π/4 is
/// nearest, by its place in `ANGLES`, and how far past that it is, the
/// arctangent of (t - c)/(1 + tc) for c 0, 1/2 or 1.
fn atan(t: f64, missed: f64) -> (usize, f64) {
    let more = missed / (1.0 + t * t);
    if t < 7.0 / 16.0 {
        (0, arc(t, more))
    } else if t < 11.0 / 16.0 {
        (1, arc((2.0 * t - 1.0) / (2.0 + t), more))
    } else {
        (2, arc((t - 1.0) / (t + 1.0), more))
    }
}

/// The angle of the point (`x`, `y`) from the x axis, in radians from -π
/// to π, as C's atan2 has it for zeros and infinities.
///
/// The angle within the first octant comes of the lesser of |x| and |y|
/// over the greater; seen from the axis it is nearer to, and from which
/// side, it is one of four in `ANGLES` plus or less that.
#[must_use]
pub fn atan2(y: f64, x: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    let toward = |angle: f64| if y.is_sign_negative() { -angle } else { angle };
    if y == 0.0 {
        return toward(if x.is_sign_negative() { PI } else { 0.0 });
    }
    if x == 0.0 {
        return toward(HALF_PI);
    }
    if x.is_infinite() && y.is_infinite() {
        return toward(if x < 0.0 {
            THREE_QUARTERS_PI
        } else {
            QUARTER_PI
        });
    }
    if x.is_infinite() {
        return toward(if x < 0.0 { PI } else { 0.0 });
    }
    if y.is_infinite() {
        return toward(HALF_PI);
    }
    let (across, up) = (x.abs(), y.abs());
    let steep = up > across;
    let (t, missed) = if steep {
        quotient(across, up)
    } else {
        quotient(up, across)
    };
    let (from, past) = atan(t, missed);
    let way = match (steep, x < 0.0) {
        (false, false) => 0,
        (true, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    };
    let [high, low] = ANGLES[way][from];
    toward(high + (low + if way % 2 == 0 { past } else { -past }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many doubles apart `a` and `b` are; none apart when both are
    /// the value that is no number.
    fn apart(a: f64, b: f64) -> u64 {
        if a.is_nan() && b.is_nan() {
            return 0;
        }
        let ordered = |x: f64| {
            let bits = x.to_bits().cast_signed();
            if bits < 0 { i64::MIN - bits } else { bits }
        };
        ordered(a).abs_diff(ordered(b))
    }

    fn next(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// A double from `seed`'s bits, finite and above zero.
    fn any_positive(seed: &mut u64) -> f64 {
        loop {
            let x = f64::from_bits(next(seed) >> 1);
            if x.is_finite() && x > 0.0 {
                return x;
            }
        }
    }

    /// A double spread evenly from `low` to `high`.
    #[allow(clippy::cast_precision_loss)]
    fn between(seed: &mut u64, low: f64, high: f64) -> f64 {
        low + (high - low) * ((next(seed) >> 11) as f64 / (1u64 << 53) as f64)
    }

    /// `count` inputs `draw` makes from the seed `seed`.
    fn drawn<T>(count: usize, mut seed: u64, mut draw: impl FnMut(&mut u64) -> T) -> Vec<T> {
        (0..count).map(|_| draw(&mut seed)).collect()
    }

    /// Checks `ours` against `theirs`, from the host's library, on each
    /// of `inputs`: always within a double, and the same double for all
    /// but `most` in a thousand. The library rounds nearly every value as
    /// near as a double can be, and so do these but for the little their
    /// reductions cannot carry, so a slip in what they carry shows as
    /// more of them a double off.
    fn close<T: Copy + core::fmt::Debug>(
        what: &str,
        inputs: &[T],
        most: usize,
        ours: impl Fn(T) -> f64,
        theirs: impl Fn(T) -> f64,
    ) {
        let mut off = 0;
        for &input in inputs {
            let (a, b) = (ours(input), theirs(input));
            assert!(apart(a, b) <= 1, "{what} of {input:?}: {a:e} and {b:e}");
            off += usize::from(a.to_bits() != b.to_bits());
        }
        assert!(
            off * 1000 <= most * inputs.len(),
            "{what}: {off} of {} a double off",
            inputs.len()
        );
    }

    #[test]
    fn square_roots_are_exact() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..200_000 {
            let x = any_positive(&mut seed);
            assert_eq!(sqrt(x).to_bits(), x.sqrt().to_bits(), "sqrt of {x:e}");
        }
        for x in [
            1.0,
            2.0,
            4.0,
            0.25,
            f64::MIN_POSITIVE,
            5e-324,
            f64::MAX,
            1e-310,
            3.0,
        ] {
            assert_eq!(sqrt(x).to_bits(), x.sqrt().to_bits(), "sqrt of {x:e}");
        }
        assert!(sqrt(-1.0).is_nan());
        assert!(sqrt(f64::NAN).is_nan());
        assert_eq!(sqrt(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(sqrt(f64::INFINITY), f64::INFINITY);
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(15), 3);
        assert_eq!(isqrt(16), 4);
        assert_eq!(isqrt(u128::MAX), u128::from(u64::MAX));
    }

    #[test]
    fn exponentials_and_logarithms_are_within_a_double() {
        let spread = drawn(100_000, 0x6a09_e667_f3bc_c908, |s| {
            between(s, -745.0, 709.7)
        });
        close("exp", &spread, 25, exp, f64::exp);
        let any = drawn(100_000, 0x510e_527f_ade6_82d1, any_positive);
        close("log", &any, 5, log, f64::ln);
        let near_one = drawn(100_000, 0x9b05_688c_2b3e_6c1f, |s| between(s, 0.5, 2.0));
        close("log", &near_one, 5, log, f64::ln);
        let edges = [
            0.0,
            710.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -746.0,
            f64::NAN,
            709.78,
            -745.1,
            -708.5,
        ];
        close("exp", &edges, 1000, exp, f64::exp);
        let edges = [
            1.0,
            0.0,
            f64::INFINITY,
            -1.0,
            f64::NAN,
            5e-324,
            f64::MAX,
            f64::MIN_POSITIVE,
        ];
        close("log", &edges, 1000, log, f64::ln);
        assert_eq!(scaled(1.0, 2000), f64::INFINITY);
        assert_eq!(scaled(1.0, -2000), 0.0);
        assert_eq!(scaled(1.5, -1074 + 1), 5e-324 * 3.0);
    }

    /// Doubles past 100000 drawn from every size, with their sines and
    /// cosines worked out from π to 1300 digits, to 90, and rounded to
    /// the nearest double, all as bits.
    const FAR: [(u64, u64, u64); 300] = [
        (
            0x6eee_aa9e_a127_93d0,
            0xbfef_fbf5_57df_d77c,
            0xbfa0_14c0_1e15_71e3,
        ),
        (
            0x45fd_989b_6295_7f0a,
            0x3fb6_d954_4aa3_a738,
            0xbfef_df4e_1c60_87a0,
        ),
        (
            0x67aa_2fb3_a5fb_c888,
            0xbfed_12fa_2b28_0383,
            0x3fda_bc41_170f_9101,
        ),
        (
            0x773d_f864_1598_bf3c,
            0xbfc8_1878_57a0_9590,
            0x3fef_6d8a_9dca_dac2,
        ),
        (
            0x7be3_7b1c_edde_a088,
            0xbfe7_99a4_ea9e_5734,
            0x3fe5_9c65_3b87_cfad,
        ),
        (
            0x4961_3b9f_131b_3f48,
            0x3fec_e466_7051_1221,
            0x3fdb_8354_cd4e_d873,
        ),
        (
            0x57ef_749f_1b29_5799,
            0x3feb_5fd7_2112_8483,
            0x3fe0_9286_fb47_ce7a,
        ),
        (
            0x735c_14d5_0e6c_b6da,
            0xbfeb_e11d_f405_9509,
            0x3fdf_6a7e_4cd4_dfda,
        ),
        (
            0x436b_a952_24f6_ea06,
            0xbfe4_1044_fac1_3deb,
            0x3fe8_edd1_6407_73d3,
        ),
        (
            0x5714_cbcc_d473_55b8,
            0xbfec_ea66_360f_0f34,
            0x3fdb_6a13_9048_8acc,
        ),
        (
            0x6820_918f_74d3_cfc4,
            0xbfc1_dda3_9ef8_ba0c,
            0xbfef_afcf_9f27_cc29,
        ),
        (
            0x45d1_6e42_befb_57d4,
            0xbfee_b431_cd02_464c,
            0xbfd2_07b7_0007_fb49,
        ),
        (
            0x7d92_576f_6e8d_9492,
            0xbfb6_55cf_0de0_f839,
            0xbfef_e0c2_fb10_74f0,
        ),
        (
            0x4aa2_2a5a_4591_1236,
            0x3fc6_66f8_dcde_c9d5,
            0xbfef_818f_2692_d818,
        ),
        (
            0x4e47_7d14_0d99_ec4b,
            0x3fc5_0b5d_85e5_8004,
            0xbfef_9086_5ee9_3d88,
        ),
        (
            0x7c11_bc7e_c2e8_4380,
            0xbfd3_f6e6_eb53_b94e,
            0x3fee_6738_a86b_76f5,
        ),
        (
            0x7d49_1a69_7b03_609c,
            0x3fee_1ce8_6800_1475,
            0x3fd5_a724_2e75_54ca,
        ),
        (
            0x7581_3499_36b9_7381,
            0x3fd7_42b6_db6e_63f6,
            0xbfed_cfca_11d0_bb39,
        ),
        (
            0x42e2_bc3d_4f0c_4c5b,
            0xbfec_b17f_ec8c_484a,
            0x3fdc_5546_4c45_79df,
        ),
        (
            0x61a1_8593_6ea7_4fa9,
            0x3fd9_756f_4dcd_c55f,
            0x3fed_5bf5_40d8_5932,
        ),
        (
            0x79cb_ec43_aa5b_1ef6,
            0x3fe9_9319_5700_23d6,
            0xbfe3_3bdb_2964_054b,
        ),
        (
            0x7241_d87b_9d54_2329,
            0xbfd9_e1a3_3c44_ddf2,
            0x3fed_4444_e042_0ada,
        ),
        (
            0x70bd_166d_1440_ec1c,
            0xbfe2_556c_f5a5_28ed,
            0x3fea_3a35_d60e_90fe,
        ),
        (
            0x7a81_1196_2550_33f9,
            0x3fa6_2a91_4a6e_0200,
            0x3fef_f851_b3d1_8015,
        ),
        (
            0x4867_298d_84a5_9483,
            0xbfef_54f9_603e_de44,
            0xbfca_04bc_84d8_e51e,
        ),
        (
            0x4fc1_1af0_afbb_869a,
            0x3fef_fd3f_991f_41f0,
            0xbf9a_89d0_bf10_ae71,
        ),
        (
            0x79ea_8c09_2b9c_6536,
            0x3fab_57a9_070d_2d99,
            0xbfef_f44f_6eb7_b21a,
        ),
        (
            0x5dfd_06c5_f313_e103,
            0x3fe7_5a6c_37ef_1fef,
            0xbfe5_e0a7_6f2f_f7fe,
        ),
        (
            0x7335_050a_94c2_ee26,
            0x3fd4_6bed_8a48_92b9,
            0xbfee_53c3_ff33_b964,
        ),
        (
            0x706d_1824_2a26_5d07,
            0xbfe1_3774_e2a3_d595,
            0x3fea_f94d_36fc_3c5a,
        ),
        (
            0x4cc8_9038_3d7e_d303,
            0xbfe3_f543_685d_8879,
            0xbfe9_0375_9be5_fe3e,
        ),
        (
            0x74d7_62dd_5bdc_1688,
            0x3fe9_9a13_e37d_988a,
            0xbfe3_3290_21c9_4959,
        ),
        (
            0x6a88_e402_72c0_afca,
            0x3fe8_d038_f646_87c9,
            0xbfe4_34d3_9082_4523,
        ),
        (
            0x6eb2_a630_e80c_aac4,
            0x3fd7_3893_4f2e_64ea,
            0x3fed_d1c3_de38_bfb1,
        ),
        (
            0x5e0b_2d49_d04b_0160,
            0xbfef_b0a2_dea1_ba4c,
            0xbfc1_c629_2cfa_b7e0,
        ),
        (
            0x5c31_1b90_01b1_5907,
            0x3fee_4c6e_ae04_c396,
            0xbfd4_9749_ec83_0c7d,
        ),
        (
            0x600e_6afe_9f9a_fd8a,
            0x3fef_18ca_3ebc_dce9,
            0xbfce_3222_71e7_fcab,
        ),
        (
            0x45ee_a708_6ef6_52a3,
            0xbfe4_cf40_f3c1_0a9e,
            0x3fe8_4f48_0c12_5178,
        ),
        (
            0x7a95_6b77_21f4_f30b,
            0x3fed_a6a6_3cd5_4e00,
            0x3fd8_1176_7f53_01d9,
        ),
        (
            0x4906_006d_05df_82a3,
            0x3fe5_d6ad_7887_2a8c,
            0xbfe7_63c0_ce37_fdf4,
        ),
        (
            0x5577_33c1_c541_d977,
            0x3fcb_4fe5_bf72_1ca6,
            0x3fef_4357_03fa_1391,
        ),
        (
            0x52b0_b39a_4f9e_22e8,
            0x3fe0_bc80_d71b_c6ff,
            0xbfeb_4641_7a14_1470,
        ),
        (
            0x5815_5284_ec88_5d7e,
            0xbf44_9314_4475_7112,
            0x3fef_ffff_962c_1492,
        ),
        (
            0x7f08_a009_6cca_f697,
            0x3fb1_828f_b2c6_c674,
            0xbfef_ecd0_a4fe_e315,
        ),
        (
            0x7402_8b92_d766_ba3e,
            0xbfd9_b641_584c_5897,
            0xbfed_4dd2_98a1_c673,
        ),
        (
            0x424e_63ee_1403_5113,
            0x3fed_9fb1_5b0b_c32b,
            0xbfd8_33a2_1733_2f07,
        ),
        (
            0x70a4_225b_0f88_9373,
            0xbfee_480a_e022_daa5,
            0x3fd4_b10e_695d_cd59,
        ),
        (
            0x5abf_855d_9120_1b0d,
            0x3fdd_d6c4_5d42_3be3,
            0xbfec_4f26_3ae0_4eb7,
        ),
        (
            0x4489_6937_7aca_4bb2,
            0x3fe7_d7ae_1fd4_58c5,
            0xbfe5_57e0_85e7_b6ca,
        ),
        (
            0x47a4_6212_1040_0243,
            0xbfed_d3e9_2888_b8c7,
            0x3fd7_2d8a_caf9_51ba,
        ),
        (
            0x4eb0_462d_1934_f8bf,
            0xbfed_0c6a_d798_b06f,
            0xbfda_d8b7_a4a1_c54b,
        ),
        (
            0x5917_d32e_e3d1_9601,
            0xbfee_b568_9f87_c298,
            0xbfd1_ff6f_b69c_a394,
        ),
        (
            0x430e_7e14_5987_35b4,
            0xbfe6_ac87_c719_1b64,
            0xbfe6_94ae_bd51_3b00,
        ),
        (
            0x5fda_2bf4_80d1_d858,
            0x3fed_1271_8280_0fed,
            0xbfda_be93_6c00_0bbf,
        ),
        (
            0x7d16_b10a_82c9_6618,
            0x3fef_f840_6e04_25c9,
            0xbfa6_436c_ce7b_e2a0,
        ),
        (
            0x726c_bee4_25dc_d6f2,
            0xbfe7_c855_5535_a074,
            0xbfe5_68f8_ffeb_9add,
        ),
        (
            0x6ed9_8854_e74c_14c6,
            0xbfe8_243b_9baa_29c3,
            0xbfe5_0122_aef6_0e2e,
        ),
        (
            0x5219_4ace_fb71_98a3,
            0x3fde_94a7_929a_6143,
            0x3fec_1c4f_dcc8_e29e,
        ),
        (
            0x585a_6d8a_7a9f_6d8f,
            0xbfef_fafb_42ba_8ed8,
            0xbfa1_eb3c_130f_404f,
        ),
        (
            0x52b6_a0c4_70f0_3fbf,
            0xbfef_b5bf_f70a_59b8,
            0x3fc1_31d1_e592_8aa4,
        ),
        (
            0x5207_7428_256e_9cc5,
            0xbfea_b1ee_10b0_42aa,
            0x3fe1_a554_582f_339b,
        ),
        (
            0x7e41_e064_cab9_b462,
            0xbfe1_006e_db82_036a,
            0xbfeb_1c1d_566a_7c23,
        ),
        (
            0x7631_df9b_8fde_7399,
            0x3feb_163d_98ec_464b,
            0xbfe1_09c9_0a21_9e86,
        ),
        (
            0x54d8_6005_bd23_2a1a,
            0xbfef_2b14_adbe_1c79,
            0xbfcc_fe46_0244_ee0c,
        ),
        (
            0x5b5b_581e_eb07_8d53,
            0x3fe9_6945_16b0_4e85,
            0xbfe3_72fd_1724_ecdb,
        ),
        (
            0x6b69_eddc_0034_9871,
            0xbfe4_ae78_eae7_6978,
            0x3fe8_6b31_ab6d_2850,
        ),
        (
            0x6e7b_453c_41cd_0f51,
            0xbfed_1e5d_526e_c977,
            0xbfda_8a80_6f0a_4b14,
        ),
        (
            0x7f90_9fe0_2275_b606,
            0xbfe9_2ea2_2d12_2726,
            0xbfe3_bead_d79d_b94f,
        ),
        (
            0x4dd6_0c99_eb66_d740,
            0xbfe5_e529_3052_afce,
            0x3fe7_5632_8d19_7068,
        ),
        (
            0x56a7_3201_4efe_2272,
            0x3fc0_3e0a_01d3_b694,
            0x3fef_bdc7_68d1_3628,
        ),
        (
            0x7cf9_fe7f_9f4f_6095,
            0x3feb_9107_b847_aebf,
            0xbfe0_4030_e727_c79e,
        ),
        (
            0x7b87_01b0_4174_c708,
            0x3fed_3d35_1525_1900,
            0xbfda_017c_cb25_8926,
        ),
        (
            0x4599_eb63_75de_a209,
            0x3fee_8a89_4a35_06a0,
            0xbfd3_1a8b_2cd0_d09c,
        ),
        (
            0x4b19_f81e_44c9_473a,
            0xbfe8_4121_dad1_b7fb,
            0x3fe4_dfbd_0bea_f8ee,
        ),
        (
            0x4cef_011d_4080_b340,
            0x3fef_dd94_6c16_2aa2,
            0x3fb7_715d_cb6d_2265,
        ),
        (
            0x476d_d360_3b36_7a4b,
            0x3fe8_1a75_28e2_35e4,
            0xbfe5_0c59_9d46_62dd,
        ),
        (
            0x6e63_e0ba_99f3_acfd,
            0x3fe1_5dcc_7593_a61c,
            0xbfea_e0ad_b9ab_9547,
        ),
        (
            0x5aeb_4d43_93a2_a9bf,
            0x3fc9_d1dc_6fe2_0097,
            0x3fef_579a_a562_1791,
        ),
        (
            0x6b36_0168_2546_2845,
            0x3fee_6e06_e222_e8c5,
            0x3fd3_cd42_23f5_f132,
        ),
        (
            0x50d1_20e7_1fcf_bdbb,
            0x3fee_98fa_d5ce_e0d6,
            0x3fd2_bd34_2be3_0ffa,
        ),
        (
            0x57b2_407d_29e3_f7ef,
            0x3fe3_7ab7_25ac_2866,
            0xbfe9_6359_4371_c805,
        ),
        (
            0x68f7_ec22_6e2c_fb0e,
            0x3fe7_2ef4_a9d7_e00c,
            0x3fe6_0eb1_ecca_602d,
        ),
        (
            0x505f_9c35_ab7b_8d11,
            0x3fed_bdf1_985d_8ba5,
            0xbfd7_9d67_35dc_96a9,
        ),
        (
            0x70b2_f326_503d_24c2,
            0x3fde_48ec_6593_4506,
            0xbfec_30c7_977b_dbcb,
        ),
        (
            0x4c20_0ebc_246a_7cbc,
            0x3fef_ff33_0f4e_3338,
            0x3f8c_a179_ebd4_eafa,
        ),
        (
            0x6bbc_bb1a_3349_8a38,
            0x3fde_7329_858b_0dae,
            0x3fec_2565_4550_58f8,
        ),
        (
            0x6e16_db1c_c82b_4767,
            0x3fe8_a80b_4735_3515,
            0xbfe4_65c7_20bb_d297,
        ),
        (
            0x77b4_2d0c_cce5_bf33,
            0xbfa9_625c_308f_3fe2,
            0xbfef_f5ec_fad7_f2ff,
        ),
        (
            0x66dc_d2f1_9a18_d9ba,
            0xbfee_b47f_4cc2_04a6,
            0xbfd2_05a6_f92e_348e,
        ),
        (
            0x681e_50ec_aa04_78d1,
            0x3fe2_f97d_5115_abda,
            0xbfe9_c47d_099a_19d6,
        ),
        (
            0x55de_c73d_3e9b_012b,
            0x3fe4_4004_4c01_3568,
            0xbfe8_c717_e98c_d47f,
        ),
        (
            0x6e57_e3a3_b023_16ba,
            0x3fee_805c_4089_f6ad,
            0x3fd3_5b25_d7da_c658,
        ),
        (
            0x7d07_bdc0_3ffe_2d56,
            0x3fe4_ad5a_6935_f14e,
            0x3fe8_6c24_4890_64eb,
        ),
        (
            0x69d4_9d1e_2d26_c1d3,
            0x3fe8_7254_b27b_ef94,
            0x3fe4_a608_ca2e_8969,
        ),
        (
            0x4ada_748c_7943_c83f,
            0x3feb_85ec_4be0_1907,
            0xbfe0_52f9_9e09_29a7,
        ),
        (
            0x6243_5d8c_2861_d708,
            0xbfef_fdb4_2c8c_0354,
            0x3f98_3e51_0871_ec68,
        ),
        (
            0x4201_b051_edb4_83aa,
            0xbfec_50be_2b1c_7a36,
            0xbfdd_d0b7_7b7b_e86d,
        ),
        (
            0x7ec4_02c4_c37e_ea17,
            0x3fd8_fa83_0cb0_b7d9,
            0xbfed_764e_fd82_f48f,
        ),
        (
            0x6bc8_d314_1848_e8c4,
            0x3fe4_226b_4060_8a34,
            0x3fe8_df2b_06de_515e,
        ),
        (
            0x7f89_f774_e6ac_5bf5,
            0x3f94_44bc_4316_befb,
            0xbfef_fe65_25d3_7819,
        ),
        (
            0x7551_7b63_26f4_0bda,
            0xbfe7_f7b6_bd07_4564,
            0xbfe5_33e1_3f3d_8ecd,
        ),
        (
            0x79a1_1a54_0a4a_dfd8,
            0xbfc0_8e85_5ed0_f323,
            0xbfef_bb2d_fe05_7c35,
        ),
        (
            0x6f90_caa7_f6f4_a745,
            0xbfd8_d12c_768b_564b,
            0xbfed_7f09_79f6_3c90,
        ),
        (
            0x633f_25f5_cb98_7bce,
            0x3fcc_497b_d0e5_aca3,
            0x3fef_3574_f5be_6068,
        ),
        (
            0x70ac_5899_18ff_4db0,
            0x3fd4_2342_4796_5385,
            0x3fee_5fe7_8bae_4878,
        ),
        (
            0x71de_4ea9_13c5_03ac,
            0x3fee_427f_1d80_da1c,
            0xbfd4_d169_34aa_8c2d,
        ),
        (
            0x4c75_e54d_c994_5775,
            0x3fbc_969b_1531_b093,
            0xbfef_ccc2_5109_d9d7,
        ),
        (
            0x7003_3639_71a5_13e8,
            0xbfe0_4a9d_729b_25fc,
            0x3feb_8adf_ed70_22eb,
        ),
        (
            0x6673_891b_fdd8_049a,
            0x3feb_5251_d229_03cb,
            0x3fe0_a8c7_f0f1_5825,
        ),
        (
            0x74af_f96a_db43_4647,
            0xbfef_a7e3_b6f4_9e62,
            0x3fc2_b90f_4576_ec33,
        ),
        (
            0x56fe_bb4b_5c03_c075,
            0x3fcf_14ba_9516_22ae,
            0xbfef_0ad3_0c43_aa15,
        ),
        (
            0x637f_a1f5_1560_607d,
            0xbfef_a08e_ed29_5539,
            0xbfc3_7b5b_2d6a_10f2,
        ),
        (
            0x5fa1_ad8d_7098_2915,
            0x3fec_4b12_3f66_1575,
            0xbfdd_e639_168b_a805,
        ),
        (
            0x5b8d_b507_6eb2_2b6f,
            0x3fca_3e99_45b7_d774,
            0x3fef_51f5_1882_b2c1,
        ),
        (
            0x56cb_076c_dd13_0410,
            0xbfda_e549_64a8_f7e5,
            0xbfed_0982_8ecb_18a3,
        ),
        (
            0x6d53_4034_5f97_db75,
            0xbfd9_3c11_59c3_498e,
            0xbfed_6854_5a35_3e94,
        ),
        (
            0x42f2_2a09_ab35_572d,
            0xbfed_dcd1_4317_3dec,
            0x3fd6_ff7d_13f7_acde,
        ),
        (
            0x7eb2_a3c5_11c6_2d7a,
            0x3fe9_fd34_3757_84ce,
            0x3fe2_ab83_3b73_06f8,
        ),
        (
            0x4b3a_f0e8_2cbd_0ee9,
            0x3fe4_fc7a_1dd0_2fd0,
            0x3fe8_2848_6a99_367f,
        ),
        (
            0x77ad_8a7d_51df_58a2,
            0xbfea_c3a6_20ac_d30c,
            0xbfe1_8a68_a4ad_2279,
        ),
        (
            0x7486_167a_9637_79e7,
            0xbfdd_e16f_5052_9160,
            0x3fec_4c55_f4f7_6179,
        ),
        (
            0x4f76_d5f8_255d_3e7f,
            0xbfdc_581b_9bc0_bdb8,
            0xbfec_b0cc_d45b_b9fb,
        ),
        (
            0x67dd_128b_a8d3_0b95,
            0xbfe9_6ccf_a38e_a97b,
            0xbfe3_6e5b_c4c8_2fc4,
        ),
        (
            0x4a7d_31f8_41f7_b3ac,
            0x3f9c_f6ec_fcc1_949f,
            0xbfef_fcb8_e302_a861,
        ),
        (
            0x62e5_b7ec_958a_56db,
            0x3fc1_01fc_3205_1a42,
            0xbfef_b75c_ae41_ee8d,
        ),
        (
            0x6b94_b1b6_c4e1_aba6,
            0xbfe1_bcda_7636_e521,
            0xbfea_a252_6dad_c9d5,
        ),
        (
            0x6b29_cce0_1317_98d7,
            0xbfcd_6bde_e75e_9c0b,
            0x3fef_24a8_c98b_c091,
        ),
        (
            0x6d91_16d5_0e3e_974b,
            0x3fbf_a62e_b41c_145b,
            0xbfef_c127_96d4_e788,
        ),
        (
            0x7282_9cd4_e9f6_7dd0,
            0x3fed_b740_3dd2_a792,
            0xbfd7_bf03_1a37_1fb8,
        ),
        (
            0x7da4_afbd_2931_eaac,
            0xbfee_5fe9_87bb_9e75,
            0xbfd4_2336_4e4d_5e58,
        ),
        (
            0x7f66_4013_ca78_bb51,
            0x3fef_6b51_4163_ae36,
            0xbfc8_46b4_3b7b_1da6,
        ),
        (
            0x6528_9768_a0e4_6ea5,
            0x3fe6_7a99_0465_cfcc,
            0x3fe6_c664_4fd1_dc86,
        ),
        (
            0x799e_e6ed_651a_bfbc,
            0x3fee_b1af_b11e_22de,
            0x3fd2_18c4_14c4_d0ee,
        ),
        (
            0x56ec_3d05_c437_ccd4,
            0x3fa6_8f5e_3a03_0efa,
            0x3fef_f80b_30d8_c12e,
        ),
        (
            0x4ba7_b571_d948_94bc,
            0xbfe9_b60b_6e78_f9f1,
            0x3fe3_0d0b_1b32_c9fe,
        ),
        (
            0x4726_a2eb_432c_724c,
            0x3fef_fe61_856c_0305,
            0xbf94_5b95_4e56_03b8,
        ),
        (
            0x67cc_268f_a76b_da35,
            0xbfe0_8e56_7d15_f52f,
            0xbfeb_625f_f9bd_ffb3,
        ),
        (
            0x6ef0_aec2_c570_3ad1,
            0xbfe0_1f17_4e3b_1bd5,
            0xbfeb_a470_4985_2f67,
        ),
        (
            0x48dd_e9db_d1ee_e344,
            0x3fef_37bb_d815_8c5f,
            0x3fcc_212c_2ead_5efd,
        ),
        (
            0x5791_97e0_2a34_9300,
            0xbfef_8ce5_ed50_6024,
            0xbfc5_61b0_ad32_1522,
        ),
        (
            0x6f30_9649_99d4_14d1,
            0x3fe9_92bf_df1c_c8b1,
            0xbfe3_3c52_1dbb_53a5,
        ),
        (
            0x470a_2611_1c2e_1281,
            0x3feb_5da0_56ed_7018,
            0x3fe0_962e_a882_d603,
        ),
        (
            0x6bc3_bd3b_29a2_7a86,
            0x3fe2_1c92_3258_725c,
            0x3fea_6198_d5d7_723c,
        ),
        (
            0x54ce_8f9c_6bdc_9333,
            0x3fe9_ecf5_03c3_c32b,
            0x3fe2_c20c_470e_8028,
        ),
        (
            0x5fe0_610c_fd1f_31ea,
            0xbfd0_06b0_5853_d491,
            0xbfee_fb01_6ff0_e472,
        ),
        (
            0x54e9_ba1c_0404_741d,
            0xbfef_f65b_0ae7_843c,
            0xbfa8_d644_e712_2ce3,
        ),
        (
            0x5d4e_e253_549a_ada6,
            0xbfc9_5b53_b27b_242f,
            0x3fef_5da6_6eb5_079f,
        ),
        (
            0x59ea_5e09_f360_f693,
            0x3fef_ff64_7075_2dce,
            0xbf88_f1bf_d5cc_6cfb,
        ),
        (
            0x7cbd_f1ed_05e1_1e0d,
            0x3fe5_ebcd_565e_e6b3,
            0xbfe7_4ff5_a51e_d364,
        ),
        (
            0x5b54_5913_60ba_6fdf,
            0xbfe4_90aa_9407_2d3a,
            0xbfe8_8451_198e_167c,
        ),
        (
            0x5df0_96b4_5be4_81ef,
            0x3feb_13e7_f359_70d9,
            0xbfe1_0d7e_98f6_4a92,
        ),
        (
            0x4506_6076_dbb7_8f99,
            0xbfeb_dbe9_f937_66fd,
            0xbfdf_7cef_4c1c_76ca,
        ),
        (
            0x58f2_cead_fe3a_8b74,
            0x3fd5_394a_2507_a7ec,
            0x3fee_306f_512a_0b50,
        ),
        (
            0x738c_866e_245c_5ba2,
            0xbfe0_b804_82ae_fd12,
            0xbfeb_4901_9dab_e2c7,
        ),
        (
            0x50f7_a953_6e6e_3f9d,
            0x3fef_6c1d_456a_3809,
            0xbfc8_362d_ac84_60cb,
        ),
        (
            0x5fc2_315e_580f_9117,
            0xbfd1_2d69_9642_6989,
            0x3fee_d36c_4150_0a37,
        ),
        (
            0x43a2_f48d_1eed_85aa,
            0x3fef_e089_fcba_496e,
            0x3fb6_6a1b_c8f1_8895,
        ),
        (
            0x7cda_89a2_1d9f_9fe7,
            0xbfd7_af17_ff0e_6cb6,
            0xbfed_ba6d_1e35_db7e,
        ),
        (
            0x4964_d848_3a1c_19bb,
            0xbfe1_c4e9_2844_51a7,
            0xbfea_9cf2_f525_3d3c,
        ),
        (
            0x6e4e_0a4e_d0c3_983f,
            0xbfeb_b5db_ede4_8baf,
            0x3fe0_0113_4f73_057d,
        ),
        (
            0x7864_9ff9_9cac_3f75,
            0xbfef_1afa_8a95_7a4e,
            0x3fce_0dfb_4c72_1aa5,
        ),
        (
            0x4bf7_cd07_2742_6d34,
            0xbfed_70d9_23d3_134b,
            0x3fd9_1436_7b6f_42ce,
        ),
        (
            0x4651_b6a4_9a79_3697,
            0xbf7c_2e4b_9b37_18e9,
            0x3fef_ffce_5d4a_edda,
        ),
        (
            0x5c92_8d19_d07e_fd0c,
            0xbfd4_404a_af74_e9ae,
            0x3fee_5b13_cfc7_b046,
        ),
        (
            0x6d71_3a59_473e_1397,
            0xbfe6_a678_96e0_989a,
            0x3fe6_9ac2_b23c_92a2,
        ),
        (
            0x644a_557d_e252_52f5,
            0xbfc0_3b48_c0e0_ff3d,
            0x3fef_bddd_f508_d96d,
        ),
        (
            0x5cf2_d876_a6b1_6896,
            0x3fd2_a9ae_07e7_1bc4,
            0xbfee_9bf6_64c6_70bd,
        ),
        (
            0x5f27_a4fd_8d0c_246e,
            0xbfe4_d9db_46ef_0634,
            0x3fe8_4630_84a4_7cd6,
        ),
        (
            0x7f7a_0ce6_582d_a791,
            0x3fee_9dbf_4c5a_1341,
            0xbfd2_9df4_7ec8_2056,
        ),
        (
            0x4628_148e_a4ed_7248,
            0x3fd3_94d2_10e4_f547,
            0x3fee_7726_e7a3_41cb,
        ),
        (
            0x59fa_87c2_63b0_3931,
            0xbfed_8e0a_02d8_2541,
            0xbfd8_895f_dc02_f562,
        ),
        (
            0x5e69_6e9d_01cf_ef04,
            0xbfef_b748_2423_cbeb,
            0xbfc1_0460_dc85_dae5,
        ),
        (
            0x4338_97cf_97df_ceae,
            0x3fe7_b85e_82e5_49c0,
            0x3fe5_7aa7_708b_efd8,
        ),
        (
            0x6444_03f4_45c4_24c4,
            0x3fe7_12f3_58f5_fa56,
            0xbfe6_2bfc_0188_09eb,
        ),
        (
            0x534c_7bae_e11b_08a0,
            0xbfc2_3c8d_8c9d_1073,
            0x3fef_ac6e_523e_26fe,
        ),
        (
            0x5118_a50e_e166_56dd,
            0xbfec_0795_2358_6f79,
            0xbfde_e066_01ca_f6d1,
        ),
        (
            0x7a6f_4512_e400_fdf8,
            0xbfe7_ce1d_82f1_1d21,
            0x3fe5_628b_0d72_116c,
        ),
        (
            0x5ff3_af9a_085e_0dd3,
            0xbfe7_643f_6ef4_455a,
            0xbfe5_d625_d654_04c1,
        ),
        (
            0x7ed8_2da0_40ed_6928,
            0xbfe3_af1a_dbe6_6dad,
            0xbfe9_3ad0_878a_84fd,
        ),
        (
            0x464f_4660_980a_bd46,
            0x3fb7_8b90_4d8c_44fc,
            0x3fef_dd47_2946_b98b,
        ),
        (
            0x52f0_b856_ac16_fce7,
            0xbfd7_c5bb_c874_62d2,
            0x3fed_b5e8_44d2_5396,
        ),
        (
            0x4a41_165f_bb9b_1c50,
            0xbfcf_777e_be6f_b14d,
            0x3fef_049a_5ff2_8138,
        ),
        (
            0x42c6_31ce_b894_772d,
            0x3fe1_0797_a70f_38d4,
            0x3feb_179e_990f_63d7,
        ),
        (
            0x5f31_98d0_c4b7_b5e7,
            0x3fef_4799_a646_02da,
            0x3fcb_0169_30d0_3031,
        ),
        (
            0x53fe_de29_92a7_f038,
            0xbfea_58a2_5b22_d9d8,
            0x3fe2_2999_6a6c_966e,
        ),
        (
            0x7ca7_9974_fbf7_97b1,
            0xbfe0_b85b_0880_b0b1,
            0xbfeb_48cc_98c6_7d14,
        ),
        (
            0x5f3e_bd36_411c_3253,
            0xbfc6_bc2b_496e_0887,
            0xbfef_7dbe_756b_6059,
        ),
        (
            0x645f_336c_2035_a5f5,
            0xbfef_228d_4579_ab37,
            0x3fcd_8f79_9801_8038,
        ),
        (
            0x7945_7384_c8bd_617a,
            0x3fd1_32e8_a7c6_33f4,
            0xbfee_d2a8_1ac5_d177,
        ),
        (
            0x7a8b_68a4_0448_3108,
            0x3feb_2233_921e_6691,
            0xbfe0_f6b6_5426_694f,
        ),
        (
            0x51f6_a99a_413d_03cd,
            0xbfe3_372a_8a7c_0c36,
            0xbfe9_969f_9d2f_934c,
        ),
        (
            0x611a_8280_3616_6019,
            0xbfeb_9a5a_0dec_3d81,
            0x3fe0_3056_8053_1afd,
        ),
        (
            0x5323_1599_29e2_964f,
            0x3fc0_922d_a9cf_7ea9,
            0x3fef_bb0f_71f4_0a98,
        ),
        (
            0x7b2b_c4c7_1253_8b34,
            0xbfe1_8db1_cae7_1e17,
            0x3fea_c17e_91e1_5fce,
        ),
        (
            0x6b52_559e_e1bb_7331,
            0xbfea_c836_7791_90c9,
            0x3fe1_836f_f6d6_0ada,
        ),
        (
            0x4a94_1137_b0e7_e57b,
            0x3fed_25a4_dbe2_541f,
            0x3fda_6a77_0830_86f9,
        ),
        (
            0x4f03_3a04_5b58_97ca,
            0xbfe0_3908_59b8_06a6,
            0xbfeb_953e_c6c7_0db7,
        ),
        (
            0x5806_eda4_d022_fc49,
            0x3fbf_361a_6b3f_4cc0,
            0x3fef_c2e3_47e2_0eed,
        ),
        (
            0x7e69_0403_9760_6daa,
            0x3feb_237c_2788_2417,
            0x3fe0_f4a8_9572_db3b,
        ),
        (
            0x5de9_9fd9_1838_f826,
            0x3fed_33f2_e541_e2bc,
            0x3fda_2af8_98a5_45fb,
        ),
        (
            0x6e18_06da_34f2_cebf,
            0x3fea_2e83_3b42_dd06,
            0xbfe2_661d_94fd_e346,
        ),
        (
            0x6fcf_7a4e_e12a_7e47,
            0x3fb4_f506_2fa5_918d,
            0xbfef_e480_f810_9702,
        ),
        (
            0x5589_e439_1a4a_3965,
            0xbfea_a8bb_7d9d_6741,
            0x3fe1_b336_8b9d_ee1d,
        ),
        (
            0x639c_ef30_b8b5_e549,
            0x3fec_cae8_a690_4c1e,
            0x3fdb_ed6c_aa7e_ecb8,
        ),
        (
            0x5974_12a1_57c0_0efd,
            0xbfd7_5183_f947_437f,
            0xbfed_cce5_e519_7c2f,
        ),
        (
            0x5083_bdb0_0e1a_4744,
            0xbfef_c463_cbeb_0bcc,
            0xbfbe_d3aa_41a0_3ea6,
        ),
        (
            0x6c76_daff_4718_2e8f,
            0x3fef_fe83_b60e_bf27,
            0xbf93_8007_5284_dc28,
        ),
        (
            0x5e48_946a_ef93_741b,
            0x3fea_8e9e_fc6f_0002,
            0xbfe1_da4c_117a_bb50,
        ),
        (
            0x5639_e32d_4882_6f62,
            0xbfea_ce7e_122a_d9fb,
            0x3fe1_79d1_cc9b_3f2d,
        ),
        (
            0x663b_2548_9533_315f,
            0xbfc4_40eb_2e9d_1d5e,
            0xbfef_98cc_47ef_c5e9,
        ),
        (
            0x5549_161d_e61c_6089,
            0xbfee_d5d1_8c99_83b4,
            0x3fd1_1c2d_e692_5cb2,
        ),
        (
            0x699f_e3df_71da_d315,
            0xbfef_fd9a_3c27_ec04,
            0xbf98_c5c0_80f9_57e0,
        ),
        (
            0x75fb_8f61_1452_4220,
            0x3fc9_aa51_f69f_95bc,
            0xbfef_59a2_341c_a01c,
        ),
        (
            0x79b5_37d7_b31b_a5b1,
            0xbfe4_8c14_7594_bd57,
            0xbfe8_8829_3cd1_60c9,
        ),
        (
            0x4d26_2a85_dd93_a246,
            0xbfe5_1418_cc25_86f8,
            0x3fe8_13af_262a_e808,
        ),
        (
            0x4328_f440_b82a_5cc4,
            0x3fca_9b99_6a33_2ac4,
            0xbfef_4d0d_36ba_2b47,
        ),
        (
            0x6197_4984_045f_2c28,
            0x3fe7_c4ac_7ff9_63b7,
            0x3fe5_6d08_f287_05d1,
        ),
        (
            0x7c09_e5f5_b2de_5b46,
            0xbfca_8ff9_505b_cb81,
            0xbfef_4dab_309a_aaf9,
        ),
        (
            0x4754_1427_3e2d_754a,
            0xbfcb_4989_8871_da87,
            0x3fef_43af_e040_bedd,
        ),
        (
            0x6cd5_75bc_ff4a_f95c,
            0xbfbd_79d7_33d7_5b20,
            0xbfef_c984_526d_ed3b,
        ),
        (
            0x4aad_ef1d_4c96_65fe,
            0x3feb_fc1a_fd36_ce45,
            0x3fdf_09ee_8088_9418,
        ),
        (
            0x701f_8f42_3237_4047,
            0x3f7a_7c87_2289_8ce9,
            0xbfef_ffd4_2761_92c2,
        ),
        (
            0x7723_4118_3025_53e6,
            0x3f9d_9488_3e2c_888e,
            0xbfef_fc94_d434_87e2,
        ),
        (
            0x6796_bcee_71a7_59cb,
            0xbfea_27e7_dac4_0b59,
            0x3fe2_6f80_d631_6f67,
        ),
        (
            0x74d1_5ecb_990d_0208,
            0x3fd5_2ffd_15d4_9ca7,
            0xbfee_3211_66b9_3d40,
        ),
        (
            0x606f_c5e0_7427_6d4a,
            0xbfc0_89ca_3dd1_3e55,
            0x3fef_bb55_7763_463a,
        ),
        (
            0x68c2_aec3_68b9_edec,
            0xbfe9_ffdd_601e_77cb,
            0xbfe2_a7ce_7bce_9acb,
        ),
        (
            0x5e95_0c3f_e66b_2053,
            0xbfef_d811_5160_69d1,
            0x3fb9_3ef9_b8f9_9369,
        ),
        (
            0x6e34_2155_899d_ae5b,
            0xbfbc_e6ab_7e11_5fd2,
            0x3fef_cba0_ceff_f6f6,
        ),
        (
            0x7437_b0a5_69da_4024,
            0xbfef_ff65_908c_0ac3,
            0x3f88_da9c_4dec_5432,
        ),
        (
            0x6a79_5980_92e5_aa73,
            0xbfc8_a164_3a6e_c63b,
            0xbfef_66e7_a4f8_1cf6,
        ),
        (
            0x5ea2_e1be_ff16_1799,
            0xbfee_b210_7719_4d29,
            0xbfd2_1633_5477_174a,
        ),
        (
            0x6740_6c52_4779_6fb6,
            0x3fdf_ef2b_d12a_6497,
            0x3feb_bb54_deed_82d1,
        ),
        (
            0x7f3e_ab81_8fb5_5096,
            0xbfe8_a8d0_40fe_8dc7,
            0xbfe4_64d8_fd4b_299d,
        ),
        (
            0x6105_a0cf_baf7_ca88,
            0x3fef_fcd3_b925_238c,
            0xbf9c_7f69_eaa2_a3d9,
        ),
        (
            0x7c2b_fe66_f04a_453a,
            0x3fd7_e66f_f63a_ee9c,
            0x3fed_af58_54cb_701d,
        ),
        (
            0x5bbb_b922_e657_3933,
            0x3fd9_3c1b_3b4f_268d,
            0x3fed_6852_3b8d_f033,
        ),
        (
            0x5675_c615_3e45_f0cb,
            0x3fee_0344_761d_ae99,
            0xbfd6_33c1_179c_0550,
        ),
        (
            0x63a3_c316_ee0c_9118,
            0x3fb1_ad0f_3162_6a24,
            0xbfef_ec72_f6ea_4390,
        ),
        (
            0x7fc5_7853_0b2f_6271,
            0xbfe7_a5f1_ab82_727e,
            0x3fe5_8eee_f2a5_4602,
        ),
        (
            0x5f87_07cf_14ef_6c7c,
            0x3fec_94e4_37fc_f160,
            0x3fdc_c807_70e1_8089,
        ),
        (
            0x68f6_9323_06fc_d8e3,
            0x3fe6_caf3_e37e_e1cd,
            0x3fe6_75f9_1fab_51ce,
        ),
        (
            0x62d9_cfdf_160f_b18b,
            0x3fe5_bd2d_c08d_3335,
            0xbfe7_7b75_ba54_5350,
        ),
        (
            0x624a_5318_bdcd_0582,
            0x3fec_05b0_8295_5fe0,
            0x3fde_e744_c8c1_4a58,
        ),
        (
            0x55ba_8812_1c13_515c,
            0x3fef_07a0_470b_e803,
            0x3fcf_47aa_3b19_7270,
        ),
        (
            0x7d89_2329_f233_3ddd,
            0x3fe0_dfbd_9501_8a04,
            0x3feb_3082_abf9_7b76,
        ),
        (
            0x6b27_6161_7d6e_219f,
            0x3fd5_51f7_8ec1_6b99,
            0xbfee_2c16_27fd_bc77,
        ),
        (
            0x5ec4_fd83_3fe2_67ee,
            0x3fef_b754_cfa2_5521,
            0xbfc1_02e6_f9dd_e70a,
        ),
        (
            0x7848_5b9d_c96e_7422,
            0x3fe4_b8e0_aea7_d24a,
            0xbfe8_625d_c57f_c4a8,
        ),
        (
            0x56b5_13bf_46f9_1495,
            0xbfe3_d24b_81b1_4aa2,
            0xbfe9_1f34_6bc1_8f3a,
        ),
        (
            0x7084_c872_cbaf_3e61,
            0xbfce_d7ab_77b7_dec2,
            0x3fef_0ea1_3949_d7f5,
        ),
        (
            0x45c4_99f9_0929_5022,
            0xbfee_7a0e_01aa_79da,
            0xbfd3_82b8_3f35_9fb9,
        ),
        (
            0x51a6_7aff_d212_5d24,
            0xbfe7_5453_2432_8cb4,
            0xbfe5_e727_fdc2_c49d,
        ),
        (
            0x53d2_3c80_12fc_7325,
            0xbfe5_ef7b_6bac_0a8a,
            0x3fe7_4c7f_3c6f_e3d6,
        ),
        (
            0x423a_a7bc_c115_80d7,
            0x3fee_067a_56ab_f0a9,
            0xbfd6_225d_514c_27dc,
        ),
        (
            0x7cd0_59b8_ba67_f58b,
            0xbfe1_4d51_ff21_d031,
            0x3fea_eb4c_3e22_64c6,
        ),
        (
            0x5eb8_431c_07fa_f9e5,
            0x3fe0_2f33_618a_4261,
            0xbfeb_9b04_c301_b507,
        ),
        (
            0x57e4_9cba_651c_e72b,
            0x3feb_d7c1_e88a_37e7,
            0xbfdf_8ba0_7ff7_5906,
        ),
        (
            0x72f5_9076_3bc1_4622,
            0x3fe7_484e_fb6c_e511,
            0x3fe5_f3ed_779a_3c88,
        ),
        (
            0x7c5c_f8d8_cdc2_baab,
            0x3fec_3a96_eac4_a736,
            0x3fde_2449_3fdc_3bb6,
        ),
        (
            0x5dd3_60a6_409e_3388,
            0x3fef_dbb8_6239_91a4,
            0xbfb8_10fe_4826_036d,
        ),
        (
            0x5544_17fc_2c98_d035,
            0xbfd3_63c2_41db_239d,
            0xbfee_7efe_30a0_1a7c,
        ),
        (
            0x7be1_7244_1c33_5556,
            0x3fe4_bc3f_821b_2db2,
            0xbfe8_5f80_1ff0_66c5,
        ),
        (
            0x5163_2b00_8d8a_8134,
            0xbfef_d90a_366d_4d96,
            0x3fb8_f000_a557_63ed,
        ),
        (
            0x74de_e806_3934_4bac,
            0xbfbc_57ad_a7a4_3ea4,
            0xbfef_cda3_9ee0_4bf3,
        ),
        (
            0x58be_e975_cb9e_a389,
            0xbfd8_91f9_a2bd_c329,
            0x3fed_8c40_a97b_6338,
        ),
        (
            0x4ce1_de22_780c_73fb,
            0xbfe6_bfa5_1974_519a,
            0xbfe6_816c_ede5_7e4d,
        ),
        (
            0x46de_290f_556d_a7e8,
            0x3fef_66b2_ce71_5651,
            0x3fc8_a599_afdd_34bb,
        ),
        (
            0x64c2_76a2_6510_6a82,
            0xbfe1_9b43_c653_a113,
            0xbfea_b892_7e12_5fda,
        ),
        (
            0x6365_4037_6a92_fe97,
            0xbfd0_587a_1fe1_24ef,
            0x3fee_f050_d68d_43c2,
        ),
        (
            0x45db_3ff4_a46c_5f2e,
            0xbfe1_407e_d8a1_fbe7,
            0xbfea_f386_257f_353f,
        ),
        (
            0x418a_0493_29ec_3ab4,
            0x3fc3_82a4_161a_7d15,
            0xbfef_a047_1355_2d2a,
        ),
        (
            0x51d3_501c_71b2_3f02,
            0xbfde_701b_f0c5_5a04,
            0xbfec_2638_9b14_5778,
        ),
        (
            0x7bc7_79b9_427f_c621,
            0x3fd1_0252_e607_af9c,
            0xbfee_d964_d4f9_05c1,
        ),
        (
            0x7d3f_e633_9c25_7fce,
            0x3fee_0bb3_05de_a495,
            0xbfd6_05f3_b7b6_057f,
        ),
        (
            0x4b07_a019_1da4_0b87,
            0x3fd6_33c7_a8bc_096c,
            0xbfee_0343_3f35_7df7,
        ),
        (
            0x7f3d_47f1_715d_6452,
            0xbfe0_c2d7_4e9d_70a9,
            0xbfeb_425c_df64_896f,
        ),
        (
            0x6bbd_715f_63db_b1e4,
            0xbfc7_3fba_c8ad_e4cf,
            0x3fef_77bd_07b9_2bea,
        ),
        (
            0x5bfb_8c7d_0f66_db14,
            0xbfe7_a564_9ff9_fabf,
            0x3fe5_8f89_a681_af9e,
        ),
        (
            0x71c0_1a60_1d26_6a1e,
            0x3fe4_7c70_ca7a_d5c2,
            0x3fe8_953a_20fc_999d,
        ),
        (
            0x5cee_e061_95d0_d3fd,
            0x3fee_bec9_7a77_c781,
            0x3fd1_bef0_99a6_3f1f,
        ),
        (
            0x5f36_f08f_7126_71f5,
            0xbfd5_4b5c_bf30_0078,
            0x3fee_2d40_a444_7d27,
        ),
        (
            0x4fb5_d10d_74be_934c,
            0xbfe3_c3e4_4851_63b6,
            0x3fe9_2a8a_fef0_f2b0,
        ),
        (
            0x7e05_220a_e34d_1232,
            0x3fe4_c8b5_9a3a_3435,
            0x3fe8_54e0_a7b4_c4d3,
        ),
        (
            0x5579_de18_0fe8_104b,
            0xbfe6_1c42_8151_923f,
            0xbfe7_2205_2b52_614b,
        ),
        (
            0x7baa_4b74_5338_a27d,
            0x3fe8_4c01_2ad3_def9,
            0xbfe4_d314_51c2_df7a,
        ),
        (
            0x6065_d8a8_0e66_4b7e,
            0x3fd2_4298_7195_310f,
            0x3fee_ab7d_9239_8165,
        ),
        (
            0x6d65_ef30_e51a_3173,
            0x3fe3_faad_9db0_305a,
            0x3fe8_ff22_9b17_d102,
        ),
        (
            0x5098_2238_661c_c5fe,
            0x3fd3_3635_47ff_7289,
            0xbfee_8632_5fad_2596,
        ),
        (
            0x6294_5e47_6fe1_0b0e,
            0x3fee_36e2_b10e_169b,
            0xbfd5_1472_6f40_20c5,
        ),
        (
            0x5370_22a1_8cde_feac,
            0x3fda_ed0a_1f88_d3a3,
            0xbfed_07b6_a791_126f,
        ),
        (
            0x5aa6_0f43_3094_030a,
            0x3fef_fbf5_6687_00d1,
            0x3fa0_14a2_f927_de75,
        ),
        (
            0x4472_97f6_1f55_504a,
            0x3fe2_b157_51d4_18ce,
            0x3fe9_f903_57b1_2c66,
        ),
        (
            0x7624_b549_ff96_1b78,
            0x3fed_ccd8_e5a4_3146,
            0xbfd7_51c6_6a7f_f1c3,
        ),
        (
            0x4ab7_54f5_f019_cca0,
            0x3fda_6271_27aa_cb03,
            0xbfed_2775_e1c2_ebf7,
        ),
        (
            0x72a4_7f7b_1c1e_ad48,
            0xbfe6_cfc7_2aee_320c,
            0x3fe6_7112_890d_a799,
        ),
        (
            0x7205_94b3_150f_a76b,
            0xbfe2_01c1_2fe0_8a13,
            0xbfea_73ed_e55e_203b,
        ),
        (
            0x643f_9488_4567_26ba,
            0xbfdc_6705_d1b1_36f8,
            0xbfec_ad1c_98e7_ee31,
        ),
        (
            0x438c_8049_06d4_35a3,
            0x3fef_2ff5_1ab6_a342,
            0x3fcc_a9e3_5acc_b557,
        ),
        (
            0x575d_a5b1_4807_7dd1,
            0x3fe4_cc68_3a95_fbff,
            0xbfe8_51b7_8f6c_aee5,
        ),
    ];

    /// The same for the double nearest a multiple of π/2 of all,
    /// 6381956970095103 2^797, which is 4.6e-19 short of one; π/2; the
    /// greatest double; 1e22; and the two doubles about π/4.
    const EDGES: [(u64, u64, u64); 6] = [
        (
            0x7506_ac5b_262c_a1ff,
            0x3ff0_0000_0000_0000,
            0xbc21_4ae7_2e6b_a22f,
        ),
        (
            0x3ff9_21fb_5444_2d18,
            0x3ff0_0000_0000_0000,
            0x3c91_a626_3314_5c07,
        ),
        (
            0x7fef_ffff_ffff_ffff,
            0x3f74_52fc_98b3_4e97,
            0xbfef_ffe6_2ecf_ab75,
        ),
        (
            0x4480_f0cf_064d_d592,
            0xbfeb_453a_b76b_f397,
            0x3fe0_be2c_ef01_c8f4,
        ),
        (
            0x3fe9_21fb_5444_2d18,
            0x3fe6_a09e_667f_3bcc,
            0x3fe6_a09e_667f_3bcd,
        ),
        (
            0x3fe9_21fb_5444_2d19,
            0x3fe6_a09e_667f_3bcd,
            0x3fe6_a09e_667f_3bcc,
        ),
    ];

    #[test]
    fn sines_and_cosines_are_within_a_double_of_any_size() {
        let near = drawn(100_000, 0xbb67_ae85_84ca_a73b, |s| {
            between(s, -100_000.0, 100_000.0)
        });
        close("sin", &near, 60, sin, f64::sin);
        close("cos", &near, 60, cos, f64::cos);
        // Past them, and at the edges, against values worked out to more
        // digits, as the host's library misses there now and then.
        let mut off = 0;
        let mut count = 0;
        for (x, sine, cosine) in FAR.iter().chain(&EDGES) {
            let x = f64::from_bits(*x);
            for (what, ours, exact) in [
                ("sin", sin(x), f64::from_bits(*sine)),
                ("cos", cos(x), f64::from_bits(*cosine)),
            ] {
                assert!(
                    apart(ours, exact) <= 1,
                    "{what} of {x:e}: {ours:e} and {exact:e}"
                );
                off += usize::from(ours.to_bits() != exact.to_bits());
                count += 1;
            }
            assert_eq!(sin(-x).to_bits(), (-sin(x)).to_bits(), "sin of -{x:e}");
            assert_eq!(cos(-x).to_bits(), cos(x).to_bits(), "cos of -{x:e}");
        }
        assert!(off * 1000 <= 40 * count, "{off} of {count} a double off");
        for x in [0.0, -0.0, 1e-300, 5e-324, 1e-8] {
            assert_eq!(sin(x).to_bits(), x.to_bits(), "sin of {x:e} is {x:e}");
            assert_eq!(cos(x), 1.0, "cos of {x:e}");
        }
        for x in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(sin(x).is_nan() && cos(x).is_nan(), "{x} has no sine");
        }
    }

    #[test]
    fn angles_are_within_a_double_and_c_s_at_the_edges() {
        let atan2 = |(y, x)| atan2(y, x);
        let theirs = |(y, x): (f64, f64)| y.atan2(x);
        let near = drawn(200_000, 0x3c6e_f372_fe94_f82b, |s| {
            (between(s, -10.0, 10.0), between(s, -10.0, 10.0))
        });
        close("atan2", &near, 50, atan2, theirs);
        let far = drawn(100_000, 0xa54f_f53a_5f1d_36f1, |s| {
            let (y, x) = (any_positive(s), any_positive(s));
            match next(s) % 4 {
                0 => (y, x),
                1 => (-y, x),
                2 => (y, -x),
                _ => (-y, -x),
            }
        });
        close("atan2", &far, 5, atan2, theirs);
        let sides = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            5e-324,
        ];
        let edges: Vec<_> = sides.iter().flat_map(|&y| sides.map(|x| (y, x))).collect();
        for &(y, x) in &edges {
            assert_eq!(
                atan2((y, x)).to_bits(),
                theirs((y, x)).to_bits(),
                "atan2 of {y} {x}"
            );
        }
    }

    #[test]
    fn powers_are_within_a_double_and_whole_ones_exact() {
        let pow = |(x, y)| pow(x, y);
        let theirs = |(x, y): (f64, f64)| x.powf(y);
        let near = drawn(100_000, 0x2f2f_6a95_c0de_0001, |s| {
            (between(s, 0.0, 10.0), between(s, -50.0, 50.0))
        });
        close("pow", &near, 25, pow, theirs);
        let wide = drawn(100_000, 0x2f2f_6a95_c0de_0002, |s| {
            (any_positive(s), between(s, -2.0, 2.0))
        });
        close("pow", &wide, 25, pow, theirs);
        #[allow(clippy::cast_precision_loss)]
        let whole = drawn(100_000, 0x2f2f_6a95_c0de_0003, |s| {
            let base = between(s, -3.0, 3.0);
            (base, (next(s) % 81) as f64 - 40.0)
        });
        close("pow", &whole, 25, pow, theirs);
        // Powers out to the ends of the doubles, where an error in ln x
        // would be multiplied by y.
        let great = drawn(100_000, 0x2f2f_6a95_c0de_0004, |s| {
            (between(s, 0.0, 10.0), between(s, -2000.0, 2000.0))
        });
        close("pow", &great, 25, pow, theirs);
        for (x, y, power) in [
            (2.0, 10.0, 1024.0),
            (10.0, 15.0, 1e15),
            (10.0, 22.0, 1e22),
            (-2.0, 3.0, -8.0),
            (3.0, -2.0, 1.0 / 9.0),
            (0.5, 3.0, 0.125),
            (7.0, 1.0, 7.0),
            (2.0, -1074.0, 5e-324),
        ] {
            assert_eq!(pow((x, y)).to_bits(), f64::to_bits(power), "{x}^{y}");
        }
        let edges = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            2.0,
            -2.0,
            3.0,
            -3.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        for &x in &edges {
            for &y in &edges {
                assert_eq!(
                    apart(pow((x, y)), theirs((x, y))),
                    0,
                    "{x}^{y}: {} and {}",
                    pow((x, y)),
                    theirs((x, y))
                );
            }
        }
        assert!(
            pow((-8.0, 1.0 / 3.0)).is_nan(),
            "a root of what is below zero is no number"
        );
        assert_eq!(pow((10.0, 400.0)), f64::INFINITY);
        assert_eq!(pow((10.0, -400.0)), 0.0);
        assert_eq!(pow((1e300, 1e300)), f64::INFINITY);
        assert_eq!(
            pow((-1e300, 1e300)),
            f64::INFINITY,
            "a power that great is even"
        );
    }

    #[test]
    fn remainders_are_exact() {
        let signed = |s: &mut u64| {
            let x = any_positive(s);
            if next(s).is_multiple_of(2) { -x } else { x }
        };
        let any = drawn(100_000, 0x9e37_79b9_0000_0001, |s| (signed(s), signed(s)));
        let near = drawn(100_000, 0x9e37_79b9_0000_0002, |s| {
            (between(s, -1e3, 1e3), between(s, -10.0, 10.0))
        });
        for &(x, y) in any.iter().chain(&near) {
            assert_eq!(fmod(x, y).to_bits(), (x % y).to_bits(), "{x:e} % {y:e}");
        }
        for (x, y) in [
            (5.5, 2.0),
            (-5.5, 2.0),
            (5.5, -2.0),
            (-0.0, 3.0),
            (3.0, 3.0),
            (1.0, f64::INFINITY),
            (-1.0, f64::NEG_INFINITY),
            (5e-324 * 7.0, 5e-324 * 3.0),
            (f64::MAX, 5e-324),
            (f64::MAX, f64::MIN_POSITIVE),
            (1e-310, 3e-311),
        ] {
            assert_eq!(fmod(x, y).to_bits(), (x % y).to_bits(), "{x:e} % {y:e}");
        }
        for (x, y) in [
            (1.0, 0.0),
            (f64::INFINITY, 1.0),
            (f64::NAN, 1.0),
            (1.0, f64::NAN),
            (0.0, 0.0),
        ] {
            assert!(fmod(x, y).is_nan(), "{x} % {y}");
        }
    }

    #[test]
    fn whole_powers_a_double_holds_are_exact() {
        for base in 2u64..=40 {
            let mut n = 0;
            while let Some(exact) = base.checked_pow(n).filter(|&p| p < 1 << 53) {
                #[allow(clippy::cast_precision_loss)]
                let (b, e) = (base as f64, exact as f64);
                assert_eq!(pow(b, f64::from(n)).to_bits(), e.to_bits(), "{base}^{n}");
                assert_eq!(
                    pow(b, -f64::from(n)).to_bits(),
                    (1.0 / e).to_bits(),
                    "{base}^-{n}"
                );
                n += 1;
            }
        }
    }

    #[test]
    fn known_values_are_the_doubles_nearest_them() {
        use core::f64::consts::{E, FRAC_PI_3, FRAC_PI_6, LN_10, PI, SQRT_2};
        for (ours, nearest, what) in [
            (exp(1.0), E, "e"),
            (log(E), 1.0, "ln e"),
            (log(10.0), LN_10, "ln 10"),
            (sqrt(2.0), SQRT_2, "root of 2"),
            (atan2(0.0, -1.0), PI, "pi"),
            (atan2(1.0, 0.0), HALF_PI, "half pi"),
            (pow(2.0, 0.5), SQRT_2, "2 to a half"),
            (exp(0.0), 1.0, "e to 0"),
        ] {
            assert_eq!(ours.to_bits(), nearest.to_bits(), "{what}");
        }
        // The doubles nearest π/6 and π/3 are not those angles, so their
        // sine and cosine are only near a half.
        assert!(apart(sin(FRAC_PI_6), FRAC_PI_6.sin()) <= 1);
        assert!(apart(cos(FRAC_PI_3), FRAC_PI_3.cos()) <= 1);
        assert!((sin(FRAC_PI_6) - 0.5).abs() < 1e-15);
        assert!(apart(atan2(-1.0, -1.0), -3.0 * QUARTER_PI) <= 1);
        assert!(apart(atan2(1.0, -1.0), THREE_QUARTERS_PI) <= 1);
        for n in 0..=20u32 {
            let x = f64::from(n * n);
            assert_eq!(sqrt(x), f64::from(n), "the root of {x}");
        }
    }

    #[test]
    fn whole_numbers_and_odd_ones_are_told_apart() {
        for x in [0.0, -0.0, 1.0, -3.0, 4_503_599_627_370_496.0, 1e300, -1e300] {
            assert!(whole_number(x), "{x}");
        }
        for x in [0.5, -1.5, 1e-300, 4_503_599_627_370_495.5] {
            assert!(!whole_number(x), "{x}");
        }
        for x in [1.0, -1.0, 3.0, 9_007_199_254_740_991.0] {
            assert!(odd(x), "{x}");
        }
        for x in [0.0, 2.0, -4.0, 1.5, 9_007_199_254_740_992.0, 1e300] {
            assert!(!odd(x), "{x}");
        }
        assert_eq!(nearest(2.5), 3);
        assert_eq!(nearest(-2.5), -3);
        assert_eq!(nearest(2.4), 2);
        assert_eq!(nearest(-0.4), 0);
    }

    #[test]
    fn parts_and_sums_carry_what_a_double_misses() {
        assert_eq!(parts(1.0), (1.0, 0));
        assert_eq!(parts(3.0), (1.5, 1));
        assert_eq!(parts(0.25), (1.0, -2));
        assert_eq!(parts(5e-324), (1.0, -1074));
        assert_eq!(parts(f64::MAX).1, 1023);
        let (sum, lost) = added(1.0, 1e-20);
        assert_eq!((sum, lost), (1.0, 1e-20));
        let (sum, lost) = added(1e-20, 1.0);
        assert_eq!((sum, lost), (1.0, 1e-20), "either way round");
        let (product, lost) = multiplied(1.0 + f64::EPSILON, 1.0 + f64::EPSILON);
        assert_eq!(product, 1.0 + 2.0 * f64::EPSILON);
        assert_eq!(lost, f64::EPSILON * f64::EPSILON);
        assert_eq!(scaled(3.0, 0), 3.0);
        assert_eq!(scaled(1.0, 1023), f64::from_bits(0x7fe0_0000_0000_0000));
        assert_eq!(scaled(1.0, 1024), f64::INFINITY);
        assert_eq!(scaled(1.0, -1074), 5e-324);
        assert_eq!(scaled(1.0, -1075), 0.0);
    }
}
