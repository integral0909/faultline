//! Reference resolution: documents, identifiers, anchors, the workspace
//! sandbox, recursion and dynamic scope.

mod common;

use std::fmt::Write as _;

use common::{lower, lower_with};
use faultline_ir::{Polarities, Polarity, TypeKind};
use faultline_jsonschema::{Dialect, Options};

#[test]
fn references_across_files_resolve_relative_to_each_document() {
    let out = lower(&[
        (
            "api/order.json",
            r#"{"type": "object", "properties": {"pet": {"$ref": "models/pet.json#/$defs/Pet"}}}"#,
        ),
        (
            "api/models/pet.json",
            r#"{"$defs": {"Pet": {"type": "object", "properties": {"owner": {"$ref": "../../shared/person.json"}}}}}"#,
        ),
        (
            "shared/person.json",
            r#"{"type": "object", "properties": {"name": {"type": "string"}}}"#,
        ),
    ]);
    let api = out.assert_clean();
    assert!(api.named_type("Pet").is_some());
    assert!(
        api.named_type("person").is_some(),
        "document roots are named after the file"
    );
    let TypeKind::Object(person) = out.named("person") else {
        panic!()
    };
    assert_eq!(person.fields[0].name, "name");
    // Spans point into the file that defined each schema.
    let origin = api
        .types
        .get(api.named_type("person").unwrap())
        .origin
        .unwrap();
    let file = out.map.get(origin.source()).unwrap();
    assert_eq!(file.name().to_string(), "shared/person.json");
}

#[test]
fn identifiers_establish_bases_and_embedded_resources() {
    let out = lower(&[(
        "root.json",
        r##"{
        "$id": "https://example.com/schemas/root",
        "type": "object",
        "properties": {
            "home": {"$ref": "address"},
            "work": {"$ref": "https://example.com/schemas/address#/properties/street"}
        },
        "$defs": {
            "addr": {
                "$id": "address",
                "type": "object",
                "properties": {"street": {"type": "string"}, "self": {"$ref": "#"}}
            }
        }
    }"##,
    )]);
    out.assert_clean();
    let TypeKind::Object(root) = out.named("root") else {
        panic!("{}", out.dump())
    };
    let home = out.kind(root.fields[0].ty);
    assert!(
        matches!(home, TypeKind::Object(o) if o.fields[0].name == "street"),
        "{}",
        out.dump()
    );
    assert!(matches!(out.kind(root.fields[1].ty), TypeKind::String(_)));
    // "#" inside the embedded resource means the resource, not the document.
    let TypeKind::Object(addr) = home else {
        unreachable!()
    };
    assert!(matches!(out.kind(addr.fields[1].ty), TypeKind::Object(o) if o.fields.len() == 2));
}

#[test]
fn anchors_in_each_dialect() {
    let modern = lower(&[(
        "s.json",
        r##"{
        "type": "object",
        "properties": {"a": {"$ref": "#item"}},
        "$defs": {"Item": {"$anchor": "item", "type": "integer"}}
    }"##,
    )]);
    modern.assert_clean();
    let TypeKind::Object(o) = modern.named("s") else {
        panic!()
    };
    assert!(matches!(modern.kind(o.fields[0].ty), TypeKind::Integer(_)));

    let draft7 = lower_with(
        &[(
            "s.json",
            r##"{
            "type": "object",
            "properties": {"a": {"$ref": "#item"}},
            "definitions": {"Item": {"$id": "#item", "type": "boolean"}}
        }"##,
        )],
        Options {
            default_dialect: Dialect::Draft7,
        },
    );
    draft7.assert_clean();
    let TypeKind::Object(o) = draft7.named("s") else {
        panic!()
    };
    assert_eq!(draft7.kind(o.fields[0].ty), &TypeKind::Boolean);
}

#[test]
fn keys_named_like_keywords_are_not_identifiers() {
    // A *property* called "$id" must not register a resource.
    let out = lower(&[(
        "s.json",
        r##"{
        "properties": {"$id": {"type": "string"}, "other": {"$ref": "#/properties/$id"}}
    }"##,
    )]);
    out.assert_clean();
}

#[test]
fn unguarded_self_reference_is_reported_with_spans() {
    let out = lower(&[(
        "s.json",
        r##"{"$defs": {"Loop": {"allOf": [{"$ref": "#/$defs/Loop"}]}}}"##,
    )]);
    assert_eq!(out.codes(), ["FLE1009"], "{}", out.rendered());
    assert!(out.primary_text("FLE1009").starts_with('{'));
}

#[test]
fn recursion_through_properties_is_fine() {
    let out = lower(&[(
        "tree.json",
        r##"{
        "type": "object",
        "properties": {"children": {"type": "array", "items": {"$ref": "#"}}}
    }"##,
    )]);
    out.assert_clean();
    let api = out.api();
    let polarity = Polarities::compute(api);
    assert_eq!(polarity.get(api.named_type("tree").unwrap()), Polarity::IN);
}

#[test]
fn reference_errors() {
    let cases: &[(&str, &str, &str)] = &[
        (r#"{"$ref": "missing.json"}"#, "FLE1006", "\"missing.json\""),
        (
            r##"{"$ref": "#/$defs/Nope"}"##,
            "FLE1003",
            "\"#/$defs/Nope\"",
        ),
        (r##"{"$ref": "#nowhere"}"##, "FLE1003", "\"#nowhere\""),
        (
            r##"{"$ref": "#/properties/a/enum", "properties": {"a": {"enum": [1]}}}"##,
            "FLE1011",
            "\"#/properties/a/enum\"",
        ),
        (
            r#"{"$ref": "../../../etc/passwd"}"#,
            "FLE1004",
            "\"../../../etc/passwd\"",
        ),
        (
            r#"{"$ref": "https://example.com/schema.json"}"#,
            "FLE1005",
            "\"https://example.com/schema.json\"",
        ),
        (r#"{"$ref": 5}"#, "FLE1001", "5"),
    ];
    for &(schema, code, span) in cases {
        let out = lower(&[("dir/s.json", schema)]);
        assert_eq!(out.codes(), [code], "{schema}\n{}", out.rendered());
        assert_eq!(out.primary_text(code), span, "{schema}");
    }
}

#[test]
fn parse_errors_in_referenced_documents_surface() {
    let out = lower(&[("s.json", r#"{"$ref": "bad.json"}"#), ("bad.json", "{")]);
    let mut codes = out.codes();
    codes.sort_unstable();
    assert_eq!(codes, ["FLE0102", "FLE1006"], "{}", out.rendered());
}

#[test]
fn draft7_ignores_ref_siblings_but_2020_combines_them() {
    let schema = r##"{"type": "object", "$defs": {"S": {"type": "string"}}, "properties": {"a": {"$ref": "#/$defs/S", "maxLength": 3}}}"##;
    let draft7 = lower_with(
        &[("s.json", schema)],
        Options {
            default_dialect: Dialect::Draft7,
        },
    );
    assert_eq!(draft7.codes(), ["FLE1008"]);
    let TypeKind::Object(o) = draft7.named("s") else {
        panic!()
    };
    assert!(matches!(draft7.kind(o.fields[0].ty), TypeKind::String(s) if s.max_length.is_none()));

    let modern = lower(&[("s.json", schema)]);
    modern.assert_clean();
    let TypeKind::Object(o) = modern.named("s") else {
        panic!()
    };
    assert!(matches!(modern.kind(o.fields[0].ty), TypeKind::AllOf(parts) if parts.len() == 2));
}

#[test]
fn duplicate_identifiers_are_errors() {
    let out = lower(&[(
        "s.json",
        r#"{"$defs": {
        "A": {"$id": "https://example.com/same", "type": "string"},
        "B": {"$id": "https://example.com/same", "type": "integer"},
        "C": {"$anchor": "x"}, "D": {"$anchor": "x"}
    }}"#,
    )]);
    assert_eq!(out.codes(), ["FLE1012", "FLE1012"], "{}", out.rendered());
}

#[test]
fn name_collisions_across_files_are_qualified() {
    let out = lower(&[
        (
            "a.json",
            r#"{"properties": {"x": {"$ref": "b.json#/$defs/Item"}}, "$defs": {"Item": {"type": "string"}}}"#,
        ),
        ("b.json", r#"{"$defs": {"Item": {"type": "integer"}}}"#),
    ]);
    let api = out.assert_clean();
    assert!(api.named_type("Item").is_some());
    assert!(api.named_type("b.Item").is_some(), "{}", out.dump());
    assert!(
        matches!(out.named("Item"), TypeKind::String(_)),
        "entry definitions keep plain names"
    );
}

/// The `$dynamicRef` example from the JSON Schema 2020-12 specification: a
/// strict tree extends a tree so that its children are strict trees too.
#[test]
fn dynamic_references_follow_the_dynamic_scope() {
    let out = lower(&[(
        "trees.json",
        r##"{
        "$ref": "https://example.com/strict-tree",
        "$defs": {
            "tree": {
                "$id": "https://example.com/tree",
                "$dynamicAnchor": "node",
                "type": "object",
                "properties": {
                    "data": true,
                    "children": {"type": "array", "items": {"$dynamicRef": "#node"}}
                }
            },
            "strict": {
                "$id": "https://example.com/strict-tree",
                "$dynamicAnchor": "node",
                "$ref": "tree",
                "unevaluatedProperties": false
            }
        }
    }"##,
    )]);
    out.assert_clean();
    let api = out.api();
    let items_of = |object_id| {
        let TypeKind::Object(o) = out.kind(object_id) else {
            panic!("{}", out.dump())
        };
        let TypeKind::Array(a) = out.kind(o.field("children").unwrap().ty) else {
            panic!()
        };
        api.types.resolve(a.items)
    };
    // Reached on its own, a tree's children are trees.
    let tree = api.named_type("tree").unwrap();
    assert_eq!(items_of(tree), api.types.resolve(tree));

    // Reached through the strict tree, the same schema's children are strict trees.
    let strict = api.named_type("strict").unwrap();
    let TypeKind::AllOf(parts) = out.kind(strict) else {
        panic!("{}", out.dump())
    };
    let specialised_tree = api.types.resolve(parts[0]);
    assert_ne!(specialised_tree, api.types.resolve(tree));
    assert_eq!(api.named_type("tree@strict-tree"), Some(specialised_tree));
    assert_eq!(items_of(specialised_tree), api.types.resolve(strict));
}

#[test]
fn recursive_references_follow_the_dynamic_scope() {
    let out = lower_with(
        &[(
            "s.json",
            r##"{
            "$ref": "https://example.com/strict",
            "$defs": {
                "tree": {
                    "$id": "https://example.com/tree",
                    "$recursiveAnchor": true,
                    "type": "object",
                    "properties": {"children": {"type": "array", "items": {"$recursiveRef": "#"}}}
                },
                "strict": {
                    "$id": "https://example.com/strict",
                    "$recursiveAnchor": true,
                    "allOf": [{"$ref": "tree"}],
                    "required": ["children"]
                }
            }
        }"##,
        )],
        Options {
            default_dialect: Dialect::Draft2019_09,
        },
    );
    out.assert_clean();
    let api = out.api();
    let strict = api.types.resolve(api.named_type("strict").unwrap());
    let dump = out.dump();
    // Somewhere under the strict tree, items refer back to the strict tree.
    let TypeKind::AllOf(parts) = out.kind(strict) else {
        panic!("{dump}")
    };
    let specialised_tree = parts
        .iter()
        .map(|&p| api.types.resolve(p))
        .find(|&p| matches!(out.kind(p), TypeKind::Object(o) if o.field("children").is_some()))
        .unwrap_or_else(|| panic!("{dump}"));
    let TypeKind::Object(o) = out.kind(specialised_tree) else {
        unreachable!()
    };
    let TypeKind::Array(a) = out.kind(o.field("children").unwrap().ty) else {
        panic!()
    };
    assert_eq!(api.types.resolve(a.items), strict, "{dump}");
}

#[test]
fn long_reference_chains_do_not_exhaust_the_stack() {
    let n = 20_000;
    let mut defs = String::new();
    for i in 0..n {
        if i > 0 {
            defs.push(',');
        }
        let next = if i + 1 < n {
            format!(r##"{{"$ref": "#/$defs/D{}"}}"##, i + 1)
        } else {
            r#"{"type": "string"}"#.to_owned()
        };
        let _ = write!(
            defs,
            r#""D{i}": {{"type": "object", "properties": {{"next": {next}}}}}"#
        );
    }
    let schema = format!(r##"{{"$ref": "#/$defs/D0", "$defs": {{{defs}}}}}"##);
    let out = lower(&[("chain.json", &schema)]);
    let api = out.assert_clean();
    assert_eq!(api.named.len(), n + 1);
}
