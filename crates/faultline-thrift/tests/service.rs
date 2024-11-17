//! Thrift services as RPC operations.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, Role, TypeKind, validate};
use faultline_rules::{Severity, classify};
use faultline_source::SourceMap;
use faultline_thrift::lower_thrift;

const BASE: &str = "service Base {\n  string version()\n  void ping()\n}\n";

fn lowered(text: &str) -> Api {
    let loader = MemoryLoader::new()
        .with("file:///pets.thrift", text)
        .with("file:///base.thrift", BASE);
    let sources = SourceMap::new();
    let lowered = lower_thrift(
        &UriRef::parse("file:///pets.thrift").unwrap(),
        &loader,
        &sources,
        "pets",
        Role::Both,
    );
    let api = lowered
        .api
        .unwrap_or_else(|| panic!("{:?}", lowered.diagnostics));
    assert_eq!(validate(&api), []);
    api
}

const PETS: &str = r#"include "base.thrift"
exception NotFound { 1: string message }
exception Throttled { 1: i32 retry_after }
struct Pet { 1: required i64 id, 2: optional string name }
service Pets extends base.Base {
  Pet get(1: i64 id) throws (1: NotFound missing),
  list<Pet> list(1: i32 limit, 2: optional string cursor),
  oneway void forget(1: i64 id),
  /** Replaces Base's. */
  void ping()
}
"#;

#[test]
fn functions_are_operations() {
    let api = lowered(PETS);
    assert!(api.schemas.is_empty(), "a file with services has no roots");
    let ops: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    assert_eq!(
        ops,
        [
            "Pets/get",
            "Pets/list",
            "Pets/forget",
            "Pets/ping",
            "Pets/version",
        ],
        "own functions, then inherited ones not redefined"
    );
    let get = &api.operations[0];
    let args = get.request_body.as_ref().unwrap().content[0].ty;
    let TypeKind::Object(args) = &api.types.get(args).kind else {
        panic!()
    };
    assert_eq!(args.fields[0].tag, Some(1));
    let result = get.responses[0].content[0].ty;
    let TypeKind::Object(result) = &api.types.get(result).kind else {
        panic!()
    };
    let fields: Vec<(&str, Option<u32>)> = result
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.tag))
        .collect();
    assert_eq!(fields, [("success", Some(0)), ("missing", Some(1))]);
    assert!(matches!(
        api.types.get(result.additional).kind,
        TypeKind::Never
    ));
    assert_eq!(result.min_properties, 1);
    assert!(
        api.operations[2].responses.is_empty(),
        "oneway has no reply"
    );
    assert_eq!(
        api.operations[3].meta.description.as_deref(),
        Some("Replaces Base's.")
    );
}

fn findings(old: &str, new: &str) -> Vec<(Severity, String)> {
    let a = lowered(old);
    let b = lowered(new);
    let mut out: Vec<(Severity, String)> = faultline_diff::diff(&a, &b)
        .iter()
        .map(|c| {
            let f = classify("pets", c);
            (f.severity, format!("{} {}", f.rule.code, c.path))
        })
        .collect();
    out.sort();
    out
}

fn worst(old: &str, new: &str) -> Severity {
    findings(old, new)
        .into_iter()
        .map(|(s, _)| s)
        .max()
        .unwrap_or(Severity::Info)
}

#[test]
fn function_changes_are_classified() {
    // A new exception fails clients that receive it.
    assert_eq!(
        worst(
            PETS,
            &PETS.replace(
                "(1: NotFound missing)",
                "(1: NotFound missing, 2: Throttled slow)"
            )
        ),
        Severity::Risky
    );
    // A removed exception is never received.
    assert_eq!(
        worst(PETS, &PETS.replace(" throws (1: NotFound missing)", "")),
        Severity::Info
    );
    // An exception moved to another id is not recognized.
    assert_eq!(
        worst(
            PETS,
            &PETS.replace("(1: NotFound missing)", "(2: NotFound missing)")
        ),
        Severity::Breaking
    );
    // A new optional argument is compatible; renumbering one is not.
    assert_eq!(
        worst(
            PETS,
            &PETS.replace(
                "2: optional string cursor",
                "2: optional string cursor, 3: optional bool all"
            )
        ),
        Severity::Info
    );
    assert_eq!(
        worst(
            PETS,
            &PETS.replace("2: optional string cursor", "3: optional string cursor")
        ),
        Severity::Breaking
    );
    // Returning another kind of collection: the client skips it.
    assert_eq!(
        worst(PETS, &PETS.replace("list<Pet> list", "set<Pet> list")),
        Severity::Breaking
    );
    // Removing a function breaks its callers, also an inherited one.
    assert_eq!(
        worst(
            PETS,
            &PETS.replace("  Pet get(1: i64 id) throws (1: NotFound missing),\n", "")
        ),
        Severity::Breaking
    );
    let unrooted = PETS.replace(" extends base.Base", "");
    let removed = findings(PETS, &unrooted);
    assert!(
        removed.contains(&(Severity::Breaking, "FL2001 op[Pets/version]".into())),
        "{removed:?}"
    );
}
