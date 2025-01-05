//! Lowering Smithy shapes, and how their changes are classified.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, QualName, Role, TypeKind, validate};
use faultline_rules::{Severity, classify};
use faultline_smithy::lower_smithy;
use faultline_source::SourceMap;

fn api(text: &str, role: Role) -> Api {
    let loader = MemoryLoader::new().with("file:///model/main.smithy", text);
    let sources = SourceMap::new();
    let lowered = lower_smithy(
        &UriRef::parse("file:///model/main.smithy").unwrap(),
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

const MODEL: &str = r#"$version: "2"
namespace example.pets

/// A pet.
structure Pet {
    @required
    id: PetId

    @required
    @length(min: 1, max: 64)
    name: String

    age: Age = 0

    kind: Kind

    level: Level

    tags: Tags

    counts: Counts

    born: Timestamp

    @timestampFormat("epoch-seconds")
    seen: Timestamp

    @deprecated
    nickname: String

    owner: Owner
}

@pattern("^[a-z0-9]+$")
string PetId

@range(min: 0, max: 200)
integer Age

enum Kind {
    DOG = "dog"
    CAT
}

intEnum Level {
    LOW = 1
    HIGH = 10
}

@uniqueItems
@length(max: 10)
list Tags {
    member: String
}

map Counts {
    @length(min: 1)
    key: String
    value: Long
}

union Owner {
    person: String
    shelter: Integer
}
"#;

fn field_kind<'a>(api: &'a Api, name: &str) -> &'a TypeKind {
    let pet = api.named[&QualName::new("Pet")];
    let TypeKind::Object(pet) = &api.types.get(pet).kind else {
        panic!()
    };
    &api.types
        .get(api.types.resolve(pet.field(name).unwrap().ty))
        .kind
}

#[test]
fn shapes_are_lowered() {
    let api = api(MODEL, Role::Both);
    let roots: Vec<&str> = api.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(roots, ["Pet", "Owner"]);
    let pet_id = api.named[&QualName::new("Pet")];
    let pet = api.types.get(pet_id);
    assert_eq!(pet.meta.description.as_deref(), Some("A pet."));
    let TypeKind::Object(object) = &pet.kind else {
        panic!()
    };
    let required: Vec<(&str, bool)> = object
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        &required[..3],
        [("id", true), ("name", true), ("age", false)]
    );
    assert_eq!(
        object
            .field("age")
            .unwrap()
            .meta
            .default
            .as_ref()
            .unwrap()
            .to_string(),
        "0"
    );
    assert!(object.field("nickname").unwrap().meta.deprecated);

    assert!(
        matches!(field_kind(&api, "id"), TypeKind::String(s) if s.pattern.as_deref() == Some("^[a-z0-9]+$"))
    );
    assert!(
        matches!(field_kind(&api, "name"), TypeKind::String(s) if s.min_length == 1 && s.max_length == Some(64))
    );
    let TypeKind::Integer(age) = field_kind(&api, "age") else {
        panic!()
    };
    assert_eq!(age.maximum.as_ref().unwrap().value.to_string(), "200");
    let TypeKind::Enum(kind) = field_kind(&api, "kind") else {
        panic!()
    };
    let values: Vec<String> = kind.values.iter().map(|v| v.value.to_string()).collect();
    assert_eq!(values, ["\"dog\"", "\"CAT\""]);
    let TypeKind::Enum(level) = field_kind(&api, "level") else {
        panic!()
    };
    assert_eq!(level.values[1].value.to_string(), "10");
    let TypeKind::Array(tags) = field_kind(&api, "tags") else {
        panic!()
    };
    assert!(tags.unique_items);
    assert_eq!(tags.max_items, Some(10));
    let TypeKind::Object(counts) = field_kind(&api, "counts") else {
        panic!()
    };
    assert!(counts.property_names.is_some());
    assert!(
        matches!(field_kind(&api, "born"), TypeKind::String(s) if s.format.as_deref() == Some("date-time"))
    );
    assert!(matches!(field_kind(&api, "seen"), TypeKind::Number(_)));
    let TypeKind::Object(owner) = field_kind(&api, "owner") else {
        panic!()
    };
    assert_eq!(owner.exclusive[0].fields, ["person", "shelter"]);
    assert_eq!(owner.min_properties, 1);
}

fn worst(old: &str, new: &str, role: Role) -> Severity {
    let a = api(old, role);
    let b = api(new, role);
    faultline_diff::diff(&a, &b)
        .iter()
        .map(|c| classify("pets", c).severity)
        .max()
        .unwrap_or(Severity::Info)
}

#[test]
fn changes_are_classified() {
    assert_eq!(
        worst(MODEL, &MODEL.replace("max: 200", "max: 100"), Role::Input),
        Severity::Breaking
    );
    assert_eq!(
        worst(
            MODEL,
            &MODEL.replace("    CAT\n", "    CAT\n    BIRD\n"),
            Role::Output
        ),
        Severity::Risky
    );
    assert_eq!(
        worst(
            MODEL,
            &MODEL.replace("    kind: Kind", "    @required\n    kind: Kind"),
            Role::Input
        ),
        Severity::Breaking
    );
    assert_eq!(
        worst(
            MODEL,
            &MODEL.replace(
                "    shelter: Integer\n",
                "    shelter: Integer\n    other: String\n"
            ),
            Role::Output
        ),
        Severity::Risky
    );
    assert_eq!(worst(MODEL, MODEL, Role::Both), Severity::Info);
}
