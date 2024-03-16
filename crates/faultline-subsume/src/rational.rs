//! Exact rational arithmetic on `i128`.
//!
//! Constraint reasoning needs exact answers to questions like "is 0.3 a
//! multiple of 0.1?" (yes, although not in binary floating point) and "what is
//! the smallest multiple of 0.25 above 1.1?". Schema numbers are short
//! decimals, so a reduced `i128` fraction is exact for all realistic inputs.
//! Every operation is checked; overflow yields `None`, which callers turn into
//! an *unknown* result rather than a wrong one.

use std::cmp::Ordering;
use std::fmt;

use faultline_doc::Decimal;

/// A reduced fraction `num / den` with `den > 0`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Q {
    num: i128,
    den: i128,
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn pow10(exp: u32) -> Option<i128> {
    10i128.checked_pow(exp)
}

impl Q {
    pub const ZERO: Q = Q { num: 0, den: 1 };
    pub const ONE: Q = Q { num: 1, den: 1 };

    /// Builds `num / den`, reducing; `None` if `den` is zero.
    pub fn new(num: i128, den: i128) -> Option<Q> {
        if den == 0 {
            return None;
        }
        let sign = if den < 0 { -1 } else { 1 };
        let g = gcd(num, den).max(1);
        Some(Q {
            num: sign * (num / g),
            den: sign * (den / g),
        })
    }

    pub fn int(value: i128) -> Q {
        Q { num: value, den: 1 }
    }

    /// The exact value of a decimal, if it fits.
    pub fn from_decimal(d: &Decimal) -> Option<Q> {
        if d.is_zero() {
            return Some(Q::ZERO);
        }
        let mut num: i128 = d.significand().parse().ok()?;
        if d.is_negative() {
            num = -num;
        }
        let exp = d.exponent();
        if exp >= 0 {
            num = num.checked_mul(pow10(u32::try_from(exp).ok()?)?)?;
            Some(Q::int(num))
        } else {
            let den = pow10(u32::try_from(-exp).ok()?)?;
            Q::new(num, den)
        }
    }

    /// The value as a decimal, if its denominator has no prime factors other
    /// than 2 and 5 (otherwise it has no finite decimal expansion).
    pub fn to_decimal(self) -> Option<Decimal> {
        let (mut twos, mut fives, mut rest) = (0u32, 0u32, self.den);
        while rest % 2 == 0 {
            rest /= 2;
            twos += 1;
        }
        while rest % 5 == 0 {
            rest /= 5;
            fives += 1;
        }
        if rest != 1 {
            return None;
        }
        let k = twos.max(fives);
        let scale = pow10(k)?.checked_div(self.den)?;
        let scaled = self.num.checked_mul(scale)?;
        Decimal::parse(&format!("{scaled}e-{k}")).ok()
    }

    pub fn is_integer(self) -> bool {
        self.den == 1
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn is_positive(self) -> bool {
        self.num > 0
    }

    pub fn checked_add(self, other: Q) -> Option<Q> {
        let num = self
            .num
            .checked_mul(other.den)?
            .checked_add(other.num.checked_mul(self.den)?)?;
        Q::new(num, self.den.checked_mul(other.den)?)
    }

    pub fn checked_sub(self, other: Q) -> Option<Q> {
        self.checked_add(Q {
            num: other.num.checked_neg()?,
            den: other.den,
        })
    }

    pub fn checked_mul(self, other: Q) -> Option<Q> {
        Q::new(
            self.num.checked_mul(other.num)?,
            self.den.checked_mul(other.den)?,
        )
    }

    pub fn checked_div(self, other: Q) -> Option<Q> {
        if other.num == 0 {
            return None;
        }
        Q::new(
            self.num.checked_mul(other.den)?,
            self.den.checked_mul(other.num)?,
        )
    }

    /// The largest integer not greater than `self`.
    pub fn floor(self) -> i128 {
        self.num.div_euclid(self.den)
    }

    /// The smallest integer not less than `self`.
    pub fn ceil(self) -> i128 {
        -((-self.num).div_euclid(self.den))
    }

    /// Whether `self` is an integer multiple of `step` (which must be
    /// positive). `None` on overflow.
    pub fn is_multiple_of(self, step: Q) -> Option<bool> {
        Some(self.checked_div(step)?.is_integer())
    }

    /// Exact comparison; `None` only on overflow.
    pub fn try_cmp(self, other: Q) -> Option<Ordering> {
        let left = self.num.checked_mul(other.den)?;
        let right = other.num.checked_mul(self.den)?;
        Some(left.cmp(&right))
    }

    /// The midpoint of `self` and `other`.
    pub fn midpoint(self, other: Q) -> Option<Q> {
        self.checked_add(other)?.checked_div(Q::int(2))
    }
}

impl fmt::Debug for Q {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn q(text: &str) -> Q {
        Q::from_decimal(&Decimal::parse(text).unwrap()).unwrap()
    }

    #[test]
    fn decimals_are_exact() {
        assert_eq!(q("0.1").checked_mul(Q::int(3)), Some(q("0.3")));
        assert_eq!(q("0.3").is_multiple_of(q("0.1")), Some(true));
        assert_eq!(q("0.35").is_multiple_of(q("0.1")), Some(false));
        assert_eq!(q("1e3"), Q::int(1000));
        assert_eq!(q("-2.50"), Q::new(-5, 2).unwrap());
    }

    #[test]
    fn floor_and_ceil_round_towards_infinities() {
        assert_eq!((q("2.5").floor(), q("2.5").ceil()), (2, 3));
        assert_eq!((q("-2.5").floor(), q("-2.5").ceil()), (-3, -2));
        assert_eq!((q("4").floor(), q("4").ceil()), (4, 4));
    }

    #[test]
    fn terminating_fractions_convert_back_to_decimals() {
        assert_eq!(
            Q::new(1, 8).unwrap().to_decimal().unwrap().to_string(),
            "0.125"
        );
        assert_eq!(
            Q::new(-7, 20).unwrap().to_decimal().unwrap().to_string(),
            "-0.35"
        );
        assert_eq!(Q::new(1, 3).unwrap().to_decimal(), None);
        assert_eq!(
            q("0.1")
                .midpoint(q("0.2"))
                .unwrap()
                .to_decimal()
                .unwrap()
                .to_string(),
            "0.15"
        );
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        let huge = Q::int(i128::MAX);
        assert_eq!(huge.checked_add(Q::ONE), None);
        assert_eq!(Q::from_decimal(&Decimal::parse("1e60").unwrap()), None);
        assert_eq!(huge.try_cmp(Q::new(1, i128::MAX).unwrap()), None);
    }

    proptest! {
        #[test]
        fn decimal_round_trip(int in -10_000_000i64..10_000_000, scale in 0u32..8) {
            let d = Decimal::parse(&format!("{int}e-{scale}")).unwrap();
            let value = Q::from_decimal(&d).unwrap();
            prop_assert_eq!(value.to_decimal().unwrap(), d);
        }

        #[test]
        fn ordering_agrees_with_decimals(a in -1_000_000i64..1_000_000, b in -1_000_000i64..1_000_000) {
            let (da, db) = (Decimal::parse(&format!("{a}e-3")).unwrap(), Decimal::parse(&format!("{b}e-3")).unwrap());
            let (qa, qb) = (Q::from_decimal(&da).unwrap(), Q::from_decimal(&db).unwrap());
            prop_assert_eq!(qa.try_cmp(qb), Some(da.cmp(&db)));
        }
    }
}
