//! The HTTP/JSON view of gRPC services, and its consistency with OpenAPI
//! descriptions of the same API.

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, ParamLocation, TypeKind, validate};
use faultline_protobuf::lower_protobuf_http;
use faultline_rules::{Finding, Severity, classify};
use faultline_source::{Limits, SourceMap};

fn lowered(text: &str) -> faultline_protobuf::Lowered {
    let loader = MemoryLoader::new().with("file:///pets.proto", text);
    let sources = SourceMap::new();
    lower_protobuf_http(
        &UriRef::parse("file:///pets.proto").unwrap(),
        &loader,
        &sources,
        "pets",
    )
}

fn view(text: &str) -> Api {
    let lowered = lowered(text);
    let problems: Vec<String> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect();
    assert!(problems.is_empty(), "{problems:?}");
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

fn openapi(text: &str) -> Api {
    let loader = MemoryLoader::new().with("file:///openapi.yaml", text);
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    let lowered = faultline_openapi::lower_openapi(
        &UriRef::parse("file:///openapi.yaml").unwrap(),
        &mut store,
        "pets",
    );
    assert!(
        !lowered.diagnostics.has_errors(),
        "{:?}",
        lowered.diagnostics
    );
    lowered.api.unwrap()
}

/// Findings of the service against its description: promises it breaks.
fn conformance(description: &str, service: &str) -> Vec<Finding> {
    faultline_diff::diff(&openapi(description), &view(service))
        .iter()
        .map(|c| classify("pets", c))
        .collect()
}

fn summary(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|f| format!("{} {} {}", f.severity, f.rule.name, f.change.path))
        .collect()
}

const PROTO: &str = r#"
syntax = "proto3";
package pets.v1;
import "google/api/annotations.proto";
import "google/api/field_behavior.proto";
import "google/protobuf/timestamp.proto";

service PetService {
  rpc GetPet(GetPetRequest) returns (Pet) {
    option (google.api.http) = {get: "/v1/{name=pets/*}"};
  }
  rpc ListPets(ListPetsRequest) returns (ListPetsResponse) {
    option (google.api.http) = {
      get: "/v1/pets"
      additional_bindings {get: "/v1/shelters/{shelter}/pets"}
    };
  }
  rpc CreatePet(CreatePetRequest) returns (Pet) {
    option (google.api.http) = {post: "/v1/pets" body: "pet"};
  }
  rpc UpdatePet(UpdatePetRequest) returns (Pet) {
    option (google.api.http) = {patch: "/v1/{pet.name=pets/*}" body: "*"};
  }
  rpc WatchPets(ListPetsRequest) returns (stream Pet);
  rpc Internal(GetPetRequest) returns (Pet);
}

message Pet {
  string name = 1;
  string display_name = 2 [(google.api.field_behavior) = REQUIRED];
  Species species = 3;
  int64 weight_grams = 4;
  google.protobuf.Timestamp create_time = 5;
  bytes photo = 6;
  map<string, string> labels = 7;
  oneof owner {
    string person = 8;
    string shelter = 9;
  }
}

enum Species {
  SPECIES_UNSPECIFIED = 0;
  DOG = 1;
  CAT = 2;
}

message GetPetRequest {
  string name = 1 [(google.api.field_behavior) = REQUIRED];
}

message Filter {
  Species species = 1;
  bool vaccinated = 2;
}

message ListPetsRequest {
  string shelter = 1;
  int32 page_size = 2;
  string page_token = 3;
  Filter filter = 4;
  repeated string tags = 5;
}

message ListPetsResponse {
  repeated Pet pets = 1;
  string next_page_token = 2;
}

message CreatePetRequest {
  Pet pet = 1 [(google.api.field_behavior) = REQUIRED];
  string pet_id = 2;
}

message UpdatePetRequest {
  Pet pet = 1;
  string update_mask = 2;
}
"#;

#[test]
fn rules_become_operations() {
    let api = view(PROTO);
    let ops: Vec<(String, Option<String>)> = api
        .operations
        .iter()
        .map(|o| (o.key.to_string(), o.operation_id.clone()))
        .collect();
    let id = |s: &str| Some(s.to_owned());
    assert_eq!(
        ops,
        [
            ("GET /v1/{name}".to_owned(), id("PetService_GetPet")),
            ("GET /v1/pets".to_owned(), id("PetService_ListPets")),
            (
                "GET /v1/shelters/{shelter}/pets".to_owned(),
                id("PetService_ListPets")
            ),
            ("POST /v1/pets".to_owned(), id("PetService_CreatePet")),
            (
                "PATCH /v1/{pet.name}".to_owned(),
                id("PetService_UpdatePet")
            ),
        ],
        "streaming RPCs and RPCs without rules are not exposed"
    );

    let get = &api.operations[0];
    assert_eq!(get.parameters.len(), 1);
    assert_eq!(get.parameters[0].location, ParamLocation::Path);
    assert!(get.request_body.is_none());
    assert_eq!(get.responses[0].status.to_string(), "200");

    // Without a body, the other fields are query parameters, nested
    // messages flattened.
    let list = &api.operations[1];
    let query: Vec<&str> = list.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        query,
        [
            "shelter",
            "pageSize",
            "pageToken",
            "filter.species",
            "filter.vaccinated",
            "tags"
        ]
    );
    // A path variable is not also a query parameter.
    let by_shelter = &api.operations[2];
    let names: Vec<(&str, ParamLocation)> = by_shelter
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location))
        .collect();
    assert_eq!(names[0], ("shelter", ParamLocation::Path));
    assert!(!names[1..].iter().any(|(n, _)| *n == "shelter"));

    // `body: "pet"`: the field is the body, the rest is the query.
    let create = &api.operations[3];
    assert_eq!(create.parameters.len(), 1);
    assert_eq!(create.parameters[0].name, "petId");
    let body = create.request_body.as_ref().unwrap();
    assert!(!body.required);
    assert_eq!(body.content[0].media_type, "application/json");
    assert_eq!(
        api.types.resolve(body.content[0].ty),
        api.named_type("pets.v1.Pet").unwrap()
    );

    // `body: "*"` with a nested path variable: the whole request.
    let update = &api.operations[4];
    assert_eq!(update.parameters[0].name, "pet.name");
    assert!(
        update
            .parameters
            .iter()
            .all(|p| p.location == ParamLocation::Path)
    );
}

#[test]
fn messages_follow_the_json_mapping() {
    let api = view(PROTO);
    let pet = api.named_type("pets.v1.Pet").unwrap();
    let TypeKind::Object(object) = &api.types.get(pet).kind else {
        panic!()
    };
    let fields: Vec<(&str, bool)> = object
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        fields,
        [
            ("name", false),
            ("displayName", true),
            ("species", false),
            ("weightGrams", false),
            ("createTime", false),
            ("photo", false),
            ("labels", false),
            ("person", false),
            ("shelter", false),
        ]
    );
    assert_eq!(object.exclusive[0].fields, ["person", "shelter"]);
    let kind = |name: &str| {
        &api.types
            .get(api.types.resolve(object.field(name).unwrap().ty))
            .kind
    };
    assert!(matches!(kind("weightGrams"), TypeKind::String(s) if s.format.is_none()));
    assert!(
        matches!(kind("createTime"), TypeKind::String(s) if s.format.as_deref() == Some("date-time"))
    );
    assert!(matches!(kind("photo"), TypeKind::String(s) if s.format.as_deref() == Some("byte")));
    assert!(matches!(kind("labels"), TypeKind::Object(_)));
    let TypeKind::Enum(species) = kind("species") else {
        panic!()
    };
    let names: Vec<String> = species.values.iter().map(|v| v.value.to_string()).collect();
    assert_eq!(names, ["\"SPECIES_UNSPECIFIED\"", "\"DOG\"", "\"CAT\""]);
}

const OPENAPI: &str = r"
openapi: 3.0.3
info: {title: Pets, version: 1.0.0}
paths:
  /v1/{name}:
    get:
      operationId: PetService_GetPet
      parameters:
        - {name: name, in: path, required: true, schema: {type: string}}
      responses:
        '200':
          description: The pet.
          content:
            application/json:
              schema: {$ref: '#/components/schemas/Pet'}
components:
  schemas:
    Pet:
      type: object
      properties:
        name: {type: string}
        displayName: {type: string}
        species: {type: string, enum: [SPECIES_UNSPECIFIED, DOG, CAT]}
        weightGrams: {type: string, format: int64}
      required: [displayName]
";

const SERVICE: &str = r#"
syntax = "proto3";
package pets.v1;
import "google/api/annotations.proto";
import "google/api/field_behavior.proto";
service PetService {
  rpc GetPet(GetPetRequest) returns (Pet) {
    option (google.api.http) = {get: "/v1/{name=pets/*}"};
  }
}
message Pet {
  string name = 1;
  string display_name = 2 [(google.api.field_behavior) = REQUIRED];
  Species species = 3;
  int64 weight_grams = 4;
}
enum Species {
  SPECIES_UNSPECIFIED = 0;
  DOG = 1;
  CAT = 2;
}
message GetPetRequest {
  string name = 1;
}
"#;

#[test]
fn a_service_matching_its_description_keeps_every_promise() {
    let found = conformance(OPENAPI, SERVICE);
    assert!(found.is_empty(), "{:?}", summary(&found));
}

#[test]
fn drift_between_service_and_description_is_found() {
    // The service sends numbers where the description promises strings.
    let found = conformance(
        OPENAPI,
        &SERVICE.replace("int64 weight_grams", "int32 weight_grams"),
    );
    assert!(
        found.iter().any(|f| f.severity == Severity::Breaking
            && f.change.path.to_string().ends_with(".weightGrams")),
        "{:?}",
        summary(&found)
    );

    // It sends an enum value the description does not list: risky, as an
    // added output enum value is everywhere.
    let found = conformance(
        OPENAPI,
        &SERVICE.replace("CAT = 2;", "CAT = 2;\n  BIRD = 3;"),
    );
    assert!(
        found
            .iter()
            .any(|f| f.severity == Severity::Risky && f.rule.name == "enum-value-added"),
        "{:?}",
        summary(&found)
    );

    // The description promises a field the service no longer sends.
    let found = conformance(
        OPENAPI,
        &SERVICE.replace(
            "  string display_name = 2 [(google.api.field_behavior) = REQUIRED];\n",
            "",
        ),
    );
    assert!(
        found.iter().any(|f| f.severity == Severity::Breaking),
        "{:?}",
        summary(&found)
    );

    // An operation the description documents is not served.
    let found = conformance(
        OPENAPI,
        &SERVICE.replace(
            "option (google.api.http) = {get: \"/v1/{name=pets/*}\"};",
            "",
        ),
    );
    assert_eq!(
        summary(&found),
        ["breaking operation-removed op[GET /v1/{}]"]
    );

    // The service answers on a different path than documented.
    let found = conformance(
        OPENAPI,
        &SERVICE.replace("/v1/{name=pets/*}", "/v2/{name=pets/*}"),
    );
    let rules: Vec<&str> = found.iter().map(|f| f.rule.name).collect();
    assert_eq!(rules, ["operation-moved"], "{:?}", summary(&found));
}

#[test]
fn unusable_rules_are_reported() {
    let warnings = |rule: &str| -> Vec<String> {
        let text = SERVICE.replace("{get: \"/v1/{name=pets/*}\"}", rule);
        lowered(&text)
            .diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{} {}",
                    d.code.map(|c| c.to_string()).unwrap_or_default(),
                    d.message
                )
            })
            .collect()
    };
    for (rule, expected) in [
        ("{get: \"/v1/{id}\"}", "names no field"),
        (
            "{post: \"/v1/pets\" body: \"pet\"}",
            "the body `pet` names no field",
        ),
        ("{get: \"v1/pets\"}", "must start with `/`"),
        (
            "{get: \"/v1/pets\" response_body: \"x\"}",
            "response body `x`",
        ),
        (
            "{custom {kind: \"LIST\" path: \"/v1/pets\"}}",
            "custom method LIST",
        ),
        ("{frob: 1}", "invalid HTTP rule"),
    ] {
        let found = warnings(rule);
        assert!(
            found
                .iter()
                .any(|w| w.starts_with("FLE3013") && w.contains(expected)),
            "{rule}: {found:?}"
        );
    }
    // Imports that only provide options are used.
    let clean = lowered(SERVICE);
    assert!(clean.diagnostics.is_empty(), "{:?}", clean.diagnostics);
}
