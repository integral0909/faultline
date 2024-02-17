//! Exact decimal numbers.
//!
//! Schema documents use numbers as constraints (`maximum`, `multipleOf`,
//! enum values), and a compatibility checker must not report "unchanged" for
//! `0.1` vs `0.10000000000000001` just because both round to the same `f64`.
//! Numbers are therefore kept as exact decimals in a canonical form that makes
//! equality and ordering exact without arbitrary-precision arithmetic.

use std::cmp::Ordering;
use std::fmt;

/// Largest accepted magnitude of a (normalized) decimal exponent. Real schemas
/// stay within `f64` range (about ±308); the ceiling only exists so that
/// pathological input cannot make formatting allocate unboundedly.
pub const MAX_ABS_EXPONENT: i64 = 1_000_000;

/// A finite decimal number `±digits × 10^exponent` in canonical form.
///
/// Invariants: `digits` holds ASCII digits with no leading or trailing zeros;
/// zero is represented by empty `digits`, exponent 0 and `negative == false`.
/// Because the form is canonical, derived equality and hashing are numeric
/// equality: `1`, `1.0`, `10e-1` and `0.1e1` are the same value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Decimal {
    negative: bool,
    digits: Box<str>,
    exponent: i64,
}

/// Length of a digit string as `i64`. Strings come from source files, which
/// are far smaller than `i64::MAX` bytes.
fn len_i64(digits: &str) -> i64 {
    i64::try_from(digits.len()).unwrap_or(i64::MAX)
}

/// Why a string could not be read as a decimal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecimalError {
    Syntax,
    ExponentOutOfRange,
}

impl fmt::Display for DecimalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecimalError::Syntax => f.write_str("invalid number"),
            DecimalError::ExponentOutOfRange => f.write_str("number exponent is out of range"),
        }
    }
}

impl std::error::Error for DecimalError {}

impl Decimal {
    pub fn zero() -> Decimal {
        Decimal {
            negative: false,
            digits: Box::default(),
            exponent: 0,
        }
    }

    /// Parses `[+-]? (D+ ('.' D*)? | '.' D+) ([eE] [+-]? D+)?`.
    ///
    /// This is a superset of the JSON number grammar that also covers YAML 1.2
    /// core-schema floats; callers that need the stricter JSON grammar validate
    /// it themselves before calling.
    pub fn parse(text: &str) -> Result<Decimal, DecimalError> {
        let bytes = text.as_bytes();
        let mut i = 0;
        let negative = match bytes.first() {
            Some(b'-') => {
                i += 1;
                true
            }
            Some(b'+') => {
                i += 1;
                false
            }
            _ => false,
        };

        let int_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let int_part = &text[int_start..i];
        let mut frac_part = "";
        if bytes.get(i) == Some(&b'.') {
            i += 1;
            let frac_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            frac_part = &text[frac_start..i];
        }
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(DecimalError::Syntax);
        }

        let mut exponent: i64 = 0;
        if matches!(bytes.get(i), Some(b'e' | b'E')) {
            i += 1;
            let exp_negative = match bytes.get(i) {
                Some(b'-') => {
                    i += 1;
                    true
                }
                Some(b'+') => {
                    i += 1;
                    false
                }
                _ => false,
            };
            let exp_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                // Saturate instead of overflowing; the range check below
                // rejects anything this large unless the mantissa is zero.
                exponent = exponent
                    .saturating_mul(10)
                    .saturating_add(i64::from(bytes[i] - b'0'));
                i += 1;
            }
            if i == exp_start {
                return Err(DecimalError::Syntax);
            }
            if exp_negative {
                exponent = -exponent;
            }
        }
        if i != bytes.len() {
            return Err(DecimalError::Syntax);
        }

        let mantissa = format!("{int_part}{frac_part}");
        let frac_len =
            i64::try_from(frac_part.len()).map_err(|_| DecimalError::ExponentOutOfRange)?;
        Self::from_parts(negative, &mantissa, exponent.saturating_sub(frac_len))
    }

    /// Builds a canonical decimal from a sign, ASCII digit string (which may
    /// have leading/trailing zeros) and exponent.
    fn from_parts(negative: bool, digits: &str, exponent: i64) -> Result<Decimal, DecimalError> {
        let trimmed_leading = digits.trim_start_matches('0');
        let significant = trimmed_leading.trim_end_matches('0');
        if significant.is_empty() {
            return Ok(Decimal::zero());
        }
        let trailing = len_i64(trimmed_leading) - len_i64(significant);
        let exponent = exponent.saturating_add(trailing);
        // Check the position of the most significant digit as well as the raw
        // exponent, so that e.g. 1e-999999 with a million digits is rejected.
        let magnitude = exponent.saturating_add(len_i64(significant));
        // `unsigned_abs`: the exponent may have saturated to i64::MIN, whose
        // `abs` overflows.
        let limit = MAX_ABS_EXPONENT.unsigned_abs();
        if exponent.unsigned_abs() > limit || magnitude.unsigned_abs() > limit {
            return Err(DecimalError::ExponentOutOfRange);
        }
        Ok(Decimal {
            negative,
            digits: significant.into(),
            exponent,
        })
    }

    pub fn from_i128(value: i128) -> Decimal {
        let negative = value < 0;
        let digits = value.unsigned_abs().to_string();
        Self::from_parts(negative, &digits, 0).expect("integers are always in range")
    }

    pub fn from_u128(value: u128) -> Decimal {
        Self::from_parts(false, &value.to_string(), 0).expect("integers are always in range")
    }

    pub fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// Whether the value has no fractional part. `1.0` is an integer, matching
    /// JSON Schema 2019-09 and later.
    pub fn is_integer(&self) -> bool {
        self.is_zero() || self.exponent >= 0
    }

    /// The significant digits, without leading or trailing zeros.
    pub fn significand(&self) -> &str {
        &self.digits
    }

    /// The power of ten applied to [`significand`](Self::significand).
    pub fn exponent(&self) -> i64 {
        self.exponent
    }

    /// Position of the decimal point relative to the first significant digit:
    /// the value lies in `[10^(m-1), 10^m)` where `m` is the result.
    fn magnitude(&self) -> i64 {
        len_i64(&self.digits) + self.exponent
    }

    pub fn to_i128(&self) -> Option<i128> {
        if !self.is_integer() {
            return None;
        }
        if self.is_zero() {
            return Some(0);
        }
        // i128 holds 38 full decimal digits.
        if self.magnitude() > 38 {
            return None;
        }
        let mut value: i128 = self.digits.parse().ok()?;
        for _ in 0..self.exponent {
            value = value.checked_mul(10)?;
        }
        Some(if self.negative { -value } else { value })
    }

    pub fn to_i64(&self) -> Option<i64> {
        self.to_i128().and_then(|v| i64::try_from(v).ok())
    }

    pub fn to_u64(&self) -> Option<u64> {
        self.to_i128().and_then(|v| u64::try_from(v).ok())
    }

    /// The nearest `f64`, rounding as the standard library's float parser does.
    /// Values beyond `f64` range become infinities.
    pub fn to_f64(&self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        let sign = if self.negative { "-" } else { "" };
        format!("{sign}{}e{}", self.digits, self.exponent)
            .parse()
            .unwrap_or(f64::NAN)
    }

    #[must_use]
    pub fn neg(&self) -> Decimal {
        if self.is_zero() {
            return self.clone();
        }
        Decimal {
            negative: !self.negative,
            ..self.clone()
        }
    }

    #[must_use]
    pub fn abs(&self) -> Decimal {
        Decimal {
            negative: false,
            ..self.clone()
        }
    }

    fn cmp_magnitude(&self, other: &Decimal) -> Ordering {
        match (self.is_zero(), other.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        // With no leading zeros, a larger magnitude means a larger number;
        // with equal magnitudes and no trailing zeros, the digit strings
        // compare lexicographically exactly as the numbers do.
        self.magnitude()
            .cmp(&other.magnitude())
            .then_with(|| self.digits.cmp(&other.digits))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.cmp_magnitude(other),
            (true, true) => other.cmp_magnitude(self),
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Decimal {
    /// Formats as a valid JSON number: plain integers and short decimals are
    /// written positionally, everything else in scientific notation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            return f.write_str("0");
        }
        if self.negative {
            f.write_str("-")?;
        }
        let len = len_i64(&self.digits);
        if (0..=15).contains(&self.exponent) {
            f.write_str(&self.digits)?;
            for _ in 0..self.exponent {
                f.write_str("0")?;
            }
            Ok(())
        } else if self.exponent < 0 && -self.exponent <= len + 6 {
            let point = len + self.exponent;
            if point > 0 {
                let (int, frac) = self.digits.split_at(point.unsigned_abs() as usize);
                write!(f, "{int}.{frac}")
            } else {
                f.write_str("0.")?;
                for _ in 0..-point {
                    f.write_str("0")?;
                }
                f.write_str(&self.digits)
            }
        } else {
            write!(f, "{}e{}", self.digits, self.exponent)
        }
    }
}

impl fmt::Debug for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A number as it appears in a document.
///
/// JSON only has finite numbers; YAML adds `.inf`, `-.inf` and `.nan`. `NaN`
/// compares equal to itself here because this type models document content,
/// not IEEE arithmetic.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Number {
    Finite(Decimal),
    PosInfinity,
    NegInfinity,
    NaN,
}

impl Number {
    pub fn as_decimal(&self) -> Option<&Decimal> {
        match self {
            Number::Finite(d) => Some(d),
            _ => None,
        }
    }

    pub fn to_f64(&self) -> f64 {
        match self {
            Number::Finite(d) => d.to_f64(),
            Number::PosInfinity => f64::INFINITY,
            Number::NegInfinity => f64::NEG_INFINITY,
            Number::NaN => f64::NAN,
        }
    }
}

impl From<Decimal> for Number {
    fn from(d: Decimal) -> Self {
        Number::Finite(d)
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Number::Finite(d) => fmt::Display::fmt(d, f),
            Number::PosInfinity => f.write_str(".inf"),
            Number::NegInfinity => f.write_str("-.inf"),
            Number::NaN => f.write_str(".nan"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn d(s: &str) -> Decimal {
        Decimal::parse(s).unwrap()
    }

    #[test]
    fn equivalent_spellings_are_equal() {
        for s in ["1", "1.0", "10e-1", "0.1e1", "+1", "1.", "001.000"] {
            assert_eq!(d(s), d("1"), "{s}");
        }
        assert_eq!(d("-0"), Decimal::zero());
        assert_eq!(d("0.000e5"), Decimal::zero());
        assert_eq!(d(".5"), d("0.5"));
    }

    #[test]
    fn precision_is_not_lost() {
        assert_ne!(d("0.1"), d("0.10000000000000001"));
        assert!(d("0.1") < d("0.10000000000000001"));
        assert_ne!(d("9007199254740993"), d("9007199254740992"));
    }

    #[test]
    fn ordering_handles_signs_and_magnitudes() {
        let mut values: Vec<_> = [
            "-10", "-1.5", "-1", "0", "0.001", "0.01", "1", "1.5", "15", "100",
        ]
        .iter()
        .map(|s| d(s))
        .collect();
        let sorted = values.clone();
        values.reverse();
        values.sort();
        assert_eq!(values, sorted);
    }

    #[test]
    fn integer_detection_and_conversion() {
        assert!(d("1.0").is_integer());
        assert!(d("1e3").is_integer());
        assert!(!d("1.5").is_integer());
        assert_eq!(d("1e3").to_i64(), Some(1000));
        assert_eq!(d("-12.00").to_i64(), Some(-12));
        assert_eq!(d("1.5").to_i64(), None);
        assert_eq!(d("18446744073709551615").to_u64(), Some(u64::MAX));
        assert_eq!(d("18446744073709551616").to_u64(), None);
        assert_eq!(d("1e40").to_i128(), None);
    }

    #[test]
    fn syntax_errors() {
        for s in [
            "", "-", ".", "e5", "1e", "1e+", "1.2.3", "0x10", "1 ", "--1", "inf",
        ] {
            assert_eq!(Decimal::parse(s), Err(DecimalError::Syntax), "{s:?}");
        }
    }

    #[test]
    fn huge_exponents_are_rejected_unless_zero() {
        assert_eq!(
            Decimal::parse("1e9999999"),
            Err(DecimalError::ExponentOutOfRange)
        );
        assert_eq!(
            Decimal::parse("1e-9999999"),
            Err(DecimalError::ExponentOutOfRange)
        );
        assert_eq!(
            Decimal::parse("0e9999999999999999999999"),
            Ok(Decimal::zero())
        );
    }

    #[test]
    fn saturated_exponents_do_not_overflow() {
        // Found by fuzzing: the exponent saturates to i64::MIN.
        let text = format!("54.{}E-{}", "9".repeat(40), "9".repeat(30));
        assert_eq!(Decimal::parse(&text), Err(DecimalError::ExponentOutOfRange));
        let text = format!("5E{}", "9".repeat(30));
        assert_eq!(Decimal::parse(&text), Err(DecimalError::ExponentOutOfRange));
    }

    #[test]
    fn display_is_compact_and_valid_json() {
        let cases = [
            ("0", "0"),
            ("-0.0", "0"),
            ("100", "100"),
            ("1e15", "1000000000000000"),
            ("1e16", "1e16"),
            ("-1.25", "-1.25"),
            ("0.001", "0.001"),
            ("1.5e-8", "15e-9"),
            ("123.456e-2", "1.23456"),
        ];
        for (input, expected) in cases {
            assert_eq!(d(input).to_string(), expected, "{input}");
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact results are the point of the test
    fn f64_conversion_rounds_like_std() {
        assert_eq!(d("0.1").to_f64(), 0.1);
        assert_eq!(d("-2.5e3").to_f64(), -2500.0);
        assert!(d("1e400").to_f64().is_infinite());
    }

    proptest! {
        #[test]
        fn display_round_trips(neg in any::<bool>(), int in "[0-9]{1,20}", frac in "[0-9]{0,20}", exp in -400i64..400) {
            let text = format!("{}{int}.{frac}e{exp}", if neg { "-" } else { "" });
            let value = d(&text);
            prop_assert_eq!(d(&value.to_string()), value);
        }

        #[test]
        fn ordering_agrees_with_integers(a in any::<i64>(), b in any::<i64>()) {
            let (da, db) = (Decimal::from_i128(a.into()), Decimal::from_i128(b.into()));
            prop_assert_eq!(da.cmp(&db), a.cmp(&b));
            prop_assert_eq!(da.to_i64(), Some(a));
        }

        #[test]
        fn ordering_agrees_with_f64_when_exact(a in -1e6f64..1e6, b in -1e6f64..1e6) {
            // Values printed with {:?} round-trip exactly, so the decimal
            // comparison must agree with the float comparison.
            let (da, db) = (d(&format!("{a:?}")), d(&format!("{b:?}")));
            prop_assert_eq!(da.cmp(&db), a.partial_cmp(&b).unwrap());
        }
    }
}
