//! Lowering of Path Items and Operations.

use std::collections::BTreeMap;

use faultline_doc::Node;
use faultline_ir::{
    Callback, Content, Header, HttpMethod, Meta, Operation, OperationKey, ParamLocation, Parameter,
    PathTemplate, RequestBody, Response, SecurityRequirement, Serialization, StatusKey,
};

use crate::codes;
use crate::context::{At, Context};

/// Header parameters OpenAPI says are ignored: they are controlled by other
/// fields (`content`, security schemes).
const IGNORED_HEADERS: [&str; 3] = ["accept", "content-type", "authorization"];

impl Context<'_, '_, '_> {
    /// The operations of the Path Item at `item` for URL template `path`.
    pub(crate) fn path_item(
        &mut self,
        path: &str,
        item: At,
        security: &[SecurityRequirement],
    ) -> Vec<Operation> {
        let Some(item) = self.resolve(item) else {
            return Vec::new();
        };
        if !self.expect_object(&item, "a path item") {
            return Vec::new();
        }
        let shared = item
            .child("parameters")
            .map(|list| self.parameter_list(&list))
            .unwrap_or_default();
        let template = PathTemplate::parse(path);
        let mut operations = Vec::new();
        for method in HttpMethod::ALL {
            let key = method.as_str().to_ascii_lowercase();
            if let Some(at) = item.child(&key) {
                operations.extend(self.operation(method, &template, &at, &shared, security));
            }
        }
        operations
    }

    fn operation(
        &mut self,
        method: HttpMethod,
        template: &PathTemplate,
        at: &At,
        shared: &[Parameter],
        security: &[SecurityRequirement],
    ) -> Option<Operation> {
        if !self.expect_object(at, "an operation") {
            return None;
        }
        let mut op = Operation::new(OperationKey::Http {
            method,
            path: template.clone(),
        });
        op.origin = Some(at.span());
        op.operation_id = self.string(at, "operationId");
        op.meta = self.meta(at);

        // Operation-level parameters override path-level ones with the same
        // location and name.
        let own = at
            .child("parameters")
            .map(|list| self.parameter_list(&list))
            .unwrap_or_default();
        let mut parameters: Vec<Parameter> = shared
            .iter()
            .filter(|p| {
                !own.iter()
                    .any(|o| o.location == p.location && o.identity_name() == p.identity_name())
            })
            .cloned()
            .collect();
        parameters.extend(own);
        self.check_path_parameters(template, &parameters, at);
        op.parameters = parameters;

        if let Some(body) = at.child("requestBody") {
            op.request_body = self.request_body(body);
        }
        if let Some(responses) = at.child("responses") {
            op.responses = self.responses(&responses);
        }
        if let Some(callbacks) = at.child("callbacks") {
            op.callbacks = self.callbacks(&callbacks, security);
        }
        op.security = Some(match at.child("security") {
            Some(own) => self.security(&own),
            None => security.to_vec(),
        });
        Some(op)
    }

    /// Every `{name}` in the template needs a required path parameter, and
    /// every path parameter needs a `{name}` in the template.
    fn check_path_parameters(
        &mut self,
        template: &PathTemplate,
        parameters: &[Parameter],
        at: &At,
    ) {
        for name in template.params() {
            if !parameters
                .iter()
                .any(|p| p.location == ParamLocation::Path && &p.name == name)
            {
                self.error(
                    codes::PATH_PARAMETER,
                    &format!("path parameter \"{name}\" is not declared"),
                    at.span(),
                );
            }
        }
        for p in parameters
            .iter()
            .filter(|p| p.location == ParamLocation::Path)
        {
            if !template.params().contains(&p.name) {
                let span = p.origin.unwrap_or_else(|| at.span());
                self.error(
                    codes::PATH_PARAMETER,
                    &format!("path parameter \"{}\" does not appear in the path", p.name),
                    span,
                );
            }
        }
    }

    fn parameter_list(&mut self, list: &At) -> Vec<Parameter> {
        if list.node().as_array().is_none() {
            let span = list.span();
            self.error(codes::INVALID_FIELD, "parameters must be an array", span);
            return Vec::new();
        }
        list.elements()
            .into_iter()
            .filter_map(|element| self.parameter(element))
            .collect()
    }

    fn parameter(&mut self, at: At) -> Option<Parameter> {
        let at = self.resolve(at)?;
        if !self.expect_object(&at, "a parameter") {
            return None;
        }
        let name = self.string(&at, "name")?;
        let location_node = at.get("in")?;
        let Some(location) = location_node.as_str().and_then(ParamLocation::parse) else {
            let span = location_node.span;
            self.error(
                codes::INVALID_LOCATION,
                "\"in\" must be path, query, header or cookie",
                span,
            );
            return None;
        };
        if location == ParamLocation::Header
            && IGNORED_HEADERS.contains(&name.to_ascii_lowercase().as_str())
        {
            return None;
        }
        let mut required = self.boolean(&at, "required").unwrap_or(false);
        if location == ParamLocation::Path && !required {
            self.error(
                codes::PATH_PARAMETER,
                &format!("path parameter \"{name}\" must be required"),
                at.span(),
            );
            required = true;
        }
        let (ty, serialization) = if let Some(schema) = at.child("schema") {
            (
                self.schema(&schema),
                Some(self.serialization(&at, location)),
            )
        } else if let Some(content) = at.child("content") {
            let ty = self.single_content_schema(&content);
            (ty, None)
        } else {
            let span = at.span();
            self.error(
                codes::MISSING_SCHEMA,
                &format!("parameter \"{name}\" has no schema or content"),
                span,
            );
            (self.any(), None)
        };
        Some(Parameter {
            location,
            name,
            required,
            ty,
            serialization,
            meta: self.meta(&at),
            origin: Some(at.span()),
        })
    }

    /// Serialization with OpenAPI's defaults applied: `form` for query and
    /// cookie parameters, `simple` for path and header ones; `explode` defaults
    /// to true exactly for `form`.
    fn serialization(&mut self, at: &At, location: ParamLocation) -> Serialization {
        let style = self.string(at, "style").unwrap_or_else(|| {
            match location {
                ParamLocation::Query | ParamLocation::Cookie => "form",
                ParamLocation::Path | ParamLocation::Header => "simple",
            }
            .to_owned()
        });
        let explode = self.boolean(at, "explode").unwrap_or(style == "form");
        Serialization {
            style,
            explode,
            allow_reserved: self.boolean(at, "allowReserved").unwrap_or(false),
            allow_empty_value: self.boolean(at, "allowEmptyValue").unwrap_or(false),
        }
    }

    /// The schema of a `content` map that must have exactly one entry
    /// (parameters and headers).
    fn single_content_schema(&mut self, content: &At) -> faultline_ir::TypeId {
        let entries = content.entries();
        if let [(_, media)] = entries.as_slice() {
            self.schema_or_any(media)
        } else {
            let span = content.span();
            self.error(
                codes::INVALID_FIELD,
                "content must have exactly one media type here",
                span,
            );
            self.any()
        }
    }

    fn request_body(&mut self, at: At) -> Option<RequestBody> {
        let at = self.resolve(at)?;
        if !self.expect_object(&at, "a request body") {
            return None;
        }
        Some(RequestBody {
            required: self.boolean(&at, "required").unwrap_or(false),
            content: at
                .child("content")
                .map(|c| self.content(&c))
                .unwrap_or_default(),
            meta: self.meta(&at),
            origin: Some(at.span()),
        })
    }

    fn content(&mut self, at: &At) -> Vec<Content> {
        if !self.expect_object(at, "content") {
            return Vec::new();
        }
        at.entries()
            .into_iter()
            .map(|(media_type, media)| {
                let ty = self.schema_or_any(&media);
                Content {
                    media_type: normalize_media_type(&media_type),
                    ty,
                    origin: Some(media.span()),
                }
            })
            .collect()
    }

    fn responses(&mut self, at: &At) -> Vec<Response> {
        if !self.expect_object(at, "responses") {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (key, response) in at.entries() {
            if key.starts_with("x-") {
                continue;
            }
            let Some(status) = StatusKey::parse(&key) else {
                let span = at
                    .object()
                    .and_then(|o| o.member(&key))
                    .map_or_else(|| response.span(), |m| m.key_span);
                self.error(
                    codes::INVALID_STATUS,
                    &format!("\"{key}\" is not a status code, range or default"),
                    span,
                );
                continue;
            };
            let Some(response) = self.resolve(response) else {
                continue;
            };
            if !self.expect_object(&response, "a response") {
                continue;
            }
            let headers = response
                .child("headers")
                .map(|h| self.headers(&h))
                .unwrap_or_default();
            let content = response
                .child("content")
                .map(|c| self.content(&c))
                .unwrap_or_default();
            out.push(Response {
                status,
                headers,
                content,
                meta: self.meta(&response),
                origin: Some(response.span()),
            });
        }
        out
    }

    fn headers(&mut self, at: &At) -> Vec<Header> {
        let mut out = Vec::new();
        for (name, header) in at.entries() {
            // "Content-Type" response headers are ignored (OpenAPI 3.x).
            if name.eq_ignore_ascii_case("content-type") {
                continue;
            }
            let Some(header) = self.resolve(header) else {
                continue;
            };
            if !self.expect_object(&header, "a header") {
                continue;
            }
            let ty = if let Some(schema) = header.child("schema") {
                self.schema(&schema)
            } else if let Some(content) = header.child("content") {
                self.single_content_schema(&content)
            } else {
                let span = header.span();
                self.error(
                    codes::MISSING_SCHEMA,
                    &format!("header \"{name}\" has no schema or content"),
                    span,
                );
                self.any()
            };
            out.push(Header {
                name,
                required: self.boolean(&header, "required").unwrap_or(false),
                ty,
                meta: self.meta(&header),
                origin: Some(header.span()),
            });
        }
        out
    }

    fn callbacks(&mut self, at: &At, security: &[SecurityRequirement]) -> Vec<Callback> {
        let mut out = Vec::new();
        for (name, callback) in at.entries() {
            let Some(callback) = self.resolve(callback) else {
                continue;
            };
            let mut operations = Vec::new();
            for (expression, item) in callback.entries() {
                if expression.starts_with("x-") {
                    continue;
                }
                operations.extend(self.path_item(&expression, item, security));
            }
            out.push(Callback {
                name,
                operations,
                origin: Some(callback.span()),
            });
        }
        out
    }

    /// A list of Security Requirement Objects, normalized: scopes sorted,
    /// duplicates removed.
    pub(crate) fn security(&mut self, at: &At) -> Vec<SecurityRequirement> {
        let Some(list) = at.node().as_array() else {
            let span = at.span();
            self.error(codes::INVALID_FIELD, "security must be an array", span);
            return Vec::new();
        };
        let mut out: Vec<SecurityRequirement> = Vec::new();
        for element in list {
            let Some(obj) = element.as_object() else {
                self.error(
                    codes::INVALID_FIELD,
                    "a security requirement must be an object",
                    element.span,
                );
                continue;
            };
            let mut schemes = BTreeMap::new();
            for (name, scopes) in obj.iter() {
                let mut scopes: Vec<String> = scopes
                    .as_array()
                    .map(|s| {
                        s.iter()
                            .filter_map(Node::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                scopes.sort();
                scopes.dedup();
                schemes.insert(name.to_owned(), scopes);
            }
            let requirement = SecurityRequirement { schemes };
            if !out.contains(&requirement) {
                out.push(requirement);
            }
        }
        out.sort();
        out
    }

    fn meta(&mut self, at: &At) -> Meta {
        let mut meta = Meta {
            title: self.string(at, "summary"),
            description: self.string(at, "description"),
            deprecated: self.boolean(at, "deprecated").unwrap_or(false),
            ..Meta::default()
        };
        if let Some(obj) = at.object() {
            for (key, value) in obj.iter() {
                if key.starts_with("x-") {
                    meta.extensions
                        .insert(key.to_owned(), faultline_ir::Literal::from(value));
                }
            }
        }
        meta
    }

    /// The type of the `schema` member of a Media Type Object; a media type
    /// without a schema accepts any payload.
    fn schema_or_any(&mut self, media: &At) -> faultline_ir::TypeId {
        match media.child("schema") {
            Some(schema) => self.schema(&schema),
            None => self.any(),
        }
    }

    fn any(&mut self) -> faultline_ir::TypeId {
        self.lowerer
            .api_mut()
            .types
            .alloc_kind(faultline_ir::TypeKind::Any)
    }
}

/// Media type keys compared case-insensitively, without surrounding space.
fn normalize_media_type(media_type: &str) -> String {
    media_type
        .split(';')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("; ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::normalize_media_type;

    #[test]
    fn media_types_normalize() {
        assert_eq!(
            normalize_media_type(" Application/JSON "),
            "application/json"
        );
        assert_eq!(
            normalize_media_type("text/plain;charset=UTF-8"),
            "text/plain; charset=utf-8"
        );
    }
}
