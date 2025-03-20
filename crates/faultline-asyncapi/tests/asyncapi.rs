//! AsyncAPI 2.x and 3.0 lowering and comparison: directions of sent and
//! received messages, migration between versions, and payloads in JSON
//! Schema, Avro and Protobuf.

use faultline_asyncapi::{SENT, lower_asyncapi};
use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, ApiFormat, Segment, TypeKind, validate};
use faultline_rules::{Finding, Severity, classify};
use faultline_source::{Limits, SourceMap};

fn lower_with(files: &[(&str, &str)]) -> faultline_jsonschema::Lowered {
    let mut loader = MemoryLoader::new();
    for (path, text) in files {
        loader.insert(&format!("file:///{path}"), *text);
    }
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    let entry = UriRef::parse(&format!("file:///{}", files[0].0)).unwrap();
    lower_asyncapi(&entry, &mut store, "events")
}

fn api_with(files: &[(&str, &str)]) -> Api {
    let lowered = lower_with(files);
    let errors: Vec<String> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

fn api(text: &str) -> Api {
    api_with(&[("events.yaml", text)])
}

fn findings(old: &Api, new: &Api) -> Vec<Finding> {
    faultline_diff::diff(old, new)
        .iter()
        .map(|c| classify("events", c))
        .collect()
}

fn summary(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|f| format!("{} {} {}", f.severity, f.rule.name, f.change.path))
        .collect()
}

fn worst(findings: &[Finding]) -> Severity {
    findings
        .iter()
        .map(|f| f.severity)
        .max()
        .unwrap_or(Severity::Info)
}

const V2: &str = r"
asyncapi: 2.6.0
info: {title: Accounts, version: 1.0.0}
channels:
  user/signedup/{userId}:
    parameters:
      userId:
        schema: {type: string}
    subscribe:
      operationId: publishSignup
      message:
        $ref: '#/components/messages/UserSignedUp'
  user/delete:
    publish:
      operationId: deleteUser
      message:
        oneOf:
          - $ref: '#/components/messages/DeleteUser'
          - name: PurgeUser
            payload: {type: object, properties: {id: {type: string}}, required: [id]}
components:
  messages:
    UserSignedUp:
      name: UserSignedUp
      headers:
        type: object
        properties: {traceId: {type: string}}
      payload:
        $ref: '#/components/schemas/User'
    DeleteUser:
      name: DeleteUser
      payload:
        type: object
        properties: {id: {type: string}, reason: {type: string}}
        required: [id]
  schemas:
    User:
      type: object
      properties:
        id: {type: string}
        email: {type: string, format: email}
      required: [id, email]
";

const V3: &str = r"
asyncapi: 3.0.0
info: {title: Accounts, version: 1.0.0}
channels:
  signups:
    address: user/signedup/{userId}
    parameters:
      userId: {}
    messages:
      UserSignedUp:
        $ref: '#/components/messages/UserSignedUp'
  deletions:
    address: user/delete
    messages:
      DeleteUser:
        $ref: '#/components/messages/DeleteUser'
      PurgeUser:
        payload: {type: object, properties: {id: {type: string}}, required: [id]}
operations:
  publishSignup:
    action: send
    channel: {$ref: '#/channels/signups'}
  deleteUser:
    action: receive
    channel: {$ref: '#/channels/deletions'}
components:
  messages:
    UserSignedUp:
      name: UserSignedUp
      headers:
        type: object
        properties: {traceId: {type: string}}
      payload:
        $ref: '#/components/schemas/User'
    DeleteUser:
      name: DeleteUser
      payload:
        type: object
        properties: {id: {type: string}, reason: {type: string}}
        required: [id]
  schemas:
    User:
      type: object
      properties:
        id: {type: string}
        email: {type: string, format: email}
      required: [id, email]
";

#[test]
fn operations_take_the_applications_point_of_view() {
    let api = api(V2);
    assert_eq!(api.format, ApiFormat::AsyncApi2);
    assert_eq!(api.version.as_deref(), Some("1.0.0"));
    // `publish` is what the application receives.
    let received: Vec<String> = api.operations.iter().map(|o| o.key.identity()).collect();
    assert_eq!(received, ["receive user/delete"]);
    let messages: Vec<&str> = api.operations[0]
        .request_body
        .as_ref()
        .unwrap()
        .content
        .iter()
        .map(|c| c.media_type.as_str())
        .collect();
    assert_eq!(messages, ["DeleteUser", "PurgeUser"]);
    // `subscribe` is what it sends, lowered as its own requests.
    assert_eq!(api.webhooks.len(), 1);
    assert_eq!(api.webhooks[0].name, SENT);
    let sent = &api.webhooks[0].operations[0];
    assert_eq!(sent.key.identity(), "send user/signedup/{}");
    assert_eq!(sent.key.to_string(), "send user/signedup/{userId}");
    assert_eq!(sent.parameters[0].name, "userId");
    let signup = &sent.request_body.as_ref().unwrap().content[0];
    assert_eq!(signup.media_type, "UserSignedUp");
    let TypeKind::Object(envelope) = &api.types.get(signup.ty).kind else {
        panic!("messages are objects");
    };
    let fields: Vec<&str> = envelope.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(fields, ["contentType", "headers", "payload"]);

    let roots: Vec<(String, bool)> = api
        .roots()
        .into_iter()
        .map(|r| (r.path.to_string(), r.polarity.is_out()))
        .collect();
    assert!(roots.contains(&("op[receive user/delete]/message[DeleteUser]".into(), false)));
    assert!(roots.contains(&(
        "webhook[sent]/op[send user/signedup/{}]/message[UserSignedUp]".into(),
        true
    )));
    assert!(roots.contains(&(
        "webhook[sent]/op[send user/signedup/{}]/param[path:userId]".into(),
        true
    )));
}

#[test]
fn a_migration_from_2_to_3_changes_nothing() {
    let v3 = api(V3);
    assert_eq!(v3.format, ApiFormat::AsyncApi3);
    let changes = findings(&api(V2), &v3);
    assert!(changes.is_empty(), "{:?}", summary(&changes));
}

#[test]
fn payload_changes_have_the_direction_of_their_messages() {
    let old = api(V3);
    // A new required field in a received message breaks its producers.
    let new = api(&V3.replace(
        "required: [id]\n  schemas:",
        "required: [id, reason]\n  schemas:",
    ));
    let found = findings(&old, &new);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
    assert!(found.iter().all(|f| {
        f.change
            .path
            .to_string()
            .starts_with("op[receive user/delete]/message[DeleteUser].payload")
    }));

    // A field removed from a sent message breaks its consumers.
    let new = api(&V3.replace("        email: {type: string, format: email}\n", ""));
    let found = findings(&old, &new);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
    assert!(found.iter().any(|f| f.change.path.to_string()
        == "webhook[sent]/op[send user/signedup/{}]/message[UserSignedUp].payload.email"));

    // An optional field added to a sent message is safe.
    let new = api(&V3.replace(
        "        email: {type: string, format: email}\n",
        "        email: {type: string, format: email}\n        name: {type: string}\n",
    ));
    let found = findings(&old, &new);
    assert!(worst(&found) < Severity::Breaking, "{:?}", summary(&found));
}

#[test]
fn messages_added_and_removed() {
    let old = api(V3);
    // The application stops accepting a message producers send.
    let without_purge = V3.replace(
        "      PurgeUser:\n        payload: {type: object, properties: {id: {type: string}}, required: [id]}\n",
        "",
    );
    let found = findings(&old, &api(&without_purge));
    assert_eq!(
        summary(&found),
        ["breaking message-removed op[receive user/delete]/message[PurgeUser]"]
    );
    // It accepts a new one: fine.
    let found = findings(&api(&without_purge), &old);
    assert_eq!(
        summary(&found),
        ["info message-added op[receive user/delete]/message[PurgeUser]"]
    );

    // It sends a new kind of message: consumers may not know it.
    let more = V3.replace(
        "      UserSignedUp:\n        $ref: '#/components/messages/UserSignedUp'\n",
        "      UserSignedUp:\n        $ref: '#/components/messages/UserSignedUp'\n      UserVerified:\n        payload: {type: object}\n",
    );
    let found = findings(&old, &api(&more));
    assert_eq!(
        summary(&found),
        ["risky message-added webhook[sent]/op[send user/signedup/{}]/message[UserVerified]"]
    );
}

#[test]
fn content_types_are_part_of_messages() {
    let old = api(V3);
    let new = api(&V3.replace(
        "    DeleteUser:\n      name: DeleteUser\n",
        "    DeleteUser:\n      name: DeleteUser\n      contentType: application/xml\n",
    ));
    let found = findings(&old, &new);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
    assert!(found.iter().any(|f| {
        f.change
            .path
            .to_string()
            .ends_with("message[DeleteUser].contentType")
    }));
    // The default content type is what an absent one means.
    let explicit = V3.replace(
        "info: {title: Accounts, version: 1.0.0}\n",
        "info: {title: Accounts, version: 1.0.0}\ndefaultContentType: application/json\n",
    );
    assert!(findings(&old, &api(&explicit)).is_empty());
}

#[test]
fn channels_that_move_are_recognized() {
    let old = api(V3);
    let new = api(&V3.replace("address: user/delete", "address: users/delete"));
    let found = findings(&old, &new);
    let rules: Vec<&str> = found.iter().map(|f| f.rule.name).collect();
    assert_eq!(rules, ["operation-moved"], "{:?}", summary(&found));
    // Renaming a parameter keeps the channel.
    let renamed = V3
        .replace("user/signedup/{userId}", "user/signedup/{id}")
        .replace("      userId: {}", "      id: {}");
    let found = findings(&old, &api(&renamed));
    assert!(
        found.iter().all(|f| f.rule.name.starts_with("parameter")),
        "{:?}",
        summary(&found)
    );
}

#[test]
fn channel_parameters_restrict_addresses() {
    let with_enum = V3.replace("      userId: {}", "      userId: {enum: [a, b]}");
    let old = api(&with_enum);
    // The application sends on fewer addresses: consumers are unaffected;
    // on more, subscribers to the known ones miss messages.
    let narrower = api(&with_enum.replace("[a, b]", "[a]"));
    assert!(worst(&findings(&old, &narrower)) < Severity::Breaking);
    let wider = api(&with_enum.replace("[a, b]", "[a, b, c]"));
    assert!(worst(&findings(&old, &wider)) >= Severity::Risky);
}

#[test]
fn replies_are_responses() {
    let text = r"
asyncapi: 3.0.0
info: {title: Pricing, version: 1.0.0}
channels:
  quotes:
    address: quotes
    messages:
      QuoteRequest: {payload: {type: object, properties: {sku: {type: string}}, required: [sku]}}
  answers:
    address: null
    messages:
      Quote: {payload: {type: object, properties: {price: {type: number}}, required: [price]}}
operations:
  quote:
    action: receive
    channel: {$ref: '#/channels/quotes'}
    reply:
      channel: {$ref: '#/channels/answers'}
";
    let old = api(text);
    let op = &old.operations[0];
    assert_eq!(op.responses[0].content[0].media_type, "Quote");
    // The reply goes out: dropping its required field breaks requesters.
    let new = api(&text.replace(
        "properties: {price: {type: number}}, required: [price]",
        "properties: {}",
    ));
    let found = findings(&old, &new);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
    assert!(
        found[0]
            .change
            .path
            .to_string()
            .starts_with("op[receive quotes]/response[default]/message[Quote]")
    );
}

const AVRO_V3: &str = r"
asyncapi: 3.0.0
info: {title: Orders, version: 1.0.0}
channels:
  orders:
    address: orders
    messages:
      OrderPlaced:
        contentType: application/octet-stream
        payload:
          schemaFormat: application/vnd.apache.avro+json;version=1.9.0
          schema:
            type: record
            name: OrderPlaced
            namespace: shop
            fields:
              - {name: id, type: string}
              - {name: total, type: long}
      Note:
        payload: {type: object, properties: {text: {type: string}}}
operations:
  takeOrders:
    action: receive
    channel: {$ref: '#/channels/orders'}
";

#[test]
fn avro_payloads_sit_beside_json_ones() {
    let old = api(AVRO_V3);
    assert!(old.named_type("shop.OrderPlaced").is_some());
    // Received data written with the old schema lacks a new field: without a
    // default the new reader cannot read it.
    let without_default = AVRO_V3.replace(
        "              - {name: total, type: long}\n",
        "              - {name: total, type: long}\n              - {name: coupon, type: string}\n",
    );
    let found = findings(&old, &api(&without_default));
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
    let with_default = AVRO_V3.replace(
        "              - {name: total, type: long}\n",
        "              - {name: total, type: long}\n              - {name: coupon, type: string, default: \"\"}\n",
    );
    let found = findings(&old, &api(&with_default));
    assert!(worst(&found) < Severity::Breaking, "{:?}", summary(&found));
}

#[test]
fn avro_schemas_can_live_in_their_own_files() {
    let text = r"
asyncapi: 2.6.0
info: {title: Orders, version: 1.0.0}
channels:
  orders:
    publish:
      message:
        name: OrderPlaced
        schemaFormat: application/vnd.apache.avro;version=1.9.0
        payload: {$ref: 'schemas/order.avsc'}
";
    let schema = r#"{"type": "record", "name": "OrderPlaced", "fields": [{"name": "id", "type": "string"}]}"#;
    let api = api_with(&[("events.yaml", text), ("schemas/order.avsc", schema)]);
    assert!(api.named_type("OrderPlaced").is_some());
    let broken = lower_with(&[
        ("events.yaml", text),
        (
            "schemas/order.avsc",
            r#"{"type": "record", "name": "OrderPlaced"}"#,
        ),
    ]);
    let codes: Vec<String> = broken
        .diagnostics
        .iter()
        .filter_map(|d| d.code.map(|c| c.to_string()))
        .collect();
    assert!(codes.contains(&"FLE6008".to_owned()), "{codes:?}");
}

const PROTOBUF_V3: &str = r#"
asyncapi: 3.0.0
info: {title: Orders, version: 1.0.0}
channels:
  orders:
    address: orders
    messages:
      OrderPlaced:
        contentType: application/protobuf
        payload:
          schemaFormat: application/vnd.google.protobuf;version=3
          schema: |
            syntax = "proto3";
            package shop;
            import "money.proto";
            message OrderPlaced {
              string id = 1;
              Money total = 2;
              Status status = 3;
            }
            enum Status {
              STATUS_UNSPECIFIED = 0;
              STATUS_PAID = 1;
            }
operations:
  takeOrders:
    action: receive
    channel: {$ref: '#/channels/orders'}
"#;

const MONEY: &str = r#"
syntax = "proto3";
package shop;
message Money { int64 units = 1; string currency = 2; }
"#;

fn protobuf_api(text: &str) -> Api {
    api_with(&[("events.yaml", text), ("money.proto", MONEY)])
}

#[test]
fn protobuf_payloads_are_their_first_message() {
    let old = protobuf_api(PROTOBUF_V3);
    assert!(old.named_type("shop.OrderPlaced").is_some());
    assert!(old.named_type("shop.Money").is_some(), "imports are read");
    // Fields match by number, so a rename is compatible on the wire...
    let renamed = PROTOBUF_V3.replace("string id = 1;", "string order_id = 1;");
    let found = findings(&old, &protobuf_api(&renamed));
    assert!(worst(&found) < Severity::Breaking, "{:?}", summary(&found));
    // ...and a new number for a field is not.
    let renumbered = PROTOBUF_V3.replace("string id = 1;", "string id = 4;");
    let found = findings(&old, &protobuf_api(&renumbered));
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
}

#[test]
fn invalid_protobuf_payloads_are_reported() {
    let codes_of = |text: &str| -> Vec<String> {
        lower_with(&[("events.yaml", text), ("money.proto", MONEY)])
            .diagnostics
            .iter()
            .filter_map(|d| d.code.map(|c| c.to_string()))
            .collect()
    };
    let unresolved = codes_of(&PROTOBUF_V3.replace("Money total", "Cash total"));
    assert!(unresolved.contains(&"FLE3003".to_owned()), "{unresolved:?}");
    assert!(unresolved.contains(&"FLE6009".to_owned()), "{unresolved:?}");
    let no_message = PROTOBUF_V3.replace(
        "message OrderPlaced",
        "enum Nothing { N = 0; }\n            message OrderPlaced",
    );
    assert!(
        codes_of(&no_message).is_empty(),
        "enums before the message are skipped"
    );
    let only_enums = PROTOBUF_V3.replace("message OrderPlaced", "enum OrderPlaced");
    let only_enums = only_enums.replace(
        "string id = 1;\n              Money total = 2;\n              Status status = 3;",
        "PLACED = 0;",
    );
    assert_eq!(codes_of(&only_enums), ["FLE3014", "FLE6009"]);
    let not_a_string = r"
asyncapi: 2.6.0
info: {title: Orders, version: 1.0.0}
channels:
  orders:
    publish:
      message:
        schemaFormat: application/vnd.google.protobuf;version=3
        payload: {type: object}
";
    assert_eq!(codes_of(not_a_string), ["FLE6009"]);
}

fn codes(text: &str) -> Vec<(String, bool)> {
    lower_with(&[("events.yaml", text)])
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.map(|c| c.to_string()).unwrap_or_default(),
                d.is_error(),
            )
        })
        .collect()
}

#[test]
fn problems_are_reported() {
    assert_eq!(codes("asyncapi: 1.2.0\n"), [("FLE6001".to_owned(), true)]);
    assert_eq!(codes("openapi: 3.1.0\n"), [("FLE6001".to_owned(), true)]);
    let bad_action = r"
asyncapi: 3.0.0
info: {title: x, version: '1'}
channels: {c: {address: c}}
operations:
  o: {action: publish, channel: {$ref: '#/channels/c'}}
";
    assert_eq!(codes(bad_action), [("FLE6007".to_owned(), true)]);
    let unresolved = r"
asyncapi: 3.0.0
info: {title: x, version: '1'}
operations:
  o: {action: send, channel: {$ref: '#/channels/missing'}}
";
    assert_eq!(codes(unresolved), [("FLE6003".to_owned(), true)]);
    let warnings = r"
asyncapi: 2.6.0
info: {title: x, version: '1'}
channels:
  c:
    subscribe:
      traits: [{description: shared}]
      message:
        schemaFormat: application/raml+yaml;version=1.0
        payload: {type: string}
";
    let found = codes(warnings);
    assert!(found.contains(&("FLE6005".to_owned(), false)), "{found:?}");
    assert!(
        !found.iter().any(|(code, _)| code == "FLE6006"),
        "traits apply: {found:?}"
    );
    assert!(found.iter().all(|(_, error)| !error));
}

#[test]
fn unnamed_messages_are_named_by_position() {
    let text = r"
asyncapi: 2.6.0
info: {title: x, version: '1'}
channels:
  c:
    publish:
      message:
        oneOf:
          - payload: {type: string}
          - messageId: second
            payload: {type: integer}
";
    let api = api(text);
    let names: Vec<&str> = api.operations[0]
        .request_body
        .as_ref()
        .unwrap()
        .content
        .iter()
        .map(|c| c.media_type.as_str())
        .collect();
    assert_eq!(names, ["message1", "second"]);
    let paths: Vec<String> = api
        .roots()
        .into_iter()
        .map(|r| r.path.to_string())
        .collect();
    assert!(
        paths
            .iter()
            .all(|p| p.parse::<faultline_ir::IrPath>().unwrap().to_string() == *p)
    );
    assert!(matches!(
        api.roots()[0].path.segments().last(),
        Some(Segment::Message(name)) if name == "message1"
    ));
}

#[test]
fn traits_apply() {
    let text = r"
asyncapi: 2.6.0
info: {title: x, version: '1'}
channels:
  orders:
    subscribe:
      traits: [{$ref: '#/components/operationTraits/Named'}]
      message:
        payload: {type: object}
        headers: {type: object, properties: {id: {type: string}}}
        traits:
          - $ref: '#/components/messageTraits/Common'
components:
  operationTraits:
    Named: {operationId: publishOrders}
  messageTraits:
    Common:
      name: OrderPlaced
      contentType: application/xml
      headers: {type: object, properties: {trace: {type: string}}, required: [trace]}
";
    let lowered = api(text);
    let op = &lowered.webhooks[0].operations[0];
    assert_eq!(op.operation_id.as_deref(), Some("publishOrders"));
    let message = &op.request_body.as_ref().unwrap().content[0];
    assert_eq!(
        message.media_type, "OrderPlaced",
        "the trait names the message"
    );
    let TypeKind::Object(envelope) = &lowered.types.get(message.ty).kind else {
        panic!()
    };
    let headers = envelope.field("headers").unwrap().ty;
    assert!(
        matches!(&lowered.types.get(headers).kind, TypeKind::AllOf(parts) if parts.len() == 2),
        "the message's and the trait's headers both apply"
    );

    // A header the trait requires is part of the contract: dropping the
    // requirement from the trait changes what consumers may rely on.
    let relaxed = text.replace("required: [trace]", "");
    let found = findings(&lowered, &api(&relaxed));
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));

    // In 2.x a trait overrides the message; in 3.0 the message wins.
    let own = text.replace(
        "        payload: {type: object}\n",
        "        payload: {type: object}\n        contentType: application/json\n",
    );
    assert_eq!(content_type_of(&api(&own)), "application/xml");
}

/// The content type the first sent message declares.
fn content_type_of(api: &Api) -> String {
    let message = api.webhooks[0].operations[0]
        .request_body
        .as_ref()
        .unwrap()
        .content[0]
        .ty;
    let TypeKind::Object(envelope) = &api.types.get(message).kind else {
        panic!()
    };
    let TypeKind::Enum(values) = &api
        .types
        .get(envelope.field("contentType").unwrap().ty)
        .kind
    else {
        panic!()
    };
    values.values[0]
        .value
        .to_string()
        .trim_matches('"')
        .to_owned()
}

#[test]
fn in_3_0_messages_take_precedence_over_traits() {
    let text = r"
asyncapi: 3.0.0
info: {title: x, version: '1'}
channels:
  orders:
    address: orders
    messages:
      placed:
        contentType: application/json
        payload: {type: object}
        traits: [{contentType: application/xml, name: OrderPlaced}]
operations:
  publish: {action: send, channel: {$ref: '#/channels/orders'}}
";
    let api = api(text);
    assert_eq!(content_type_of(&api), "application/json");
    let message = &api.webhooks[0].operations[0]
        .request_body
        .as_ref()
        .unwrap()
        .content[0];
    assert_eq!(
        message.media_type, "OrderPlaced",
        "traits fill in what is left out"
    );
}

const SECURED: &str = r##"asyncapi: 3.0.0
info: {title: Orders, version: 1.0.0}
channels:
  orders:
    address: orders
    messages:
      placed:
        payload: {type: object, properties: {id: {type: string}}}
        bindings:
          kafka:
            key: {type: string, maxLength: 36}
operations:
  receiveOrders:
    action: receive
    channel: {$ref: "#/channels/orders"}
    security:
      - $ref: "#/components/securitySchemes/sasl"
components:
  securitySchemes:
    sasl: {type: scramSha512}
    mtls: {type: X509}
"##;

#[test]
fn security_and_kafka_keys_are_compared() {
    let old = api(SECURED);
    let op = &old.operations[0];
    let security = op.security.as_ref().unwrap();
    assert_eq!(security.len(), 1);
    assert!(security[0].schemes.contains_key("sasl"));

    let other_scheme = SECURED.replace("securitySchemes/sasl\"", "securitySchemes/mtls\"");
    let found = findings(&old, &api(&other_scheme));
    assert!(
        summary(&found)
            .iter()
            .any(|s| s.contains("security-requirement")),
        "{:?}",
        summary(&found)
    );

    let shorter_key = SECURED.replace("maxLength: 36", "maxLength: 16");
    let found = findings(&old, &api(&shorter_key));
    assert!(
        summary(&found).iter().any(|s| s.contains("[placed].key")),
        "the key is compared: {:?}",
        summary(&found)
    );
}

#[test]
fn version_2_operation_security() {
    let text = r"asyncapi: 2.6.0
info: {title: Orders, version: 1.0.0}
channels:
  orders:
    publish:
      security: [{oauth: [orders.read]}]
      message:
        payload: {type: string}
components:
  securitySchemes:
    oauth: {type: oauth2, flows: {}}
";
    let old = api(text);
    let requirement = &old.operations[0].security.as_ref().unwrap()[0];
    assert_eq!(requirement.schemes["oauth"], ["orders.read"]);
    let wider = api(&text.replace("[orders.read]", "[orders.read, orders.admin]"));
    assert!(worst(&findings(&old, &wider)) >= Severity::Risky);
}
