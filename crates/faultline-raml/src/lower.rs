//! Lowering a RAML API's resources and methods into operations.
//!
//! Resources nest: `/pets` with a nested `/{petId}` is the path
//! `/pets/{petId}`. Each method of a resource is an operation:
//!
//! * URI parameters (declared on the resource or an ancestor, or implied by
//!   the template as strings) are path parameters;
//! * query parameters, a `queryString` type's properties, and headers are
//!   parameters, required unless named with `?` or declared
//!   `required: false`;
//! * a body is keyed by media type, or, when written as a type directly,
//!   applies to each of the API's default media types (`mediaType`, or
//!   `application/json`);
//! * responses are keyed by status code, with their headers and bodies;
//! * `securedBy` lists the security schemes, `null` meaning none needed.

use std::collections::BTreeMap;

use faultline_doc::loader::Loader;
use faultline_doc::{Node, UriRef, Value};
use faultline_ir::{
    Api, ApiFormat, Content, Header, HttpMethod, Meta, Operation, OperationKey, ParamLocation,
    Parameter, PathTemplate, RequestBody, Response, SecurityRequirement, StatusKey, TypeId,
    TypeKind,
};
use faultline_source::{Diagnostic, Diagnostics, Limits, SourceMap, Span};

use crate::codes;
use crate::load::RamlLoader;
use crate::templates::{Expander, Templates};
use crate::types::{Scope, TypeLowerer};

/// The media type bodies have when the API names none.
const DEFAULT_MEDIA_TYPE: &str = "application/json";

/// The methods a resource may have.
const METHODS: [HttpMethod; 7] = [
    HttpMethod::Get,
    HttpMethod::Put,
    HttpMethod::Post,
    HttpMethod::Delete,
    HttpMethod::Options,
    HttpMethod::Head,
    HttpMethod::Patch,
];

/// The result of lowering one version.
#[derive(Debug)]
pub struct Lowered {
    /// `None` when the description has errors.
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Reads the RAML API at `entry` with what it includes, and lowers it into
/// an [`Api`] named `name`.
pub fn lower_raml(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
    limits: Limits,
) -> Lowered {
    let mut raml = RamlLoader::new(loader, sources, limits.clone());
    let document = raml.document(entry, None);
    let libraries = document
        .as_ref()
        .filter(|d| d.kind.is_empty())
        .map(|d| crate::libraries::scope_of(&mut raml, d, ""));
    let mut diagnostics = std::mem::take(&mut raml.diagnostics);
    let Some(document) = document.filter(|_| !diagnostics.has_errors()) else {
        diagnostics.sort();
        return Lowered {
            api: None,
            diagnostics,
        };
    };
    if !document.kind.is_empty() {
        diagnostics.push(
            Diagnostic::error(format!(
                "{entry} is a RAML {} fragment; compare the API that uses it",
                document.kind
            ))
            .with_code(codes::NOT_RAML),
        );
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let root = &document.root;
    let mut api = Api::new(name, ApiFormat::Raml);
    api.version = root.get("version").map(|v| match &v.value {
        Value::String(s) => s.clone(),
        _ => faultline_ir::Literal::from(v).to_string(),
    });
    let (scope, templates) =
        libraries.unwrap_or_else(|| (Scope::of(root, ""), Templates::of(root)));
    let mut types = TypeLowerer::new(&mut api);
    types.json_includes = std::mem::take(&mut raml.json_includes);
    let mut lowering = Lowering {
        types,
        templates: Expander::new(&templates),
        media_types: media_types(root),
        security: root.get("securedBy").map(security),
        diagnostics: Diagnostics::new(),
    };
    for (key, resource) in root
        .as_object()
        .map(|o| o.iter().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        if key.starts_with('/') {
            lowering.resource(key, resource, &BTreeMap::new(), &scope);
        }
    }
    diagnostics.extend(lowering.diagnostics);
    diagnostics.extend(lowering.templates.diagnostics);
    diagnostics.extend(std::mem::take(&mut lowering.types.diagnostics));
    let pending = std::mem::take(&mut lowering.types.pending);
    diagnostics.extend(crate::jsonschema::lower_schemas(
        &mut api, &pending, entry, loader, sources, limits,
    ));
    diagnostics.sort();
    let api = (!diagnostics.has_errors()).then_some(api);
    Lowered { api, diagnostics }
}

/// The API's default media types.
fn media_types(root: &Node) -> Vec<String> {
    match root.get("mediaType").map(|n| &n.value) {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(|m| m.as_str().map(str::to_owned))
            .collect(),
        _ => vec![DEFAULT_MEDIA_TYPE.to_owned()],
    }
}

/// `securedBy`: scheme names, or `null` for no credentials, each possibly
/// with parameters (`oauth: {scopes: [read]}`).
fn security(node: &Node) -> Vec<SecurityRequirement> {
    let mut out = Vec::new();
    let items: Vec<&Node> = match &node.value {
        Value::Array(items) => items.iter().collect(),
        _ => vec![node],
    };
    for item in items {
        let mut requirement = SecurityRequirement::default();
        match &item.value {
            Value::Null => {}
            Value::String(name) => {
                requirement.schemes.insert(name.clone(), Vec::new());
            }
            Value::Object(object) => {
                for (name, parameters) in object.iter() {
                    let mut scopes: Vec<String> = parameters
                        .get("scopes")
                        .and_then(Node::as_array)
                        .map(|s| {
                            s.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    scopes.sort();
                    scopes.dedup();
                    requirement.schemes.insert(name.to_owned(), scopes);
                }
            }
            _ => continue,
        }
        if !out.contains(&requirement) {
            out.push(requirement);
        }
    }
    out.sort();
    out
}

struct Lowering<'a> {
    types: TypeLowerer<'a>,
    templates: Expander<'a>,
    media_types: Vec<String>,
    security: Option<Vec<SecurityRequirement>>,
    diagnostics: Diagnostics,
}

impl Lowering<'_> {
    fn resource(
        &mut self,
        path: &str,
        node: &Node,
        inherited: &BTreeMap<String, Parameter>,
        scope: &Scope,
    ) {
        let expanded = self.templates.resource(path, node);
        let node = &expanded;
        let mut uri_parameters = inherited.clone();
        if let Some(declared) = node.get("uriParameters").and_then(Node::as_object) {
            for (name, declaration) in declared.iter() {
                let mut parameter = self.parameter(ParamLocation::Path, name, declaration, scope);
                parameter.required = true;
                uri_parameters.insert(parameter.name.clone(), parameter);
            }
        }
        let template = PathTemplate::parse(path);
        for method in METHODS {
            let key = method.as_str().to_ascii_lowercase();
            if let Some(member) = node.as_object().and_then(|o| o.member(&key)) {
                let mut op = self.method(method, &template, &member.value, &uri_parameters, scope);
                // The method's key: its value may be empty, or only what
                // traits and resource types gave it.
                op.origin = Some(member.key_span);
                self.types.api.operations.push(op);
            }
        }
        for (key, child) in node
            .as_object()
            .map(|o| o.iter().collect::<Vec<_>>())
            .unwrap_or_default()
        {
            if key.starts_with('/') {
                self.resource(&format!("{path}{key}"), child, &uri_parameters, scope);
            }
        }
    }

    fn method(
        &mut self,
        method: HttpMethod,
        template: &PathTemplate,
        node: &Node,
        uri_parameters: &BTreeMap<String, Parameter>,
        scope: &Scope,
    ) -> Operation {
        let mut op = Operation::new(OperationKey::Http {
            method,
            path: template.clone(),
        });
        op.meta = meta(node);
        // Every variable of the template is a path parameter, declared or
        // not.
        for name in template.params() {
            let parameter = uri_parameters.get(name).cloned().unwrap_or_else(|| {
                let ty = self
                    .types
                    .api
                    .types
                    .alloc_kind(TypeKind::String(faultline_ir::StringType::default()));
                Parameter {
                    location: ParamLocation::Path,
                    name: name.clone(),
                    required: true,
                    ty,
                    serialization: None,
                    meta: Meta::default(),
                    origin: Some(node.span),
                }
            });
            op.parameters.push(parameter);
        }
        for (key, location) in [
            ("queryParameters", ParamLocation::Query),
            ("headers", ParamLocation::Header),
        ] {
            if let Some(declared) = node.get(key).and_then(Node::as_object) {
                for (name, declaration) in declared.iter() {
                    let parameter = self.parameter(location, name, declaration, scope);
                    op.parameters.push(parameter);
                }
            }
        }
        if let Some(query_string) = node.get("queryString") {
            self.query_string(query_string, scope, &mut op.parameters);
        }
        if let Some(body) = node.get("body") {
            op.request_body = Some(RequestBody {
                // A method that declares a body expects one.
                required: true,
                content: self.contents(body, scope),
                meta: Meta::default(),
                origin: Some(body.span),
            });
        }
        if let Some(responses) = node.get("responses").and_then(Node::as_object) {
            for (code, response) in responses.iter() {
                let Some(status) =
                    StatusKey::parse(code).filter(|s| matches!(s, StatusKey::Code(_)))
                else {
                    self.error(format!("{code:?} is not a status code"), response.span);
                    continue;
                };
                let headers = response
                    .get("headers")
                    .and_then(Node::as_object)
                    .map(|headers| {
                        headers
                            .iter()
                            .map(|(name, declaration)| {
                                let p =
                                    self.parameter(ParamLocation::Header, name, declaration, scope);
                                Header {
                                    name: p.name,
                                    required: p.required,
                                    ty: p.ty,
                                    meta: p.meta,
                                    origin: p.origin,
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let content = response
                    .get("body")
                    .map(|b| self.contents(b, scope))
                    .unwrap_or_default();
                op.responses.push(Response {
                    status,
                    headers,
                    content,
                    meta: meta(response),
                    origin: Some(response.span),
                });
            }
        }
        op.security = node
            .get("securedBy")
            .map(security)
            .or_else(|| self.security.clone());
        op
    }

    fn error(&mut self, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::INVALID)
                .with_primary(span, ""),
        );
    }

    /// A parameter: required unless its name ends in `?` or it says
    /// `required: false`.
    fn parameter(
        &mut self,
        location: ParamLocation,
        key: &str,
        declaration: &Node,
        scope: &Scope,
    ) -> Parameter {
        let (name, mut required) = match key.strip_suffix('?') {
            Some(name) => (name, false),
            None => (key, true),
        };
        if let Some(explicit) = declaration.get("required").and_then(Node::as_bool) {
            required = explicit;
        }
        // A parameter without a type is a string.
        let ty = if declaration.is_null() {
            self.types
                .api
                .types
                .alloc_kind(TypeKind::String(faultline_ir::StringType::default()))
        } else {
            self.types.declaration(declaration, scope)
        };
        Parameter {
            location,
            name: name.to_owned(),
            required,
            ty,
            serialization: None,
            meta: meta(declaration),
            origin: Some(declaration.span),
        }
    }

    /// A `queryString` type: its properties are the query parameters.
    fn query_string(&mut self, node: &Node, scope: &Scope, out: &mut Vec<Parameter>) {
        let ty = self.types.declaration(node, scope);
        let resolved = self.types.api.types.resolve(ty);
        let TypeKind::Object(object) = self.types.api.types.get(resolved).kind.clone() else {
            self.error("queryString must be an object type".into(), node.span);
            return;
        };
        for field in object.fields {
            out.push(Parameter {
                location: ParamLocation::Query,
                name: field.name,
                required: field.required,
                ty: field.ty,
                serialization: None,
                meta: field.meta,
                origin: field.origin,
            });
        }
    }

    /// The content of a body: by media type, or one type for each default
    /// media type.
    fn contents(&mut self, body: &Node, scope: &Scope) -> Vec<Content> {
        let keyed = body
            .as_object()
            .filter(|o| !o.is_empty() && o.keys().all(|k| k.contains('/')));
        if let Some(by_media_type) = keyed {
            return by_media_type
                .iter()
                .map(|(media_type, declaration)| {
                    let ty = self.body_type(declaration, scope);
                    Content {
                        media_type: media_type.to_ascii_lowercase(),
                        ty,
                        origin: Some(declaration.span),
                    }
                })
                .collect();
        }
        let ty = self.body_type(body, scope);
        self.media_types
            .clone()
            .into_iter()
            .map(|media_type| Content {
                media_type,
                ty,
                origin: Some(body.span),
            })
            .collect()
    }

    fn body_type(&mut self, declaration: &Node, scope: &Scope) -> TypeId {
        if declaration.is_null() {
            return self.types.api.types.alloc_kind(TypeKind::Any);
        }
        self.types.declaration(declaration, scope)
    }
}

fn meta(node: &Node) -> Meta {
    Meta {
        title: node
            .get("displayName")
            .and_then(Node::as_str)
            .map(str::to_owned),
        description: node
            .get("description")
            .and_then(Node::as_str)
            .map(str::to_owned),
        ..Meta::default()
    }
}
