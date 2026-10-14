//! Big-endian reads that return `None` instead of reading past the end.

pub(crate) fn u8_at(data: &[u8], at: usize) -> Option<u8> {
    data.get(at).copied()
}

pub(crate) fn i8_at(data: &[u8], at: usize) -> Option<i8> {
    u8_at(data, at).map(|b| i8::from_be_bytes([b]))
}

pub(crate) fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

pub(crate) fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    u16_at(data, at).map(|v| i16::from_be_bytes(v.to_be_bytes()))
}

pub(crate) fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// A 2.14 fixed point number, as composite glyphs store their scales.
pub(crate) fn f2dot14_at(data: &[u8], at: usize) -> Option<f32> {
    i16_at(data, at).map(|v| f32::from(v) / 16384.0)
}

/// The bytes from `start` for `len`, if they are all there.
pub(crate) fn slice(data: &[u8], start: usize, len: usize) -> Option<&[u8]> {
    data.get(start..start.checked_add(len)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: [u8; 6] = [0x12, 0x34, 0xfe, 0xdc, 0x80, 0x01];

    #[test]
    fn reads_big_endian() {
        assert_eq!(u8_at(&DATA, 0), Some(0x12));
        assert_eq!(i8_at(&DATA, 2), Some(-2));
        assert_eq!(u16_at(&DATA, 0), Some(0x1234));
        assert_eq!(i16_at(&DATA, 2), Some(-292));
        assert_eq!(u32_at(&DATA, 0), Some(0x1234_fedc));
        assert_eq!(u32_at(&DATA, 2), Some(0xfedc_8001));
    }

    #[test]
    fn stops_at_the_end() {
        assert_eq!(u8_at(&DATA, 6), None);
        assert_eq!(u16_at(&DATA, 5), None);
        assert_eq!(u16_at(&DATA, 4), Some(0x8001));
        assert_eq!(u32_at(&DATA, 3), None);
        assert_eq!(u16_at(&DATA, usize::MAX), None);
        assert_eq!(u32_at(&DATA, usize::MAX - 1), None);
        assert_eq!(slice(&DATA, 4, 2), Some(&DATA[4..6]));
        assert_eq!(slice(&DATA, 4, 3), None);
        assert_eq!(slice(&DATA, usize::MAX, 2), None);
    }

    #[test]
    fn fixed_point_scales() {
        let one = 0x4000u16.to_be_bytes();
        let half = 0x2000u16.to_be_bytes();
        let minus_two = 0x8000u16.to_be_bytes();
        assert_eq!(f2dot14_at(&one, 0), Some(1.0));
        assert_eq!(f2dot14_at(&half, 0), Some(0.5));
        assert_eq!(f2dot14_at(&minus_two, 0), Some(-2.0));
        assert_eq!(f2dot14_at(&[0x40], 0), None);
    }
}
