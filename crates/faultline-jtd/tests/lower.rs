//! Lowering JSON Type Definitions, and how their changes are classified.

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, QualName, Role, TypeKind, UnionMode, validate};
use faultline_jtd::lower_jtd;
use faultline_rules::{Severity, classify};
use faultline_source::{Limits, SourceMap};

fn lowered(text: &str, role: Role) -> (Option<Api>, Vec<String>) {
    let loader = MemoryLoader::new().with("file:///user.jtd.json", text);
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    let lowered = lower_jtd(
        &UriRef::parse("file:///user.jtd.json").unwrap(),
        &mut store,
        "user",
        role,
    );
    let codes = lowered
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowered.api, codes)
}

fn api(text: &str, role: Role) -> Api {
    let (api, codes) = lowered(text, role);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

const USER: &str = r#"{
  "metadata": {"description": "A user."},
  "definitions": {
    "address": {
      "properties": {"street": {"type": "string"}, "zip": {"type": "string"}},
      "optionalProperties": {"unit": {"type": "string"}}
    },
    "node": {"properties": {"next": {"ref": "node", "nullable": true}}}
  },
  "properties": {
    "id": {"type": "string"},
    "createdAt": {"type": "timestamp"},
    "karma": {"type": "int32"},
    "isAdmin": {"type": "boolean"},
    "role": {"enum": ["USER", "ADMIN"]},
    "tags": {"elements": {"type": "string"}},
    "labels": {"values": {"type": "uint8"}},
    "home": {"ref": "address"},
    "chain": {"ref": "node"},
    "event": {
      "discriminator": "kind",
      "mapping": {
        "login": {"properties": {"ip": {"type": "string"}}},
        "logout": {"properties": {}, "additionalProperties": true}
      }
    }
  },
  "optionalProperties": {
    "nickname": {"type": "string", "nullable": true, "metadata": {"description": "Shown instead of the name."}}
  }
}"#;

#[test]
fn every_form_is_lowered() {
    let api = api(USER, Role::Both);
    let root = api.types.get(api.schemas[0].ty);
    assert_eq!(root.meta.description.as_deref(), Some("A user."));
    let TypeKind::Object(user) = &root.kind else {
        panic!()
    };
    assert!(
        matches!(api.types.get(user.additional).kind, TypeKind::Never),
        "closed"
    );
    let kind = |name: &str| {
        &api.types
            .get(api.types.resolve(user.field(name).unwrap().ty))
            .kind
    };
    assert!(matches!(kind("id"), TypeKind::String(s) if s.format.is_none()));
    assert!(
        matches!(kind("createdAt"), TypeKind::String(s) if s.format.as_deref() == Some("date-time"))
    );
    let TypeKind::Integer(karma) = kind("karma") else {
        panic!()
    };
    assert_eq!(
        karma.maximum.as_ref().unwrap().value.to_string(),
        "2147483647"
    );
    assert!(matches!(kind("isAdmin"), TypeKind::Boolean));
    assert!(matches!(kind("role"), TypeKind::Enum(e) if e.values.len() == 2));
    assert!(matches!(kind("tags"), TypeKind::Array(_)));
    let TypeKind::Object(labels) = kind("labels") else {
        panic!()
    };
    assert!(labels.fields.is_empty());
    assert!(matches!(
        api.types.get(labels.additional).kind,
        TypeKind::Integer(_)
    ));
    assert!(api.named.contains_key(&QualName::new("address")));
    let TypeKind::Union(event) = kind("event") else {
        panic!()
    };
    assert_eq!(event.mode, UnionMode::Tagged);
    let discriminator = event.discriminator.as_ref().unwrap();
    assert_eq!(discriminator.property, "kind");
    let TypeKind::Object(login) = &api.types.get(discriminator.mapping[0].1).kind else {
        panic!()
    };
    let fields: Vec<(&str, bool)> = login
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        fields,
        [("kind", true), ("ip", true)],
        "the tag is fixed to the value"
    );
    let nickname = user.field("nickname").unwrap();
    assert!(!nickname.required);
    assert_eq!(
        nickname.meta.description.as_deref(),
        Some("Shown instead of the name.")
    );
    assert!(
        matches!(api.types.get(nickname.ty).kind, TypeKind::Union(_)),
        "nullable"
    );
}

#[test]
fn invalid_schemas_are_reported() {
    let cases: &[(&str, &str)] = &[
        (r#"{"type": "int64"}"#, "FLE8201"),
        (r#"{"type": "string", "enum": ["a"]}"#, "FLE8201"),
        (r#"{"format": "email"}"#, "FLE8201"),
        (r#"{"elements": {"definitions": {}}}"#, "FLE8201"),
        (r#"{"enum": []}"#, "FLE8201"),
        (r#"{"ref": "missing"}"#, "FLE8202"),
        (r#"{"enum": ["a", "a"]}"#, "FLE8203"),
        (
            r#"{"properties": {"a": {}}, "optionalProperties": {"a": {}}}"#,
            "FLE8203",
        ),
        (
            r#"{"discriminator": "k", "mapping": {"a": {"type": "string"}}}"#,
            "FLE8204",
        ),
        (
            r#"{"discriminator": "k", "mapping": {"a": {"properties": {"k": {}}}}}"#,
            "FLE8204",
        ),
        (
            r#"{"discriminator": "k", "mapping": {"a": {"properties": {}, "nullable": true}}}"#,
            "FLE8204",
        ),
        (
            r#"{"definitions": {"a": {"ref": "b"}, "b": {"ref": "a"}}, "ref": "a"}"#,
            "FLE8205",
        ),
        (r#"{"nullable": "yes"}"#, "FLE8201"),
        ("[]", "FLE8201"),
    ];
    for (schema, code) in cases {
        let (api, codes) = lowered(schema, Role::Both);
        assert!(api.is_none(), "{schema}");
        assert!(codes.iter().any(|c| c == code), "{schema}: {codes:?}");
    }
}

fn worst(old: &str, new: &str, role: Role) -> Severity {
    let a = api(old, role);
    let b = api(new, role);
    faultline_diff::diff(&a, &b)
        .iter()
        .map(|c| classify("user", c).severity)
        .max()
        .unwrap_or(Severity::Info)
}

#[test]
fn changes_are_classified() {
    let base = r#"{"properties": {"id": {"type": "string"}}, "optionalProperties": {"age": {"type": "uint8"}}}"#;
    // Objects are closed: a new optional property is rejected by old
    // readers of requests, as JTD validators refuse unknown properties.
    assert_eq!(
        worst(
            base,
            &base.replace(r#""age""#, r#""nick": {"type": "string"}, "age""#),
            Role::Output
        ),
        Severity::Risky
    );
    // A wider type accepts every old value, but generated code changes its
    // field's type.
    assert_eq!(
        worst(base, &base.replace("uint8", "uint16"), Role::Input),
        Severity::Risky
    );
    assert_eq!(
        worst(base, &base.replace("uint8", "int8"), Role::Input),
        Severity::Breaking
    );
    assert_eq!(
        worst(
            base,
            &base.replace(
                r#""properties": {"id""#,
                r#""properties": {"name": {"type": "string"}, "id""#
            ),
            Role::Input
        ),
        Severity::Breaking,
        "a new required property"
    );
    let union =
        r#"{"discriminator": "k", "mapping": {"a": {"properties": {}}, "b": {"properties": {}}}}"#;
    assert_eq!(
        worst(
            union,
            &union.replace(r#", "b": {"properties": {}}"#, ""),
            Role::Input
        ),
        Severity::Breaking
    );
}
