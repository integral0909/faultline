//! Compatibility of GraphQL schema changes, end to end: lower both versions,
//! diff, classify. Expectations follow the usual GraphQL evolution rules:
//! clients select fields and pass arguments, the server returns values.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_graphql::lower_graphql;
use faultline_ir::Api;
use faultline_rules::classify;
use faultline_source::SourceMap;

fn api(text: &str) -> Api {
    let loader = MemoryLoader::new().with("file:///schema.graphql", text);
    let lowered = lower_graphql(
        &UriRef::parse("file:///schema.graphql").unwrap(),
        &loader,
        &SourceMap::new(),
        "api",
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

/// `code severity path` per finding.
fn summary(old: &str, new: &str) -> Vec<String> {
    let changes = faultline_diff::diff(&api(old), &api(new));
    changes
        .iter()
        .map(|c| {
            let f = classify("api", c);
            format!("{} {} {}", f.rule.code, f.severity, f.change.path)
        })
        .collect()
}

fn schema(pet: &str, extra: &str) -> String {
    format!(
        r"
type Query {{
  pet(id: ID!): Pet
  pets(first: Int): [Pet!]!
}}
type Pet {{
{pet}
}}
{extra}
"
    )
}

const PET: &str = "id: ID!\nname: String!\nnickname: String";

fn assert_has(found: &[String], expected: &str) {
    assert!(
        found.iter().any(|s| s == expected),
        "missing {expected:?} in {found:?}"
    );
}

fn assert_none_at_least(found: &[String], severity: &str) {
    assert!(
        !found.iter().any(|s| s.contains(severity)),
        "unexpected {severity} findings: {found:?}"
    );
}

const PET_RESPONSE: &str = "op[query/pet]/response[default]/body[application/graphql]|0";

#[test]
fn removing_an_output_field_breaks_clients_that_select_it() {
    let found = summary(&schema(PET, ""), &schema("id: ID!\nname: String!", ""));
    assert_has(&found, &format!("FL1003 breaking {PET_RESPONSE}"));
}

#[test]
fn adding_an_output_field_is_compatible() {
    let found = summary(&schema(PET, ""), &schema(&format!("{PET}\nage: Int"), ""));
    assert_none_at_least(&found, "breaking");
    assert_none_at_least(&found, "risky");
}

#[test]
fn nullability_of_outputs() {
    // Non-null to nullable: clients may now receive null.
    let found = summary(
        &schema(PET, ""),
        &schema("id: ID!\nname: String\nnickname: String", ""),
    );
    assert_has(&found, &format!("FL1012 breaking {PET_RESPONSE}.name"));
    // Nullable to non-null: clients just never see null.
    let found = summary(
        &schema(PET, ""),
        &schema("id: ID!\nname: String!\nnickname: String!", ""),
    );
    assert_none_at_least(&found, "breaking");
    assert_none_at_least(&found, "risky");
}

#[test]
fn field_arguments_flow_from_the_client() {
    let old = schema(&format!("{PET}\ntags(first: Int): [String!]!"), "");
    let tags = format!("{PET_RESPONSE}.tags()");

    let required = summary(
        &old,
        &schema(
            &format!("{PET}\ntags(first: Int, locale: String!): [String!]!"),
            "",
        ),
    );
    assert_has(&required, &format!("FL1002 breaking {tags}"));

    let optional = summary(
        &old,
        &schema(
            &format!("{PET}\ntags(first: Int, locale: String): [String!]!"),
            "",
        ),
    );
    assert_none_at_least(&optional, "breaking");

    let removed = summary(&old, &schema(&format!("{PET}\ntags: [String!]!"), ""));
    assert_has(&removed, &format!("FL1003 breaking {tags}"));

    let now_required = summary(
        &old,
        &schema(&format!("{PET}\ntags(first: Int!): [String!]!"), ""),
    );
    assert_has(&now_required, &format!("FL1004 breaking {tags}"));
}

#[test]
fn root_field_arguments_are_the_request() {
    let old = schema(PET, "");
    let new = old.replace("pets(first: Int)", "pets(first: Int, species: String!)");
    let found = summary(&old, &new);
    assert_has(
        &found,
        "FL1002 breaking op[query/pets]/request[application/graphql]",
    );
}

#[test]
fn input_objects() {
    let with_input = |fields: &str| {
        format!(
            "type Query {{ a: Int }}\ntype Mutation {{ addPet(input: NewPet!): Int }}\ninput NewPet {{ {fields} }}"
        )
    };
    let old = with_input("name: String!");
    let input = "op[mutation/addPet]/request[application/graphql].input";
    let required = summary(&old, &with_input("name: String! species: String!"));
    assert_has(&required, &format!("FL1002 breaking {input}"));
    let optional = summary(&old, &with_input("name: String! species: String"));
    assert_none_at_least(&optional, "breaking");
    let defaulted = summary(
        &old,
        &with_input("name: String! species: String! = \"cat\""),
    );
    assert_none_at_least(&defaulted, "breaking");
    let removed = summary(&old, &with_input("nickname: String"));
    assert_has(&removed, &format!("FL1003 breaking {input}"));
}

#[test]
fn enums_and_unions() {
    let with = |species: &str, search: &str| {
        schema(
            &format!("{PET}\nspecies: Species!"),
            &format!(
                "enum Species {{ {species} }}\nunion Search = {search}\ntype Person {{ id: ID! }}"
            ),
        )
        .replace(
            "pets(first: Int): [Pet!]!",
            "pets(first: Int): [Pet!]!\n  search: [Search!]!",
        )
    };
    let base = with("CAT DOG", "Pet");
    let added = summary(&base, &with("CAT DOG BIRD", "Pet"));
    assert!(
        added.iter().any(|s| s.starts_with("FL1005 risky")),
        "a new enum value may surprise clients: {added:?}"
    );
    let member = summary(&base, &with("CAT DOG", "Pet | Person"));
    assert!(
        member.iter().any(|s| s.starts_with("FL1012")),
        "a new union member: {member:?}"
    );
}

#[test]
fn operations_types_and_deprecation() {
    let old = schema(PET, "");
    let removed = summary(&old, &old.replace("pet(id: ID!): Pet\n", ""));
    assert_has(&removed, "FL2001 breaking op[query/pet]");

    let changed = summary(&old, &schema("id: ID!\nname: Int!\nnickname: String", ""));
    assert_has(&changed, &format!("FL1001 breaking {PET_RESPONSE}.name"));

    let deprecated = summary(
        &old,
        &schema(
            "id: ID!\nname: String! @deprecated(reason: \"Use nickname\")\nnickname: String",
            "",
        ),
    );
    assert_has(&deprecated, &format!("FL1020 info {PET_RESPONSE}.name"));
}

#[test]
fn argument_defaults_and_scalars() {
    let old = schema(
        &format!("{PET}\ntags(first: Int = 10): [String!]!\nborn: DateTime"),
        "scalar DateTime",
    );
    let new = schema(
        &format!("{PET}\ntags(first: Int = 20): [String!]!\nborn: Date"),
        "scalar Date",
    );
    let found = summary(&old, &new);
    assert_has(&found, &format!("FL1022 risky {PET_RESPONSE}.tags().first"));
    assert!(
        found
            .iter()
            .any(|s| s.starts_with("FL1010") && s.contains(".born")),
        "{found:?}"
    );
}

#[test]
fn identical_schemas_have_no_findings() {
    let text = schema(PET, "enum E { A }");
    assert!(summary(&text, &text).is_empty());
}

const ONE_OF: &str = r"
type Query { pet(by: PetBy!): String }
input PetBy @oneOf { id: ID, name: String }
";

#[test]
fn one_of_inputs_take_exactly_one_field() {
    let api = api(ONE_OF);
    let by = api.named[&faultline_ir::QualName::new("PetBy")];
    let faultline_ir::TypeKind::Object(object) = &api.types.get(by).kind else {
        panic!()
    };
    assert_eq!(object.exclusive[0].fields, ["id", "name"]);
    assert_eq!(object.min_properties, 1);
    assert!(
        matches!(
            api.types.get(object.fields[0].ty).kind,
            faultline_ir::TypeKind::String(_)
        ),
        "the chosen field is not null"
    );

    // Dropping @oneOf lets clients send several fields, or none: servers
    // written for one field are sent inputs they never handled.
    let plain = summary(ONE_OF, &ONE_OF.replace(" @oneOf", ""));
    assert!(!plain.is_empty());
    // Making an input @oneOf rejects clients that send several fields.
    let made = summary(&ONE_OF.replace(" @oneOf", ""), ONE_OF);
    assert!(made.iter().any(|s| s.contains("breaking")), "{made:?}");
    // A new alternative is compatible for requests.
    let wider = summary(
        ONE_OF,
        &ONE_OF.replace("name: String", "name: String, tag: String"),
    );
    assert!(wider.iter().all(|s| !s.contains("breaking")), "{wider:?}");
}

#[test]
fn directives_used_in_queries_are_part_of_the_api() {
    let with = |directives: &str| schema(PET, directives);
    let old = with(
        "directive @cached(ttl: Int) on FIELD | QUERY\ndirective @key(fields: String!) on OBJECT",
    );
    assert!(
        summary(&old, &old).is_empty(),
        "an unchanged schema has no changes"
    );
    // Queries using the directive stop validating.
    let removed = with("directive @key(fields: String!) on OBJECT");
    assert_eq!(
        summary(&old, &removed),
        ["FL2001 breaking op[directive/cached]"]
    );
    let narrowed =
        with("directive @cached(ttl: Int) on FIELD\ndirective @key(fields: String!) on OBJECT");
    assert_eq!(
        summary(&old, &narrowed),
        ["FL1006 breaking op[directive/cached]/request[application/graphql].location"]
    );
    let required = with(
        "directive @cached(ttl: Int, scope: String!) on FIELD | QUERY\ndirective @key(fields: String!) on OBJECT",
    );
    assert_eq!(
        summary(&old, &required),
        ["FL1002 breaking op[directive/cached]/request[application/graphql].arguments"]
    );
    // Widening is compatible, and directives only schemas use are not
    // compared.
    let widened = with(
        "directive @cached(ttl: Int, scope: String) on FIELD | QUERY | FRAGMENT_SPREAD\ndirective @key(fields: [String!]!) on OBJECT | INTERFACE",
    );
    let found = summary(&old, &widened);
    assert_eq!(
        found,
        [
            "FL1002 info op[directive/cached]/request[application/graphql].arguments",
            "FL1005 info op[directive/cached]/request[application/graphql].location",
        ]
    );
}
