//! Avro schema validation, lowering, and compatibility under the
//! BACKWARD (`input`), FORWARD (`output`) and FULL (`both`) modes.

use faultline_avro::lower_avro;
use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, Role, TypeKind, validate};
use faultline_rules::{Severity, classify};
use faultline_source::{Limits, SourceMap};

fn lower(text: &str, role: Role) -> faultline_avro::Lowered {
    let loader = MemoryLoader::new().with("file:///pet.avsc", text);
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    lower_avro(
        &UriRef::parse("file:///pet.avsc").unwrap(),
        &mut store,
        "pet",
        role,
    )
}

fn api(text: &str, role: Role) -> Api {
    let lowered = lower(text, role);
    let errors: Vec<&str> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.as_str())
        .collect();
    assert!(errors.is_empty(), "{text}\n{errors:?}");
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

fn errors(text: &str) -> Vec<(String, String)> {
    lower(text, Role::Both)
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.map(|c| c.to_string()).unwrap_or_default(),
                d.message.clone(),
            )
        })
        .collect()
}

/// The most severe finding between two versions in `role`.
fn worst(old: &str, new: &str, role: Role) -> (Severity, Vec<String>) {
    let changes = faultline_diff::diff(&api(old, role), &api(new, role));
    let findings: Vec<_> = changes.iter().map(|c| classify("pet", c)).collect();
    let worst = findings
        .iter()
        .map(|f| f.severity)
        .max()
        .unwrap_or(Severity::Info);
    let summary = findings
        .iter()
        .map(|f| format!("{} {} {}", f.rule.code, f.severity, f.change.path))
        .collect();
    (worst, summary)
}

/// A `Pet` record with `fields` (JSON field objects).
fn record(fields: &str) -> String {
    format!(
        r#"{{"type": "record", "name": "Pet", "namespace": "com.example",
            "fields": [{{"name": "id", "type": "string"}}, {fields}]}}"#
    )
}

const NAME: &str = r#"{"name": "name", "type": "string"}"#;

#[test]
fn records_lower_as_readers_see_them() {
    let text = record(
        r#"{"name": "name", "type": "string", "aliases": ["title"], "doc": "The name."},
           {"name": "age", "type": "int", "default": 0},
           {"name": "tags", "type": {"type": "array", "items": "string"}, "default": []},
           {"name": "owner", "type": ["null", {"type": "record", "name": "Person", "fields": [{"name": "id", "type": "long"}]}], "default": null},
           {"name": "kind", "type": {"type": "enum", "name": "Kind", "symbols": ["CAT", "DOG"], "default": "CAT"}},
           {"name": "chip", "type": {"type": "fixed", "name": "Chip", "size": 16}},
           {"name": "born", "type": {"type": "long", "logicalType": "timestamp-millis"}}"#,
    );
    let api = api(&text, Role::Both);
    assert_eq!(api.schemas[0].name, "com.example.Pet");
    let pet = api.named_type("com.example.Pet").unwrap();
    let TypeKind::Object(obj) = &api.types.get(pet).kind else {
        panic!()
    };
    assert!(
        matches!(api.types.get(obj.additional).kind, TypeKind::Any),
        "readers ignore unknown fields"
    );
    let required: Vec<(&str, bool)> = obj
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        required,
        [
            ("id", true),
            ("name", true),
            ("age", false),
            ("tags", false),
            ("owner", false),
            ("kind", true),
            ("chip", true),
            ("born", true),
        ]
    );
    assert_eq!(obj.fields[1].aliases, ["title"]);
    assert!(
        api.named_type("com.example.Person").is_some(),
        "nested names take the namespace"
    );
    let kind = api.named_type("com.example.Kind").unwrap();
    let TypeKind::Enum(e) = &api.types.get(kind).kind else {
        panic!()
    };
    assert!(e.open, "an enum with a default reads unknown symbols");
    let chip = api.named_type("com.example.Chip").unwrap();
    let TypeKind::String(s) = &api.types.get(chip).kind else {
        panic!()
    };
    assert_eq!((s.min_length, s.max_length), (16, Some(16)));
    let TypeKind::Integer(born) = &api.types.get(obj.fields[7].ty).kind else {
        panic!()
    };
    assert_eq!(born.format.as_deref(), Some("timestamp-millis"));
}

#[test]
fn recursive_records() {
    api(
        r#"{"type": "record", "name": "Node", "fields": [
             {"name": "children", "type": {"type": "array", "items": "Node"}},
             {"name": "next", "type": ["null", "Node"], "default": null}]}"#,
        Role::Both,
    );
}

#[test]
fn validation() {
    let cases: &[(&str, &str, &str)] = &[
        (
            r#"{"type": "record", "name": "A", "fields": [{"name": "x", "type": "Missing"}]}"#,
            "FLE5002",
            "unknown type `Missing`",
        ),
        (
            r#"{"type": "record", "name": "A", "fields": [{"name": "x", "type": "int"}, {"name": "x", "type": "int"}]}"#,
            "FLE5003",
            "field `x` is defined twice in `A`",
        ),
        (
            r#"{"type": "record", "name": "A", "fields": [{"name": "x", "type": ["string", "null"], "default": null}]}"#,
            "FLE5007",
            "the default of `x` does not match its type",
        ),
        (
            r#"{"type": "record", "name": "A", "fields": [{"name": "x", "type": "int", "default": 1.5}]}"#,
            "FLE5007",
            "the default of `x` does not match its type",
        ),
        (
            r#"{"type": "record", "name": "A", "fields": [{"name": "x", "type": ["int", "int"]}]}"#,
            "FLE5005",
            "a union has two `int` branches",
        ),
        (
            r#"{"type": "record", "name": "A", "fields": [{"name": "x", "type": ["int", ["null"]]}]}"#,
            "FLE5005",
            "a union cannot contain another union",
        ),
        (
            r#"{"type": "enum", "name": "E", "symbols": ["A", "A"]}"#,
            "FLE5003",
            "symbol `A` is listed twice",
        ),
        (
            r#"{"type": "enum", "name": "E", "symbols": ["A"], "default": "B"}"#,
            "FLE5007",
            "the enum default must be one of its symbols",
        ),
        (
            r#"{"type": "enum", "name": "E", "symbols": ["1A"]}"#,
            "FLE5004",
            "`1A` is not a valid symbol",
        ),
        (
            r#"{"type": "record", "name": "int", "fields": []}"#,
            "FLE5004",
            "`int` is a primitive type and cannot be redefined",
        ),
        (
            r#"{"type": "record", "name": "A"}"#,
            "FLE5001",
            "a record schema needs `fields`",
        ),
        (
            r#"{"type": "fixed", "name": "F", "size": -1}"#,
            "FLE5001",
            "`size` must be a non-negative integer",
        ),
        (
            r#"{"type": "record", "name": "A", "fields": [{"name": "b", "type": {"type": "record", "name": "A", "fields": []}}]}"#,
            "FLE5003",
            "`A` is defined twice",
        ),
        (
            "42",
            "FLE5001",
            "a schema is a type name, an object or a union, not a number",
        ),
    ];
    for (text, code, message) in cases {
        let found = errors(text);
        assert!(
            found
                .iter()
                .any(|(c, m)| c == code && m.starts_with(message)),
            "{text}\nexpected {code} {message:?}, got {found:?}"
        );
    }
    let warned = errors(r#"{"type": "string", "logicalType": "date"}"#);
    assert_eq!(warned[0].0, "FLE5006");
    assert!(
        lower(r#"{"type": "string", "logicalType": "date"}"#, Role::Both)
            .api
            .is_some()
    );
}

#[test]
fn backward_compatibility_new_readers_read_old_data() {
    let old = record(NAME);
    let backward = |new: &str| worst(&old, new, Role::Input);

    let with_default = record(&format!(
        r#"{NAME}, {{"name": "age", "type": "int", "default": 0}}"#
    ));
    assert_eq!(
        backward(&with_default).0,
        Severity::Info,
        "{:?}",
        backward(&with_default).1
    );

    let without_default = record(&format!(r#"{NAME}, {{"name": "age", "type": "int"}}"#));
    let (severity, found) = backward(&without_default);
    assert_eq!(severity, Severity::Breaking);
    assert!(
        found.iter().any(|f| f.starts_with("FL1002 breaking")),
        "{found:?}"
    );

    let removed = r#"{"type": "record", "name": "Pet", "namespace": "com.example", "fields": [{"name": "id", "type": "string"}]}"#;
    assert_eq!(
        backward(removed).0,
        Severity::Info,
        "{:?}",
        backward(removed).1
    );

    let aliased = record(r#"{"name": "title", "type": "string", "aliases": ["name"]}"#);
    let (severity, found) = backward(&aliased);
    assert!(severity <= Severity::Risky, "{found:?}");
    assert!(
        found.iter().any(|f| f.starts_with("FL1025")),
        "a rename through an alias: {found:?}"
    );
    assert!(!found.iter().any(|f| f.starts_with("FL1003")), "{found:?}");
}

#[test]
fn type_promotion() {
    let int = record(r#"{"name": "n", "type": "int"}"#);
    let long = record(r#"{"name": "n", "type": "long"}"#);
    let double = record(r#"{"name": "n", "type": "double"}"#);
    // A long reader reads int data; an int reader cannot read all longs.
    assert_eq!(worst(&int, &long, Role::Input).0, Severity::Info);
    assert_eq!(worst(&long, &int, Role::Input).0, Severity::Breaking);
    assert_eq!(worst(&int, &double, Role::Input).0, Severity::Info);
    // FORWARD: old int readers get long data.
    assert!(worst(&int, &long, Role::Output).0 >= Severity::Risky);
    // bytes and string resolve into each other.
    let string = record(r#"{"name": "n", "type": "string"}"#);
    let bytes = record(r#"{"name": "n", "type": "bytes"}"#);
    assert!(worst(&string, &bytes, Role::Both).0 <= Severity::Risky);
}

#[test]
fn forward_compatibility_old_readers_read_new_data() {
    let old = record(&format!(
        r#"{NAME}, {{"name": "age", "type": "int", "default": 0}}"#
    ));
    let forward = |new: &str| worst(&old, new, Role::Output);

    let added = record(&format!(
        r#"{NAME}, {{"name": "age", "type": "int", "default": 0}}, {{"name": "weight", "type": "double"}}"#
    ));
    assert_eq!(forward(&added).0, Severity::Info, "{:?}", forward(&added).1);

    // `age` had a default for old readers: dropping it is fine.
    let without_age = record(NAME);
    assert_eq!(
        forward(&without_age).0,
        Severity::Info,
        "{:?}",
        forward(&without_age).1
    );

    // `name` had no default: old readers need it.
    let without_name = r#"{"type": "record", "name": "Pet", "namespace": "com.example",
        "fields": [{"name": "id", "type": "string"}, {"name": "age", "type": "int", "default": 0}]}"#;
    let (severity, found) = forward(without_name);
    assert_eq!(severity, Severity::Breaking, "{found:?}");
}

#[test]
fn enum_symbols() {
    let e = |symbols: &str, default: &str| {
        record(&format!(
            r#"{{"name": "kind", "type": {{"type": "enum", "name": "Kind", "symbols": [{symbols}]{default}}}}}"#
        ))
    };
    let old = e(r#""CAT", "DOG""#, "");
    let more = e(r#""CAT", "DOG", "BIRD""#, "");
    // BACKWARD: new readers know more symbols.
    assert_eq!(worst(&old, &more, Role::Input).0, Severity::Info);
    // FORWARD: old readers get a symbol they do not know.
    assert!(worst(&old, &more, Role::Output).0 >= Severity::Risky);
    // ... unless they have a default.
    let old_with_default = e(r#""CAT", "DOG""#, r#", "default": "CAT""#);
    let more_with_default = e(r#""CAT", "DOG", "BIRD""#, r#", "default": "CAT""#);
    assert_eq!(
        worst(&old_with_default, &more_with_default, Role::Output).0,
        Severity::Info
    );
    // Removing a symbol breaks new readers of old data.
    let fewer = e(r#""CAT""#, "");
    assert_eq!(worst(&old, &fewer, Role::Input).0, Severity::Breaking);
}
