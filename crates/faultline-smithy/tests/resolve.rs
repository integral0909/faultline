//! Loading a model's files and resolving its names.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_smithy::ast::ShapeKind;
use faultline_smithy::load::load;
use faultline_smithy::resolve::{Resolved, resolve};
use faultline_source::SourceMap;

fn resolved(files: &[(&str, &str)]) -> (Resolved, Vec<String>) {
    let mut loader = MemoryLoader::new();
    for (uri, text) in files {
        loader = loader.with(uri, *text);
    }
    let sources = SourceMap::new();
    let loaded = load(&UriRef::parse(files[0].0).unwrap(), &loader, &sources);
    let model = resolve(&loaded.files);
    let codes = loaded
        .diagnostics
        .iter()
        .chain(model.diagnostics.iter())
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (model, codes)
}

const MAIN: &str = r#"$version: "2"
namespace example.pets
use example.common#Tag

service Pets {
    version: "2024-01-01"
    resources: [Pet]
    errors: [Throttled]
}

resource Pet {
    identifiers: { petId: PetId }
    properties: { name: String }
    read: GetPet
    operations: [TagPet]
}

string PetId

@readonly
operation GetPet {
    input := for Pet {
        @required
        $petId
    }
    output := for Pet with [Audited] {
        $name
        tags: TagList
    }
}

operation TagPet {
    input: TagPetInput
    errors: [NotFound]
}

structure TagPetInput {
    @required
    tag: Tag
}

list TagList { member: Tag }

@mixin
structure Audited {
    @documentation("Who last changed it.")
    changedBy: String
}

apply Audited$changedBy @required
"#;

const OTHER: &str = r#"$version: "2"
namespace example.pets

@error("client")
structure NotFound {}

@error("server")
@retryable
structure Throttled {}
"#;

const COMMON: &str = "$version: \"2\"\nnamespace example.common\nstring Tag\n";

#[test]
fn a_model_across_files() {
    let (model, codes) = resolved(&[
        ("file:///model/main.smithy", MAIN),
        ("file:///model/errors.smithy", OTHER),
        ("file:///model/common.smithy", COMMON),
        ("file:///model/README.md", "not a model"),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    let service = model.get("example.pets#Pets").unwrap();
    assert_eq!(service.version.as_deref(), Some("2024-01-01"));
    assert_eq!(service.bindings.resources, ["example.pets#Pet"]);
    assert_eq!(service.bindings.errors, ["example.pets#Throttled"]);
    let pet = model.get("example.pets#Pet").unwrap();
    assert_eq!(
        pet.bindings.operations,
        ["example.pets#TagPet", "example.pets#GetPet"]
    );

    let get = model.get("example.pets#GetPet").unwrap();
    let op = get.operation.as_ref().unwrap();
    assert_eq!(op.input.as_deref(), Some("example.pets#GetPetInput"));
    let input = model.get("example.pets#GetPetInput").unwrap();
    assert_eq!(
        input.members[0].target, "example.pets#PetId",
        "elided, from the resource"
    );
    assert!(input.members[0].has_trait("required"));

    let output = model.get("example.pets#GetPetOutput").unwrap();
    let members: Vec<(&str, &str)> = output
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.target.as_str()))
        .collect();
    assert_eq!(
        members,
        [
            ("changedBy", "smithy.api#String"),
            ("name", "smithy.api#String"),
            ("tags", "example.pets#TagList"),
        ],
        "mixin members first"
    );
    assert!(
        output.members[0].has_trait("required"),
        "applied to the mixin's member"
    );
    assert!(output.members[0].has_trait("documentation"));
    assert!(!output.has_trait("mixin"), "@mixin is not inherited");

    let tag = &model.get("example.pets#TagPetInput").unwrap().members[0];
    assert_eq!(tag.target, "example.common#Tag", "through `use`");
    assert_eq!(
        model.get("example.pets#Throttled").unwrap().kind,
        ShapeKind::Structure
    );
}

#[test]
fn problems_are_reported() {
    let cases: &[(&str, &[&str])] = &[
        ("namespace a\nstructure A { b: Missing }", &["FLE8402"]),
        ("namespace a\nstructure A {}\nstring A", &["FLE8403"]),
        (
            "namespace a\nstructure A { b: String, b: String }",
            &["FLE8403"],
        ),
        (
            "namespace a\noperation O { input: S }\nstring S",
            &["FLE8404"],
        ),
        (
            "namespace a\noperation O { errors: [E] }\nstructure E {}",
            &["FLE8404"],
        ),
        (
            "namespace a\nservice S { operations: [T] }\nstructure T {}",
            &["FLE8404"],
        ),
        (
            "namespace a\nstructure A with [B] {}\nstructure B {}",
            &["FLE8404"],
        ),
        ("namespace a\nstructure A with [Nope] {}", &["FLE8402"]),
        ("namespace a\n@mixin\nstructure A with [A] {}", &["FLE8406"]),
        ("namespace a\nstructure A { $b }", &["FLE8406"]),
        ("structure A {}", &["FLE8401"]),
        ("namespace a\napply Nope @required", &["FLE8402"]),
        (
            "namespace a\nstructure A {}\napply A$b @required",
            &["FLE8402"],
        ),
        ("$version: \"1.0\"\nnamespace a\nstring S", &["FLE8405"]),
    ];
    for (text, expected) in cases {
        let (_, mut codes) = resolved(&[("file:///m/a.smithy", text)]);
        codes.sort();
        assert_eq!(&codes, expected, "{text}");
    }
}

#[test]
fn build_configurations_name_the_model_files() {
    let config =
        r#"{"version": "1.0", "sources": ["model/"], "imports": ["../shared/common.smithy"]}"#;
    let service = "$version: \"2\"\nnamespace example.weather\nservice Weather { operations: [Ping] }\noperation Ping { output: PingOutput }\n";
    let output = "$version: \"2\"\nnamespace example.weather\nuse example.common#Stamp\nstructure PingOutput { at: Stamp }\n";
    let common = "$version: \"2\"\nnamespace example.common\ntimestamp Stamp\n";
    let (model, codes) = resolved(&[
        ("file:///weather/smithy-build.json", config),
        ("file:///weather/model/a.smithy", output),
        ("file:///weather/model/b.smithy", service),
        ("file:///weather/model/nested/ignored.smithy", "not read"),
        ("file:///shared/common.smithy", common),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    let ping = model.get("example.weather#PingOutput").unwrap();
    assert_eq!(ping.members[0].target, "example.common#Stamp");

    let (_, codes) = resolved(&[("file:///w/smithy-build.json", r#"{"version": "1.0"}"#)]);
    assert_eq!(codes, ["FLE8402"]);
}
