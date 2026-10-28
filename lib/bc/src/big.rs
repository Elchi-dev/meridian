//! Whole numbers of any size, which bc's numbers are made of: limbs of
//! nine decimal digits, so that a number's digits come out of its limbs
//! without dividing it.

use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// What a limb counts to.
pub const BASE: u32 = 1_000_000_000;

/// The decimal digits a limb holds.
pub const DIGITS: usize = 9;

const WIDE: u64 = BASE as u64;

/// The powers of ten a limb holds.
const TENS: [u32; DIGITS] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
];

/// A whole number of any size: limbs of [`DIGITS`] decimal digits, the
/// lowest first, and none for zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Big {
    limbs: Vec<u32>,
}

/// `n`, which is below [`BASE`], as a limb.
#[allow(clippy::cast_possible_truncation)]
fn limb(n: u64) -> u32 {
    debug_assert!(n < WIDE);
    n as u32
}

impl Big {
    /// The number `limbs`, lowest first, each below [`BASE`], make.
    fn of(mut limbs: Vec<u32>) -> Big {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Big { limbs }
    }

    #[must_use]
    pub fn from_u64(mut n: u64) -> Big {
        let mut limbs = Vec::new();
        while n > 0 {
            limbs.push(limb(n % WIDE));
            n /= WIDE;
        }
        Big { limbs }
    }

    /// The number, when it fits.
    #[must_use]
    pub fn to_u64(&self) -> Option<u64> {
        let mut n: u64 = 0;
        for &l in self.limbs.iter().rev() {
            n = n.checked_mul(WIDE)?.checked_add(u64::from(l))?;
        }
        Some(n)
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    /// The number the decimal `digits`, most significant first, make.
    #[must_use]
    pub fn from_digits(digits: &[u8]) -> Big {
        let mut limbs = Vec::with_capacity(digits.len() / DIGITS + 1);
        for chunk in digits.rchunks(DIGITS) {
            let mut l = 0;
            for &d in chunk {
                l = l * 10 + u32::from(d);
            }
            limbs.push(l);
        }
        Big::of(limbs)
    }

    /// The number's decimal digits, most significant first: none for
    /// zero.
    #[must_use]
    pub fn digits(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.limbs.len() * DIGITS);
        for (at, &l) in self.limbs.iter().rev().enumerate() {
            let mut held = [0u8; DIGITS];
            let mut l = l;
            for d in held.iter_mut().rev() {
                *d = (l % 10).to_le_bytes()[0];
                l /= 10;
            }
            // The top limb has no zeros before it.
            let from = if at == 0 {
                held.iter().position(|&d| d != 0).unwrap_or(DIGITS)
            } else {
                0
            };
            out.extend_from_slice(&held[from..]);
        }
        out
    }

    /// How many decimal digits the number has: none for zero.
    #[must_use]
    pub fn digit_count(&self) -> usize {
        let Some(&top) = self.limbs.last() else {
            return 0;
        };
        let top_digits = TENS.iter().take_while(|&&t| t <= top).count();
        (self.limbs.len() - 1) * DIGITS + top_digits
    }

    #[must_use]
    pub fn add(&self, other: &Big) -> Big {
        let (long, short) = if self.limbs.len() >= other.limbs.len() {
            (self, other)
        } else {
            (other, self)
        };
        let mut limbs = Vec::with_capacity(long.limbs.len() + 1);
        let mut carry = 0;
        for (at, &l) in long.limbs.iter().enumerate() {
            let sum = u64::from(l) + u64::from(short.limbs.get(at).copied().unwrap_or(0)) + carry;
            limbs.push(limb(sum % WIDE));
            carry = sum / WIDE;
        }
        limbs.push(limb(carry));
        Big::of(limbs)
    }

    /// `self` less `other`, which is no greater.
    #[must_use]
    pub fn sub(&self, other: &Big) -> Big {
        debug_assert!(*self >= *other);
        let mut limbs = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0;
        for (at, &l) in self.limbs.iter().enumerate() {
            let take = u64::from(other.limbs.get(at).copied().unwrap_or(0)) + borrow;
            let l = u64::from(l);
            if l >= take {
                limbs.push(limb(l - take));
                borrow = 0;
            } else {
                limbs.push(limb(l + WIDE - take));
                borrow = 1;
            }
        }
        Big::of(limbs)
    }

    #[must_use]
    pub fn mul(&self, other: &Big) -> Big {
        if self.is_zero() || other.is_zero() {
            return Big::default();
        }
        let mut wide = vec![0u64; self.limbs.len() + other.limbs.len()];
        for (i, &a) in self.limbs.iter().enumerate() {
            let mut carry = 0;
            for (j, &b) in other.limbs.iter().enumerate() {
                let t = wide[i + j] + u64::from(a) * u64::from(b) + carry;
                wide[i + j] = t % WIDE;
                carry = t / WIDE;
            }
            wide[i + other.limbs.len()] += carry;
        }
        Big::of(wide.into_iter().map(limb).collect())
    }

    /// `self` times `m`, which is below [`BASE`].
    #[must_use]
    pub fn mul_small(&self, m: u32) -> Big {
        let mut limbs = Vec::with_capacity(self.limbs.len() + 1);
        let mut carry = 0;
        for &l in &self.limbs {
            let t = u64::from(l) * u64::from(m) + carry;
            limbs.push(limb(t % WIDE));
            carry = t / WIDE;
        }
        limbs.push(limb(carry));
        Big::of(limbs)
    }

    /// `self` divided by `d`, which is not zero and below [`BASE`], and
    /// what is left.
    #[must_use]
    pub fn divrem_small(&self, d: u32) -> (Big, u32) {
        let d = u64::from(d);
        let mut limbs = vec![0; self.limbs.len()];
        let mut rest = 0;
        for (at, &l) in self.limbs.iter().enumerate().rev() {
            let t = rest * WIDE + u64::from(l);
            limbs[at] = limb(t / d);
            rest = t % d;
        }
        (Big::of(limbs), limb(rest))
    }

    /// `self` divided by `divisor`, which is not zero, and what is left:
    /// Knuth's algorithm D, a limb of the quotient at a time.
    #[must_use]
    pub fn divrem(&self, divisor: &Big) -> (Big, Big) {
        debug_assert!(!divisor.is_zero());
        if *self < *divisor {
            return (Big::default(), self.clone());
        }
        let width = divisor.limbs.len();
        if width == 1 {
            let (quotient, rest) = self.divrem_small(divisor.limbs[0]);
            return (quotient, Big::from_u64(u64::from(rest)));
        }
        // Scaled so the divisor's top limb is at least half the base, a
        // quotient limb guessed from the top two limbs is at most two too
        // great.
        let scale = BASE / (divisor.limbs[width - 1] + 1);
        let mut left = self.mul_small(scale).limbs;
        left.resize(self.limbs.len() + 1, 0);
        let by = divisor.mul_small(scale).limbs;
        let mut quotient = vec![0u32; self.limbs.len() - width + 1];
        let (top, next) = (u64::from(by[width - 1]), u64::from(by[width - 2]));
        for place in (0..quotient.len()).rev() {
            let high = u64::from(left[place + width]) * WIDE + u64::from(left[place + width - 1]);
            let mut guess = high / top;
            let mut over = high % top;
            while guess >= WIDE || guess * next > over * WIDE + u64::from(left[place + width - 2]) {
                guess -= 1;
                over += top;
                if over >= WIDE {
                    break;
                }
            }
            // Takes guess times the divisor off the limbs from place.
            let mut borrow: i64 = 0;
            let mut carry: u64 = 0;
            for (i, &b) in by.iter().enumerate() {
                let product = guess * u64::from(b) + carry;
                carry = product / WIDE;
                let diff = i64::from(left[place + i]) - borrow - i64::from(limb(product % WIDE));
                borrow = i64::from(diff < 0);
                left[place + i] = limb(diff.rem_euclid(i64::from(BASE)).unsigned_abs());
            }
            let diff = i64::from(left[place + width]) - borrow - i64::from(limb(carry));
            if diff < 0 {
                // One too great: the divisor goes back once.
                guess -= 1;
                let mut carry = 0;
                for (i, &b) in by.iter().enumerate() {
                    let sum = u64::from(left[place + i]) + u64::from(b) + carry;
                    left[place + i] = limb(sum % WIDE);
                    carry = sum / WIDE;
                }
                left[place + width] = 0;
            } else {
                left[place + width] = limb(diff.unsigned_abs());
            }
            quotient[place] = limb(guess);
        }
        left.truncate(width);
        let (rest, _) = Big::of(left).divrem_small(scale);
        (Big::of(quotient), rest)
    }

    /// Ten to the power `k`.
    #[must_use]
    pub fn pow10(k: usize) -> Big {
        let mut limbs = vec![0; k / DIGITS];
        limbs.push(TENS[k % DIGITS]);
        Big { limbs }
    }

    /// `self` times ten to the power `k`.
    #[must_use]
    pub fn shl10(&self, k: usize) -> Big {
        if self.is_zero() {
            return Big::default();
        }
        let mut limbs = vec![0; k / DIGITS];
        limbs.extend_from_slice(&self.mul_small(TENS[k % DIGITS]).limbs);
        Big { limbs }
    }

    /// `self` divided by ten to the power `k`, what is left dropped.
    #[must_use]
    pub fn shr10(&self, k: usize) -> Big {
        let whole = k / DIGITS;
        if whole >= self.limbs.len() {
            return Big::default();
        }
        let shifted = Big {
            limbs: self.limbs[whole..].to_vec(),
        };
        shifted.divrem_small(TENS[k % DIGITS]).0
    }

    /// The greatest number whose square is no greater than `self`, by
    /// Newton's steps down from above.
    #[must_use]
    pub fn sqrt(&self) -> Big {
        if self.is_zero() {
            return Big::default();
        }
        let mut x = Big::pow10(self.digit_count().div_ceil(2));
        loop {
            let (q, _) = self.divrem(&x);
            let (next, _) = x.add(&q).divrem_small(2);
            if next >= x {
                return x;
            }
            x = next;
        }
    }
}

impl Ord for Big {
    fn cmp(&self, other: &Big) -> Ordering {
        self.limbs
            .len()
            .cmp(&other.limbs.len())
            .then_with(|| self.limbs.iter().rev().cmp(other.limbs.iter().rev()))
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, other: &Big) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(n: u128) -> Big {
        let mut limbs = Vec::new();
        let mut n = n;
        while n > 0 {
            limbs.push(u32::try_from(n % u128::from(BASE)).unwrap());
            n /= u128::from(BASE);
        }
        Big { limbs }
    }

    fn small(b: &Big) -> u128 {
        b.limbs
            .iter()
            .rev()
            .fold(0, |n, &l| n * u128::from(BASE) + u128::from(l))
    }

    /// The bytes of a random number generator that needs nothing.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// A number of up to `digits` decimal digits, its digits often
        /// nines and zeros, which carries and borrows run through.
        fn number(&mut self, digits: usize) -> Vec<u8> {
            let len = usize::try_from(self.next() % (digits as u64 + 1)).unwrap();
            (0..len)
                .map(|_| match self.next() % 4 {
                    0 => 9,
                    1 => 0,
                    _ => (self.next() % 10).to_le_bytes()[0],
                })
                .collect()
        }

        fn big_below(&mut self, top: u128) -> u128 {
            (u128::from(self.next()) << 64 | u128::from(self.next())) % top
        }
    }

    #[test]
    fn digits_go_in_and_come_out_as_they_were() {
        assert_eq!(Big::from_digits(&[]), Big::default());
        assert_eq!(
            Big::from_digits(&[0, 0, 0]),
            Big::default(),
            "zeros are none"
        );
        assert_eq!(Big::from_digits(&[1, 2, 3]).limbs, [123]);
        let ten_digits = [1, 0, 0, 0, 0, 0, 0, 0, 0, 5];
        assert_eq!(Big::from_digits(&ten_digits).limbs, [5, 1]);
        assert_eq!(Big::from_digits(&ten_digits).digits(), ten_digits);
        assert_eq!(Big::default().digits(), Vec::<u8>::new());
        let nines = [9; 18];
        assert_eq!(Big::from_digits(&nines).limbs, [999_999_999; 2]);
        assert_eq!(
            Big::from_digits(&[0, 0, 4, 2]).digits(),
            [4, 2],
            "no zeros before"
        );
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for _ in 0..500 {
            let mut digits = random.number(60);
            let first = digits.iter().position(|&d| d != 0).unwrap_or(digits.len());
            let n = Big::from_digits(&digits);
            digits.drain(..first);
            assert_eq!(n.digits(), digits);
            assert_eq!(n.digit_count(), digits.len());
        }
    }

    #[test]
    fn u64s_go_in_and_come_out() {
        for n in [0, 1, 999_999_999, 1_000_000_000, u64::MAX] {
            assert_eq!(Big::from_u64(n).to_u64(), Some(n));
            assert_eq!(small(&Big::from_u64(n)), u128::from(n));
        }
        assert_eq!(big(u128::from(u64::MAX) + 1).to_u64(), None);
        assert_eq!(Big::pow10(20).to_u64(), None);
        assert_eq!(Big::pow10(19).to_u64(), Some(10_000_000_000_000_000_000));
    }

    #[test]
    fn sums_differences_and_products_are_as_u128_has_them() {
        let mut random = Random(0x2545_f491_4f6c_dd1d);
        for _ in 0..3000 {
            let a = random.big_below(1 << 63);
            let b = random.big_below(1 << 63);
            let (first, second) = (big(a), big(b));
            assert_eq!(small(&first.add(&second)), a + b, "{a} + {b}");
            let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
            assert_eq!(small(&big(hi).sub(&big(lo))), hi - lo, "{hi} - {lo}");
            assert_eq!(small(&first.mul(&second)), a * b, "{a} * {b}");
            assert_eq!(first.cmp(&second), a.cmp(&b), "{a} against {b}");
            let factor = u32::try_from(b % u128::from(BASE)).unwrap();
            assert_eq!(small(&first.mul_small(factor)), a * u128::from(factor));
        }
        assert_eq!(big(999_999_999).add(&big(1)).limbs, [0, 1], "a carry");
        assert_eq!(
            big(1_000_000_000).sub(&big(1)).limbs,
            [999_999_999],
            "a borrow"
        );
        assert_eq!(big(5).sub(&big(5)), Big::default());
        assert_eq!(big(7).mul(&Big::default()), Big::default());
        assert_eq!(Big::default().mul(&big(7)), Big::default());
    }

    #[test]
    fn quotients_and_what_is_left_are_as_u128_has_them() {
        let mut random = Random(0x1405_7b7e_f767_814f);
        for _ in 0..5000 {
            // Divisors of every length up to the dividend's.
            let a = random.big_below(u128::MAX);
            let top = 1u128 << (random.next() % 127 + 1);
            let b = random.big_below(top).max(1);
            let (q, r) = big(a).divrem(&big(b));
            assert_eq!((small(&q), small(&r)), (a / b, a % b), "{a} / {b}");
        }
        for (a, b) in [
            (u128::MAX, 1),
            (u128::MAX, u128::MAX),
            (u128::MAX - 1, u128::MAX),
            (10u128.pow(36), 10u128.pow(18) + 1),
            (10u128.pow(27) - 1, 10u128.pow(18) - 1),
            (999_999_999_999_999_999_000_000_000, 999_999_999_999_999_999),
        ] {
            let (q, r) = big(a).divrem(&big(b));
            assert_eq!((small(&q), small(&r)), (a / b, a % b), "{a} / {b}");
        }
        let (q, r) = big(1_000_000_007).divrem_small(10);
        assert_eq!((small(&q), r), (100_000_000, 7));
    }

    #[test]
    fn long_division_undoes_long_multiplication() {
        let mut random = Random(0x6a09_e667_f3bc_c908);
        for _ in 0..400 {
            let quotient = Big::from_digits(&random.number(200));
            let divisor = Big::from_digits(&random.number(120));
            let rest = Big::from_digits(&random.number(110));
            if divisor.is_zero() || rest >= divisor {
                continue;
            }
            let whole = quotient.mul(&divisor).add(&rest);
            assert_eq!(whole.divrem(&divisor), (quotient, rest));
        }
    }

    #[test]
    fn powers_of_ten_move_the_digits() {
        assert_eq!(Big::pow10(0).limbs, [1]);
        assert_eq!(Big::pow10(9).limbs, [0, 1]);
        assert_eq!(Big::pow10(20).digits().len(), 21);
        let n = Big::from_digits(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 1, 2]);
        for k in 0..30 {
            let mut more = n.digits();
            more.resize(more.len() + k, 0);
            assert_eq!(n.shl10(k).digits(), more, "{k}");
            let digits = n.digits();
            let fewer = &digits[..digits.len().saturating_sub(k)];
            assert_eq!(n.shr10(k).digits(), fewer, "{k}");
        }
        assert_eq!(Big::default().shl10(5), Big::default());
        assert_eq!(n.shr10(11), Big::default());
        assert_eq!(n.shr10(9).limbs, [12]);
    }

    #[test]
    fn square_roots_are_the_greatest_that_fit() {
        for n in [
            0u128,
            1,
            2,
            3,
            4,
            8,
            9,
            10,
            99,
            100,
            101,
            u128::from(u64::MAX),
        ] {
            let root = small(&big(n).sqrt());
            assert!(
                root * root <= n && (root + 1) * (root + 1) > n,
                "{n}: {root}"
            );
        }
        let mut random = Random(0xbb67_ae85_84ca_a73b);
        for _ in 0..300 {
            let n = Big::from_digits(&random.number(150));
            let root = n.sqrt();
            let next = root.add(&Big::from_u64(1));
            assert!(root.mul(&root) <= n && next.mul(&next) > n);
        }
        let ten_to_the_hundred = Big::pow10(100);
        assert_eq!(ten_to_the_hundred.sqrt(), Big::pow10(50));
    }
}
