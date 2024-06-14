//! Randomized checks against an independent JSON Schema validator.

mod common;

use common::Schema;
use faultline_ir::Literal;
use faultline_subsume::{Checker, Membership, Sampler, Validator, Verdict};
use proptest::prelude::*;
use serde_json::{Value, json};

/// Random schemas built from the constructs the checker reasons about.
fn schema() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(json!(true)),
        Just(json!({"type": "null"})),
        Just(json!({"type": "boolean"})),
        (-5i32..5, 0i32..6)
            .prop_map(|(lo, span)| json!({"type": "integer", "minimum": lo, "maximum": lo + span})),
        prop_oneof![Just(json!(0.5)), Just(json!(2)), Just(json!(0.25))]
            .prop_map(|m| json!({"type": "number", "multipleOf": m})),
        (0u32..3, 0u32..4).prop_map(
            |(lo, span)| json!({"type": "string", "minLength": lo, "maxLength": lo + span})
        ),
        prop_oneof![
            Just("^a"),
            Just("b$"),
            Just("^[ab]{2}$"),
            Just("ab"),
            Just("^a*b*$"),
            Just("^(a|bb)+$"),
            Just("[^a]"),
        ]
        .prop_map(|p| json!({"type": "string", "pattern": p})),
        prop::collection::vec(
            prop_oneof![
                Just(json!("a")),
                Just(json!(1)),
                Just(json!(null)),
                Just(json!("bb"))
            ],
            1..3
        )
        .prop_map(|values| {
            let mut unique: Vec<Value> = Vec::new();
            for v in values {
                if !unique.contains(&v) {
                    unique.push(v);
                }
            }
            json!({"enum": unique})
        }),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone(), any::<bool>(), any::<bool>()).prop_map(|(a, b, req, closed)| {
                let mut s = json!({"type": "object", "properties": {"a": a, "b": b}});
                if req {
                    s["required"] = json!(["a"]);
                }
                if closed {
                    s["additionalProperties"] = json!(false);
                }
                s
            }),
            (inner.clone(), 0u32..2, any::<bool>()).prop_map(|(items, min, unique)| {
                json!({"type": "array", "items": items, "minItems": min, "maxItems": min + 2, "uniqueItems": unique})
            }),
            prop::collection::vec(inner.clone(), 2..3).prop_map(|v| json!({"anyOf": v})),
            prop::collection::vec(inner.clone(), 2..3).prop_map(|v| json!({"oneOf": v})),
            prop::collection::vec(inner.clone(), 2..3).prop_map(|v| json!({"allOf": v})),
            inner.prop_map(|v| json!({"not": v})),
        ]
    })
}

/// Random JSON values of the shapes the schemas above talk about.
fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-8i32..8).prop_map(|i| json!(i)),
        prop_oneof![Just(json!(0.5)), Just(json!(0.25)), Just(json!(1.5))],
        "[ab]{0,6}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(
                prop_oneof![
                    Just("a".to_owned()),
                    Just("b".to_owned()),
                    Just("c".to_owned())
                ],
                inner,
                0..3
            )
            .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

fn literal(value: &Value) -> Literal {
    let map = faultline_source::SourceMap::new();
    let file = map
        .add(
            faultline_source::SourceName::Virtual("v".into()),
            value.to_string(),
        )
        .unwrap();
    let doc = faultline_doc::parse(
        &file,
        faultline_doc::Format::Json,
        &faultline_source::Limits::default(),
    )
    .document
    .unwrap();
    Literal::from(&doc.root)
}

proptest! {
    #[test]
    fn validator_agrees_with_an_independent_implementation(s in schema(), v in value()) {
        let schema = Schema::new(&s.to_string());
        let value = literal(&v);
        match Validator::new(&schema.api).validate(schema.root, &value) {
            Membership::Valid => prop_assert!(schema.accepts(&value), "{s} should accept {v}"),
            Membership::Invalid(reason) => prop_assert!(!schema.accepts(&value), "{s} should reject {v}: {reason:?}"),
            Membership::Unknown(_) => {}
        }
    }

    #[test]
    fn sampled_values_are_members(s in schema()) {
        let schema = Schema::new(&s.to_string());
        for candidate in Sampler::new(&schema.api).candidates(schema.root) {
            prop_assert!(schema.accepts(&candidate), "{s} rejects sample {candidate}");
        }
    }

    #[test]
    fn verdicts_are_sound(a in schema(), b in schema()) {
        let left = Schema::new(&a.to_string());
        let right = Schema::new(&b.to_string());
        let mut checker = Checker::new(&left.api, &right.api);
        match checker.subsumes(left.root, right.root) {
            Verdict::No(w) => {
                prop_assert!(left.accepts(&w), "witness {w} not in {a}");
                prop_assert!(!right.accepts(&w), "witness {w} is in {b}");
            }
            Verdict::Yes => {
                for candidate in Sampler::new(&left.api).candidates(left.root) {
                    prop_assert!(right.accepts(&candidate), "claimed {a} ⊆ {b}, but {candidate} is not in the right side");
                }
            }
            Verdict::Unknown(_) => {}
        }
    }

    #[test]
    fn a_schema_never_excludes_itself(s in schema()) {
        let left = Schema::new(&s.to_string());
        let right = Schema::new(&s.to_string());
        let mut checker = Checker::new(&left.api, &right.api);
        let verdict = checker.subsumes(left.root, right.root);
        prop_assert!(!matches!(verdict, Verdict::No(_)), "{s}: {verdict:?}");
    }
}
