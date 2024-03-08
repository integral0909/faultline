//! Keyword semantics: how each JSON Schema construct lowers into the IR.

mod common;

use common::{lower, lower_with};
use faultline_ir::{Access, TypeKind, UnionMode};
use faultline_jsonschema::{Dialect, Options};

#[test]
fn object_schema_dump() {
    let out = lower(&[(
        "pet.json",
        r##"{
      "type": "object",
      "required": ["name", "kind"],
      "properties": {
        "name": {"type": "string", "maxLength": 64},
        "age": {"type": ["integer", "null"], "minimum": 0},
        "tags": {"type": "array", "items": {"$ref": "#/$defs/Tag"}},
        "status": {"type": "string", "enum": ["available", "sold"]}
      },
      "additionalProperties": false,
      "$defs": {
        "Tag": {"type": "object", "properties": {"label": {"type": "string"}}},
        "Unused": {"const": 3}
      }
    }"##,
    )]);
    out.assert_clean();
    let expected = "\
api pet (jsonschema)

schema[pet] [in] -> pet

pet [in]: object additional=t10
  .name required: t1
  .age: t2
  .tags: t5
  .status: t9
  .kind required: t10
t1 [in]: string maxLength=64
t2 [in]: union any [t3, t4]
t3 [in]: null
t4 [in]: integer min=0
t5 [in]: array items=t6
t6 [in]: ref Tag
Tag [in]: object additional=t8
  .label: t7
t7 [in]: string
t8 [in]: any
t9 [in]: enum
  = \"available\"
  = \"sold\"
t10 [in]: never
Unused [none]: enum
  = 3
";
    assert_eq!(out.dump(), expected, "\n{}", out.dump());
}

#[test]
fn keywords_without_type_constrain_only_their_own_type() {
    let out = lower(&[("s.json", r#"{"maxLength": 3}"#)]);
    let TypeKind::Union(u) = out.named("s") else {
        panic!("{}", out.dump())
    };
    let kinds: Vec<&str> = u.variants.iter().map(|&v| out.kind(v).describe()).collect();
    assert_eq!(
        kinds,
        ["null", "boolean", "object", "array", "number", "string"]
    );
    // Only the string variant carries the constraint.
    let TypeKind::String(s) = out.kind(u.variants[5]) else {
        panic!()
    };
    assert_eq!(s.max_length, Some(3));

    let out = lower(&[(
        "s.json",
        r#"{"title": "anything", "description": "no assertions"}"#,
    )]);
    assert_eq!(out.named("s"), &TypeKind::Any);
}

#[test]
fn integer_is_folded_into_number() {
    let out = lower(&[(
        "s.json",
        r#"{"type": ["integer", "number"], "multipleOf": 0.5}"#,
    )]);
    let TypeKind::Number(n) = out.named("s") else {
        panic!("{}", out.dump())
    };
    assert_eq!(n.multiple_of.as_ref().unwrap().to_string(), "0.5");
}

#[test]
fn enums_keep_the_typed_part_only_when_it_constrains() {
    let plain = lower(&[("s.json", r#"{"type": "string", "enum": ["a", "b"]}"#)]);
    assert!(matches!(plain.named("s"), TypeKind::Enum(_)));

    let constrained = lower(&[(
        "s.json",
        r#"{"type": "string", "maxLength": 1, "enum": ["a", "bb"]}"#,
    )]);
    let TypeKind::AllOf(parts) = constrained.named("s") else {
        panic!("{}", constrained.dump())
    };
    assert_eq!(parts.len(), 2);

    // A value of another type is not covered by `type`, so both parts matter.
    let mismatch = lower(&[("s.json", r#"{"type": "string", "enum": ["a", 1]}"#)]);
    assert!(matches!(mismatch.named("s"), TypeKind::AllOf(_)));

    let with_const = lower(&[("s.json", r#"{"const": {"b": 1, "a": [true]}}"#)]);
    let TypeKind::Enum(e) = with_const.named("s") else {
        panic!()
    };
    assert_eq!(e.values[0].value.to_string(), r#"{"a":[true],"b":1}"#);
}

#[test]
fn exclusive_bounds_per_dialect() {
    let draft4 = lower(&[(
        "s.json",
        r#"{
        "$schema": "http://json-schema.org/draft-04/schema#",
        "type": "number", "minimum": 1, "exclusiveMinimum": true, "maximum": 9
    }"#,
    )]);
    draft4.assert_clean();
    let TypeKind::Number(n) = draft4.named("s") else {
        panic!()
    };
    assert!(n.minimum.as_ref().unwrap().exclusive);
    assert!(!n.maximum.as_ref().unwrap().exclusive);

    let later = lower(&[(
        "s.json",
        r#"{"type": "integer", "minimum": 5, "exclusiveMinimum": 5, "maximum": 10, "exclusiveMaximum": 12}"#,
    )]);
    let TypeKind::Integer(n) = later.named("s") else {
        panic!()
    };
    let min = n.minimum.as_ref().unwrap();
    let max = n.maximum.as_ref().unwrap();
    assert_eq!(
        (min.value.to_string().as_str(), min.exclusive),
        ("5", true),
        "tighter bound wins"
    );
    assert_eq!(
        (max.value.to_string().as_str(), max.exclusive),
        ("10", false)
    );
}

#[test]
fn arrays_in_each_tuple_style() {
    let modern = lower(&[(
        "s.json",
        r#"{"type": "array", "prefixItems": [{"type": "string"}], "items": false, "contains": {"type": "null"}, "minContains": 2}"#,
    )]);
    let TypeKind::Array(a) = modern.named("s") else {
        panic!()
    };
    assert_eq!(a.prefix_items.len(), 1);
    assert_eq!(modern.kind(a.items), &TypeKind::Never);
    assert_eq!(a.contains.as_ref().unwrap().min, 2);

    let draft7 = lower_with(
        &[(
            "s.json",
            r#"{"type": "array", "items": [{"type": "string"}, {"type": "integer"}], "additionalItems": {"type": "boolean"}, "uniqueItems": true}"#,
        )],
        Options {
            default_dialect: Dialect::Draft7,
        },
    );
    let TypeKind::Array(a) = draft7.named("s") else {
        panic!()
    };
    assert_eq!(a.prefix_items.len(), 2);
    assert_eq!(draft7.kind(a.items), &TypeKind::Boolean);
    assert!(a.unique_items);
}

#[test]
fn draft4_allows_boolean_additional_properties() {
    let out = lower(&[(
        "s.json",
        r#"{
        "$schema": "http://json-schema.org/draft-04/schema#",
        "type": "object", "additionalProperties": false
    }"#,
    )]);
    out.assert_clean();
    let TypeKind::Object(o) = out.named("s") else {
        panic!()
    };
    assert_eq!(out.kind(o.additional), &TypeKind::Never);
}

#[test]
fn combinators_and_negation() {
    let out = lower(&[(
        "s.json",
        r#"{
        "allOf": [{"type": "object"}, {"required": ["id"]}],
        "anyOf": [{"type": "object"}, {"type": "null"}],
        "oneOf": [{"minProperties": 1}, {"maxProperties": 0}],
        "not": {"type": "array"}
    }"#,
    )]);
    out.assert_clean();
    let TypeKind::AllOf(parts) = out.named("s") else {
        panic!("{}", out.dump())
    };
    let kinds: Vec<String> = parts
        .iter()
        .map(|&p| match out.kind(p) {
            TypeKind::Union(u) => format!("union-{:?}", u.mode),
            other => other.describe().to_owned(),
        })
        .collect();
    assert_eq!(
        kinds,
        ["object", "union-Any", "union-Any", "union-Exclusive", "not"]
    );
}

#[test]
fn conditionals_become_unions() {
    let out = lower(&[(
        "s.json",
        r#"{
        "if": {"properties": {"kind": {"const": "cat"}}},
        "then": {"required": ["meows"]}
    }"#,
    )]);
    out.assert_clean();
    let TypeKind::Union(u) = out.named("s") else {
        panic!("{}", out.dump())
    };
    assert_eq!(u.mode, UnionMode::Any);
    let TypeKind::AllOf(when_false) = out.kind(u.variants[1]) else {
        panic!()
    };
    assert!(matches!(out.kind(when_false[0]), TypeKind::Not(_)));
    assert_eq!(
        out.kind(when_false[1]),
        &TypeKind::Any,
        "missing else accepts everything"
    );

    // `if` without `then` or `else` has no effect.
    let out = lower(&[("s.json", r#"{"if": {"type": "string"}}"#)]);
    assert_eq!(out.named("s"), &TypeKind::Any);
}

#[test]
fn dependencies_split_into_required_and_schemas() {
    let out = lower_with(
        &[(
            "s.json",
            r#"{"type": "object", "dependencies": {"card": ["billing"], "vip": {"required": ["tier"]}}}"#,
        )],
        Options {
            default_dialect: Dialect::Draft7,
        },
    );
    out.assert_clean();
    let TypeKind::AllOf(parts) = out.named("s") else {
        panic!("{}", out.dump())
    };
    let TypeKind::Object(o) = out.kind(parts[0]) else {
        panic!()
    };
    assert_eq!(
        o.dependent_required,
        [("card".to_owned(), vec!["billing".to_owned()])]
    );
    let TypeKind::Union(u) = out.kind(parts[1]) else {
        panic!()
    };
    let TypeKind::Not(present) = out.kind(u.variants[0]) else {
        panic!()
    };
    let TypeKind::Object(present) = out.kind(*present) else {
        panic!()
    };
    assert_eq!(present.fields[0].name, "vip");
    assert!(present.fields[0].required);
}

#[test]
fn metadata_is_preserved() {
    let out = lower(&[(
        "s.json",
        r#"{
        "type": "object",
        "properties": {
            "id": {"type": "string", "readOnly": true, "deprecated": true, "default": "x", "x-internal": {"owner": "core"}},
            "secret": {"type": "string", "writeOnly": true}
        }
    }"#,
    )]);
    out.assert_clean();
    let api = out.api();
    let TypeKind::Object(o) = out.named("s") else {
        panic!()
    };
    let id = api.types.get(o.fields[0].ty);
    assert_eq!(id.meta.access, Access::ReadOnly);
    assert!(id.meta.deprecated);
    assert_eq!(id.meta.default.as_ref().unwrap().to_string(), "\"x\"");
    assert_eq!(
        id.meta.extensions["x-internal"].to_string(),
        r#"{"owner":"core"}"#
    );
    assert_eq!(api.types.get(o.fields[1].ty).meta.access, Access::WriteOnly);
}

#[test]
fn unevaluated_properties_exact_through_all_of() {
    let out = lower(&[(
        "s.json",
        r##"{
        "$defs": {"Base": {"properties": {"id": {"type": "string"}}}},
        "allOf": [{"$ref": "#/$defs/Base"}],
        "properties": {"name": {"type": "string"}},
        "unevaluatedProperties": false
    }"##,
    )]);
    out.assert_clean();
    let dump = out.dump();
    assert!(dump.contains(".id: "), "{dump}");
    assert!(dump.contains(".name: "), "{dump}");
}

#[test]
fn unevaluated_properties_through_branches_warns() {
    let out = lower(&[(
        "s.json",
        r#"{
        "anyOf": [{"properties": {"a": true}}, {"properties": {"b": true}}],
        "unevaluatedProperties": false
    }"#,
    )]);
    assert_eq!(out.codes(), ["FLE1010"]);
    assert_eq!(out.primary_text("FLE1010"), "false");
}

#[test]
fn unevaluated_properties_are_moot_after_additional_properties() {
    let out = lower(&[(
        "s.json",
        r#"{
        "allOf": [{"additionalProperties": true}],
        "unevaluatedProperties": false
    }"#,
    )]);
    out.assert_clean();
    assert!(!out.dump().contains("never"), "{}", out.dump());
}

#[test]
fn invalid_keyword_values_are_reported_and_ignored() {
    let out = lower(&[(
        "s.json",
        r#"{
        "type": ["string", "text"],
        "maxLength": -1,
        "minLength": "3",
        "multipleOf": 0,
        "required": "name",
        "anyOf": [],
        "readOnly": true, "writeOnly": true
    }"#,
    )]);
    let mut codes = out.codes();
    codes.sort_unstable();
    assert_eq!(
        codes,
        [
            "FLE1001", "FLE1001", "FLE1001", "FLE1001", "FLE1001", "FLE1001", "FLE1002"
        ],
        "{}",
        out.rendered()
    );
    assert_eq!(out.primary_text("FLE1002"), "\"text\"");
    let TypeKind::String(s) = out.named("s") else {
        panic!("{}", out.dump())
    };
    assert_eq!((s.min_length, s.max_length), (0, None));
}

#[test]
fn unknown_dialects_fall_back_with_a_warning() {
    let out = lower(&[(
        "s.json",
        r#"{"$schema": "https://example.com/dialect", "type": "string"}"#,
    )]);
    assert_eq!(out.codes(), ["FLE1007"]);
    assert!(!out.diagnostics.has_errors());
}
