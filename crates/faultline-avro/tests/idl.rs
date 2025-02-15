//! Avro IDL: the protocols and schemas it declares.

use faultline_avro::{lower_avro, lower_avro_idl};
use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, QualName, Role, TypeKind, validate};
use faultline_source::{Limits, SourceMap};

fn idl(files: &[(&str, &str)]) -> (Option<Api>, Vec<String>) {
    let mut loader = MemoryLoader::new();
    for (uri, text) in files {
        loader = loader.with(uri, *text);
    }
    let sources = SourceMap::new();
    let lowered = lower_avro_idl(
        &UriRef::parse(files[0].0).unwrap(),
        &loader,
        &sources,
        "mail",
        Role::Both,
    );
    let codes = lowered
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowered.api, codes)
}

fn json(text: &str) -> Api {
    let loader = MemoryLoader::new().with("file:///mail.avpr", text);
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    lower_avro(
        &UriRef::parse("file:///mail.avpr").unwrap(),
        &mut store,
        "mail",
        Role::Both,
    )
    .api
    .unwrap()
}

const MAIL_IDL: &str = r#"
/** Sends mail. */
@namespace("example.mail")
protocol Mail {
  enum Priority { LOW, NORMAL, HIGH } = NORMAL;

  fixed Hash(16);

  /** A message. */
  record Message {
    string to, cc = "";
    string body;
    Priority priority = "NORMAL";
    union { null, Hash } hash = null;
    string? subject = null;
    array<string> tags = [];
    map<long> counts = {};
    timestamp_ms sent;
    decimal(9, 2) cost;
    @aliases(["sender"]) string `from`;
  }

  error Rejected {
    string reason;
  }

  /** Sends a message. */
  string send(Message message, boolean urgent = false) throws Rejected;

  void ping() oneway;
}
"#;

const MAIL_JSON: &str = r#"{
  "protocol": "Mail",
  "namespace": "example.mail",
  "types": [
    {"type": "enum", "name": "Priority", "symbols": ["LOW", "NORMAL", "HIGH"], "default": "NORMAL"},
    {"type": "fixed", "name": "Hash", "size": 16},
    {"type": "record", "name": "Message", "doc": "A message.", "fields": [
      {"name": "to", "type": "string"},
      {"name": "cc", "type": "string", "default": ""},
      {"name": "body", "type": "string"},
      {"name": "priority", "type": "Priority", "default": "NORMAL"},
      {"name": "hash", "type": ["null", "Hash"], "default": null},
      {"name": "subject", "type": ["null", "string"], "default": null},
      {"name": "tags", "type": {"type": "array", "items": "string"}, "default": []},
      {"name": "counts", "type": {"type": "map", "values": "long"}, "default": {}},
      {"name": "sent", "type": {"type": "long", "logicalType": "timestamp-millis"}},
      {"name": "cost", "type": {"type": "bytes", "logicalType": "decimal", "precision": 9, "scale": 2}},
      {"name": "from", "type": "string", "aliases": ["sender"]}
    ]},
    {"type": "error", "name": "Rejected", "fields": [{"name": "reason", "type": "string"}]}
  ],
  "messages": {
    "send": {
      "doc": "Sends a message.",
      "request": [{"name": "message", "type": "Message"}, {"name": "urgent", "type": "boolean", "default": false}],
      "response": "string",
      "errors": ["Rejected"]
    },
    "ping": {"request": [], "response": "null", "one-way": true}
  }
}"#;

#[test]
fn the_idl_is_the_protocol() {
    let (from_idl, codes) = idl(&[("file:///mail.avdl", MAIL_IDL)]);
    assert!(codes.is_empty(), "{codes:?}");
    let from_idl = from_idl.unwrap();
    assert_eq!(validate(&from_idl), []);
    let changes = faultline_diff::diff(&json(MAIL_JSON), &from_idl);
    assert!(changes.is_empty(), "{:?}", changes.changes);
    assert!(from_idl.operations[1].one_way);
}

#[test]
fn optional_fields_put_their_default_first() {
    let text = "protocol P { record R { string? name = \"none\"; } }";
    let (api, codes) = idl(&[("file:///p.avdl", text)]);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    let record = api.named[&QualName::new("R")];
    let TypeKind::Object(record) = &api.types.get(record).kind else {
        panic!()
    };
    let TypeKind::Union(union) = &api.types.get(record.fields[0].ty).kind else {
        panic!()
    };
    assert!(
        matches!(api.types.get(union.variants[0]).kind, TypeKind::String(_)),
        "a non-null default needs the string branch first"
    );
}

#[test]
fn schemas_and_imports() {
    let main = r#"namespace example.shop;
schema Order;
import idl "common.avdl";
import schema "money.avsc";
record Order {
  Item item;
  Money price;
}
"#;
    let common = "protocol Common { record Item { string sku; } }";
    let money = r#"{"type": "record", "name": "Money", "namespace": "example.shop", "fields": [{"name": "cents", "type": "long"}]}"#;
    let (api, codes) = idl(&[
        ("file:///shop/main.avdl", main),
        ("file:///shop/common.avdl", common),
        ("file:///shop/money.avsc", money),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    assert!(api.operations.is_empty());
    let roots: Vec<&str> = api.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        roots,
        ["Item", "example.shop.Money", "example.shop.Order"],
        "imported types keep their own namespace"
    );
}

#[test]
fn problems_are_reported() {
    for (files, code) in [
        (
            vec![("file:///a.avdl", "protocol P { record R { string } }")],
            "FLE5001",
        ),
        (
            vec![("file:///a.avdl", "protocol P { record R { Missing m; } }")],
            "FLE5002",
        ),
        (
            vec![("file:///a.avdl", "protocol P { import idl \"a.avdl\"; }")],
            "FLE5001",
        ),
        (
            vec![("file:///a.avdl", "protocol P { import idl \"nope.avdl\"; }")],
            "FLE5001",
        ),
        (
            vec![(
                "file:///a.avdl",
                "protocol P { record R { int x = \"no\"; } }",
            )],
            "FLE5007",
        ),
        (
            vec![("file:///a.avdl", "protocol P { int f() oneway; }")],
            "FLE5001",
        ),
        (
            vec![(
                "file:///a.avdl",
                "protocol P { record R { string s; } /* open",
            )],
            "FLE5001",
        ),
    ] {
        let (api, codes) = idl(&files);
        assert!(api.is_none(), "{files:?}");
        assert!(codes.iter().any(|c| c == code), "{files:?}: {codes:?}");
    }
}
