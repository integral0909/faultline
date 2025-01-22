//! Lowering a Smithy model's services into operations.
//!
//! A service's operations are those it lists and those of its resources,
//! their child resources included: lifecycle operations (`create`, `read`,
//! ...), `operations` and `collectionOperations`.
//!
//! An operation with `@http` is an HTTP operation (`op[GET /cities/{}]`),
//! bound as the HTTP binding traits say: `@httpLabel` members are path
//! parameters, `@httpQuery` and `@httpHeader` members query and header
//! parameters, an `@httpPayload` member is the body, and otherwise the
//! members bound to nothing make up a JSON body. The response has the
//! status `@http` gives (200 by default), with its headers and body bound
//! the same way.
//!
//! An operation without `@http` is a remote procedure call
//! (`op[Weather/GetCity]`) whose request is its input and whose response
//! is its output.
//!
//! Either way, errors are responses with the status of their
//! `@httpError`, or 400 for `@error("client")` and 500 for
//! `@error("server")`; errors that share a status are the alternatives of
//! one response. The service's common errors apply to each operation.

use std::collections::{BTreeMap, HashSet};

use faultline_ir::{
    Content, Header, HttpMethod, Meta, Operation, OperationKey, ParamLocation, Parameter,
    PathTemplate, RequestBody, Response, StatusKey, Streaming, TypeId, TypeKind, TypeNode,
    UnionMode, UnionType,
};

use crate::ast::{NodeValue, ShapeKind};
use crate::lower::{Lowerer, meta};
use crate::resolve::{RMember, RShape, prelude};

/// The media type recorded for bodies of structured data.
const JSON: &str = "application/json";

impl<'m> Lowerer<'m> {
    /// Lowers the operations of the services in `namespace`.
    pub(crate) fn services(&mut self, namespace: &str) {
        let model = self.model;
        let prefix = format!("{namespace}#");
        let mut services: Vec<&RShape> = model
            .shapes
            .iter()
            .filter(|s| s.kind == ShapeKind::Service && s.id.starts_with(&prefix))
            .collect();
        services.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(version) = services.iter().find_map(|s| s.version.clone()) {
            self.api.version = Some(version);
        }
        for service in services {
            for id in self.operations_of(service) {
                if let Some(op) = model.get(&id) {
                    let lowered = self.operation(service, op);
                    self.api.operations.push(lowered);
                }
            }
        }
    }

    /// The operations a service exposes, directly or through resources.
    fn operations_of(&self, service: &RShape) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut resources = service.bindings.resources.clone();
        let mut visited = HashSet::new();
        for op in &service.bindings.operations {
            if seen.insert(op.clone()) {
                out.push(op.clone());
            }
        }
        while let Some(resource) = resources.pop() {
            if !visited.insert(resource.clone()) {
                continue;
            }
            let Some(shape) = self.model.get(&resource) else {
                continue;
            };
            for op in &shape.bindings.operations {
                if seen.insert(op.clone()) {
                    out.push(op.clone());
                }
            }
            resources.extend(shape.bindings.resources.iter().cloned());
        }
        out
    }

    fn operation(&mut self, service: &RShape, op: &'m RShape) -> Operation {
        let model = self.model;
        let body = op.operation.clone().unwrap_or_default();
        let input = body.input.as_deref().and_then(|id| model.get(id));
        let output = body.output.as_deref().and_then(|id| model.get(id));
        let http = op.get_trait(&prelude("http")).map(|t| &t.value);
        let method = http
            .and_then(|h| h.get("method"))
            .and_then(|m| m.as_str())
            .and_then(HttpMethod::parse);
        let uri = http.and_then(|h| h.get("uri")).and_then(|u| u.as_str());
        let key = match (method, uri) {
            (Some(method), Some(uri)) => OperationKey::Http {
                method,
                path: PathTemplate::parse(&path_of(uri)),
            },
            _ => OperationKey::Rpc {
                service: service.name.clone(),
                method: op.name.clone(),
            },
        };
        let bound = matches!(key, OperationKey::Http { .. });
        let mut lowered = Operation::new(key);
        lowered.meta = meta(op.doc.as_ref(), op);
        lowered.origin = Some(op.span);
        let mut streaming = Streaming::default();
        if let Some(input) = input {
            let (parameters, request) = self.request(input, bound, &mut streaming.request);
            lowered.parameters = parameters;
            lowered.request_body = request;
        }
        let code = http
            .and_then(|h| h.get("code"))
            .and_then(|c| match &c.value {
                NodeValue::Number(n) => n.parse::<u16>().ok(),
                _ => None,
            })
            .unwrap_or(200);
        lowered
            .responses
            .push(self.response(code, output, bound, &mut streaming.response, op));
        lowered
            .responses
            .extend(self.errors(service, op, &body.errors));
        if streaming.request || streaming.response {
            lowered.streaming = Some(streaming);
        }
        lowered
    }

    /// The parameters and body of a request.
    fn request(
        &mut self,
        input: &'m RShape,
        bound: bool,
        streaming: &mut bool,
    ) -> (Vec<Parameter>, Option<RequestBody>) {
        let mut parameters = Vec::new();
        let mut body = Vec::new();
        for member in &input.members {
            let location = if !bound {
                None
            } else if member.has_trait("httpLabel") {
                Some((ParamLocation::Path, member.name.clone()))
            } else {
                binding_name(member, "httpQuery")
                    .map(|name| (ParamLocation::Query, name))
                    .or_else(|| {
                        binding_name(member, "httpHeader").map(|name| (ParamLocation::Header, name))
                    })
            };
            match location {
                Some((location, name)) => {
                    let ty = self.target(&member.target, Some(member));
                    parameters.push(Parameter {
                        location,
                        name,
                        required: location == ParamLocation::Path || member.has_trait("required"),
                        ty,
                        serialization: None,
                        meta: Meta {
                            description: member.doc.clone(),
                            deprecated: member.has_trait("deprecated"),
                            ..Meta::default()
                        },
                        origin: Some(member.span),
                    });
                }
                None if bound && ignored(member) => {}
                None => body.push(member),
            }
        }
        let request = if bound {
            self.bound_body(input, &body, streaming)
        } else {
            Some(self.whole(input))
        }
        .map(|(media_type, ty, required)| RequestBody {
            required,
            content: vec![Content {
                media_type,
                ty,
                origin: Some(input.span),
            }],
            meta: Meta::default(),
            origin: Some(input.span),
        });
        (parameters, request)
    }

    fn response(
        &mut self,
        code: u16,
        output: Option<&'m RShape>,
        bound: bool,
        streaming: &mut bool,
        op: &RShape,
    ) -> Response {
        let mut headers = Vec::new();
        let mut body = Vec::new();
        for member in output.map(|o| o.members.as_slice()).unwrap_or_default() {
            match binding_name(member, "httpHeader").filter(|_| bound) {
                Some(name) => {
                    let ty = self.target(&member.target, Some(member));
                    headers.push(Header {
                        name,
                        required: member.has_trait("required"),
                        ty,
                        meta: Meta::default(),
                        origin: Some(member.span),
                    });
                }
                None if bound && (ignored(member) || member.has_trait("httpResponseCode")) => {}
                None => body.push(member),
            }
        }
        let content = match output {
            Some(output) if bound => self.bound_body(output, &body, streaming),
            Some(output) => Some(self.whole(output)),
            None => None,
        };
        Response {
            status: StatusKey::Code(code),
            headers,
            content: content
                .into_iter()
                .map(|(media_type, ty, _)| Content {
                    media_type,
                    ty,
                    origin: output.map(|o| o.span),
                })
                .collect(),
            meta: Meta::default(),
            origin: Some(output.map_or(op.span, |o| o.span)),
        }
    }

    /// An unbound input or output: the whole structure.
    fn whole(&mut self, shape: &RShape) -> (String, TypeId, bool) {
        let ty = self.target(&shape.id, None);
        (JSON.to_owned(), ty, true)
    }

    /// The body of an HTTP message: the `@httpPayload` member, or the
    /// members bound to nothing as a JSON object. `None` when there is no
    /// body.
    fn bound_body(
        &mut self,
        shape: &RShape,
        members: &[&'m RMember],
        streaming: &mut bool,
    ) -> Option<(String, TypeId, bool)> {
        if let Some(payload) = members.iter().find(|m| m.has_trait("httpPayload")) {
            let ty = self.target(&payload.target, Some(payload));
            let target = self.model.get(&payload.target);
            *streaming |= target.is_some_and(|t| t.has_trait("streaming"));
            let media_type = payload_media_type(target, &payload.target);
            return Some((media_type, ty, payload.has_trait("required")));
        }
        if members.is_empty() {
            return None;
        }
        let any = self.api.types.alloc(TypeNode::new(TypeKind::Any));
        let mut object = faultline_ir::ObjectType::new(any);
        let mut required = false;
        for member in members {
            let ty = self.target(&member.target, Some(member));
            let mut field =
                faultline_ir::Field::new(member.name.clone(), ty, member.has_trait("required"));
            required |= field.required;
            field.origin = Some(member.span);
            field.meta.description.clone_from(&member.doc);
            field.meta.deprecated = member.has_trait("deprecated");
            object.fields.push(field);
        }
        let ty = self
            .api
            .types
            .alloc(TypeNode::new(TypeKind::Object(object)).with_origin(shape.span));
        Some((JSON.to_owned(), ty, required))
    }

    /// Responses for an operation's errors and its service's, one per
    /// status.
    fn errors(&mut self, service: &RShape, op: &RShape, own: &[String]) -> Vec<Response> {
        let mut by_status: BTreeMap<u16, Vec<&'m RShape>> = BTreeMap::new();
        let mut seen = HashSet::new();
        for id in own.iter().chain(&service.bindings.errors) {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(error) = self.model.get(id) else {
                continue;
            };
            by_status
                .entry(error_status(error))
                .or_default()
                .push(error);
        }
        by_status
            .into_iter()
            .map(|(status, errors)| {
                let variants: Vec<TypeId> =
                    errors.iter().map(|e| self.target(&e.id, None)).collect();
                let ty = if let [one] = variants.as_slice() {
                    *one
                } else {
                    self.api
                        .types
                        .alloc(TypeNode::new(TypeKind::Union(UnionType {
                            mode: UnionMode::Tagged,
                            variants,
                            discriminator: None,
                        })))
                };
                Response {
                    status: StatusKey::Code(status),
                    headers: Vec::new(),
                    content: vec![Content {
                        media_type: JSON.to_owned(),
                        ty,
                        origin: Some(errors[0].span),
                    }],
                    meta: Meta::default(),
                    origin: Some(op.span),
                }
            })
            .collect()
    }
}

/// The name a binding trait gives a member (`@httpQuery("limit")`).
fn binding_name(member: &RMember, name: &str) -> Option<String> {
    member
        .get_trait(&prelude(name))
        .and_then(|t| t.value.as_str())
        .map(str::to_owned)
}

/// Members bound to maps of query parameters or prefixed headers, which
/// are not compared.
fn ignored(member: &RMember) -> bool {
    member.has_trait("httpQueryParams") || member.has_trait("httpPrefixHeaders")
}

/// The path of an `@http` URI: without a literal query string, and with
/// greedy labels (`{key+}`) as plain ones.
fn path_of(uri: &str) -> String {
    let path = uri.split_once('?').map_or(uri, |(path, _)| path);
    path.replace("+}", "}")
}

/// The media type of a payload: `@mediaType`, or the default for its
/// kind.
fn payload_media_type(target: Option<&RShape>, id: &str) -> String {
    if let Some(media_type) = target
        .and_then(|t| t.get_trait(&prelude("mediaType")))
        .and_then(|t| t.value.as_str())
    {
        return media_type.to_owned();
    }
    let kind = target.map(|t| t.kind);
    match (kind, id) {
        (Some(ShapeKind::Simple(crate::ast::SimpleType::Blob)), _) | (None, "smithy.api#Blob") => {
            "application/octet-stream".to_owned()
        }
        (Some(ShapeKind::Simple(crate::ast::SimpleType::String)), _)
        | (None, "smithy.api#String") => "text/plain".to_owned(),
        _ => JSON.to_owned(),
    }
}

/// The HTTP status of an error structure.
fn error_status(error: &RShape) -> u16 {
    if let Some(NodeValue::Number(code)) = error
        .get_trait(&prelude("httpError"))
        .map(|t| &t.value.value)
        && let Ok(code) = code.parse::<u16>()
    {
        return code;
    }
    match error
        .get_trait(&prelude("error"))
        .and_then(|t| t.value.as_str())
    {
        Some("server") => 500,
        _ => 400,
    }
}
