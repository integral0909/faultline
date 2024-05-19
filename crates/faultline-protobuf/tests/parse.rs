//! The `.proto` grammar: declarations, contextual keywords, options, and
//! recovery from syntax errors.

use faultline_protobuf::ast::{FieldType, ImportKind, Label, OptionValue, Scalar, Syntax, option};
use faultline_protobuf::parser::parse;
use faultline_source::SourceId;

fn ok(text: &str) -> faultline_protobuf::ast::File {
    let (file, diagnostics) = parse(SourceId::from_raw(0), text);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    file
}

fn errors(text: &str) -> Vec<String> {
    let (_, diagnostics) = parse(SourceId::from_raw(0), text);
    diagnostics.iter().map(|d| d.message.clone()).collect()
}

const PETSTORE: &str = r#"
syntax = "proto3";

package pets.v1;

import "google/protobuf/timestamp.proto";
import public "pets/v1/common.proto";

option go_package = "example.com/pets/v1;petsv1";
option (my.file_option) = { name: "x" nested { a: 1 } };

// A pet in the store.
message Pet {
  string id = 1;
  string name = 2 [json_name = "petName", deprecated = true];
  repeated string tags = 3;
  map<string, int64> counters = 4;
  optional int32 age = 5;
  google.protobuf.Timestamp born_at = 6;
  .pets.v1.Status status = 7;

  oneof owner {
    string person = 10;
    Shelter shelter = 11;
  }

  message Shelter {
    string name = 1;
  }

  reserved 8, 9, 100 to 199, 1000 to max;
  reserved "legacy", "old_name";
}

enum Status {
  option allow_alias = true;
  STATUS_UNSPECIFIED = 0;
  STATUS_AVAILABLE = 1;
  STATUS_FOR_SALE = 1;
  STATUS_SOLD = -2 [deprecated = true];
  reserved 5 to 7;
}

service PetService {
  option deprecated = false;
  rpc GetPet(GetPetRequest) returns (Pet);
  rpc WatchPets(stream WatchRequest) returns (stream Pet) {
    option idempotency_level = NO_SIDE_EFFECTS;
  };
  rpc Upload(stream Chunk) returns (Pet) {}
}
"#;

#[test]
fn a_complete_file() {
    let file = ok(PETSTORE);
    assert_eq!(file.syntax, Syntax::Proto3);
    assert_eq!(file.package.as_ref().unwrap().text, "pets.v1");
    assert_eq!(file.imports.len(), 2);
    assert_eq!(file.imports[1].kind, ImportKind::Public);
    assert_eq!(file.imports[0].path, "google/protobuf/timestamp.proto");
    assert_eq!(
        option(&file.options, "go_package").unwrap().value,
        OptionValue::Str(b"example.com/pets/v1;petsv1".to_vec())
    );
    assert_eq!(
        option(&file.options, "(my.file_option)").unwrap().value,
        OptionValue::Aggregate(r#"{ name: "x" nested { a: 1 } }"#.into())
    );

    let pet = &file.messages[0];
    assert_eq!(pet.name.text, "Pet");
    assert_eq!(pet.doc.as_deref(), Some("A pet in the store."));
    let names: Vec<&str> = pet.fields.iter().map(|f| f.name.text.as_str()).collect();
    assert_eq!(
        names,
        [
            "id", "name", "tags", "counters", "age", "born_at", "status", "person", "shelter"
        ]
    );
    let name = &pet.fields[1];
    assert_eq!(
        option(&name.options, "json_name").unwrap().value.as_str(),
        Some("petName")
    );
    assert_eq!(
        option(&name.options, "deprecated").unwrap().value.as_bool(),
        Some(true)
    );
    assert_eq!(pet.fields[2].label, Some(Label::Repeated));
    assert!(matches!(
        &pet.fields[3].ty,
        FieldType::Map(k, v) if **k == FieldType::Scalar(Scalar::String) && **v == FieldType::Scalar(Scalar::Int64)
    ));
    assert_eq!(pet.fields[4].label, Some(Label::Optional));
    let FieldType::Named(status) = &pet.fields[6].ty else {
        panic!()
    };
    assert!(status.absolute);
    assert_eq!(status.to_string(), ".pets.v1.Status");
    assert_eq!(pet.oneofs[0].name.text, "owner");
    assert_eq!(pet.fields[7].oneof, Some(0));
    assert_eq!(pet.fields[8].oneof, Some(0));
    assert_eq!(pet.messages[0].name.text, "Shelter");
    assert_eq!(
        pet.reserved[0].ranges,
        [(8, 8), (9, 9), (100, 199), (1000, i64::MAX)]
    );
    assert_eq!(pet.reserved[1].names, ["legacy", "old_name"]);

    let status = &file.enums[0];
    assert_eq!(
        option(&status.options, "allow_alias")
            .unwrap()
            .value
            .as_bool(),
        Some(true)
    );
    let numbers: Vec<i64> = status.values.iter().map(|v| v.number).collect();
    assert_eq!(numbers, [0, 1, 1, -2]);
    assert_eq!(status.reserved[0].ranges, [(5, 7)]);

    let service = &file.services[0];
    let methods: Vec<(&str, bool, bool)> = service
        .methods
        .iter()
        .map(|m| (m.name.text.as_str(), m.input_stream, m.output_stream))
        .collect();
    assert_eq!(
        methods,
        [
            ("GetPet", false, false),
            ("WatchPets", true, true),
            ("Upload", true, false)
        ]
    );
    assert_eq!(
        service.methods[1].options[0].value,
        OptionValue::Ident("NO_SIDE_EFFECTS".into())
    );
}

#[test]
fn keywords_are_contextual() {
    let file = ok(r#"
        syntax = "proto3";
        message message { }
        message M {
          message message = 1;
          enum enum = 2;
          string syntax = 3;
          int32 map = 4;
          stream stream = 5;
          string rpc = 6;
          string package = 7;
        }
        message enum { }
        message stream { }
    "#);
    let m = &file.messages[1];
    let names: Vec<&str> = m.fields.iter().map(|f| f.name.text.as_str()).collect();
    assert_eq!(
        names,
        [
            "message", "enum", "syntax", "map", "stream", "rpc", "package"
        ]
    );
    assert!(matches!(&m.fields[0].ty, FieldType::Named(t) if t.parts == ["message"]));
}

#[test]
fn proto2_groups_labels_defaults_and_extensions() {
    let file = ok(r#"
        syntax = "proto2";
        message Search {
          required string query = 1;
          optional int32 page = 2 [default = -1];
          optional double ratio = 3 [default = -inf];
          repeated group Result = 4 {
            required string url = 5;
          }
          extensions 100 to 199, 500 to max;
        }
        extend Search {
          optional string trace_id = 150;
        }
    "#);
    assert_eq!(file.syntax, Syntax::Proto2);
    let search = &file.messages[0];
    assert_eq!(search.fields[0].label, Some(Label::Required));
    assert_eq!(
        option(&search.fields[1].options, "default").unwrap().value,
        OptionValue::Int(-1)
    );
    assert_eq!(
        option(&search.fields[2].options, "default").unwrap().value,
        OptionValue::Float(f64::NEG_INFINITY)
    );
    let group = &search.fields[3];
    assert!(group.group);
    assert_eq!(group.name.text, "result");
    assert_eq!(search.messages[0].name.text, "Result");
    assert_eq!(search.messages[0].fields[0].name.text, "url");
    assert_eq!(search.extension_ranges, [(100, 199), (500, i64::MAX)]);
    assert_eq!(file.extends[0].fields[0].name.text, "trace_id");
}

#[test]
fn editions_and_default_syntax() {
    let file = ok(
        r#"edition = "2023"; message A { string a = 1 [features.field_presence = IMPLICIT]; reserved b, c; }"#,
    );
    assert_eq!(file.syntax, Syntax::Edition("2023".into()));
    assert_eq!(
        file.messages[0].fields[0].options[0].name,
        "features.field_presence"
    );
    assert_eq!(file.messages[0].reserved[0].names, ["b", "c"]);
    assert_eq!(ok("message A {}").syntax, Syntax::Proto2);
}

#[test]
fn adjacent_strings_concatenate() {
    let file = ok(r#"syntax = "proto" "3"; import "a/" 'b.proto';"#);
    assert_eq!(file.syntax, Syntax::Proto3);
    assert_eq!(file.imports[0].path, "a/b.proto");
}

#[test]
fn errors_are_reported_once_and_parsing_continues() {
    let errors = errors(
        r#"
        syntax = "proto3";
        message A {
          string a = ;
          int32 b = 2;
          string c 3;
        }
        message B { int32 x = 1; }
        garbage here;
        enum E { X = 0 Y = 1; }
    "#,
    );
    assert_eq!(
        errors,
        [
            "expected a field number, found `;`",
            "expected `=`, found number 3",
            "expected `message`, `enum`, `service` or another declaration, found `garbage`",
            "expected `;`, found `Y`",
        ]
    );
}

#[test]
fn structural_errors() {
    assert_eq!(errors("message A { int32 a = 1;"), ["unclosed `{`"]);
    assert_eq!(errors("}"), ["unmatched `}`"]);
    assert_eq!(
        errors(r#"message A {} syntax = "proto3";"#),
        ["`syntax` must be the first statement of the file"]
    );
    assert_eq!(
        errors(r#"syntax = "proto4";"#),
        [r#"unknown syntax "proto4""#]
    );
    assert_eq!(errors("package a; package b;"), ["a file has one package"]);
    assert_eq!(
        errors("message A { oneof o { repeated int32 x = 1; } }"),
        ["fields in a oneof cannot be `repeated`"]
    );
    assert_eq!(
        errors(r#"message A { string s = 1 [x = "unterminated]; }"#)[0],
        "unterminated string"
    );
}

#[test]
fn deep_nesting_is_refused_without_overflowing() {
    let depth = 10_000;
    let text = format!("{}{}", "message M { ".repeat(depth), "}".repeat(depth));
    let errors = errors(&text);
    assert!(
        errors
            .iter()
            .any(|e| e == "declarations are nested too deeply"),
        "{errors:?}"
    );
    let text = format!("option x = {}{};", "{".repeat(depth), "}".repeat(depth));
    let errors = self::errors(&text);
    assert!(
        errors.iter().any(|e| e == "value is nested too deeply"),
        "{errors:?}"
    );
}
