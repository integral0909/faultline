//! Parsing Thrift IDL.

use faultline_source::SourceId;
use faultline_thrift::ast::{
    BaseType, ConstValue, Definition, Document, FieldTypeKind, Requiredness, StructKind,
};
use faultline_thrift::parser::parse;

fn parsed(text: &str) -> (Document, Vec<String>) {
    let (document, diagnostics) = parse(SourceId::from_raw(0), text);
    let messages = diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}: {}",
                d.code.map(|c| c.to_string()).unwrap_or_default(),
                d.message
            )
        })
        .collect();
    (document, messages)
}

const PETS: &str = r#"
include "shared.thrift"
namespace java com.example.pets
namespace * pets
cpp_include "<map>"

const i32 MAX_PETS = 100
const map<string, list<i32>> GROUPS = {"a": [1, 2], "b": []};

typedef i64 PetId (json.type = "string")

/** The kind of pet. */
enum Kind {
  DOG = 1,
  CAT,
  /** Anything else. */
  OTHER = 10 (deprecated)
}

struct Pet {
  1: required PetId id,
  2: optional string name = "Rex";
  3: Kind kind = Kind.DOG
  4: list<shared.Tag> tags,
  5: map<string, set<binary>> extra (python.immutable = "")
  string implicit
} (final = "true")

union Payload { 1: i32 number; 2: string text }

exception NotFound { 1: string message }

service Base { void ping() }

/** Manages pets. */
service Pets extends shared.Base {
  Pet get(1: PetId id) throws (1: NotFound missing),
  oneway void forget(1: PetId id);
  list<Pet> list(1: i32 limit = 10, 2: optional Kind kind)
}
"#;

#[test]
fn a_complete_file() {
    let (document, messages) = parsed(PETS);
    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(document.includes[0].path, "shared.thrift");
    let namespaces: Vec<(&str, &str)> = document
        .namespaces
        .iter()
        .map(|(scope, name)| (scope.as_str(), name.text.as_str()))
        .collect();
    assert_eq!(namespaces, [("java", "com.example.pets"), ("*", "pets")]);
    let names: Vec<&str> = document
        .definitions
        .iter()
        .map(|d| d.name().text.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "MAX_PETS", "GROUPS", "PetId", "Kind", "Pet", "Payload", "NotFound", "Base", "Pets"
        ]
    );

    let Definition::Enum(kind) = &document.definitions[3] else {
        panic!()
    };
    assert_eq!(kind.doc.as_deref(), Some("The kind of pet."));
    let values: Vec<Option<i64>> = kind
        .values
        .iter()
        .map(|v| v.value.map(|(n, _)| n))
        .collect();
    assert_eq!(values, [Some(1), None, Some(10)]);
    assert_eq!(kind.values[2].doc.as_deref(), Some("Anything else."));
    assert_eq!(kind.values[2].annotations[0].0.text, "deprecated");

    let Definition::Struct(pet) = &document.definitions[4] else {
        panic!()
    };
    assert_eq!(pet.kind, StructKind::Struct);
    let fields: Vec<(Option<i64>, Requiredness, &str)> = pet
        .fields
        .iter()
        .map(|f| (f.id.map(|(n, _)| n), f.requiredness, f.name.text.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            (Some(1), Requiredness::Required, "id"),
            (Some(2), Requiredness::Optional, "name"),
            (Some(3), Requiredness::Default, "kind"),
            (Some(4), Requiredness::Default, "tags"),
            (Some(5), Requiredness::Default, "extra"),
            (None, Requiredness::Default, "implicit"),
        ]
    );
    assert_eq!(pet.fields[1].default, Some(ConstValue::Str("Rex".into())));
    assert_eq!(
        pet.fields[2].default,
        Some(ConstValue::Ident("Kind.DOG".into()))
    );
    let FieldTypeKind::List(item) = &pet.fields[3].ty.kind else {
        panic!()
    };
    assert!(matches!(&item.kind, FieldTypeKind::Named(n) if n == "shared.Tag"));
    let FieldTypeKind::Map(key, value) = &pet.fields[4].ty.kind else {
        panic!()
    };
    assert!(matches!(key.kind, FieldTypeKind::Base(BaseType::String)));
    assert!(matches!(&value.kind, FieldTypeKind::Set(_)));
    assert_eq!(pet.annotations[0].1.as_deref(), Some("true"));

    let Definition::Service(service) = &document.definitions[8] else {
        panic!()
    };
    assert_eq!(service.extends.as_ref().unwrap().text, "shared.Base");
    assert_eq!(service.doc.as_deref(), Some("Manages pets."));
    let get = &service.functions[0];
    assert!(!get.oneway);
    assert_eq!(get.throws[0].name.text, "missing");
    assert!(service.functions[1].oneway && service.functions[1].returns.is_none());
    assert_eq!(service.functions[2].arguments.len(), 2);
}

#[test]
fn syntax_errors_are_reported_and_parsing_resumes() {
    let (document, messages) = parsed(
        "struct A { 1: i32 }\nstruct B { 1: i32 b }\nenum E { X = 'a' }\nservice S { void f( }\nstruct C {}",
    );
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(messages.iter().all(|m| m.starts_with("FLE8001")));
    let names: Vec<&str> = document
        .definitions
        .iter()
        .map(|d| d.name().text.as_str())
        .collect();
    assert_eq!(names, ["B", "C"]);
}

#[test]
fn deprecated_syntax_is_read_with_a_warning() {
    let (document, messages) =
        parsed("senum S { 'a', 'b' }\nstruct A { 1: slist s }\nservice T { async void f() }");
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(messages.iter().all(|m| m.starts_with("FLE8009")));
    let Definition::Service(service) = &document.definitions[1] else {
        panic!()
    };
    assert!(service.functions[0].oneway);
}

#[test]
fn deep_nesting_is_refused() {
    let deep = format!("const {}i32{} X = 1", "list<".repeat(100), ">".repeat(100));
    let (_, messages) = parsed(&deep);
    assert!(!messages.is_empty());
}
