//! Matching and inclusion checked against an independent engine.
//!
//! Patterns are generated from a subset on which ECMA-262 and the `regex`
//! crate agree: literals, classes, groups, alternation, quantifiers and
//! anchors over a three-letter alphabet.

use faultline_regex::{Regex, difference};
use proptest::prelude::*;

fn pattern() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        Just("a".to_owned()),
        Just("b".to_owned()),
        Just("c".to_owned()),
        Just("[ab]".to_owned()),
        Just("[^a]".to_owned()),
        Just(".".to_owned()),
    ];
    let unit = atom.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("{a}{b}")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("(?:{a}|{b})")),
            inner.clone().prop_map(|a| format!("(?:{a})*")),
            inner.clone().prop_map(|a| format!("(?:{a})+")),
            inner.clone().prop_map(|a| format!("(?:{a})?")),
            (inner, 0u32..3, 0u32..3)
                .prop_map(|(a, lo, extra)| format!("(?:{a}){{{lo},{}}}", lo + extra)),
        ]
    });
    (any::<bool>(), unit, any::<bool>()).prop_map(|(start, body, end)| {
        format!(
            "{}{body}{}",
            if start { "^" } else { "" },
            if end { "$" } else { "" }
        )
    })
}

fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(prop_oneof![Just('a'), Just('b'), Just('c')], 0..8)
        .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #[test]
    fn matching_agrees_with_an_independent_engine(p in pattern(), t in text()) {
        let ours = Regex::new(&p).unwrap();
        let oracle = regex::Regex::new(&p).unwrap();
        prop_assert_eq!(ours.is_match(&t), oracle.is_match(&t), "pattern {} on {:?}", p, t);
    }

    #[test]
    fn differences_are_witnessed_and_absences_hold(a in pattern(), b in pattern(), probes in proptest::collection::vec(text(), 8)) {
        let (left, right) = (Regex::new(&a).unwrap(), Regex::new(&b).unwrap());
        let (oracle_left, oracle_right) = (regex::Regex::new(&a).unwrap(), regex::Regex::new(&b).unwrap());
        match difference(&left, &right, 0, None) {
            Some(witness) => {
                prop_assert!(oracle_left.is_match(&witness), "{} rejects its witness {:?}", a, witness);
                prop_assert!(!oracle_right.is_match(&witness), "{} accepts the witness {:?}", b, witness);
            }
            None => {
                for probe in probes {
                    prop_assert!(
                        !oracle_left.is_match(&probe) || oracle_right.is_match(&probe),
                        "{} is claimed included in {}, but {:?} refutes it", a, b, probe
                    );
                }
            }
        }
    }

    #[test]
    fn samples_match(p in pattern()) {
        let r = Regex::new(&p).unwrap();
        let oracle = regex::Regex::new(&p).unwrap();
        for sample in r.samples(5, 0, Some(10)) {
            prop_assert!(oracle.is_match(&sample), "{} rejects its sample {:?}", p, sample);
        }
    }
}
