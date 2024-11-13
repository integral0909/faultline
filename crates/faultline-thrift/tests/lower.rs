//! Lowering Thrift types, and how their changes are classified.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, Role, TypeKind, validate};
use faultline_rules::{Severity, classify};
use faultline_source::SourceMap;
use faultline_thrift::lower_thrift;

fn lowered(text: &str, role: Role) -> Api {
    let loader = MemoryLoader::new().with("file:///pets.thrift", text).with(
        "file:///shared.thrift",
        "struct Tag { 1: required string name }\n",
    );
    let sources = SourceMap::new();
    let lowered = lower_thrift(
        &UriRef::parse("file:///pets.thrift").unwrap(),
        &loader,
        &sources,
        "pets",
        role,
    );
    let api = lowered
        .api
        .unwrap_or_else(|| panic!("{:?}", lowered.diagnostics));
    assert_eq!(validate(&api), []);
    api
}

const PETS: &str = r#"include "shared.thrift"
typedef i64 PetId
enum Kind { DOG = 1, CAT, OTHER = 10 }
/** A pet. */
struct Pet {
  1: required PetId id,
  2: optional string name = "Rex",
  3: Kind kind = Kind.DOG,
  4: list<shared.Tag> tags,
  5: set<i16> lucky,
  6: map<string, binary> blobs,
  7: uuid token,
  8: double weight (deprecated)
}
union Payload { 1: i32 number, 2: string text }
"#;

fn kind<'a>(api: &'a Api, name: &str) -> &'a TypeKind {
    let id = api.named[&faultline_ir::QualName::new(name)];
    &api.types.get(id).kind
}

#[test]
fn types_are_lowered() {
    let api = lowered(PETS, Role::Both);
    let roots: Vec<&str> = api.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        roots,
        ["Pet", "Payload"],
        "structs are roots without services"
    );

    let TypeKind::Object(pet) = kind(&api, "Pet") else {
        panic!()
    };
    let fields: Vec<(&str, Option<u32>, bool)> = pet
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.tag, f.required))
        .collect();
    assert_eq!(
        fields,
        [
            ("id", Some(1), true),
            ("name", Some(2), false),
            ("kind", Some(3), false),
            ("tags", Some(4), false),
            ("lucky", Some(5), false),
            ("blobs", Some(6), false),
            ("token", Some(7), false),
            ("weight", Some(8), false),
        ]
    );
    let TypeKind::Integer(id) = &api.types.get(pet.fields[0].ty).kind else {
        panic!("a typedef is the type it names")
    };
    assert_eq!(id.format.as_deref(), Some("i64"));
    assert_eq!(
        pet.fields[1].meta.default.as_ref().unwrap().to_string(),
        "\"Rex\""
    );
    assert_eq!(
        pet.fields[2].meta.default.as_ref().unwrap().to_string(),
        "\"DOG\""
    );
    assert!(pet.fields[7].meta.deprecated);
    let TypeKind::Array(lucky) = &api.types.get(pet.fields[4].ty).kind else {
        panic!()
    };
    assert!(lucky.unique_items);
    assert!(matches!(
        api.types.get(pet.fields[5].ty).kind,
        TypeKind::Map(_)
    ));
    assert!(
        api.named
            .contains_key(&faultline_ir::QualName::new("shared.Tag"))
    );

    let TypeKind::Enum(kind) = kind(&api, "Kind") else {
        panic!()
    };
    let numbers: Vec<i64> = kind.values.iter().map(|v| v.number.unwrap()).collect();
    assert_eq!(numbers, [1, 2, 10]);

    let TypeKind::Object(payload) = self::kind(&api, "Payload") else {
        panic!()
    };
    assert_eq!(payload.exclusive[0].fields, ["number", "text"]);
    assert_eq!(payload.min_properties, 1);
}

fn worst(old: &str, new: &str, role: Role) -> Severity {
    let a = lowered(old, role);
    let b = lowered(new, role);
    faultline_diff::diff(&a, &b)
        .iter()
        .map(|c| classify("pets", c).severity)
        .max()
        .unwrap_or(Severity::Info)
}

#[test]
fn wire_changes_are_classified() {
    let with = |field: &str| format!("struct Pet {{ 1: required i64 id, {field} }}\n");
    let base = with("2: optional string name");
    // Renaming a field keeps its id: only the JSON protocol notices.
    assert!(worst(&base, &with("2: optional string title"), Role::Both) < Severity::Breaking);
    // Moving a field to another id is removing it and adding another.
    assert_eq!(
        worst(&base, &with("3: optional string name"), Role::Input),
        Severity::Breaking
    );
    // A new required field breaks writers that do not send it.
    assert_eq!(
        worst(
            &base,
            &format!("{}\n", base.replace(" }", ", 3: required bool b }")),
            Role::Input
        ),
        Severity::Breaking
    );
    // A new optional field is compatible in either direction.
    for role in [Role::Input, Role::Output] {
        assert_eq!(
            worst(&base, &base.replace(" }", ", 3: optional bool b }"), role),
            Severity::Info
        );
    }
    // A wider integer is another wire type.
    assert_eq!(
        worst(
            &with("2: optional i32 n"),
            &with("2: optional i64 n"),
            Role::Both
        ),
        Severity::Breaking
    );
    // Making a field required breaks writers that leave it out.
    assert_eq!(
        worst(&base, &with("2: required string name"), Role::Input),
        Severity::Breaking
    );
}

#[test]
fn enum_changes_are_classified() {
    let base = "enum Kind { DOG = 1, CAT = 2 }\nstruct Pet { 1: required Kind kind }\n";
    assert_eq!(
        worst(base, &base.replace("CAT = 2", "CAT = 3"), Role::Both),
        Severity::Breaking,
        "a value's number is what is sent"
    );
    assert_eq!(
        worst(
            base,
            &base.replace("CAT = 2", "CAT = 2, BIRD = 3"),
            Role::Output
        ),
        Severity::Risky
    );
}
