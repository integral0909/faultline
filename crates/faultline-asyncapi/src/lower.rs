//! Channels and operations of AsyncAPI 2.x and 3.0.

use faultline_doc::Value;
use faultline_ir::SecurityRequirement;
use faultline_ir::{
    Callback, Content, EnumType, EnumValue, Literal, MessageAction, Meta, Operation, OperationKey,
    ParamLocation, Parameter, PathTemplate, RequestBody, Response, Retired, StatusKey, StringType,
    TypeKind,
};
use faultline_jsonschema::Dialect;
use faultline_jsonschema::nav::At;

use crate::SENT;
use crate::codes;
use crate::context::{Context, referenced_name};
use crate::message::content;

/// Operations collected so far, merged by identity: 3.0 allows several
/// operations with the same action on one channel, which together carry
/// the union of their messages.
#[derive(Default)]
struct Operations {
    receive: Vec<Operation>,
    send: Vec<Operation>,
}

impl Operations {
    fn add(&mut self, op: Operation) {
        let list = match op.key {
            OperationKey::Message {
                action: MessageAction::Send,
                ..
            } => &mut self.send,
            _ => &mut self.receive,
        };
        let identity = op.key.identity();
        let Some(existing) = list.iter_mut().find(|o| o.key.identity() == identity) else {
            list.push(op);
            return;
        };
        for parameter in op.parameters {
            if !existing.parameters.iter().any(|p| p.name == parameter.name) {
                existing.parameters.push(parameter);
            }
        }
        if let (Some(into), Some(from)) = (&mut existing.request_body, op.request_body) {
            merge_contents(&mut into.content, from.content);
        }
        for response in op.responses {
            match existing
                .responses
                .iter_mut()
                .find(|r| r.status == response.status)
            {
                Some(into) => merge_contents(&mut into.content, response.content),
                None => existing.responses.push(response),
            }
        }
    }

    fn finish(self, cx: &mut Context<'_, '_, '_>) {
        let api = cx.lowerer.api_mut();
        api.operations.extend(self.receive);
        if !self.send.is_empty() {
            api.webhooks.push(Callback {
                name: SENT.to_owned(),
                operations: self.send,
                origin: None,
            });
        }
    }
}

fn merge_contents(into: &mut Vec<Content>, from: Vec<Content>) {
    for content in from {
        if !into.iter().any(|c| c.media_type == content.media_type) {
            into.push(content);
        }
    }
}

/// AsyncAPI 2.x: operations live inside channels, keyed by address.
pub(crate) fn version_2(cx: &mut Context<'_, '_, '_>, root: &At) {
    let mut operations = Operations::default();
    for (address, entry) in root
        .child("channels")
        .map(|c| c.entries())
        .unwrap_or_default()
    {
        let Some(channel) = cx.resolve(entry) else {
            continue;
        };
        if !cx.expect_object(&channel, "a channel") {
            continue;
        }
        let parameters = parameters(cx, &channel);
        // `publish` means others publish to the application, which receives.
        for (key, action) in [
            ("publish", MessageAction::Receive),
            ("subscribe", MessageAction::Send),
        ] {
            let Some(entry) = channel.child(key) else {
                continue;
            };
            let origin = entry.key_span();
            let Some(at) = cx.resolve(entry) else {
                continue;
            };
            if !cx.expect_object(&at, "an operation") {
                continue;
            }
            let traits = cx.trait_list(&at);
            let mut contents = Vec::new();
            if let Some(message) = at.child("message") {
                match message.child("oneOf") {
                    Some(one_of) => {
                        for (i, element) in one_of.elements().into_iter().enumerate() {
                            let fallback = format!("message{}", i + 1);
                            contents.extend(content(cx, element, &fallback));
                        }
                    }
                    None => contents.extend(content(cx, message, "message")),
                }
            }
            let mut op = Operation::new(OperationKey::Message {
                action,
                channel: PathTemplate::parse(&address),
            });
            op.operation_id = cx.traited_string(&at, &traits, "operationId");
            op.parameters.clone_from(&parameters);
            op.request_body = Some(RequestBody {
                required: true,
                content: contents,
                meta: Meta::default(),
                origin: Some(at.span()),
            });
            op.meta.description = cx.string(&at, "description");
            op.security = at.child("security").map(|s| security_2(cx, &s));
            op.origin = Some(origin);
            operations.add(op);
        }
    }
    component_schemas(cx, root);
    operations.finish(cx);
}

/// A 2.x operation's `security`: requirements naming schemes and the
/// scopes they need, as OpenAPI writes them.
fn security_2(cx: &mut Context<'_, '_, '_>, at: &At) -> Vec<SecurityRequirement> {
    let Some(list) = at.node().as_array() else {
        cx.error(codes::INVALID_FIELD, "security must be an array", at.span());
        return Vec::new();
    };
    list.iter()
        .filter_map(|element| {
            let object = element.as_object()?;
            let schemes = object
                .iter()
                .map(|(name, scopes)| {
                    let mut scopes: Vec<String> = scopes
                        .as_array()
                        .map(|s| {
                            s.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    scopes.sort();
                    scopes.dedup();
                    (name.to_owned(), scopes)
                })
                .collect();
            Some(SecurityRequirement { schemes })
        })
        .collect()
}

/// A 3.0 operation's `security`: security schemes, any one of which
/// suffices, each named by the component it references (or its `type`
/// when written in place), with the scopes it lists.
fn security_3(cx: &mut Context<'_, '_, '_>, at: &At) -> Vec<SecurityRequirement> {
    let mut out = Vec::new();
    for element in at.elements() {
        let name = referenced_name(&element);
        let Some(scheme) = cx.resolve(element) else {
            continue;
        };
        let Some(name) = name.or_else(|| cx.string(&scheme, "type")) else {
            continue;
        };
        let mut scopes: Vec<String> = scheme
            .get("scopes")
            .and_then(|s| s.as_array())
            .map(|s| {
                s.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        scopes.sort();
        scopes.dedup();
        out.push(SecurityRequirement {
            schemes: std::iter::once((name, scopes)).collect(),
        });
    }
    out
}

/// AsyncAPI 3.0: operations are top-level and point at channels.
pub(crate) fn version_3(cx: &mut Context<'_, '_, '_>, root: &At) {
    let mut operations = Operations::default();
    for (id, entry) in root
        .child("operations")
        .map(|o| o.entries())
        .unwrap_or_default()
    {
        let origin = entry.key_span();
        let Some(at) = cx.resolve(entry) else {
            continue;
        };
        if !cx.expect_object(&at, "an operation") {
            continue;
        }
        // 3.0 operation traits carry nothing the comparison uses.
        let action = match cx.string(&at, "action").as_deref() {
            Some("send") => MessageAction::Send,
            Some("receive") => MessageAction::Receive,
            other => {
                let span = at.get("action").map_or(origin, |n| n.span);
                let message = match other {
                    Some(value) => format!("action must be send or receive, not {value}"),
                    None => "an operation needs an action, send or receive".to_owned(),
                };
                cx.error(codes::INVALID_ACTION, &message, span);
                continue;
            }
        };
        let Some(channel_entry) = at.child("channel") else {
            cx.error(codes::INVALID_FIELD, "an operation needs a channel", origin);
            continue;
        };
        let channel_name = referenced_name(&channel_entry).unwrap_or_else(|| id.clone());
        let Some(channel) = cx.resolve(channel_entry) else {
            continue;
        };
        let address = channel_address(cx, &channel, &channel_name);
        let parameters = parameters(cx, &channel);
        let contents = operation_messages(cx, &at, &channel);

        let mut op = Operation::new(OperationKey::Message {
            action,
            channel: PathTemplate::parse(&address),
        });
        op.operation_id = Some(id);
        op.parameters = parameters;
        op.request_body = Some(RequestBody {
            required: true,
            content: contents,
            meta: Meta::default(),
            origin: Some(at.span()),
        });
        if let Some(reply) = reply(cx, &at) {
            op.responses.push(reply);
        }
        op.security = at.child("security").map(|s| security_3(cx, &s));
        op.meta.description = cx.string(&at, "description");
        op.origin = Some(origin);
        operations.add(op);
    }
    component_schemas(cx, root);
    operations.finish(cx);
}

/// A 3.0 channel's address; channels whose address is unknown (`null` or
/// absent) are identified by name.
fn channel_address(cx: &mut Context<'_, '_, '_>, channel: &At, name: &str) -> String {
    match channel.get("address").map(|n| &n.value) {
        Some(Value::String(address)) => address.clone(),
        Some(Value::Null) | None => name.to_owned(),
        Some(_) => {
            let span = channel.get("address").map_or(channel.span(), |n| n.span);
            cx.error(
                codes::INVALID_FIELD,
                "address must be a string or null",
                span,
            );
            name.to_owned()
        }
    }
}

/// The messages a 3.0 operation carries: those it lists, or every message
/// of its channel.
fn operation_messages(cx: &mut Context<'_, '_, '_>, at: &At, channel: &At) -> Vec<Content> {
    let mut contents = Vec::new();
    match at.child("messages") {
        Some(list) => {
            for (i, element) in list.elements().into_iter().enumerate() {
                let fallback = format!("message{}", i + 1);
                contents.extend(content(cx, element, &fallback));
            }
        }
        None => {
            for (key, entry) in channel
                .child("messages")
                .map(|m| m.entries())
                .unwrap_or_default()
            {
                contents.extend(content(cx, entry, &key));
            }
        }
    }
    contents
}

/// A 3.0 operation's reply, as its response.
fn reply(cx: &mut Context<'_, '_, '_>, at: &At) -> Option<Response> {
    let entry = at.child("reply")?;
    let reply = cx.resolve(entry)?;
    if !cx.expect_object(&reply, "a reply") {
        return None;
    }
    let channel = match reply.child("channel") {
        Some(entry) => cx.resolve(entry),
        None => None,
    };
    let mut contents = Vec::new();
    match (reply.child("messages"), &channel) {
        (Some(list), _) => {
            for (i, element) in list.elements().into_iter().enumerate() {
                let fallback = format!("reply{}", i + 1);
                contents.extend(content(cx, element, &fallback));
            }
        }
        (None, Some(channel)) => {
            for (key, entry) in channel
                .child("messages")
                .map(|m| m.entries())
                .unwrap_or_default()
            {
                contents.extend(content(cx, entry, &key));
            }
        }
        (None, None) => {}
    }
    Some(Response {
        status: StatusKey::Default,
        headers: Vec::new(),
        content: contents,
        meta: Meta::default(),
        origin: Some(reply.span()),
    })
}

/// A channel's address parameters. They are always present in a concrete
/// address, so they are required.
fn parameters(cx: &mut Context<'_, '_, '_>, channel: &At) -> Vec<Parameter> {
    let mut out = Vec::new();
    for (name, entry) in channel
        .child("parameters")
        .map(|p| p.entries())
        .unwrap_or_default()
    {
        let origin = entry.key_span();
        let Some(at) = cx.resolve(entry) else {
            continue;
        };
        if !cx.expect_object(&at, "a parameter") {
            continue;
        }
        let ty = if cx.v3 {
            parameter_type_3(cx, &at)
        } else {
            match at.child("schema") {
                Some(schema) => cx.schema(&schema, Dialect::Draft7),
                None => cx
                    .lowerer
                    .api_mut()
                    .types
                    .alloc_kind(TypeKind::String(StringType::default())),
            }
        };
        let meta = Meta {
            description: cx.string(&at, "description"),
            ..Meta::default()
        };
        out.push(Parameter {
            location: ParamLocation::Path,
            name,
            required: true,
            ty,
            serialization: None,
            meta,
            origin: Some(origin),
        });
    }
    out
}

/// A 3.0 Parameter Object: a string, restricted to `enum` when given.
fn parameter_type_3(cx: &mut Context<'_, '_, '_>, at: &At) -> faultline_ir::TypeId {
    let values: Option<Vec<String>> = at.get("enum").map(|node| {
        node.as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    });
    let types = &mut cx.lowerer.api_mut().types;
    match values {
        Some(values) => types.alloc_kind(TypeKind::Enum(EnumType {
            values: values
                .into_iter()
                .map(|v| EnumValue::new(Literal::string(v)))
                .collect(),
            open: false,
            retired: Retired::default(),
            writer_exact: false,
        })),
        None => types.alloc_kind(TypeKind::String(StringType::default())),
    }
}

/// Every schema in `components/schemas` becomes a named type, used or not,
/// as in OpenAPI. Schemas in other formats are lowered where messages use
/// them.
fn component_schemas(cx: &mut Context<'_, '_, '_>, root: &At) {
    let schemas = root
        .child("components")
        .and_then(|c| c.child("schemas"))
        .map(|s| s.entries())
        .unwrap_or_default();
    for (_, schema) in schemas {
        if schema.get("schemaFormat").is_none() {
            cx.schema(&schema, Dialect::Draft7);
        }
    }
}
