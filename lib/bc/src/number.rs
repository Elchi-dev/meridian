//! Numbers as bc has them: decimal, of any length, with a scale, the
//! digits they have after the point. Each operation gives its result the
//! scale POSIX says and cuts off the digits past it, never rounding.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::big::Big;

/// Why an operation has no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    DivideByZero,
    ModuloByZero,
    NegativeRoot,
    /// A power with more digits than bc would hold.
    TooGreat,
}

/// The most digits a power may have.
const MOST_DIGITS: usize = 10_000_000;

/// A number: its sign, its digits as a whole number, and how many of them
/// are after the point.
#[derive(Clone, Debug, Default)]
pub struct Number {
    negative: bool,
    /// The number without its sign, times ten to the power `scale`.
    magnitude: Big,
    scale: usize,
}

impl Number {
    /// The number `magnitude` over ten to the power `scale`, negative when
    /// `negative` and not zero.
    #[must_use]
    pub fn new(negative: bool, magnitude: Big, scale: usize) -> Number {
        Number {
            negative: negative && !magnitude.is_zero(),
            magnitude,
            scale,
        }
    }

    #[must_use]
    pub fn from_i64(n: i64) -> Number {
        Number::new(n < 0, Big::from_u64(n.unsigned_abs()), 0)
    }

    #[must_use]
    pub fn scale(&self) -> usize {
        self.scale
    }

    #[must_use]
    pub fn magnitude(&self) -> &Big {
        &self.magnitude
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.magnitude.is_zero()
    }

    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// The number with `scale` digits after the point: those past it cut
    /// off, or zeros put on.
    #[must_use]
    pub fn rescaled(&self, scale: usize) -> Number {
        let magnitude = if scale >= self.scale {
            self.magnitude.shl10(scale - self.scale)
        } else {
            self.magnitude.shr10(self.scale - scale)
        };
        Number::new(self.negative, magnitude, scale)
    }

    #[must_use]
    pub fn negated(&self) -> Number {
        Number::new(!self.negative, self.magnitude.clone(), self.scale)
    }

    /// The two magnitudes with as many digits after the point, the most
    /// either has, and that scale.
    fn aligned(&self, other: &Number) -> (Big, Big, usize) {
        let scale = self.scale.max(other.scale);
        (
            self.magnitude.shl10(scale - self.scale),
            other.magnitude.shl10(scale - other.scale),
            scale,
        )
    }

    #[must_use]
    pub fn compare(&self, other: &Number) -> Ordering {
        let (a, b, _) = self.aligned(other);
        match (self.negative, other.negative) {
            (false, false) => a.cmp(&b),
            (true, true) => b.cmp(&a),
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
        }
    }

    /// The sum, to the greater of the two scales.
    #[must_use]
    pub fn add(&self, other: &Number) -> Number {
        let (a, b, scale) = self.aligned(other);
        if self.negative == other.negative {
            return Number::new(self.negative, a.add(&b), scale);
        }
        if a >= b {
            Number::new(self.negative, a.sub(&b), scale)
        } else {
            Number::new(other.negative, b.sub(&a), scale)
        }
    }

    /// The difference, to the greater of the two scales.
    #[must_use]
    pub fn sub(&self, other: &Number) -> Number {
        self.add(&other.negated())
    }

    /// The product, to the scale both together have, but no more than the
    /// greatest of `scale` and theirs.
    #[must_use]
    pub fn mul(&self, other: &Number, scale: usize) -> Number {
        let full = self.scale + other.scale;
        let wanted = full.min(scale.max(self.scale).max(other.scale));
        let magnitude = self.magnitude.mul(&other.magnitude).shr10(full - wanted);
        Number::new(self.negative != other.negative, magnitude, wanted)
    }

    /// The quotient, to `scale` digits after the point.
    ///
    /// # Errors
    ///
    /// For a divisor of zero.
    pub fn div(&self, other: &Number, scale: usize) -> Result<Number, Fault> {
        if other.is_zero() {
            return Err(Fault::DivideByZero);
        }
        Ok(self.quotient(other, scale))
    }

    fn quotient(&self, other: &Number, scale: usize) -> Number {
        // Both times what makes the quotient a whole number of `scale`
        // digits after the point.
        let (dividend, divisor) = if other.scale + scale >= self.scale {
            (
                self.magnitude.shl10(other.scale + scale - self.scale),
                other.magnitude.clone(),
            )
        } else {
            (
                self.magnitude.clone(),
                other.magnitude.shl10(self.scale - other.scale - scale),
            )
        };
        let (magnitude, _) = dividend.divrem(&divisor);
        Number::new(self.negative != other.negative, magnitude, scale)
    }

    /// What is left of `self` after `other` times the quotient to `scale`
    /// digits, to as many digits as that product and `self` have.
    ///
    /// # Errors
    ///
    /// For a divisor of zero.
    pub fn rem(&self, other: &Number, scale: usize) -> Result<Number, Fault> {
        if other.is_zero() {
            return Err(Fault::ModuloByZero);
        }
        let quotient = self.quotient(other, scale);
        let product = quotient.mul(other, scale + other.scale);
        Ok(self.sub(&product))
    }

    /// `self` to the power of the whole part of `exponent`: to the scale
    /// `self` has that many times, but no more than the greater of
    /// `scale` and its own; and to `scale` for a power below zero.
    ///
    /// # Errors
    ///
    /// For zero to a power below zero, and for a power too great.
    pub fn pow(&self, exponent: &Number, scale: usize) -> Result<Number, Fault> {
        let times = exponent.magnitude.shr10(exponent.scale);
        if times.is_zero() {
            return Ok(Number::from_i64(1));
        }
        if self.is_zero() {
            return if exponent.negative {
                Err(Fault::DivideByZero)
            } else {
                Ok(Number::default())
            };
        }
        let times = times
            .to_u64()
            .and_then(|t| usize::try_from(t).ok())
            .filter(|&t| self.magnitude.digit_count().saturating_mul(t) <= MOST_DIGITS)
            .ok_or(Fault::TooGreat)?;
        let mut power = Big::from_u64(1);
        let mut square = self.magnitude.clone();
        let mut left = times;
        while left > 0 {
            if left & 1 == 1 {
                power = power.mul(&square);
            }
            left >>= 1;
            if left > 0 {
                square = square.mul(&square);
            }
        }
        let negative = self.negative && times & 1 == 1;
        let exact = Number::new(negative, power, self.scale * times);
        if exponent.negative {
            return Ok(Number::from_i64(1).quotient(&exact, scale));
        }
        let wanted = exact.scale.min(scale.max(self.scale));
        Ok(exact.rescaled(wanted))
    }

    /// The square root, to the greater of `scale` and the number's own.
    ///
    /// # Errors
    ///
    /// For a number below zero.
    pub fn sqrt(&self, scale: usize) -> Result<Number, Fault> {
        if self.negative {
            return Err(Fault::NegativeRoot);
        }
        let wanted = scale.max(self.scale);
        let root = self.magnitude.shl10(2 * wanted - self.scale).sqrt();
        Ok(Number::new(false, root, wanted))
    }

    /// The digits bc counts as significant: all of them, from the first
    /// that is not zero or from the point if that comes first.
    #[must_use]
    pub fn length(&self) -> usize {
        self.magnitude.digit_count().max(self.scale).max(1)
    }

    /// The whole part, when it fits.
    #[must_use]
    pub fn to_i64(&self) -> Option<i64> {
        let whole = i64::try_from(self.magnitude.shr10(self.scale).to_u64()?).ok()?;
        Some(if self.negative { -whole } else { whole })
    }

    /// The number in base ten as bc writes it: no zero before the point,
    /// every digit of its scale after it, and zero as `0`.
    #[must_use]
    pub fn decimal(&self) -> String {
        if self.is_zero() {
            return String::from("0");
        }
        let mut digits = self.magnitude.digits();
        if digits.len() < self.scale {
            let mut zeros = alloc::vec![0; self.scale - digits.len()];
            zeros.extend_from_slice(&digits);
            digits = zeros;
        }
        let point = digits.len() - self.scale;
        let mut text = String::with_capacity(digits.len() + 2);
        if self.negative {
            text.push('-');
        }
        let shown: Vec<char> = digits.iter().map(|&d| char::from(b'0' + d)).collect();
        text.extend(&shown[..point]);
        if self.scale > 0 {
            text.push('.');
            text.extend(&shown[point..]);
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number a decimal text writes, as bc reads one in base ten.
    fn num(text: &str) -> Number {
        let (negative, text) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        let digits: Vec<u8> = whole
            .bytes()
            .chain(fraction.bytes())
            .map(|b| b - b'0')
            .collect();
        Number::new(negative, Big::from_digits(&digits), fraction.len())
    }

    fn shown(n: Result<Number, Fault>) -> Result<String, Fault> {
        n.map(|n| n.decimal())
    }

    #[test]
    fn numbers_are_written_as_gnu_bc_writes_them() {
        for (text, written) in [
            ("0.00", "0"),
            ("-0.0", "0"),
            ("1.50", "1.50"),
            ("-.5", "-.5"),
            ("0.5", ".5"),
            ("000123", "123"),
            ("12.", "12"),
            ("-0.0001", "-.0001"),
            ("100.000", "100.000"),
        ] {
            assert_eq!(num(text).decimal(), written, "{text}");
        }
        assert!(!num("-0.0").is_negative(), "zero has no sign");
        assert_eq!(Number::from_i64(-42).decimal(), "-42");
        assert_eq!(Number::from_i64(0).decimal(), "0");
    }

    #[test]
    fn length_and_scale_are_as_gnu_bc_counts_them() {
        for (text, length, scale) in [
            ("0.001", 3, 3),
            ("123.45", 5, 2),
            ("0", 1, 0),
            ("0.0", 1, 1),
            ("0.00", 2, 2),
            ("0.100", 3, 3),
            ("-0.05", 2, 2),
            ("1.000", 4, 3),
            ("10.01", 4, 2),
            ("000123", 3, 0),
            ("100", 3, 0),
        ] {
            assert_eq!(
                (num(text).length(), num(text).scale()),
                (length, scale),
                "{text}"
            );
        }
    }

    #[test]
    fn sums_take_the_greater_scale() {
        assert_eq!(num(".1").add(&num(".02")).decimal(), ".12");
        assert_eq!(num("1").sub(&num("1.000")).decimal(), "0");
        assert_eq!(num("1").sub(&num("1.000")).scale(), 3);
        assert_eq!(num("-1.5").add(&num("1.5")).decimal(), "0");
        assert_eq!(num("-1.5").add(&num("0.25")).decimal(), "-1.25");
        assert_eq!(num("1.5").add(&num("-2")).decimal(), "-.5");
        assert_eq!(num("-1").add(&num("-2.5")).decimal(), "-3.5");
        assert_eq!(num("0.9").sub(&num("-0.1")).decimal(), "1.0");
        assert_eq!(
            num("999999999.999").add(&num("0.001")).decimal(),
            "1000000000.000"
        );
    }

    #[test]
    fn products_keep_the_scale_posix_gives_them() {
        for (a, b, scale, product) in [
            ("1.5", "1.25", 0, "1.87"),
            ("1.5", "1.25", 1, "1.87"),
            ("1.5", "1.25", 3, "1.875"),
            ("1.5", "1.25", 9, "1.875"),
            ("0.05", "0.05", 1, "0"),
            ("0.05", "0.05", 4, ".0025"),
            ("4.0", "1", 0, "4.0"),
            ("-3", "2.5", 0, "-7.5"),
            ("-3", "-2.5", 0, "7.5"),
        ] {
            assert_eq!(
                num(a).mul(&num(b), scale).decimal(),
                product,
                "{a} * {b}, {scale}"
            );
        }
        assert_eq!(num("0.05").mul(&num("0.05"), 1).scale(), 2, "zero keeps it");
    }

    #[test]
    fn quotients_have_the_scale_asked_for() {
        for (a, b, scale, quotient) in [
            ("7", "2", 0, "3"),
            ("-7", "2", 0, "-3"),
            ("7", "3", 3, "2.333"),
            ("-7", "3", 3, "-2.333"),
            ("10.5", "2", 0, "5"),
            ("10.5", "2", 4, "5.2500"),
            ("-1", "3", 1, "-.3"),
            ("-7", "0.3", 4, "-23.3333"),
            ("1", "3", 2, ".33"),
            ("1.23456", "1", 2, "1.23"),
            ("1", "0.0001", 0, "10000"),
        ] {
            assert_eq!(
                shown(num(a).div(&num(b), scale)),
                Ok(quotient.into()),
                "{a} / {b}"
            );
        }
        assert_eq!(
            num("1").div(&num("0.00"), 5).err(),
            Some(Fault::DivideByZero)
        );
    }

    #[test]
    fn remainders_are_what_the_quotient_leaves() {
        for (a, b, scale, rest) in [
            ("7", "3", 0, "1"),
            ("-7", "3", 0, "-1"),
            ("7", "-3", 0, "1"),
            ("7", "3", 2, ".01"),
            ("7.5", "2", 2, "0"),
            ("5.000", "3", 0, "2.000"),
            ("7.5", "2", 0, "1.5"),
        ] {
            assert_eq!(
                shown(num(a).rem(&num(b), scale)),
                Ok(rest.into()),
                "{a} % {b}"
            );
        }
        assert_eq!(num("7.5").rem(&num("2"), 2).map(|r| r.scale()), Ok(2));
        assert_eq!(num("3").rem(&num("0"), 0).err(), Some(Fault::ModuloByZero));
    }

    #[test]
    fn powers_are_exact_then_cut_to_their_scale() {
        for (a, e, scale, power) in [
            ("2", "10", 0, "1024"),
            ("1.5", "2", 0, "2.2"),
            ("1.5", "3", 0, "3.3"),
            ("1.5", "3", 10, "3.375"),
            ("2", "-1", 2, ".50"),
            ("2", "-3", 5, ".12500"),
            ("2", "-1", 0, "0"),
            ("1.1", "5", 3, "1.610"),
            ("1.1", "5", 20, "1.61051"),
            ("0.5", "2", 0, ".2"),
            ("-2", "3", 0, "-8"),
            ("-2", "2", 0, "4"),
            ("0", "0", 0, "1"),
            ("2", "0.5", 0, "1"),
            ("2", "1.0", 0, "2"),
            ("2", "-1.0", 3, ".500"),
            ("-1", "-1", 0, "-1"),
            ("0.1", "-1", 0, "10"),
            ("1.00", "-1", 0, "1"),
            ("-1.5", "3", 0, "-3.3"),
            ("0", "5", 3, "0"),
            (
                "7",
                "-20",
                50,
                ".00000000000000001253254289419684899836964752574345",
            ),
        ] {
            assert_eq!(
                shown(num(a).pow(&num(e), scale)),
                Ok(power.into()),
                "{a} ^ {e}, {scale}"
            );
        }
        assert_eq!(num("0").pow(&num("-1"), 0).err(), Some(Fault::DivideByZero));
        assert_eq!(
            num("12").pow(&num("99999999999"), 0).err(),
            Some(Fault::TooGreat)
        );
        assert_eq!(
            num("2").pow(&num("99999999999999999999999"), 0).err(),
            Some(Fault::TooGreat),
            "past a u64"
        );
        let big = num("3").pow(&num("1000"), 0).unwrap();
        assert_eq!(big.length(), 478, "3^1000 has 478 digits");
    }

    #[test]
    fn square_roots_take_the_greater_scale() {
        for (a, scale, root) in [
            ("2", 0, "1"),
            ("2", 10, "1.4142135623"),
            ("16", 0, "4"),
            ("0.25", 0, ".50"),
            ("1000000", 5, "1000.00000"),
            (".0004", 0, ".0200"),
            ("0", 0, "0"),
        ] {
            assert_eq!(
                shown(num(a).sqrt(scale)),
                Ok(root.into()),
                "sqrt({a}), {scale}"
            );
        }
        assert_eq!(num("2.000").sqrt(0).map(|r| r.scale()), Ok(3));
        assert_eq!(num("-1").sqrt(0).err(), Some(Fault::NegativeRoot));
        assert_eq!(
            shown(num("-0.0").sqrt(2)),
            Ok("0".into()),
            "zero has no sign"
        );
    }

    #[test]
    fn numbers_compare_whatever_their_scales() {
        assert_eq!(num("1.0").compare(&num("1")), Ordering::Equal);
        assert_eq!(num("1.01").compare(&num("1.1")), Ordering::Less);
        assert_eq!(num("-1").compare(&num("-2")), Ordering::Greater);
        assert_eq!(num("-1").compare(&num("0.5")), Ordering::Less);
        assert_eq!(num("0.5").compare(&num("-1")), Ordering::Greater);
        assert_eq!(num("-0.0").compare(&num("0")), Ordering::Equal);
        assert_eq!(num("-2.5").compare(&num("-2.50")), Ordering::Equal);
    }

    #[test]
    fn whole_parts_fit_or_do_not() {
        assert_eq!(num("12.99").to_i64(), Some(12));
        assert_eq!(num("-12.99").to_i64(), Some(-12));
        assert_eq!(num("9223372036854775807").to_i64(), Some(i64::MAX));
        assert_eq!(num("9223372036854775808").to_i64(), None);
        assert_eq!(num("0.5").rescaled(0).decimal(), "0");
        assert_eq!(num("1.5").rescaled(3).decimal(), "1.500");
        assert_eq!(num("-1.59").rescaled(1).decimal(), "-1.5");
    }

    /// Random numbers worked out by GNU bc 1.08, at the scale given.
    const WORKED: [(&str, char, &str, usize, &str); 70] = [
        (
            "-8909307824073939090.345",
            '*',
            "337193013708522262372.640",
            22,
            "-3004156355256408392339039606589374423016.160800",
        ),
        (
            "-23515761178.60800068",
            '+',
            "24014665968.1751",
            11,
            "498904789.56709932",
        ),
        (
            "2671490018139635464794286.797",
            '%',
            "-880.4968796370",
            17,
            ".000000000000000925850647320",
        ),
        (
            "114297794833128598773020.33838",
            '*',
            "507802805657067854717141.798944464199",
            16,
            "58040740896678616232563108855243791271541976580.5546439387756576",
        ),
        (
            "2345579157684898808925579.557468282504",
            '/',
            "-49327805.08",
            9,
            "-47550851976503528.807034840",
        ),
        (
            "49530519868",
            '*',
            "4944971.327506690416",
            23,
            "244927000583760464.552613185088",
        ),
        ("-934615327", '^', "-2", 1, "0"),
        ("-.71", '-', "73375.99305324", 23, "-73376.70305324"),
        (
            "-461974186028.738624",
            '+',
            "-3685523865900646477059910.308462369421",
            10,
            "-3685523865901108451245939.047086369421",
        ),
        (
            "592.3",
            '/',
            "-9593195.8165075714",
            24,
            "-.000061741677260542816635",
        ),
        (
            "2811875320497258903709888.06683",
            '/',
            "4203107005201945297208",
            16,
            "668.9992229598631540",
        ),
        ("87121.5119", '^', "-3", 1, "0"),
        (
            "28917699680252751366758.208297904",
            '/',
            "239353216664294215896.0",
            0,
            "120",
        ),
        (
            "7.08403932845",
            '*',
            "2194716421919959367767.439",
            20,
            "15547457447676055819290.53514933633955",
        ),
        (
            "512166170688602081894.608805604143",
            '+',
            "-43.7",
            14,
            "512166170688602081850.908805604143",
        ),
        (
            "-6205202.46090644",
            '*',
            "2539375486306598.19",
            16,
            "-15757339016795190918627.3002633436",
        ),
        (
            "635563203824107952652.223",
            '+',
            "92474522095135.7670006",
            21,
            "635563296298630047787.9900006",
        ),
        (
            "70546789014555697",
            '-',
            "-920.04",
            4,
            "70546789014556617.04",
        ),
        (
            "659652387025753972939.6",
            '/',
            "-87973186.47406727",
            12,
            "-7498334588803.444777809665",
        ),
        (
            "-15651304064.461705404569",
            '-',
            "-348761706600094003970.61",
            15,
            "348761706584442699906.148294595431",
        ),
        (
            "222539147906690.766203",
            '/',
            "-741724392.2",
            24,
            "-300029.431749744694728943282553",
        ),
        (
            "-0",
            '+',
            "2670306946586.503596160",
            28,
            "2670306946586.503596160",
        ),
        (
            "-8156884583331010.09",
            '-',
            "-306613378089571275456506.2",
            17,
            "306613369932686692125496.11",
        ),
        (
            "793659330157734154606239.6167785",
            '%',
            "-71.5553412",
            4,
            ".00246978220",
        ),
        (
            "21329503956.30",
            '%',
            "-4462951331.65159998703",
            26,
            ".0000000000000000138295567898564548099",
        ),
        ("79893493.0", '^', "-2", 20, ".00000000000000015666"),
        (
            "258939932610255617222.7144163",
            '%',
            "-34950872009.35",
            9,
            "15.93801113605",
        ),
        ("71", '^', "0", 26, "1"),
        (
            "53544162611185035315590.9823871661",
            '+',
            "4716176165158396095.38598",
            4,
            "53548878787350193711686.3683671661",
        ),
        (
            "-969717168604334.396",
            '*',
            "-1897338421136",
            6,
            "1839881641628220131592818193.856",
        ),
        (
            "70791548719375.014695604400",
            '/',
            "2.062",
            2,
            "34331497924042.19",
        ),
        (
            "842985.89350931550",
            '-',
            "-62784380066795807536379.37577068848",
            10,
            "62784380066795808379365.26928000398",
        ),
        ("2.90490", '+', "-9523863052.8", 10, "-9523863049.89510"),
        (
            "-610376544",
            '^',
            "12",
            10,
            "2674077750808397690919393428777868461639698184983645243908646090566659480563587552208113327271596036980736",
        ),
        (
            "757484425944070229.643267855718",
            '*',
            "26579529831998443.218278",
            22,
            "20133579896654630194397319297895459.797397868084413604",
        ),
        (
            "7309821303.4832088",
            '%',
            "-246.853175",
            15,
            ".000000000000116011200",
        ),
        (
            "14919727659.6",
            '/',
            "-56.22531320360",
            8,
            "-265356061.34506544",
        ),
        (
            "6721177.739234149",
            '+',
            "407366631253.0707",
            5,
            "407373352430.809934149",
        ),
        (
            "60457287591.00471",
            '*',
            "4240760353595906.3762483951",
            4,
            "256384868301878536770425310.9319273340",
        ),
        (
            "376975522324992323618",
            '*',
            "-14096971066349",
            26,
            "-5314213030937218292006476757730682",
        ),
        (
            "7871937797622532879493094.18892404",
            '/',
            "-1664326558273.8",
            4,
            "-4729803630476.8938",
        ),
        (
            "-3888538546391483716983062.48400",
            '*',
            "-31472388146555.98141507217",
            3,
            "122381594454877358373315170692029690654.74240247947",
        ),
        (
            "6554.229177442",
            '-',
            "-5090133.93333",
            12,
            "5096688.162507442",
        ),
        (
            "605321",
            '/',
            "22.629819999",
            20,
            "26748.82080488262040108505",
        ),
        (
            "-169813.5278",
            '+',
            "-254003445.4600",
            21,
            "-254173258.9878",
        ),
        (
            "989520040.22944261965",
            '-',
            "-2427",
            9,
            "989522467.22944261965",
        ),
        (
            "1020.936074130",
            '+',
            "266234.013008586",
            3,
            "267254.949082716",
        ),
        (
            "51477429",
            '+',
            "-315034579827351243.867560874",
            22,
            "-315034579775873814.867560874",
        ),
        (
            "1221498883819063562084343.20698449",
            '%',
            "81798442333628.385687830",
            19,
            ".0000065898152703177466892540",
        ),
        (
            "898443755430642892290417.092268850490",
            '-',
            "9429177054127895472.30697",
            13,
            "898434326253588764394944.785298850490",
        ),
        (
            "-222557386824678350868813",
            '*',
            "-90665862453843022115786.3",
            8,
            "20178357421933022584963482856182672410087642661.9",
        ),
        (
            "-8304914802469433548.00",
            '/',
            "286091242494731584020.411850772518",
            9,
            "-.029028902",
        ),
        (
            "1685777279042910136.77200435",
            '/',
            "2127946269452155131351861.04739",
            18,
            ".000000792208573704",
        ),
        (
            "261942220118433",
            '+',
            "6922970088356948176273.3",
            2,
            "6922970350299168294706.3",
        ),
        (
            "71546580351539.3281",
            '%',
            "188551124.8507763760",
            17,
            ".000000001738091387704213360",
        ),
        (
            "7484183308",
            '*',
            "-8629285.10963",
            8,
            "-64583151577465796.05604",
        ),
        (
            "9076194015179077929.8",
            '+',
            "-1.6",
            3,
            "9076194015179077928.2",
        ),
        (
            "1999247944418.94",
            '+',
            "774583551432564188247777.357972144038",
            7,
            "774583551434563436192196.297972144038",
        ),
        (
            "-8859418828145.1744009500",
            '-',
            ".725",
            28,
            "-8859418828145.8994009500",
        ),
        ("-97.6264013758", '/', "6113930606991743676.35101", 4, "0"),
        (
            "4055881",
            '/',
            "-6327761719.951700023",
            18,
            "-.000640966139292450",
        ),
        (
            "2451096657343391220098.17",
            '%',
            "797381970096.285",
            29,
            ".00000000000000000576479409441865",
        ),
        (
            "-5679525033351628.3118",
            '-',
            "6.000764983",
            17,
            "-5679525033351634.312564983",
        ),
        (
            "687657331093.3768020078",
            '*',
            "6027200056809658104.737887343",
            3,
            "4144648305031578533692690421842.9818345425",
        ),
        ("-93786", '+', "2828051.833648", 13, "2734265.833648"),
        (
            "-276768178639.0220959",
            '-',
            "957721294382.5",
            23,
            "-1234489473021.5220959",
        ),
        (
            "-257047057662069337280.38",
            '%',
            "7601665193955440917.1790189896",
            30,
            "-.0000000000075144875950757526997362239160",
        ),
        (
            "4607.198000367",
            '*',
            "85902477607881952848.04091320393",
            13,
            "395769723061604726679681.0759265509032",
        ),
        (
            "9005437686997538.06540",
            '+',
            "-10797.6992",
            24,
            "9005437686986740.36620",
        ),
        (
            "5215131957801.4724717",
            '-',
            "-528158720244879197.38050005159",
            24,
            "528163935376836998.85297175159",
        ),
    ];

    #[test]
    fn random_numbers_work_out_as_gnu_bc_works_them_out() {
        for (a, op, b, scale, wanted) in WORKED {
            let (a, b) = (num(a), num(b));
            let got = match op {
                '+' => Ok(a.add(&b)),
                '-' => Ok(a.sub(&b)),
                '*' => Ok(a.mul(&b, scale)),
                '/' => a.div(&b, scale),
                '%' => a.rem(&b, scale),
                _ => a.pow(&b, scale),
            };
            assert_eq!(shown(got), Ok(wanted.into()), "{a:?} {op} {b:?}, {scale}");
        }
    }
}
