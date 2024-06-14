//! Proof rules for numbers and strings.
//!
//! These rules only ever answer "proven included"; when they cannot, the
//! checker falls back to a counterexample search, so a rule that is too
//! conservative costs precision (an `Unknown`), never correctness.

use std::cmp::Ordering;

use faultline_ir::{Bound, NumericType, StringType};

use crate::rational::Q;

/// A bound as an exact value plus exclusivity.
type Edge = (Q, bool);

/// Exact arithmetic went out of range; the rule gives up.
struct Overflow;

/// Whether every number allowed by `a` (integers only if `a_int`) is allowed
/// by `b` (integers only if `b_int`).
pub(super) fn numeric_proven(a: &NumericType, a_int: bool, b: &NumericType, b_int: bool) -> bool {
    let (Ok(lower), Ok(upper)) = (effective_lower(a, a_int), effective_upper(a, a_int)) else {
        return false;
    };
    if is_empty(lower, upper) {
        return true;
    }
    if b_int && !a_int {
        // Every allowed value must be an integer: only a whole-number step
        // guarantees that.
        let integral_step = a
            .multiple_of
            .as_ref()
            .and_then(Q::from_decimal)
            .is_some_and(Q::is_integer);
        if !integral_step {
            return false;
        }
    }
    lower_within(lower, b.minimum.as_ref())
        && upper_within(upper, b.maximum.as_ref())
        && multiple_within(a, a_int, b)
}

/// Whether no number satisfies `a` (integers only if `integer`).
pub(super) fn numeric_empty(a: &NumericType, integer: bool) -> bool {
    match (effective_lower(a, integer), effective_upper(a, integer)) {
        (Ok(lower), Ok(upper)) => is_empty(lower, upper),
        _ => false,
    }
}

/// Whether no string satisfies `a`'s length bounds.
pub(super) fn string_empty(a: &StringType) -> bool {
    a.max_length.is_some_and(|max| max < a.min_length)
}

/// The smallest value `a` can take, after rounding up to integers or to the
/// step. `Ok(None)` means unbounded.
fn effective_lower(a: &NumericType, integer: bool) -> Result<Option<Edge>, Overflow> {
    let Some(bound) = &a.minimum else {
        return Ok(None);
    };
    let mut value = Q::from_decimal(&bound.value).ok_or(Overflow)?;
    let mut exclusive = bound.exclusive;
    if integer {
        value = Q::int(if exclusive {
            value.floor() + 1
        } else {
            value.ceil()
        });
        exclusive = false;
    }
    if let Some(step) = a.multiple_of.as_ref().and_then(Q::from_decimal) {
        let k = value.checked_div(step).ok_or(Overflow)?;
        let mut n = k.ceil();
        if exclusive && k.is_integer() {
            n += 1;
        }
        value = step.checked_mul(Q::int(n)).ok_or(Overflow)?;
        exclusive = false;
    }
    Ok(Some((value, exclusive)))
}

/// The largest value `a` can take; see [`effective_lower`].
fn effective_upper(a: &NumericType, integer: bool) -> Result<Option<Edge>, Overflow> {
    let Some(bound) = &a.maximum else {
        return Ok(None);
    };
    let mut value = Q::from_decimal(&bound.value).ok_or(Overflow)?;
    let mut exclusive = bound.exclusive;
    if integer {
        value = Q::int(if exclusive {
            value.ceil() - 1
        } else {
            value.floor()
        });
        exclusive = false;
    }
    if let Some(step) = a.multiple_of.as_ref().and_then(Q::from_decimal) {
        let k = value.checked_div(step).ok_or(Overflow)?;
        let mut n = k.floor();
        if exclusive && k.is_integer() {
            n -= 1;
        }
        value = step.checked_mul(Q::int(n)).ok_or(Overflow)?;
        exclusive = false;
    }
    Ok(Some((value, exclusive)))
}

fn is_empty(lower: Option<Edge>, upper: Option<Edge>) -> bool {
    let (Some((lo, lo_ex)), Some((hi, hi_ex))) = (lower, upper) else {
        return false;
    };
    match lo.try_cmp(hi) {
        Some(Ordering::Greater) => true,
        Some(Ordering::Equal) => lo_ex || hi_ex,
        _ => false,
    }
}

/// Whether a left lower edge satisfies a right minimum.
fn lower_within(left: Option<Edge>, right: Option<&Bound>) -> bool {
    let Some(right) = right else {
        return true;
    };
    let (Some((value, exclusive)), Some(limit)) = (left, Q::from_decimal(&right.value)) else {
        return false;
    };
    match value.try_cmp(limit) {
        Some(Ordering::Greater) => true,
        Some(Ordering::Equal) => !right.exclusive || exclusive,
        _ => false,
    }
}

/// Whether a left upper edge satisfies a right maximum.
fn upper_within(left: Option<Edge>, right: Option<&Bound>) -> bool {
    let Some(right) = right else {
        return true;
    };
    let (Some((value, exclusive)), Some(limit)) = (left, Q::from_decimal(&right.value)) else {
        return false;
    };
    match value.try_cmp(limit) {
        Some(Ordering::Less) => true,
        Some(Ordering::Equal) => !right.exclusive || exclusive,
        _ => false,
    }
}

/// Whether every value of `a` is a multiple of `b`'s step.
fn multiple_within(a: &NumericType, a_int: bool, b: &NumericType) -> bool {
    let Some(required) = b.multiple_of.as_ref().and_then(Q::from_decimal) else {
        return b.multiple_of.is_none();
    };
    if let Some(step) = a.multiple_of.as_ref().and_then(Q::from_decimal)
        && step.is_multiple_of(required) == Some(true)
    {
        return true;
    }
    // Every integer is a multiple of `required` exactly when 1 is.
    a_int && Q::ONE.is_multiple_of(required) == Some(true)
}

/// Whether every string allowed by `a` is allowed by `b`. Patterns are only
/// compared textually here.
/// Whether every length `a` allows, `b` allows.
pub(super) fn lengths_proven(a: &StringType, b: &StringType) -> bool {
    a.min_length >= b.min_length
        && b.max_length
            .is_none_or(|b_max| a.max_length.is_some_and(|a_max| a_max <= b_max))
}

/// `a`'s length bounds for searching its pattern language.
pub(super) fn length_window(a: &StringType) -> (usize, Option<usize>) {
    (
        usize::try_from(a.min_length).unwrap_or(usize::MAX),
        a.max_length.and_then(|m| usize::try_from(m).ok()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use faultline_doc::Decimal;

    fn d(s: &str) -> Decimal {
        Decimal::parse(s).unwrap()
    }

    fn range(
        min: Option<(&str, bool)>,
        max: Option<(&str, bool)>,
        step: Option<&str>,
    ) -> NumericType {
        let bound = |(v, ex): (&str, bool)| Bound {
            value: d(v),
            exclusive: ex,
        };
        NumericType {
            minimum: min.map(bound),
            maximum: max.map(bound),
            multiple_of: step.map(d),
            format: None,
            encoding: None,
        }
    }

    #[test]
    fn integer_rounding_tightens_bounds() {
        // Integers above 0.5 start at 1.
        let left = range(Some(("0.5", true)), None, None);
        let right = range(Some(("1", false)), None, None);
        assert!(numeric_proven(&left, true, &right, false));
        assert!(!numeric_proven(&left, false, &right, false));
    }

    #[test]
    fn steps_tighten_bounds() {
        // Multiples of 5 up to 12 are at most 10.
        let left = range(None, Some(("12", false)), Some("5"));
        let right = range(None, Some(("10", false)), None);
        assert!(numeric_proven(&left, false, &right, false));
        let right = range(None, Some(("10", true)), None);
        assert!(!numeric_proven(&left, false, &right, false));
    }

    #[test]
    fn exclusivity_matters_at_equal_bounds() {
        let inclusive = range(Some(("0", false)), None, None);
        let exclusive = range(Some(("0", true)), None, None);
        assert!(numeric_proven(&exclusive, false, &inclusive, false));
        assert!(!numeric_proven(&inclusive, false, &exclusive, false));
    }

    #[test]
    fn multiples_compose() {
        let tenths = range(None, None, Some("0.1"));
        let fifths = range(None, None, Some("0.2"));
        assert!(numeric_proven(&fifths, false, &tenths, false));
        assert!(!numeric_proven(&tenths, false, &fifths, false));
        let halves = range(None, None, Some("0.5"));
        assert!(
            numeric_proven(&range(None, None, None), true, &halves, false),
            "integers are multiples of 0.5"
        );
        assert!(numeric_proven(
            &range(None, None, Some("2")),
            false,
            &range(None, None, None),
            true
        ));
    }

    #[test]
    fn empty_ranges_are_included_in_anything() {
        let empty = range(Some(("5", false)), Some(("4", false)), None);
        let narrow = range(Some(("100", false)), Some(("101", false)), None);
        assert!(numeric_proven(&empty, false, &narrow, true));
        let no_integers = range(Some(("0.2", false)), Some(("0.8", false)), None);
        assert!(numeric_proven(&no_integers, true, &narrow, false));
    }

    #[test]
    fn string_lengths() {
        let s = |min: u64, max: Option<u64>| StringType {
            min_length: min,
            max_length: max,
            pattern: None,
            format: None,
        };
        assert!(lengths_proven(&s(2, Some(5)), &s(1, Some(8))));
        assert!(!lengths_proven(&s(0, None), &s(0, Some(8))));
        assert!(string_empty(&s(5, Some(2))));
        assert_eq!(length_window(&s(3, Some(7))), (3, Some(7)));
    }
}
