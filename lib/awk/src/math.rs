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

    #[test]
    fn sines_and_cosines_are_within_a_double_of_any_size() {
        let near = drawn(100_000, 0xbb67_ae85_84ca_a73b, |s| {
            between(s, -100_000.0, 100_000.0)
        });
        close("sin", &near, 60, sin, f64::sin);
        close("cos", &near, 60, cos, f64::cos);
        let far = drawn(100_000, 0x1f83_d9ab_fb41_bd6b, |s| {
            let x = any_positive(s);
            if next(s).is_multiple_of(2) { -x } else { x }
        });
        close("sin", &far, 40, sin, f64::sin);
        close("cos", &far, 40, cos, f64::cos);
        // The double nearest a multiple of π/2 of all: 6381956970095103
        // 2^797, which is 4.6e-19 short of one.
        let nearest_turn = scaled(6_381_956_970_095_103.0, 797);
        let edges = [
            nearest_turn,
            HALF_PI,
            f64::MAX,
            1e22,
            FRAC_PI_4,
            0.785_398_163_397_448_4,
        ];
        close("sin", &edges, 1000, sin, f64::sin);
        close("cos", &edges, 1000, cos, f64::cos);
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
