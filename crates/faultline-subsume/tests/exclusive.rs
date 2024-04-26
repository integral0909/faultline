//! Exclusive member groups (Protobuf `oneof`), which JSON Schema cannot
//! express: objects are lowered from JSON Schema and the groups added to the
//! IR afterwards.

mod common;

use common::Schema;
use faultline_ir::{ExclusiveGroup, Literal, TypeKind};
use faultline_subsume::{Checker, Membership, Reason, Validator, Verdict};

const CONTACT: &str = r#"{
  "type": "object",
  "properties": {
    "email": {"type": "string"},
    "phone": {"type": "string"},
    "fax": {"type": "string"},
    "name": {"type": "string"}
  },
  "additionalProperties": false
}"#;

/// `CONTACT` with the given groups.
fn contact(groups: &[&[&str]]) -> Schema {
    let mut schema = Schema::new(CONTACT);
    let TypeKind::Object(obj) = &mut schema.api.types.get_mut(schema.root).kind else {
        panic!("the schema lowers to an object");
    };
    obj.exclusive = groups
        .iter()
        .enumerate()
        .map(|(i, fields)| ExclusiveGroup {
            name: format!("g{i}"),
            fields: fields.iter().map(|f| (*f).to_owned()).collect(),
            origin: None,
        })
        .collect();
    schema
}

fn value(json: &str) -> Literal {
    convert(&serde_json::from_str(json).unwrap())
}

fn convert(v: &serde_json::Value) -> Literal {
    match v {
        serde_json::Value::String(s) => Literal::string(s),
        serde_json::Value::Object(m) => {
            Literal::Object(m.iter().map(|(k, v)| (k.clone(), convert(v))).collect())
        }
        other => panic!("unsupported test value {other}"),
    }
}

fn verdict(old: &Schema, new: &Schema) -> Verdict {
    let mut checker = Checker::new(&old.api, &new.api);
    let verdict = checker.subsumes(old.root, new.root);
    if let Verdict::No(w) = &verdict {
        assert!(
            Validator::new(&old.api).validate(old.root, w).is_valid(),
            "witness {w} is not in the old type"
        );
        assert!(
            Validator::new(&new.api).validate(new.root, w).is_invalid(),
            "witness {w} is in the new type"
        );
    }
    verdict
}

#[test]
fn members_of_a_group_exclude_each_other() {
    let schema = contact(&[&["email", "phone"]]);
    let validator = Validator::new(&schema.api);
    assert!(
        validator
            .validate(schema.root, &value(r#"{"email": "a", "fax": "b"}"#))
            .is_valid()
    );
    assert_eq!(
        validator.validate(schema.root, &value(r#"{"email": "a", "phone": "b"}"#)),
        Membership::Invalid(Reason::ExclusiveMembers {
            group: "g0".into(),
            first: "email".into(),
            second: "phone".into(),
        })
    );
}

#[test]
fn adding_a_group_loses_values_and_removing_one_gains_them() {
    let plain = contact(&[]);
    let grouped = contact(&[&["email", "phone"]]);
    match verdict(&plain, &grouped) {
        Verdict::No(w) => {
            let Literal::Object(members) = &w else {
                panic!("{w}")
            };
            assert!(members.contains_key("email") && members.contains_key("phone"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(verdict(&grouped, &plain), Verdict::Yes);
}

#[test]
fn growing_a_group_loses_values_and_shrinking_it_does_not() {
    let small = contact(&[&["email", "phone"]]);
    let large = contact(&[&["email", "phone", "fax"]]);
    assert!(matches!(verdict(&small, &large), Verdict::No(_)));
    assert_eq!(verdict(&large, &small), Verdict::Yes);
}

#[test]
fn splitting_a_group_gains_values() {
    let one = contact(&[&["email", "phone", "fax"]]);
    let two = contact(&[&["email", "phone"], &["fax"]]);
    assert_eq!(verdict(&one, &two), Verdict::Yes);
    assert!(matches!(verdict(&two, &one), Verdict::No(_)));
}

#[test]
fn renaming_a_group_changes_no_values() {
    let a = contact(&[&["email", "phone"]]);
    let mut b = contact(&[&["email", "phone"]]);
    let TypeKind::Object(obj) = &mut b.api.types.get_mut(b.root).kind else {
        unreachable!()
    };
    obj.exclusive[0].name = "contact".into();
    assert_eq!(verdict(&a, &b), Verdict::Yes);
    assert_eq!(verdict(&b, &a), Verdict::Yes);
}
