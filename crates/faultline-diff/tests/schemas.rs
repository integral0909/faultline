//! Diffs of standalone JSON Schemas. Every witness is checked against an
//! independent JSON Schema validator.

use faultline_diff::{Change, ChangeKind, Evidence, Facet, diff};
use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, Literal, Polarity, Role};
use faultline_jsonschema::{Options, lower_document};
use faultline_source::{Limits, SourceMap};

struct Version {
    api: Api,
    oracle: jsonschema::Validator,
}

fn version(text: &str) -> Version {
    let loader = MemoryLoader::new().with("file:///s.json", text);
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    let lowered = lower_document(
        &UriRef::parse("file:///s.json").unwrap(),
        &mut store,
        Options::default(),
        Role::Input,
    );
    assert!(
        !lowered.diagnostics.has_errors(),
        "{text}: {:?}",
        lowered.diagnostics
    );
    let json: serde_json::Value = serde_json::from_str(text).unwrap();
    Version {
        api: lowered.api.unwrap(),
        oracle: jsonschema::draft202012::options()
            .should_validate_formats(false)
            .build(&json)
            .unwrap(),
    }
}

fn accepts(v: &Version, value: &Literal, at: &str) -> bool {
    // Witnesses are values *at* the change's position; wrap them back into a
    // document only for positions directly under the root.
    let value: serde_json::Value = serde_json::from_str(&value.to_string()).unwrap();
    let document = match at {
        "" => value,
        name => serde_json::json!({ name: value }),
    };
    v.oracle.is_valid(&document)
}

/// Diffs two schemas, checking every witness at the root or at a top-level
/// property against the oracle.
fn changes(old: &str, new: &str) -> Vec<Change> {
    let (a, b) = (version(old), version(new));
    let set = diff(&a.api, &b.api);
    for change in &set {
        let path = change.path.to_string();
        let at = match path.strip_prefix("schema[s]") {
            Some("") => "",
            Some(rest) if rest.starts_with('.') && !rest[1..].contains(['.', '[', '{', '|']) => {
                &rest[1..]
            }
            _ => continue,
        };
        if let Some(effect) = &change.effect {
            if let Evidence::Witness(w) = &effect.lost {
                assert!(
                    accepts(&a, w, at) && !accepts(&b, w, at),
                    "bad lost witness {w} for {change}"
                );
            }
            if let Evidence::Witness(w) = &effect.gained {
                assert!(
                    !accepts(&a, w, at) && accepts(&b, w, at),
                    "bad gained witness {w} for {change}"
                );
            }
        }
    }
    set.changes
}

fn summary(changes: &[Change]) -> Vec<String> {
    changes.iter().map(ToString::to_string).collect()
}

#[test]
fn identical_schemas_have_no_changes() {
    let s = r#"{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}"#;
    assert!(changes(s, s).is_empty());
}

#[test]
fn narrowed_bound_loses_values() {
    let c = changes(
        r#"{"type":"object","properties":{"name":{"type":"string","maxLength":64}}}"#,
        r#"{"type":"object","properties":{"name":{"type":"string","maxLength":32}}}"#,
    );
    assert_eq!(c.len(), 1, "{:?}", summary(&c));
    let change = &c[0];
    assert_eq!(change.path.to_string(), "schema[s].name");
    assert_eq!(change.polarity, Polarity::IN);
    assert_eq!(
        change.kind,
        ChangeKind::BoundChanged {
            facet: Facet::MaxLength,
            old: Some("64".into()),
            new: Some("32".into())
        }
    );
    let effect = change.effect.as_ref().unwrap();
    assert!(effect.lost.witness().is_some());
    assert_eq!(effect.gained, Evidence::None);
}

#[test]
fn each_narrowed_facet_gets_its_own_witness() {
    let c = changes(
        r#"{"type":"string","minLength":1,"maxLength":10}"#,
        r#"{"type":"string","minLength":3,"maxLength":5}"#,
    );
    assert_eq!(c.len(), 2, "{:?}", summary(&c));
    for change in &c {
        let Literal::String(w) = change
            .effect
            .as_ref()
            .unwrap()
            .lost
            .witness()
            .expect("witness")
            .clone()
        else {
            panic!()
        };
        match &change.kind {
            ChangeKind::BoundChanged {
                facet: Facet::MinLength,
                ..
            } => assert!(w.chars().count() < 3),
            ChangeKind::BoundChanged {
                facet: Facet::MaxLength,
                ..
            } => assert!(w.chars().count() > 5),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn required_members_and_closed_objects() {
    let c = changes(
        r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a"]}"#,
        r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a","b"],"additionalProperties":false}"#,
    );
    let kinds: Vec<&str> = c.iter().map(|x| x.kind.code()).collect();
    assert_eq!(
        kinds,
        ["field-required-changed", "type-changed"],
        "{:?}",
        summary(&c)
    );
    let Literal::Object(w) = c[0].effect.as_ref().unwrap().lost.witness().unwrap() else {
        panic!()
    };
    assert!(
        !w.contains_key("b"),
        "the witness omits the newly required member"
    );
    assert_eq!(
        c[1].path.to_string(),
        "schema[s]{*}",
        "closing the object changes additional members"
    );
}

#[test]
fn enum_values() {
    let c = changes(r#"{"enum":["a","b","c"]}"#, r#"{"enum":["a","b","d"]}"#);
    assert_eq!(
        summary(&c),
        [
            r#"enum-value-removed schema[s] [in] lost="c" gained=none"#,
            r#"enum-value-added schema[s] [in] lost=none gained="d""#,
        ]
    );
}

#[test]
fn nullable_becomes_a_variant_change() {
    let c = changes(
        r#"{"type":"object","properties":{"a":{"type":"string","maxLength":3}}}"#,
        r#"{"type":"object","properties":{"a":{"type":["string","null"],"maxLength":3}}}"#,
    );
    assert_eq!(
        summary(&c),
        ["variant-added schema[s].a [in] lost=none gained=null"]
    );
}

#[test]
fn type_changes_carry_evidence_both_ways() {
    let c = changes(r#"{"type":"string"}"#, r#"{"type":"integer"}"#);
    assert_eq!(c.len(), 1);
    let effect = c[0].effect.as_ref().unwrap();
    assert!(matches!(c[0].kind, ChangeKind::TypeChanged { .. }));
    assert!(effect.lost.witness().is_some() && effect.gained.witness().is_some());
}

#[test]
fn recursive_changes_are_reported_once() {
    let tree = |name: &str| {
        format!(
            r##"{{"type":"object","properties":{{"name":{name},"children":{{"type":"array","items":{{"$ref":"#"}}}}}}}}"##
        )
    };
    let c = changes(
        &tree(r#"{"type":"string"}"#),
        &tree(r#"{"type":"string","minLength":1}"#),
    );
    assert_eq!(c.len(), 1, "{:?}", summary(&c));
    assert_eq!(c[0].path.to_string(), "schema[s].name");
}

#[test]
fn widening_and_metadata() {
    let c = changes(
        r#"{"type":"integer","minimum":0,"maximum":10,"deprecated":false,"default":1}"#,
        r#"{"type":"number","minimum":-1,"maximum":10,"deprecated":true,"default":2}"#,
    );
    let kinds: Vec<&str> = c.iter().map(|x| x.kind.code()).collect();
    assert_eq!(
        kinds,
        [
            "deprecated-changed",
            "default-changed",
            "numeric-kind-changed",
            "bound-changed"
        ]
    );
    for change in c.iter().filter(|c| c.effect.is_some()) {
        let effect = change.effect.as_ref().unwrap();
        assert_eq!(effect.lost, Evidence::None, "{change}");
        assert!(effect.gained.witness().is_some(), "{change}");
    }
}

#[test]
fn unreachable_definitions_are_compared_without_polarity() {
    let c = changes(
        r#"{"$defs":{"Tag":{"type":"string","maxLength":5}}}"#,
        r#"{"$defs":{"Tag":{"type":"string","maxLength":4}}}"#,
    );
    assert_eq!(c.len(), 1, "{:?}", summary(&c));
    assert_eq!(c[0].path.to_string(), "type[Tag]");
    assert_eq!(c[0].polarity, Polarity::NONE);
}

mod properties {
    use proptest::prelude::*;
    use serde_json::{Value, json};

    fn schema() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(json!({"type": "string"})),
            (0u32..4).prop_map(|n| json!({"type": "string", "maxLength": n})),
            (-3i32..3).prop_map(|n| json!({"type": "integer", "minimum": n})),
            Just(json!({"enum": ["a", 1, null]})),
            Just(json!({"type": ["string", "null"]})),
        ];
        leaf.prop_recursive(3, 16, 3, |inner| {
            prop_oneof![
                (inner.clone(), inner.clone()).prop_map(|(a, b)| json!({"type": "object", "properties": {"a": a, "b": b}, "required": ["a"]})),
                inner.clone().prop_map(|i| json!({"type": "array", "items": i})),
                prop::collection::vec(inner.clone(), 2..3).prop_map(|v| json!({"oneOf": v})),
                prop::collection::vec(inner, 2..3).prop_map(|v| json!({"allOf": v})),
            ]
        })
    }

    proptest! {
        #[test]
        fn a_schema_does_not_differ_from_itself(s in schema()) {
            let text = s.to_string();
            let set = faultline_diff::diff(&super::version(&text).api, &super::version(&text).api);
            prop_assert!(set.is_empty(), "{text}: {:?}", set.changes);
        }
    }
}
