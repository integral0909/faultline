//! Parsing, schema validation and lowering.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_graphql::ast::{TypeDefKind, Value};
use faultline_graphql::parser::parse;
use faultline_graphql::{Lowered, lower_graphql};
use faultline_ir::{TypeKind, dump, validate};
use faultline_source::{SourceId, SourceMap};

fn lower(text: &str) -> Lowered {
    let loader = MemoryLoader::new().with("file:///schema.graphql", text);
    lower_graphql(
        &UriRef::parse("file:///schema.graphql").unwrap(),
        &loader,
        &SourceMap::new(),
        "pets",
    )
}

fn ok(text: &str) -> faultline_ir::Api {
    let lowered = lower(text);
    let errors: Vec<&str> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.as_str())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

fn errors(text: &str) -> Vec<(String, String)> {
    lower(text)
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| {
            (
                d.code.map(|c| c.to_string()).unwrap_or_default(),
                d.message.clone(),
            )
        })
        .collect()
}

const PETS: &str = r#"
"""
The pet store.
"""
schema {
  query: Query
  mutation: Mutation
  subscription: Subscription
}

type Query {
  "A pet by id."
  pet(id: ID!): Pet
  pets(first: Int = 10, after: String, species: [Species!]): [Pet!]!
  search(text: String!): [SearchResult!]!
}

type Mutation {
  addPet(input: NewPet!): Pet!
}

type Subscription {
  petAdded: Pet!
}

interface Node {
  id: ID!
}

type Pet implements Node @key(fields: "id") {
  id: ID!
  name: String!
  species: Species!
  tags(first: Int): [String!]
  owner: Person @deprecated(reason: "Use `owners`.")
  born: DateTime
}

type Person implements Node {
  id: ID!
  name: String
}

union SearchResult = Pet | Person

enum Species {
  CAT
  DOG
  BIRD @deprecated
}

input NewPet {
  name: String!
  species: Species! = CAT
  tags: [String!] = []
}

scalar DateTime

directive @key(fields: String!) repeatable on OBJECT | INTERFACE
"#;

#[test]
fn parse_a_schema() {
    let (document, diagnostics) = parse(SourceId::from_raw(0), PETS);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(document.schemas.len(), 1);
    assert_eq!(
        document.schemas[0].description.as_deref(),
        Some("The pet store.")
    );
    let pet = document
        .types
        .iter()
        .find(|t| t.name.text == "Pet")
        .unwrap();
    let TypeDefKind::Object { interfaces, fields } = &pet.kind else {
        panic!()
    };
    assert_eq!(interfaces[0].text, "Node");
    assert_eq!(pet.directives[0].name.text, "key");
    assert_eq!(fields[3].ty.to_string(), "[String!]");
    assert_eq!(fields[3].arguments[0].name.text, "first");
    let pets = &document.types[0];
    let TypeDefKind::Object { fields, .. } = &pets.kind else {
        panic!()
    };
    assert_eq!(
        fields[1].arguments[0].default,
        Some(Value::Int("10".into()))
    );
    assert!(document.directives[0].repeatable);
    assert_eq!(document.directives[0].locations.len(), 2);
}

#[test]
fn root_fields_become_operations() {
    let api = ok(PETS);
    let ops: Vec<String> = api.operations.iter().map(|o| o.key.identity()).collect();
    assert_eq!(
        ops,
        [
            "query/pet",
            "query/pets",
            "query/search",
            "mutation/addPet",
            "subscription/petAdded"
        ]
    );
    assert!(api.operations[4].streaming.unwrap().response);
    let text = dump(&api);
    for line in [
        "op[query/pet]/request[application/graphql] [in] -> t",
        "op[subscription/petAdded] streams response",
        "  .tags required args=",
        "SearchResult [out]: union tagged",
    ] {
        assert!(text.contains(line), "missing {line:?} in\n{text}");
    }
}

#[test]
fn output_fields_are_present_and_nullable_types_include_null() {
    let api = ok(PETS);
    let pet = api.named_type("Pet").unwrap();
    let TypeKind::Object(obj) = &api.types.get(pet).kind else {
        panic!()
    };
    assert!(obj.fields.iter().all(|f| f.required));
    assert!(matches!(api.types.get(obj.additional).kind, TypeKind::Any));
    let name = &obj.fields[1];
    assert!(
        matches!(api.types.get(name.ty).kind, TypeKind::String(_)),
        "String! is a string"
    );
    let owner = obj.fields.iter().find(|f| f.name == "owner").unwrap();
    assert!(owner.meta.deprecated);
    let TypeKind::Union(nullable) = &api.types.get(owner.ty).kind else {
        panic!("a nullable field is a union with null")
    };
    assert!(matches!(
        api.types.get(nullable.variants[1]).kind,
        TypeKind::Null
    ));
    let born = obj.fields.iter().find(|f| f.name == "born").unwrap();
    let TypeKind::Union(u) = &api.types.get(born.ty).kind else {
        panic!()
    };
    let TypeKind::String(s) = &api.types.get(u.variants[0]).kind else {
        panic!("custom scalars are strings")
    };
    assert_eq!(s.format.as_deref(), Some("DateTime"));
}

#[test]
fn inputs_are_closed_and_require_non_null_values_without_defaults() {
    let api = ok(PETS);
    let input = api.named_type("NewPet").unwrap();
    let TypeKind::Object(obj) = &api.types.get(input).kind else {
        panic!()
    };
    assert!(matches!(
        api.types.get(obj.additional).kind,
        TypeKind::Never
    ));
    let required: Vec<(&str, bool)> = obj
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        required,
        [("name", true), ("species", false), ("tags", false)]
    );
    assert_eq!(
        obj.fields[1].meta.default.as_ref().unwrap().to_string(),
        "\"CAT\""
    );
}

#[test]
fn extensions_merge_into_their_types() {
    let api = ok(r"
        type Query { a: Int }
        extend type Query { b: Int }
        enum E { X }
        extend enum E { Y }
        type T { e: E }
        extend type Query { t: T }
    ");
    let ops: Vec<String> = api.operations.iter().map(|o| o.key.identity()).collect();
    assert_eq!(ops, ["query/a", "query/b", "query/t"]);
    let e = api.named_type("E").unwrap();
    let TypeKind::Enum(e) = &api.types.get(e).kind else {
        panic!()
    };
    assert_eq!(e.values.len(), 2);
}

#[test]
fn default_root_types_are_used_without_a_schema_definition() {
    let api = ok("type Query { a: Int } type Mutation { b: Int }");
    let ops: Vec<String> = api.operations.iter().map(|o| o.key.identity()).collect();
    assert_eq!(ops, ["query/a", "mutation/b"]);
}

#[test]
fn validation_errors() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "type Query { a: Missing }",
            "FLE4002",
            "unknown type `Missing`",
        ),
        (
            "type Query { a: Int } type Query { b: Int }",
            "FLE4003",
            "type `Query` is defined twice",
        ),
        (
            "type Query { a: Int a: String }",
            "FLE4003",
            "field `a` is defined twice",
        ),
        (
            "type Query { a(x: Int, x: Int): Int }",
            "FLE4003",
            "argument `x` is defined twice",
        ),
        (
            "type Query { a(x: Query): Int }",
            "FLE4004",
            "`Query` is an object type, but an argument or input field needs one of the input types",
        ),
        (
            "input I { x: Int } type Query { a: I }",
            "FLE4004",
            "`I` is an input object, but a field needs one of the output types",
        ),
        (
            "type Query { a: U } union U = Int",
            "FLE4004",
            "union members must be object types, and `Int` is",
        ),
        (
            "interface N { id: ID! } type Query implements N { a: Int }",
            "FLE4005",
            "`Query` does not provide the field `id` of interface `N`",
        ),
        (
            "type Query { a: Int } extend type Missing { b: Int }",
            "FLE4002",
            "cannot extend unknown type `Missing`",
        ),
        (
            "type Query { a: Int } enum E { X } extend type E { b: Int }",
            "FLE4004",
            "`E` is an enum; an extension of it must be of the same kind",
        ),
        (
            "type Mutation { a: Int }",
            "FLE4006",
            "the schema has no query root type",
        ),
        (
            "type Query { __a: Int }",
            "FLE4008",
            "`__a` starts with `__`, which introspection reserves",
        ),
        (
            "type Query { a: Int } enum E {}",
            "FLE4009",
            "`E` must have at least one of its values",
        ),
        (
            "type Query { a: Int } query { a }",
            "FLE4007",
            "a schema document cannot contain operations or fragments",
        ),
        (
            "schema { query: Q } type Query { a: Int }",
            "FLE4002",
            "unknown type `Q`",
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
}

#[test]
fn syntax_errors_recover_at_the_next_definition() {
    let found = errors(
        r"
        type Query { a: Int }
        type Broken { b: }
        type Pet { name: String }
        enum E { true }
    ",
    );
    let messages: Vec<&str> = found.iter().map(|(_, m)| m.as_str()).collect();
    assert_eq!(
        messages,
        [
            "expected a type, found `}`",
            "`true` cannot be an enum value"
        ]
    );
}
