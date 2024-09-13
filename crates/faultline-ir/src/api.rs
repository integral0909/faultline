use std::fmt;

use indexmap::IndexMap;

use crate::arena::{TypeArena, TypeId};
use crate::path::{IrPath, Segment};
use crate::polarity::Polarity;
use crate::surface::{Callback, Operation, Role, SchemaRoot};
use crate::types::QualName;

/// The format an [`Api`] was lowered from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApiFormat {
    Swagger2,
    OpenApi30,
    OpenApi31,
    JsonSchema,
    Protobuf,
    GraphQl,
    Avro,
    AsyncApi2,
    AsyncApi3,
    XmlSchema,
}

impl ApiFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ApiFormat::Swagger2 => "swagger-2.0",
            ApiFormat::OpenApi30 => "openapi-3.0",
            ApiFormat::OpenApi31 => "openapi-3.1",
            ApiFormat::JsonSchema => "jsonschema",
            ApiFormat::Protobuf => "protobuf",
            ApiFormat::GraphQl => "graphql",
            ApiFormat::Avro => "avro",
            ApiFormat::AsyncApi2 => "asyncapi-2",
            ApiFormat::AsyncApi3 => "asyncapi-3",
            ApiFormat::XmlSchema => "xsd",
        }
    }
}

impl fmt::Display for ApiFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One API contract in format-independent form.
#[derive(Clone, Debug)]
pub struct Api {
    pub name: String,
    pub format: ApiFormat,
    pub version: Option<String>,
    pub operations: Vec<Operation>,
    /// Requests the API sends on its own initiative (OpenAPI 3.1 webhooks).
    /// Values flow the opposite way from ordinary operations.
    pub webhooks: Vec<Callback>,
    pub schemas: Vec<SchemaRoot>,
    pub types: TypeArena,
    /// Named type definitions, in declaration order.
    pub named: IndexMap<QualName, TypeId>,
}

/// A type position at the boundary of the API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub path: IrPath,
    pub ty: TypeId,
    pub polarity: Polarity,
}

impl Api {
    pub fn new(name: impl Into<String>, format: ApiFormat) -> Self {
        Self {
            name: name.into(),
            format,
            version: None,
            operations: Vec::new(),
            webhooks: Vec::new(),
            schemas: Vec::new(),
            types: TypeArena::new(),
            named: IndexMap::new(),
        }
    }

    /// Every type position where values cross the API boundary, with the
    /// direction they cross it in. The order is deterministic: document order
    /// of operations and, within each, parameters, request body, responses,
    /// then callbacks; then webhooks; schema roots come last.
    pub fn roots(&self) -> Vec<Root> {
        let mut roots = Vec::new();
        for op in &self.operations {
            operation_roots(op, &IrPath::new(), false, &mut roots);
        }
        for webhook in &self.webhooks {
            let at = IrPath::new().join(Segment::Webhook(webhook.name.clone()));
            for op in &webhook.operations {
                operation_roots(op, &at, true, &mut roots);
            }
        }
        for schema in &self.schemas {
            roots.push(Root {
                path: IrPath::new().join(Segment::Schema(schema.name.clone())),
                ty: schema.ty,
                polarity: role_polarity(schema.role),
            });
        }
        roots
    }

    /// Looks up a named type.
    pub fn named_type(&self, name: &str) -> Option<TypeId> {
        self.named.get(&QualName::new(name)).copied()
    }
}

fn role_polarity(role: Role) -> Polarity {
    match role {
        Role::Input => Polarity::IN,
        Role::Output => Polarity::OUT,
        Role::Both => Polarity::BOTH,
    }
}

type SegmentFn = fn(String) -> Segment;

fn operation_roots(op: &Operation, prefix: &IrPath, callback: bool, out: &mut Vec<Root>) {
    // Inside a callback the API is the client: it sends the request and
    // receives the response.
    let (request_flow, response_flow) = if callback {
        (Polarity::OUT, Polarity::IN)
    } else {
        (Polarity::IN, Polarity::OUT)
    };
    let base = prefix.join(Segment::Operation(op.key.identity()));
    // Message operations key their payloads by message, not media type.
    let (request_segment, body_segment): (SegmentFn, SegmentFn) = if op.key.is_message() {
        (Segment::Message, Segment::Message)
    } else {
        (Segment::RequestBody, Segment::Body)
    };

    for param in &op.parameters {
        out.push(Root {
            path: base.join(Segment::Parameter(param.location, param.identity_name())),
            ty: param.ty,
            polarity: request_flow,
        });
    }
    if let Some(body) = &op.request_body {
        for content in &body.content {
            out.push(Root {
                path: base.join(request_segment(content.media_type.clone())),
                ty: content.ty,
                polarity: request_flow,
            });
        }
    }
    for response in &op.responses {
        let at = base.join(Segment::Response(response.status.to_string()));
        for header in &response.headers {
            out.push(Root {
                path: at.join(Segment::Header(header.name.to_ascii_lowercase())),
                ty: header.ty,
                polarity: response_flow,
            });
        }
        for content in &response.content {
            out.push(Root {
                path: at.join(body_segment(content.media_type.clone())),
                ty: content.ty,
                polarity: response_flow,
            });
        }
    }
    for cb in &op.callbacks {
        let at = base.join(Segment::Callback(cb.name.clone()));
        for inner in &cb.operations {
            operation_roots(inner, &at, !callback, out);
        }
    }
}
