//! Numbers read in the base `ibase` names and written in the one `obase`
//! names, as GNU's bc reads and writes them.

use alloc::string::String;
use alloc::vec::Vec;

use crate::big::Big;
use crate::number::Number;

/// The value of a digit bc reads: 0 to 9, then A to F for 10 to 15.
#[must_use]
pub fn digit_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The number the digits of `text`, with at most one point among them,
/// make in base `base`, from 2 to 16. A digit alone keeps its value; among
/// others one past the base counts as the base's greatest. A fraction
/// gets as many decimal digits as it has digits.
#[must_use]
pub fn read(text: &[u8], base: u32) -> Number {
    let value = |c: u8| u32::from(digit_value(c).unwrap_or(0));
    if let [single] = text {
        return Number::from_i64(i64::from(value(*single)));
    }
    let (whole, fraction) = match text.iter().position(|&c| c == b'.') {
        Some(at) => (&text[..at], &text[at + 1..]),
        None => (text, &text[..0]),
    };
    let fold = |digits: &[u8]| {
        digits.iter().fold(Big::default(), |n, &c| {
            n.mul_small(base)
                .add(&Big::from_u64(u64::from(value(c).min(base - 1))))
        })
    };
    let mut number = Number::new(false, fold(whole), 0);
    if !fraction.is_empty() {
        let mut denominator = Big::from_u64(1);
        for _ in fraction {
            denominator = denominator.mul_small(base);
        }
        let part = Number::new(false, fold(fraction), 0)
            .div(&Number::new(false, denominator, 0), fraction.len())
            .unwrap_or_default();
        number = number.add(&part);
    }
    number
}

/// What `n` is written as in base `base`, 2 or more, on one line: digits
/// 0 to 9 and A to F up to base 16, and past it each digit in decimal,
/// as wide as the base's greatest, after a blank. A fraction gets the
/// fewest digits whose last is worth no more than the scale's.
#[must_use]
pub fn write(n: &Number, base: u32) -> String {
    if n.is_zero() {
        return String::from("0");
    }
    if base == 10 {
        return n.decimal();
    }
    let width = if base > 16 {
        decimal_digits(base - 1)
    } else {
        0
    };
    let mut text = String::new();
    if n.is_negative() {
        text.push('-');
    }
    let one = Big::pow10(n.scale());
    let (mut whole, mut fraction) = n.magnitude().divrem(&one);
    let mut digits = Vec::new();
    while !whole.is_zero() {
        let (rest, digit) = whole.divrem_small(base);
        digits.push(digit);
        whole = rest;
    }
    for &digit in digits.iter().rev() {
        if width > 0 {
            text.push(' ');
        }
        push_digit(&mut text, digit, width);
    }
    if n.scale() > 0 {
        text.push('.');
        let mut reach = Big::from_u64(1);
        let mut first = true;
        while reach < one {
            reach = reach.mul_small(base);
            let (digit, rest) = fraction.mul_small(base).divrem(&one);
            fraction = rest;
            if width > 0 && !first {
                text.push(' ');
            }
            first = false;
            let digit = digit
                .to_u64()
                .and_then(|d| u32::try_from(d).ok())
                .unwrap_or(0);
            push_digit(&mut text, digit, width);
        }
    }
    text
}

/// How many decimal digits `n` has.
fn decimal_digits(n: u32) -> usize {
    Big::from_u64(u64::from(n)).digit_count().max(1)
}

/// Writes `digit` onto `text`: as a letter up to base 16, or else in
/// decimal, `width` wide.
fn push_digit(text: &mut String, digit: u32, width: usize) {
    if width == 0 {
        text.push(char::from(b"0123456789ABCDEF"[digit as usize % 16]));
        return;
    }
    let decimal = Big::from_u64(u64::from(digit)).digits();
    for _ in decimal.len()..width {
        text.push('0');
    }
    text.extend(decimal.iter().map(|&d| char::from(b'0' + d)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number a decimal text writes, a minus before it negating it,
    /// as bc's operator does.
    fn num(text: &str) -> Number {
        match text.strip_prefix('-') {
            Some(rest) => read(rest.as_bytes(), 10).negated(),
            None => read(text.as_bytes(), 10),
        }
    }

    #[test]
    fn digits_read_in_their_base_as_gnu_bc_reads_them() {
        for (text, base, value) in [
            ("12", 2, "3"),
            ("A", 10, "10"),
            ("1A", 10, "19"),
            ("FF.8", 16, "255.5"),
            ("0.4", 8, ".5"),
            (".F", 16, ".9"),
            (".FF", 16, ".99"),
            (".1", 2, ".5"),
            (".11", 2, ".75"),
            (".1", 3, ".3"),
            ("A.A", 16, "10.6"),
            ("0.0", 16, "0"),
            ("F", 2, "15"),
            ("1.50", 10, "1.50"),
            ("000", 10, "0"),
            ("10", 16, "16"),
            (".", 10, "0"),
        ] {
            assert_eq!(
                read(text.as_bytes(), base).decimal(),
                value,
                "{text} in base {base}"
            );
        }
        assert_eq!(read(b"0.0", 16).scale(), 1);
        assert_eq!(read(b"12.", 10).scale(), 0);
    }

    #[test]
    fn numbers_are_written_in_their_base_as_gnu_bc_writes_them() {
        for (text, base, written) in [
            (".5", 2, ".1000"),
            ("0.1", 2, ".0001"),
            ("0.1", 16, ".1"),
            ("255.5", 16, "FF.8"),
            ("-255", 16, "-FF"),
            ("255", 17, " 15 00"),
            ("123456", 100, " 12 34 56"),
            ("12.34", 100, " 12.34"),
            ("1234567", 1000, " 001 234 567"),
            ("0", 2, "0"),
            (".001", 16, ".004"),
            ("8.25", 8, "10.200"),
            ("2.0", 3, "2.000"),
            ("1.50", 10, "1.50"),
            ("10.0000", 16, "A.0000"),
            ("-1.5", 2, "-1.1000"),
            (".3333", 100, ".33 33"),
            ("-12.34", 100, "- 12.34"),
            (".33", 17, ".05 10"),
            (".5", 1000, ".500"),
            (".333", 2, ".0101010100"),
        ] {
            assert_eq!(write(&num(text), base), written, "{text} in base {base}");
        }
    }

    #[test]
    fn a_digit_is_its_letter_or_its_decimal_digits() {
        assert_eq!(digit_value(b'0'), Some(0));
        assert_eq!(digit_value(b'9'), Some(9));
        assert_eq!(digit_value(b'A'), Some(10));
        assert_eq!(digit_value(b'F'), Some(15));
        assert_eq!(digit_value(b'G'), None);
        assert_eq!(digit_value(b'a'), None);
        assert_eq!(digit_value(b'.'), None);
        assert_eq!(decimal_digits(0), 1);
        assert_eq!(decimal_digits(16), 2);
        assert_eq!(decimal_digits(999), 3);
        let mut text = String::new();
        push_digit(&mut text, 0, 2);
        push_digit(&mut text, 7, 3);
        push_digit(&mut text, 15, 0);
        assert_eq!(text, "00007F");
    }

    /// Random numbers as GNU bc 1.08 writes them in random bases.
    const WRITTEN: [(&str, u32, &str); 60] = [
        ("937302559.7", 16, "37DE1A1F.B"),
        ("-68.606254049", 11, "-62.673A19035"),
        ("47945829.95", 256, " 002 219 152 101.243"),
        ("32.5445", 16, "20.8B64"),
        ("0.6128", 3, ".121112201"),
        ("38782556989.9239264", 8, "440747555475.73103160"),
        ("762539.36972332", 36, " 16 12 13 23.13 11 05 29 07 12"),
        ("-6.7190", 8, "-6.56010"),
        ("730404427201581.5", 1000, " 730 404 427 201 581.500"),
        (
            "684287626338279712253.664765670",
            11,
            "102091020A2490954249A.734891A31",
        ),
        ("620222.5025", 11, "393A89.5589"),
        (
            "-6050378760210940958.53724993",
            16,
            "-53F7436D16F1041E.8989361",
        ),
        ("7", 20, " 07"),
        ("90.879", 16, "5A.E10"),
        (
            "18780.816882046",
            17,
            " 03 13 16 12.13 15 01 05 13 11 12 12",
        ),
        ("-3396922625067.5465", 16, "-316E85DCC2B.8BE7"),
        (
            "-93360888254588393645.445911",
            8,
            "-12076444023415153414255.3442347",
        ),
        ("-221811677160", 8, "-3164500313750"),
        ("-205976119650754.63", 1000, "- 205 976 119 650 754.630"),
        ("9.72398145", 7, "12.5032164566"),
        (
            "669976919211892983433.34413398",
            7,
            "3332334131556060416435161.2260160062",
        ),
        (
            "3231073147186355.752063366",
            2,
            "1011011110101010010011010111001011100110110010110011.110000001000011100111001100010",
        ),
        ("4002.82", 12, "2396.9A"),
        (
            "193781070865264163.644615",
            256,
            " 002 176 114 219 117 030 110 035.165 005 125",
        ),
        ("149838201875270463.9190", 11, "329630836137A8275.A122"),
        ("0.4225958", 2, ".011011000010111100111101"),
        ("79542578707", 36, " 01 00 19 17 21 18 19 07"),
        ("174052089106031.14410", 16, "9E4CA8F18E6F.24E3B"),
        ("-14.469283", 7, "-20.31665145"),
        (
            "110682757163.2575623",
            2,
            "1100111000101001101001101100000101011.010000011110111110011010",
        ),
        ("9517078054086170684.905", 16, "841374110978D83C.E7A"),
        ("540429535983663385.3463", 12, "2B0B09287B72B8021.41A4"),
        (
            "690967877803491.7",
            2,
            "10011101000110111001111110111010000101110111100011.1011",
        ),
        ("53.1", 3, "1222.002"),
        (
            "-30369094947097479606.21620859",
            2,
            "-11010010101110100101100101111001110110111110110110000100110110110.001101110101100101110010001",
        ),
        ("-50108983.5151107", 3, "-10111021210121021.111220111220221"),
        (
            "6590353028260050894.532699",
            36,
            " 01 14 02 19 09 06 09 05 13 10 33 08 30.19 06 13 21",
        ),
        ("2.86", 2, "10.1101110"),
        ("95028720584929002", 1000, " 095 028 720 584 929 002"),
        (
            "963416708389190916.1690271",
            36,
            " 07 11 18 06 25 33 30 12 23 14 17 00.06 03 02 04 22",
        ),
        ("975733.7117", 1000, " 975 733.711 700"),
        ("-58722897985656.01218004", 9, "-250825051566856.008781867"),
        ("5950214933022334328.20", 8, "512233217227401256570.146"),
        ("33699304182", 8, "373050511366"),
        ("771024.9", 3, "1110011122110.220"),
        (
            "86667375154760495960.80",
            20,
            " 02 12 17 19 00 10 17 01 19 13 02 13 01 19 18 00.16 00",
        ),
        ("-0", 8, "0"),
        (
            "-7291437853889842476807.7629",
            17,
            "- 08 13 14 05 00 07 10 15 10 08 12 16 12 03 15 13 01 05.12 16 08 02",
        ),
        (
            "80480360307638965012.031195448",
            100,
            " 80 48 03 60 30 76 38 96 50 12.03 11 95 44 80",
        ),
        ("7.73066724", 11, "7.80457764"),
        ("-8985937116153429849.8", 16, "-7CB476329652BF59.C"),
        ("-388500083840.958207", 8, "-5516432152200.7524641"),
        ("-0.641758646", 1000, "-.641 758 646"),
        (
            "-854478746907297386693.3",
            7,
            "-4313553533354343652020410.20",
        ),
        (
            "-912170636268767907.46",
            1000,
            "- 912 170 636 268 767 907.460",
        ),
        (
            "39819530772147260.3608429",
            9,
            "234354228037172232.32204363",
        ),
        ("391503.65250", 9, "656033.577604"),
        ("727317.0", 9, "1327620.00"),
        ("6632285100.65", 12, "135117BB50.79"),
        (
            "9511186552540667175962.04069",
            256,
            " 002 003 154 058 145 028 018 165 228 026.010 106 168",
        ),
    ];

    /// Random digits as GNU bc 1.08 reads them in random bases, some past
    /// the base.
    const READ: [(&str, u32, &str); 40] = [
        ("6A0B57A.B747", 14, "50586728.8230"),
        ("44120140.302", 5, "379420.616"),
        ("001A031B35601837.0978A74B", 12, "196349975454571.06697939"),
        ("12225088814.6250", 11, "31129998651.5657"),
        ("315400532255122454.054", 6, "56267490363298.157"),
        ("4.84666", 9, "4.94751"),
        ("10100.11", 2, "20.75"),
        ("1660A371.89A006A2", 11, "31096594.80916994"),
        ("943BA5554BA911A650.B4B", 12, "20768499727016981532.950"),
        ("2310222.201203F3", 4, "11562.52439880"),
        ("A331B8A2.05", 12, "368063546.03"),
        ("4434.3", 6, "1030.5"),
        ("96B2.FAC5A0B6", 14, "26028.98411387"),
        ("0011.01112110", 3, "4.17055326"),
        ("04087B14765A.177C", 13, "558452649687.1219"),
        ("423013", 5, "14133"),
        ("245131441012325.140002", 6, "220417175933.277820"),
        ("7.072", 8, "7.113"),
        ("3303132222023.010300", 4, "63826571.074218"),
        ("7CB9F.B87E1", 16, "510879.72067"),
        ("5245001", 6, "255097"),
        ("562.0", 14, "1066.0"),
        ("2102201.01102012", 3, "1774.15714067"),
        ("81781AB429622.A55062", 12, "72547333947962.870973"),
        ("10323113310331.3130303", 4, "82672957.8624877"),
        ("00303313123312.001", 4, "13596406.015"),
        ("13000310032.03011", 4, "1838350.19238"),
        ("AF3864784A12.12", 14, "44330418348344.08"),
        ("3202200.1213", 4, "14496.4023"),
        ("BB3772A9423B32", 13, "3594323134921417"),
        ("4741742370.6520F", 9, "1871113671.73127"),
        ("964A759DD0A04.A98", 14, "535917164333212.763"),
        ("4034.34011003", 5, "519.76192768"),
        ("62", 8, "50"),
        ("B74610.7BD5A693", 14, "6197142.56100969"),
        ("7636.7788", 9, "5622.8763"),
        ("5148005.7923", 14, "38361125.5467"),
        ("713.4256990", 11, "861.3843926"),
        ("10223102.0330313", 4, "19154.2377319"),
        ("02212250421022.006561", 7, "32016130057.019940"),
    ];

    #[test]
    fn random_numbers_go_out_and_in_as_gnu_bc_has_them() {
        for (text, base, written) in WRITTEN {
            assert_eq!(write(&num(text), base), written, "{text} in base {base}");
        }
        for (text, base, value) in READ {
            assert_eq!(
                read(text.as_bytes(), base).decimal(),
                value,
                "{text} in base {base}"
            );
        }
    }
}
