//! Loading, linking and lowering multi-file workspaces.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{OperationKey, Role, TypeKind, dump, validate};
use faultline_protobuf::{Lowered, lower_protobuf};
use faultline_source::{Severity, SourceMap};

/// Lowers `entry` from a workspace of `(path, text)` files.
fn lower(files: &[(&str, &str)], entry: &str) -> Lowered {
    let mut loader = MemoryLoader::new();
    for (path, text) in files {
        loader.insert(&format!("file:///{path}"), *text);
    }
    let sources = SourceMap::new();
    lower_protobuf(
        &UriRef::parse(&format!("file:///{entry}")).unwrap(),
        &loader,
        &sources,
        "pets",
        Role::Both,
    )
}

fn ok(files: &[(&str, &str)], entry: &str) -> faultline_ir::Api {
    let lowered = lower(files, entry);
    let errors: Vec<&str> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.as_str())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), [], "the IR is well-formed");
    api
}

/// `(code, message)` of every diagnostic.
fn diagnostics(files: &[(&str, &str)], entry: &str) -> Vec<(String, String)> {
    lower(files, entry)
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

const COMMON: &str = r#"
syntax = "proto3";
package pets.v1;
enum Species {
  SPECIES_UNSPECIFIED = 0;
  SPECIES_CAT = 1;
  SPECIES_DOG = 2;
}
"#;

const PETS: &str = r#"
syntax = "proto3";
package pets.v1;

import "google/protobuf/timestamp.proto";
import "pets/v1/common.proto";

// A pet.
message Pet {
  string id = 1;
  string display_name = 2 [json_name = "name"];
  Species species = 3;
  repeated string tags = 4;
  map<string, int64> counters = 5;
  google.protobuf.Timestamp born_at = 6;
  uint32 age = 7 [deprecated = true];
  oneof owner {
    string person = 8;
    Pet parent = 9;
  }
}

message GetPetRequest { string id = 1; }
message ListPetsRequest { int32 page_size = 1; }

service PetService {
  rpc GetPet(GetPetRequest) returns (Pet);
  rpc ListPets(ListPetsRequest) returns (stream Pet);
}
"#;

fn workspace() -> Vec<(&'static str, &'static str)> {
    vec![
        ("proto/pets/v1/pets.proto", PETS),
        ("proto/pets/v1/common.proto", COMMON),
    ]
}

#[test]
fn services_become_rpc_operations() {
    let api = ok(&workspace(), "proto/pets/v1/pets.proto");
    let keys: Vec<String> = api.operations.iter().map(|op| op.key.identity()).collect();
    assert_eq!(
        keys,
        ["pets.v1.PetService/GetPet", "pets.v1.PetService/ListPets"]
    );
    assert!(matches!(
        &api.operations[1].key,
        OperationKey::Rpc { service, method } if service == "pets.v1.PetService" && method == "ListPets"
    ));
    let streaming = api.operations[1].streaming.unwrap();
    assert!(!streaming.request && streaming.response);
    assert!(
        api.schemas.is_empty(),
        "files with services have no schema roots"
    );

    let text = dump(&api);
    for line in [
        "op[pets.v1.PetService/GetPet]/request[application/grpc] [in] -> pets.v1.GetPetRequest",
        "op[pets.v1.PetService/ListPets]/response[default]/body[application/grpc] [out] -> pets.v1.Pet",
        "op[pets.v1.PetService/ListPets] streams response",
        "at most one of person, parent (owner)",
    ] {
        assert!(text.contains(line), "missing {line:?} in\n{text}");
    }
}

#[test]
fn messages_carry_wire_details() {
    let api = ok(&workspace(), "proto/pets/v1/pets.proto");
    let pet = api.named_type("pets.v1.Pet").unwrap();
    let TypeKind::Object(obj) = &api.types.get(pet).kind else {
        panic!()
    };
    let tags: Vec<(String, Option<u32>)> =
        obj.fields.iter().map(|f| (f.name.clone(), f.tag)).collect();
    assert_eq!(tags[0], ("id".into(), Some(1)));
    assert_eq!(obj.fields[1].aliases, ["name"], "explicit json_name");
    assert_eq!(
        obj.fields[6].aliases,
        Vec::<String>::new(),
        "json name equals the name"
    );
    assert!(obj.fields[6].meta.deprecated);
    assert!(
        matches!(
            api.types.get(api.types.resolve(obj.fields[3].ty)).kind,
            TypeKind::Array(_)
        ),
        "repeated fields are arrays"
    );
    assert!(matches!(
        api.types.get(api.types.resolve(obj.fields[4].ty)).kind,
        TypeKind::Map(_)
    ));
    let TypeKind::Integer(age) = &api.types.get(obj.fields[6].ty).kind else {
        panic!()
    };
    assert_eq!(age.format.as_deref(), Some("uint32"));
    assert_eq!(age.encoding.as_deref(), Some("varint"));
    assert_eq!(
        age.maximum.as_ref().unwrap().value.to_string(),
        "4294967295"
    );
    assert!(api.named_type("google.protobuf.Timestamp").is_some());

    let species = api.named_type("pets.v1.Species").unwrap();
    let TypeKind::Enum(e) = &api.types.get(species).kind else {
        panic!()
    };
    assert!(e.open, "proto3 enums are open");
    assert_eq!(e.values[1].number, Some(1));
    assert_eq!(e.values[1].name.as_deref(), Some("SPECIES_CAT"));
}

#[test]
fn data_files_expose_top_level_messages_as_roots() {
    let api = ok(
        &[(
            "events.proto",
            r#"syntax = "proto2";
               package events;
               message Created { required string id = 1; optional int32 version = 2 [default = 1]; }
               message Deleted { required string id = 1; }
               enum Kind { CREATED = 1; DELETED = 2; }"#,
        )],
        "events.proto",
    );
    let roots: Vec<&str> = api.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(roots, ["events.Created", "events.Deleted"]);
    let created = api.named_type("events.Created").unwrap();
    let TypeKind::Object(obj) = &api.types.get(created).kind else {
        panic!()
    };
    assert!(obj.fields[0].required, "proto2 required");
    assert_eq!(
        obj.fields[1].meta.default.as_ref().unwrap().to_string(),
        "1"
    );
    let kind = api.named_type("events.Kind").unwrap();
    let TypeKind::Enum(e) = &api.types.get(kind).kind else {
        panic!()
    };
    assert!(!e.open, "proto2 enums are closed");
}

#[test]
fn imports_resolve_from_ancestor_directories_and_the_root() {
    // `pets/v1/common.proto` from proto/pets/v1/ is proto/pets/v1/common.proto,
    // found through the `proto/` ancestor.
    ok(&workspace(), "proto/pets/v1/pets.proto");
    // From the workspace root.
    ok(
        &[
            (
                "a/main.proto",
                r#"syntax = "proto3"; import "lib/x.proto"; message M { X x = 1; }"#,
            ),
            ("lib/x.proto", r#"syntax = "proto3"; message X {}"#),
        ],
        "a/main.proto",
    );
}

#[test]
fn name_resolution_follows_protoc_scoping() {
    let files = [
        (
            "main.proto",
            r#"syntax = "proto3";
               package a.b;
               import "c.proto";
               message Outer {
                 message Inner {}
                 Inner inner = 1;            // nested
                 Outer.Inner qualified = 2;  // from the parent scope
                 c.D other = 3;              // another package
                 .c.D absolute = 4;
               }"#,
        ),
        ("c.proto", r#"syntax = "proto3"; package c; message D {}"#),
    ];
    ok(&files, "main.proto");

    // A nearer `c` shadows the package: `c.D` then means `a.b.c.D`.
    let shadowed = [
        (
            "main.proto",
            r#"syntax = "proto3";
               package a.b;
               import "c.proto";
               message c {}
               message M { c.D d = 1; }"#,
        ),
        ("c.proto", r#"syntax = "proto3"; package c; message D {}"#),
    ];
    let found = diagnostics(&shadowed, "main.proto");
    assert!(
        found.contains(&("FLE3003".into(), "cannot find type `c.D`".into())),
        "{found:?}"
    );
}

#[test]
fn types_must_be_imported_to_be_visible() {
    let files = [
        (
            "a.proto",
            r#"syntax = "proto3"; import "b.proto"; message A { C c = 1; B b = 2; }"#,
        ),
        (
            "b.proto",
            r#"syntax = "proto3"; import "c.proto"; message B { C c = 1; }"#,
        ),
        ("c.proto", r#"syntax = "proto3"; message C {}"#),
    ];
    assert_eq!(
        diagnostics(&files, "a.proto"),
        [(
            "FLE3003".into(),
            "`C` is defined in c.proto, which is not imported".into()
        )]
    );
    // `import public` re-exports.
    let public = [
        files[0],
        (
            "b.proto",
            r#"syntax = "proto3"; import public "c.proto"; message B { C c = 1; }"#,
        ),
        files[2],
    ];
    ok(&public, "a.proto");
}

#[test]
fn semantic_errors() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "message A { string a = 1; int32 b = 1; }",
            "FLE3006",
            "field number 1 is used twice",
        ),
        (
            "message A { string a = 1; int32 a = 2; }",
            "FLE3004",
            "field `a` is declared twice",
        ),
        (
            "message A { string a = 0; }",
            "FLE3005",
            "field numbers must be between 1 and 536870911",
        ),
        (
            "message A { string a = 19500; }",
            "FLE3005",
            "field numbers 19000 to 19999 are reserved for the Protobuf implementation",
        ),
        (
            "message A { reserved 3; string a = 3; }",
            "FLE3007",
            "field number 3 is reserved",
        ),
        (
            "message A { reserved \"a\"; string a = 3; }",
            "FLE3007",
            "field name `a` is reserved",
        ),
        (
            "message A { required string a = 1; }",
            "FLE3011",
            "`required` is only allowed in proto2",
        ),
        (
            "message A { string a = 1 [default = \"x\"]; }",
            "FLE3011",
            "default values are only allowed in proto2",
        ),
        (
            "enum E { A = 1; }",
            "FLE3011",
            "the first value of an open enum must be zero",
        ),
        (
            "enum E { A = 0; B = 0; }",
            "FLE3006",
            "enum number 0 is used twice without `allow_alias`",
        ),
        (
            "enum E { A = 0; } enum F { A = 0; }",
            "FLE3004",
            "`A` is already defined as an enum value",
        ),
        (
            "message A {} message A {}",
            "FLE3004",
            "`A` is already defined as a message",
        ),
        (
            "message A { map<double, string> m = 1; }",
            "FLE3008",
            "map keys must be integers, `bool` or `string`",
        ),
        (
            "enum E { A = 0; } service S { rpc R(E) returns (E); }",
            "FLE3009",
            "an RPC input or output must be a message",
        ),
        (
            "message A { Missing m = 1; }",
            "FLE3003",
            "cannot find type `Missing`",
        ),
    ];
    for (body, code, message) in cases {
        let text = format!("syntax = \"proto3\";\n{body}");
        let found = diagnostics(&[("a.proto", &text)], "a.proto");
        assert!(
            found.iter().any(|(c, m)| c == code && m == message),
            "{body}\nexpected {code} {message:?}, got {found:?}"
        );
    }
}

#[test]
fn proto2_fields_need_labels() {
    let found = diagnostics(
        &[(
            "a.proto",
            "syntax = \"proto2\"; message A { string a = 1; }",
        )],
        "a.proto",
    );
    assert_eq!(
        found,
        [(
            "FLE3011".into(),
            "proto2 fields need a label: `optional`, `required` or `repeated`".into()
        )]
    );
}

#[test]
fn import_problems() {
    let missing = diagnostics(
        &[("a.proto", r#"syntax = "proto3"; import "nowhere.proto";"#)],
        "a.proto",
    );
    assert_eq!(missing[0].0, "FLE3002");
    assert_eq!(missing[0].1, "cannot find import \"nowhere.proto\"");

    let cycle = diagnostics(
        &[
            (
                "a.proto",
                r#"syntax = "proto3"; import "b.proto"; message A {}"#,
            ),
            (
                "b.proto",
                r#"syntax = "proto3"; import "a.proto"; message B { A a = 1; }"#,
            ),
        ],
        "a.proto",
    );
    assert!(
        cycle
            .iter()
            .any(|(c, m)| c == "FLE3010" && m == "import cycle: a.proto -> b.proto -> a.proto"),
        "{cycle:?}"
    );

    let unused = lower(
        &[
            (
                "a.proto",
                r#"syntax = "proto3"; import "b.proto"; message A {}"#,
            ),
            ("b.proto", r#"syntax = "proto3"; message B {}"#),
        ],
        "a.proto",
    );
    assert!(unused.api.is_some(), "an unused import is only a warning");
    let warning = unused.diagnostics.iter().next().unwrap();
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.message, "\"b.proto\" is imported but not used");

    let escape = diagnostics(
        &[(
            "a.proto",
            r#"syntax = "proto3"; import "../../etc/x.proto";"#,
        )],
        "a.proto",
    );
    assert_eq!(escape[0].0, "FLE3002");
}

#[test]
fn recursive_messages_lower_without_recursion() {
    let api = ok(
        &[(
            "tree.proto",
            r#"syntax = "proto3"; message Node { repeated Node children = 1; Node parent = 2; }"#,
        )],
        "tree.proto",
    );
    assert!(api.named_type("Node").is_some());
}
