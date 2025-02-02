//! Avro protocols (`.avpr`): named types, and messages that are remote
//! procedure calls.
//!
//! The protocol's types are defined in order, as a schema's are. Each
//! message is an operation (`op[Mail/send]`):
//!
//! * the request is a record of the message's parameters;
//! * the response is the response type, or, when the message declares
//!   errors, a union of the response type and the errors, since a call's
//!   result is one or the other. A new error is then a union branch that an
//!   old client's reader cannot resolve, which Avro makes an error;
//! * a `one-way` message has no response and is a one-way operation.
//!
//! A protocol without messages is compared as its named types, each a
//! schema root.

use faultline_doc::Node;
use faultline_ir::{
    Content, Meta, Operation, OperationKey, RequestBody, Response, Role, SchemaRoot, StatusKey,
    TypeKind, TypeNode, UnionMode, UnionType,
};

use crate::codes;
use crate::lower::Lowerer;

/// The media type recorded for message bodies.
pub const AVRO_MEDIA_TYPE: &str = "avro/binary";

impl Lowerer {
    /// Lowers a protocol document.
    pub(crate) fn protocol(&mut self, node: &Node, role: Role) {
        let Some(protocol) = node.get("protocol").and_then(Node::as_str) else {
            self.error(
                codes::INVALID_SCHEMA,
                node.span,
                "a protocol names itself in `protocol`".into(),
            );
            return;
        };
        let namespace = node
            .get("namespace")
            .and_then(Node::as_str)
            .unwrap_or("")
            .to_owned();
        let mut named = Vec::new();
        for ty in node
            .get("types")
            .and_then(Node::as_array)
            .unwrap_or_default()
        {
            if let Some(lowered) = self.schema(ty, &namespace)
                && let Some(name) = lowered.name
            {
                named.push((name, lowered.id, ty.span));
            }
        }
        let messages = node.get("messages").and_then(Node::as_object);
        if messages.is_none_or(faultline_doc::Object::is_empty) {
            for (name, ty, span) in named {
                self.api.schemas.push(SchemaRoot {
                    name,
                    ty,
                    role,
                    origin: Some(span),
                });
            }
            return;
        }
        for (name, message) in messages.expect("checked above").members() {
            if let Some(op) = self.message(protocol, &namespace, name, &message.value) {
                self.api.operations.push(op);
            }
        }
    }

    fn message(
        &mut self,
        protocol: &str,
        namespace: &str,
        name: &str,
        message: &Node,
    ) -> Option<Operation> {
        let mut op = Operation::new(OperationKey::Rpc {
            service: protocol.to_owned(),
            method: name.to_owned(),
        });
        op.origin = Some(message.span);
        op.meta = Meta {
            description: message.get("doc").and_then(Node::as_str).map(str::to_owned),
            ..Meta::default()
        };
        let parameters = message
            .get("request")
            .and_then(Node::as_array)
            .unwrap_or_default();
        let request = self.fields(parameters, namespace, name);
        let request = self
            .api
            .types
            .alloc(TypeNode::new(TypeKind::Object(request)).with_origin(message.span));
        op.request_body = Some(RequestBody {
            required: true,
            content: vec![Content {
                media_type: AVRO_MEDIA_TYPE.to_owned(),
                ty: request,
                origin: Some(message.span),
            }],
            meta: Meta::default(),
            origin: Some(message.span),
        });
        let one_way = message.get("one-way").and_then(Node::as_bool) == Some(true);
        let errors = message
            .get("errors")
            .and_then(Node::as_array)
            .unwrap_or_default();
        if one_way {
            let response = message.get("response").and_then(Node::as_str);
            if response.is_some_and(|r| r != "null") || !errors.is_empty() {
                self.error(
                    codes::INVALID_SCHEMA,
                    message.span,
                    format!("one-way message `{name}` can neither return a value nor throw"),
                );
            }
            op.one_way = true;
            return Some(op);
        }
        let Some(response_node) = message.get("response") else {
            self.error(
                codes::INVALID_SCHEMA,
                message.span,
                format!("message `{name}` needs a `response`"),
            );
            return None;
        };
        let response = self.schema(response_node, namespace)?;
        let mut variants = vec![response.id];
        for error in errors {
            if let Some(lowered) = self.schema(error, namespace) {
                variants.push(lowered.id);
            }
        }
        let ty = if variants.len() == 1 {
            response.id
        } else {
            self.api.types.alloc(
                TypeNode::new(TypeKind::Union(UnionType {
                    mode: UnionMode::Tagged,
                    variants,
                    discriminator: None,
                }))
                .with_origin(response_node.span),
            )
        };
        op.responses.push(Response {
            status: StatusKey::Default,
            headers: Vec::new(),
            content: vec![Content {
                media_type: AVRO_MEDIA_TYPE.to_owned(),
                ty,
                origin: Some(response_node.span),
            }],
            meta: Meta::default(),
            origin: Some(response_node.span),
        });
        Some(op)
    }
}
