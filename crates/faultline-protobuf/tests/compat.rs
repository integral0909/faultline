//! Compatibility of realistic Protobuf changes, end to end: lower both
//! versions, diff, classify.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, Role};
use faultline_protobuf::lower_protobuf;
use faultline_rules::{Finding, Severity, classify};
use faultline_source::SourceMap;

fn api(text: &str) -> Api {
    let loader = MemoryLoader::new().with("file:///api.proto", text);
    let sources = SourceMap::new();
    let lowered = lower_protobuf(
        &UriRef::parse("file:///api.proto").unwrap(),
        &loader,
        &sources,
        "api",
        Role::Both,
    );
    let errors: Vec<&str> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.as_str())
        .collect();
    assert!(errors.is_empty(), "{text}\n{errors:?}");
    lowered.api.unwrap()
}

fn findings(old: &str, new: &str) -> Vec<Finding> {
    let changes = faultline_diff::diff(&api(old), &api(new));
    changes.iter().map(|c| classify("api", c)).collect()
}

/// `code severity path` per finding.
fn summary(old: &str, new: &str) -> Vec<String> {
    findings(old, new)
        .iter()
        .map(|f| format!("{} {} {}", f.rule.code, f.severity, f.change.path))
        .collect()
}

/// A service whose request and response are both `Pet`, with `body` as the
/// message's fields.
fn service(body: &str) -> String {
    format!(
        r#"syntax = "proto3";
package pets.v1;
message Pet {{
{body}
}}
service PetService {{
  rpc UpdatePet(Pet) returns (Pet);
}}
"#
    )
}

const REQUEST: &str = "op[pets.v1.PetService/UpdatePet]/request[application/grpc]";
const RESPONSE: &str = "op[pets.v1.PetService/UpdatePet]/response[default]/body[application/grpc]";

fn has(list: &[String], expected: &str) -> bool {
    list.iter().any(|s| s == expected)
}

#[test]
fn identical_files_have_no_findings() {
    let text = service("string name = 1;\nint32 age = 2;");
    assert!(summary(&text, &text).is_empty());
}

#[test]
fn renaming_a_field_is_risky_but_renumbering_breaks() {
    let old = service("string name = 1;");
    let renamed = summary(&old, &service("string title = 1;"));
    assert!(
        has(&renamed, &format!("FL1025 risky {REQUEST}.title")),
        "{renamed:?}"
    );
    assert!(
        !renamed
            .iter()
            .any(|s| s.starts_with("FL1002") || s.starts_with("FL1003")),
        "a rename is not a removal and an addition: {renamed:?}"
    );
    let renumbered = summary(&old, &service("string name = 2;"));
    assert!(
        has(&renumbered, &format!("FL1026 breaking {REQUEST}.name")),
        "{renumbered:?}"
    );
}

#[test]
fn wire_encodings() {
    let old = service("int32 count = 1;");
    let zigzag = summary(&old, &service("sint32 count = 1;"));
    assert!(
        has(&zigzag, &format!("FL1031 breaking {REQUEST}.count")),
        "{zigzag:?}"
    );
    // int32 to int64: same encoding. Requests accept more; responses may
    // carry values old clients truncate.
    let widened = summary(&old, &service("int64 count = 1;"));
    assert!(
        !widened.iter().any(|s| s.starts_with("FL1031")),
        "{widened:?}"
    );
    assert!(
        has(&widened, &format!("FL1007 info {REQUEST}.count")),
        "{widened:?}"
    );
    assert!(
        has(&widened, &format!("FL1007 risky {RESPONSE}.count")),
        "{widened:?}"
    );
    let findings = findings(&old, &service("int64 count = 1;"));
    let bound = findings
        .iter()
        .find(|f| {
            f.rule.code == "FL1007" && f.change.polarity.is_out() && f.message.contains("maximum")
        })
        .unwrap();
    assert_eq!(
        bound.witness().unwrap().to_string(),
        "9223372036854775807",
        "a value an int32 cannot hold"
    );
}

#[test]
fn moving_fields_into_a_oneof_breaks_senders() {
    let old = service("string email = 1;\nstring phone = 2;");
    let new = service("oneof contact {\n string email = 1;\n string phone = 2;\n}");
    let found = findings(&old, &new);
    let into: Vec<&Finding> = found
        .iter()
        .filter(|f| f.rule.code == "FL1028" && f.change.polarity.is_in())
        .collect();
    assert_eq!(into.len(), 2, "{:?}", summary(&old, &new));
    assert_eq!(into[0].severity, Severity::Breaking);
    assert_eq!(into[0].message, "field `email` moved into oneof `contact`");
    assert_eq!(
        into[0].witness().unwrap().to_string(),
        r#"{"email":"","phone":""}"#
    );
    // Responses: receivers never see both set, which they could before;
    // nothing new arrives.
    assert!(
        found
            .iter()
            .filter(|f| f.rule.code == "FL1028" && !f.change.polarity.is_in())
            .all(|f| f.severity == Severity::Info),
        "{:?}",
        summary(&old, &new)
    );
    // Adding a new field to an existing oneof is fine for senders.
    let grown = summary(
        &new,
        &service("oneof contact {\n string email = 1;\n string phone = 2;\n string fax = 3;\n}"),
    );
    assert!(
        !grown
            .iter()
            .any(|s| s.contains("breaking") && s.contains(REQUEST)),
        "{grown:?}"
    );
}

fn enums(values: &str) -> String {
    format!(
        r#"syntax = "proto3";
package pets.v1;
enum Species {{
{values}
}}
message Pet {{ Species species = 1; }}
service PetService {{
  rpc UpdatePet(Pet) returns (Pet);
}}
"#
    )
}

#[test]
fn enum_values_by_number_and_name() {
    let old = enums("SPECIES_UNSPECIFIED = 0;\nSPECIES_CAT = 1;\nSPECIES_DOG = 2;");
    let renamed = summary(
        &old,
        &enums("SPECIES_UNSPECIFIED = 0;\nSPECIES_FELINE = 1;\nSPECIES_DOG = 2;"),
    );
    // Reported where the enum is used.
    assert_eq!(
        renamed,
        [
            format!("FL1030 risky {REQUEST}.species"),
            format!("FL1030 risky {RESPONSE}.species")
        ],
        "{renamed:?}"
    );

    let renumbered = summary(
        &old,
        &enums("SPECIES_UNSPECIFIED = 0;\nSPECIES_CAT = 3;\nSPECIES_DOG = 2;"),
    );
    assert!(
        renumbered.iter().any(|s| s.starts_with("FL1029 breaking")),
        "{renumbered:?}"
    );
}

#[test]
fn streaming_and_operations() {
    let unary = service("string name = 1;");
    let streaming = unary.replace("returns (Pet)", "returns (stream Pet)");
    let found = summary(&unary, &streaming);
    assert_eq!(
        found,
        ["FL3001 breaking op[pets.v1.PetService/UpdatePet]"],
        "{found:?}"
    );

    let removed = unary.replace("rpc UpdatePet(Pet) returns (Pet);", "");
    let found = summary(&unary, &removed);
    assert!(
        has(&found, "FL2001 breaking op[pets.v1.PetService/UpdatePet]"),
        "{found:?}"
    );

    // A new package moves every method to a new gRPC path.
    let moved = unary.replace("package pets.v1;", "package pets.v2;");
    let found = summary(&unary, &moved);
    assert!(
        has(&found, "FL2001 breaking op[pets.v1.PetService/UpdatePet]"),
        "{found:?}"
    );
    assert!(
        has(&found, "FL2002 info op[pets.v2.PetService/UpdatePet]"),
        "{found:?}"
    );
}

#[test]
fn renaming_a_message_type_is_invisible_on_the_wire() {
    let old = service("string name = 1;");
    let new = old
        .replace("message Pet", "message Animal")
        .replace("(Pet)", "(Animal)");
    let found = summary(&old, &new);
    assert!(!found.iter().any(|s| s.contains("breaking")), "{found:?}");
}

#[test]
fn proto2_presence_and_defaults() {
    let v1 = r#"syntax = "proto2";
        message Pet { optional string name = 1 [default = "rex"]; }
        service S { rpc Put(Pet) returns (Pet); }"#;
    let required = summary(v1, &v1.replace("optional string", "required string"));
    assert!(
        required
            .iter()
            .any(|s| s.starts_with("FL1004 breaking op[S/Put]/request")),
        "{required:?}"
    );
    let default = summary(v1, &v1.replace("\"rex\"", "\"max\""));
    assert!(
        default
            .iter()
            .any(|s| s.starts_with("FL1022 risky") && s.ends_with("request[application/grpc].name")),
        "{default:?}"
    );
}

#[test]
fn deprecating_a_field_is_informational() {
    let old = service("string name = 1;");
    let found = summary(&old, &service("string name = 1 [deprecated = true];"));
    assert!(
        has(&found, &format!("FL1020 info {REQUEST}.name")),
        "{found:?}"
    );
}

#[test]
fn json_names_are_aliases() {
    let old = service("string display_name = 1;");
    let found = summary(
        &old,
        &service("string display_name = 1 [json_name = \"name\"];"),
    );
    assert!(
        has(&found, &format!("FL1027 risky {REQUEST}.display_name")),
        "{found:?}"
    );
}
