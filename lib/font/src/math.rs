//! The float roundings `core` does not offer without the standard library.
//! Inputs must lie within the range of `i32`, which callers make sure of by
//! clamping to the canvas or limiting the pixel size; NaN gives 0.

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn floor(x: f32) -> i32 {
    let t = x as i32;
    if (t as f32) > x { t - 1 } else { t }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn ceil(x: f32) -> i32 {
    let t = x as i32;
    if (t as f32) < x { t + 1 } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLES: [f32; 14] = [
        0.0, -0.0, 0.25, 0.5, 0.999, 1.0, 1.5, 7.0, -0.25, -0.5, -1.0, -1.5, -7.0, 123.75,
    ];

    #[test]
    fn floor_and_ceil_match_std() {
        for x in SAMPLES {
            #[allow(clippy::cast_possible_truncation)]
            let (f, c) = (x.floor() as i32, x.ceil() as i32);
            assert_eq!(floor(x), f, "floor({x})");
            assert_eq!(ceil(x), c, "ceil({x})");
        }
    }
}
