//! Swagger 2.0 (OpenAPI 2.0) descriptions.
//!
//! Swagger 2.0 is lowered into the same IR as OpenAPI 3, the way the
//! official converters translate it, so that a description migrated from
//! 2.0 to 3.0 compares cleanly with its old self:
//!
//! * a `body` parameter becomes the request body, with one content entry
//!   per media type the operation `consumes` (`application/json` by
//!   default);
//! * `formData` parameters become an object request body whose members are
//!   the parameters, as `application/x-www-form-urlencoded`, or
//!   `multipart/form-data` when the operation consumes it or uploads a
//!   `file`;
//! * other parameters carry their schema keywords inline (`type`,
//!   `format`, `items`, `enum`, ...), which are lowered in the Swagger 2.0
//!   dialect; `collectionFormat` becomes the equivalent OpenAPI 3 `style`
//!   and `explode`;
//! * a response's `schema` becomes one content entry per media type the
//!   operation `produces`, and its headers carry their schema inline;
//! * `definitions` are the named types, as `components/schemas` are.
//!
//! `host`, `basePath` and `schemes` are not compared, as OpenAPI 3
//! `servers` are not.

use faultline_ir::{
    Content, Field, Header, HttpMethod, ObjectType, Operation, OperationKey, ParamLocation,
    Parameter, PathTemplate, RequestBody, Response, SecurityRequirement, Serialization, StatusKey,
    TypeId, TypeKind, TypeNode,
};

use crate::codes;
use crate::context::{At, Context};
use crate::operations::normalize_media_type;

const FORM_URLENCODED: &str = "application/x-www-form-urlencoded";
const MULTIPART: &str = "multipart/form-data";
const DEFAULT_MEDIA_TYPE: &str = "application/json";

/// Header parameters Swagger defines through other fields.
const IGNORED_HEADERS: [&str; 3] = ["accept", "content-type", "authorization"];

/// The methods a Swagger 2.0 path item may have (no `trace`).
const METHODS: [HttpMethod; 7] = [
    HttpMethod::Get,
    HttpMethod::Put,
    HttpMethod::Post,
    HttpMethod::Delete,
    HttpMethod::Options,
    HttpMethod::Head,
    HttpMethod::Patch,
];

/// A parameter before it is placed: body and form parameters become the
/// request body rather than parameters.
enum Declared {
    Parameter(Parameter),
    Body {
        schema: At,
        required: bool,
        at: At,
    },
    Form {
        name: String,
        required: bool,
        ty: TypeId,
        file: bool,
        at: At,
    },
}

impl Declared {
    /// The key operation-level parameters override path-level ones by.
    fn key(&self) -> (&'static str, String) {
        match self {
            Declared::Parameter(p) => (p.location.as_str(), p.identity_name()),
            Declared::Body { .. } => ("body", String::new()),
            Declared::Form { name, .. } => ("formData", name.clone()),
        }
    }
}

/// The media types in force: the document's, overridden per operation.
#[derive(Clone)]
struct MediaTypes {
    consumes: Vec<String>,
    produces: Vec<String>,
}

pub(crate) fn lower_description(context: &mut Context<'_, '_, '_>, root: &At) {
    let media = MediaTypes {
        consumes: media_types(context, root, "consumes").unwrap_or_default(),
        produces: media_types(context, root, "produces").unwrap_or_default(),
    };
    let security = root
        .child("security")
        .map(|s| context.security(&s))
        .unwrap_or_default();

    let mut seen = std::collections::HashSet::new();
    if let Some(paths) = root.child("paths") {
        for (template, item) in paths.entries() {
            if template.starts_with("x-") {
                continue;
            }
            let key_span = item.key_span();
            if !template.starts_with('/') {
                context.error(
                    codes::INVALID_PATH,
                    "path templates must start with '/'",
                    key_span,
                );
            }
            for op in path_item(context, &template, item, &media, &security) {
                if seen.insert(op.key.identity()) {
                    context.lowerer.api_mut().operations.push(op);
                } else {
                    context.error(
                        codes::DUPLICATE_OPERATION,
                        &format!("operation {} is declared twice", op.key.identity()),
                        key_span,
                    );
                }
            }
        }
    }
    if let Some(definitions) = root.child("definitions") {
        for (_, schema) in definitions.entries() {
            context.schema(&schema);
        }
    }
}

/// The media types listed under `key`, if the key is present.
fn media_types(context: &mut Context<'_, '_, '_>, at: &At, key: &str) -> Option<Vec<String>> {
    let node = at.get(key)?;
    let Some(items) = node.as_array() else {
        let span = node.span;
        context.error(
            codes::INVALID_FIELD,
            &format!("\"{key}\" must be an array of media types"),
            span,
        );
        return Some(Vec::new());
    };
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if let Some(text) = item.as_str() {
            let normalized = normalize_media_type(text);
            if !out.contains(&normalized) {
                out.push(normalized);
            }
        }
    }
    Some(out)
}

fn path_item(
    context: &mut Context<'_, '_, '_>,
    path: &str,
    item: At,
    media: &MediaTypes,
    security: &[SecurityRequirement],
) -> Vec<Operation> {
    let Some(item) = context.resolve(item) else {
        return Vec::new();
    };
    if !context.expect_object(&item, "a path item") {
        return Vec::new();
    }
    let shared = item
        .child("parameters")
        .map(|list| parameter_list(context, &list))
        .unwrap_or_default();
    let template = PathTemplate::parse(path);
    let mut operations = Vec::new();
    for method in METHODS {
        let field = method.as_str().to_ascii_lowercase();
        if let Some(at) = item.child(&field) {
            operations.extend(operation(
                context, method, &template, &at, &shared, media, security,
            ));
        }
    }
    operations
}

fn parameter_list(context: &mut Context<'_, '_, '_>, list: &At) -> Vec<Declared> {
    if list.node().as_array().is_none() {
        let span = list.span();
        context.error(codes::INVALID_FIELD, "parameters must be an array", span);
        return Vec::new();
    }
    list.elements()
        .into_iter()
        .filter_map(|element| parameter(context, element))
        .collect()
}

fn parameter(context: &mut Context<'_, '_, '_>, at: At) -> Option<Declared> {
    let at = context.resolve(at)?;
    if !context.expect_object(&at, "a parameter") {
        return None;
    }
    let name = context.string(&at, "name")?;
    let location_node = at.get("in")?;
    let location = location_node.as_str().unwrap_or_default().to_owned();
    let mut required = context.boolean(&at, "required").unwrap_or(false);
    match location.as_str() {
        "body" => {
            let Some(schema) = at.child("schema") else {
                let span = at.span();
                context.error(
                    codes::MISSING_SCHEMA,
                    &format!("body parameter \"{name}\" has no schema"),
                    span,
                );
                return None;
            };
            return Some(Declared::Body {
                schema,
                required,
                at,
            });
        }
        "formData" => {
            let file = at.get("type").and_then(|t| t.as_str()) == Some("file");
            let ty = context.schema(&at);
            return Some(Declared::Form {
                name,
                required,
                ty,
                file,
                at,
            });
        }
        _ => {}
    }
    let Some(location) = ParamLocation::parse(&location).filter(|l| *l != ParamLocation::Cookie)
    else {
        let span = location_node.span;
        context.error(
            codes::INVALID_LOCATION,
            "\"in\" must be path, query, header, body or formData",
            span,
        );
        return None;
    };
    if location == ParamLocation::Header
        && IGNORED_HEADERS.contains(&name.to_ascii_lowercase().as_str())
    {
        return None;
    }
    if location == ParamLocation::Path && !required {
        context.error(
            codes::PATH_PARAMETER,
            &format!("path parameter \"{name}\" must be required"),
            at.span(),
        );
        required = true;
    }
    // The parameter object is its own schema.
    let ty = context.schema(&at);
    let serialization = serialization(context, &at, location);
    Some(Declared::Parameter(Parameter {
        location,
        name,
        required,
        ty,
        serialization: Some(serialization),
        meta: context.meta(&at),
        origin: Some(at.span()),
    }))
}

/// The OpenAPI 3 serialization equivalent to a Swagger parameter. Only
/// arrays are affected by `collectionFormat`; other values get OpenAPI 3's
/// defaults, which serialize them identically.
fn serialization(
    context: &mut Context<'_, '_, '_>,
    at: &At,
    location: ParamLocation,
) -> Serialization {
    let array = at.get("type").and_then(|t| t.as_str()) == Some("array");
    let simple = matches!(location, ParamLocation::Path | ParamLocation::Header);
    let (style, explode) = if array {
        let format = context
            .string(at, "collectionFormat")
            .unwrap_or_else(|| "csv".to_owned());
        match (format.as_str(), simple) {
            ("multi", false) => ("form", true),
            ("ssv", _) => ("spaceDelimited", false),
            ("pipes", _) => ("pipeDelimited", false),
            ("tsv", _) => ("tabDelimited", false),
            (_, true) => ("simple", false),
            (_, false) => ("form", false),
        }
    } else if simple {
        ("simple", false)
    } else {
        ("form", true)
    };
    Serialization {
        style: style.to_owned(),
        explode,
        allow_reserved: false,
        allow_empty_value: context.boolean(at, "allowEmptyValue").unwrap_or(false),
    }
}

#[allow(clippy::too_many_arguments)]
fn operation(
    context: &mut Context<'_, '_, '_>,
    method: HttpMethod,
    template: &PathTemplate,
    at: &At,
    shared: &[Declared],
    media: &MediaTypes,
    security: &[SecurityRequirement],
) -> Option<Operation> {
    if !context.expect_object(at, "an operation") {
        return None;
    }
    let mut op = Operation::new(OperationKey::Http {
        method,
        path: template.clone(),
    });
    op.origin = Some(at.span());
    op.operation_id = context.string(at, "operationId");
    op.meta = context.meta(at);
    let media = MediaTypes {
        consumes: media_types(context, at, "consumes").unwrap_or_else(|| media.consumes.clone()),
        produces: media_types(context, at, "produces").unwrap_or_else(|| media.produces.clone()),
    };

    let own = at
        .child("parameters")
        .map(|list| parameter_list(context, &list))
        .unwrap_or_default();
    let mut declared: Vec<&Declared> = shared
        .iter()
        .filter(|p| !own.iter().any(|o| o.key() == p.key()))
        .collect();
    declared.extend(own.iter());

    let mut body = None;
    let mut form: Vec<&Declared> = Vec::new();
    for parameter in declared {
        match parameter {
            Declared::Parameter(p) => op.parameters.push(p.clone()),
            Declared::Body { .. } => body = Some(parameter),
            Declared::Form { .. } => form.push(parameter),
        }
    }
    context.check_path_parameters(template, &op.parameters, at);
    op.request_body = match (body, form.is_empty()) {
        (
            Some(Declared::Body {
                schema,
                required,
                at: body_at,
            }),
            true,
        ) => {
            let ty = context.schema(schema);
            Some(RequestBody {
                required: *required,
                content: contents(&media.consumes, ty, body_at),
                meta: context.meta(body_at),
                origin: Some(body_at.span()),
            })
        }
        (Some(Declared::Body { at: body_at, .. }), false) => {
            let span = body_at.span();
            context.error(
                codes::INVALID_FIELD,
                "an operation cannot have both a body and formData parameters",
                span,
            );
            None
        }
        (_, false) => Some(form_body(context, &form, &media.consumes, at)),
        _ => None,
    };
    if let Some(responses) = at.child("responses") {
        op.responses = responses_of(context, &responses, &media.produces);
    }
    op.security = Some(match at.child("security") {
        Some(own) => context.security(&own),
        None => security.to_vec(),
    });
    Some(op)
}

/// One content entry per media type, each with the payload type.
fn contents(media_types: &[String], ty: TypeId, at: &At) -> Vec<Content> {
    let defaults = [DEFAULT_MEDIA_TYPE.to_owned()];
    let media_types = if media_types.is_empty() {
        &defaults[..]
    } else {
        media_types
    };
    media_types
        .iter()
        .map(|media_type| Content {
            media_type: media_type.clone(),
            ty,
            origin: Some(at.span()),
        })
        .collect()
}

/// `formData` parameters as an object body.
fn form_body(
    context: &mut Context<'_, '_, '_>,
    form: &[&Declared],
    consumes: &[String],
    at: &At,
) -> RequestBody {
    let types = &mut context.lowerer.api_mut().types;
    let any = types.alloc_kind(TypeKind::Any);
    let mut object = ObjectType::new(any);
    let mut uploads = false;
    let mut required = false;
    for parameter in form {
        let Declared::Form {
            name,
            required: field_required,
            ty,
            file,
            at,
        } = parameter
        else {
            continue;
        };
        uploads |= *file;
        required |= *field_required;
        let mut field = Field::new(name.clone(), *ty, *field_required);
        field.origin = Some(at.span());
        object.fields.push(field);
    }
    let ty = types.alloc(TypeNode::new(TypeKind::Object(object)).with_origin(at.span()));
    let mut media_types: Vec<String> = consumes
        .iter()
        .filter(|m| m.as_str() == FORM_URLENCODED || m.as_str() == MULTIPART)
        .cloned()
        .collect();
    if media_types.is_empty() {
        media_types.push(if uploads { MULTIPART } else { FORM_URLENCODED }.to_owned());
    }
    RequestBody {
        required,
        content: contents(&media_types, ty, at),
        meta: faultline_ir::Meta::default(),
        origin: Some(at.span()),
    }
}

fn responses_of(context: &mut Context<'_, '_, '_>, at: &At, produces: &[String]) -> Vec<Response> {
    if !context.expect_object(at, "responses") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (key, response) in at.entries() {
        if key.starts_with("x-") {
            continue;
        }
        let Some(status) = StatusKey::parse(&key).filter(|s| !matches!(s, StatusKey::Range(_)))
        else {
            let span = response.key_span();
            context.error(
                codes::INVALID_STATUS,
                &format!("\"{key}\" is not a status code or default"),
                span,
            );
            continue;
        };
        let Some(response) = context.resolve(response) else {
            continue;
        };
        if !context.expect_object(&response, "a response") {
            continue;
        }
        let content = match response.child("schema") {
            Some(schema) => {
                let ty = context.schema(&schema);
                contents(produces, ty, &schema)
            }
            None => Vec::new(),
        };
        let headers = response
            .child("headers")
            .map(|h| headers(context, &h))
            .unwrap_or_default();
        out.push(Response {
            status,
            headers,
            content,
            meta: context.meta(&response),
            origin: Some(response.span()),
        });
    }
    out
}

/// Response headers, whose schema keywords are inline. Swagger 2.0 headers
/// are not marked required.
fn headers(context: &mut Context<'_, '_, '_>, at: &At) -> Vec<Header> {
    let mut out = Vec::new();
    for (name, header) in at.entries() {
        if name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        if !context.expect_object(&header, "a header") {
            continue;
        }
        let ty = context.schema(&header);
        out.push(Header {
            name,
            required: false,
            ty,
            meta: context.meta(&header),
            origin: Some(header.span()),
        });
    }
    out
}
