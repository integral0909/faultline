//! Avro protocols: named types and messages.

use faultline_avro::lower_avro;
use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, Role, TypeKind, validate};
use faultline_rules::{Severity, classify};
use faultline_source::{Limits, SourceMap};

fn lowered(text: &str) -> (Option<Api>, Vec<String>) {
    let loader = MemoryLoader::new().with("file:///mail.avpr", text);
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    let lowered = lower_avro(
        &UriRef::parse("file:///mail.avpr").unwrap(),
        &mut store,
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

fn api(text: &str) -> Api {
    let (api, codes) = lowered(text);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

const MAIL: &str = r#"{
  "protocol": "Mail",
  "namespace": "example.mail",
  "types": [
    {"type": "record", "name": "Message", "fields": [
      {"name": "to", "type": "string"},
      {"name": "body", "type": "string"},
      {"name": "priority", "type": "int", "default": 0}
    ]},
    {"type": "error", "name": "Rejected", "fields": [{"name": "reason", "type": "string"}]},
    {"type": "error", "name": "Full", "fields": []}
  ],
  "messages": {
    "send": {
      "doc": "Sends a message.",
      "request": [{"name": "message", "type": "Message"}],
      "response": "string",
      "errors": ["Rejected"]
    },
    "ping": {"request": [], "response": "null", "one-way": true}
  }
}"#;

#[test]
fn messages_are_operations() {
    let api = api(MAIL);
    let keys: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    assert_eq!(keys, ["Mail/send", "Mail/ping"]);
    let send = &api.operations[0];
    assert_eq!(send.meta.description.as_deref(), Some("Sends a message."));
    let request = send.request_body.as_ref().unwrap().content[0].ty;
    let TypeKind::Object(request) = &api.types.get(request).kind else {
        panic!()
    };
    assert_eq!(request.fields[0].name, "message");
    let response = send.responses[0].content[0].ty;
    let TypeKind::Union(result) = &api.types.get(response).kind else {
        panic!()
    };
    assert_eq!(result.variants.len(), 2, "the response or the error");
    let ping = &api.operations[1];
    assert!(ping.one_way && ping.responses.is_empty());
}

#[test]
fn a_protocol_without_messages_is_its_types() {
    let types_only = r#"{"protocol": "Types", "types": [
        {"type": "record", "name": "A", "fields": []},
        {"type": "enum", "name": "E", "symbols": ["X"]}
    ]}"#;
    let api = api(types_only);
    let roots: Vec<&str> = api.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(roots, ["A", "E"]);
}

fn worst(new: &str) -> Severity {
    faultline_diff::diff(&api(MAIL), &api(new))
        .iter()
        .map(|c| classify("mail", c).severity)
        .max()
        .unwrap_or(Severity::Info)
}

#[test]
fn message_changes_are_classified() {
    assert_eq!(
        worst(&MAIL.replace(
            r#""errors": ["Rejected"]"#,
            r#""errors": ["Rejected", "Full"]"#
        )),
        Severity::Breaking,
        "an old client's reader cannot resolve the new error"
    );
    assert_eq!(
        worst(&MAIL.replace(r#", "one-way": true"#, "")),
        Severity::Breaking,
        "the call now has a reply"
    );
    assert_eq!(
        worst(&MAIL.replace(
            r#""request": [{"name": "message", "type": "Message"}]"#,
            r#""request": [{"name": "message", "type": "Message"}, {"name": "copies", "type": "int"}]"#
        )),
        Severity::Breaking,
        "a parameter without a default"
    );
    assert_eq!(worst(MAIL), Severity::Info);
}

#[test]
fn malformed_protocols_are_reported() {
    for (text, code) in [
        (r#"{"protocol": 1}"#, "FLE5001"),
        (
            r#"{"protocol": "P", "messages": {"m": {"request": []}}}"#,
            "FLE5001",
        ),
        (
            r#"{"protocol": "P", "messages": {"m": {"request": [], "response": "int", "one-way": true}}}"#,
            "FLE5001",
        ),
        (
            r#"{"protocol": "P", "messages": {"m": {"request": [], "response": "Missing"}}}"#,
            "FLE5002",
        ),
    ] {
        let (api, codes) = lowered(text);
        assert!(api.is_none(), "{text}");
        assert!(codes.iter().any(|c| c == code), "{text}: {codes:?}");
    }
}
