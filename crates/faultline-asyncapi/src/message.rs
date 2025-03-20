//! Messages: the object a message becomes, and payloads in each schema
//! format.

use faultline_doc::Value;
use faultline_ir::{
    Api, Content, EnumType, EnumValue, Field, Literal, ObjectType, Retired, TypeId, TypeKind,
    TypeNode,
};
use faultline_jsonschema::Dialect;
use faultline_jsonschema::nav::At;

use crate::codes;
use crate::context::{Context, referenced_name};

/// The formats a payload schema can be written in.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SchemaFormat {
    Json(Dialect),
    Avro,
    Protobuf,
    Unsupported(String),
}

/// Reads a `schemaFormat` value (a media type with an optional `version`
/// parameter). `None` is the AsyncAPI Schema Object.
fn schema_format(format: Option<&str>) -> SchemaFormat {
    let Some(format) = format else {
        return SchemaFormat::Json(Dialect::Draft7);
    };
    let lower = format.to_ascii_lowercase();
    let (media_type, parameters) = lower.split_once(';').unwrap_or((lower.as_str(), ""));
    let version = parameters
        .split(';')
        .find_map(|p| p.trim().strip_prefix("version="))
        .unwrap_or_default();
    match media_type.trim() {
        "application/vnd.aai.asyncapi"
        | "application/vnd.aai.asyncapi+json"
        | "application/vnd.aai.asyncapi+yaml" => SchemaFormat::Json(Dialect::Draft7),
        "application/schema+json" | "application/schema+yaml" => {
            SchemaFormat::Json(match version {
                "draft-04" => Dialect::Draft4,
                "draft-06" => Dialect::Draft6,
                "2019-09" | "draft-2019-09" => Dialect::Draft2019_09,
                "2020-12" | "draft-2020-12" => Dialect::Draft2020_12,
                _ => Dialect::Draft7,
            })
        }
        "application/vnd.oai.openapi"
        | "application/vnd.oai.openapi+json"
        | "application/vnd.oai.openapi+yaml" => SchemaFormat::Json(if version.starts_with("3.1") {
            Dialect::Draft2020_12
        } else {
            Dialect::OpenApi30
        }),
        "application/vnd.apache.avro"
        | "application/vnd.apache.avro+json"
        | "application/vnd.apache.avro+yaml" => SchemaFormat::Avro,
        "application/vnd.google.protobuf" => SchemaFormat::Protobuf,
        _ => SchemaFormat::Unsupported(format.to_owned()),
    }
}

/// The content an operation carries for the message at `entry` (a message
/// object or a reference to one). `fallback` names messages that have no
/// name of their own and are not referenced by one.
pub(crate) fn content(cx: &mut Context<'_, '_, '_>, entry: At, fallback: &str) -> Option<Content> {
    let referenced = referenced_name(&entry);
    let message = cx.resolve(entry)?;
    if !cx.expect_object(&message, "a message") {
        return None;
    }
    let traits = cx.trait_list(&message);
    let name = cx
        .traited_string(&message, &traits, "name")
        .or_else(|| {
            (!cx.v3)
                .then(|| cx.traited_string(&message, &traits, "messageId"))
                .flatten()
        })
        .or(referenced)
        .unwrap_or_else(|| fallback.to_owned());
    let content_type = cx
        .traited_string(&message, &traits, "contentType")
        .unwrap_or_else(|| cx.default_content_type.clone());
    // In 2.x the message says which format its payload is in; in 3.0 a
    // payload may carry its own (a Multi Format Schema Object).
    let format = if cx.v3 {
        None
    } else {
        cx.traited_string(&message, &traits, "schemaFormat")
    };
    let payload = match message.child("payload") {
        Some(at) => {
            let span = at.span();
            Some((payload_type(cx, at, format), span))
        }
        None => None,
    };
    let headers = headers(cx, &message, &traits);
    let key = kafka_key(cx, &message, &traits);
    let ty = envelope(cx, &content_type, headers, key, payload, &message);
    Some(Content {
        media_type: name,
        ty,
        origin: Some(message.span()),
    })
}

/// A message's headers, with those its traits declare. Traits are merged
/// into messages as JSON Merge Patches, which combine the members of header
/// schemas; the combination is compared as their `allOf`, which accepts
/// the same objects when the schemas constrain different members.
fn headers(
    cx: &mut Context<'_, '_, '_>,
    message: &At,
    traits: &[At],
) -> Option<(TypeId, faultline_source::Span)> {
    let mut sources: Vec<At> = message.child("headers").into_iter().collect();
    sources.extend(traits.iter().filter_map(|t| t.child("headers")));
    let span = sources.first()?.span();
    // 2.x headers are always AsyncAPI schemas.
    let mut types: Vec<TypeId> = sources
        .into_iter()
        .map(|at| payload_type(cx, at, None))
        .collect();
    let ty = if types.len() == 1 {
        types.remove(0)
    } else {
        cx.lowerer
            .api_mut()
            .types
            .alloc(TypeNode::new(TypeKind::AllOf(types)).with_origin(span))
    };
    Some((ty, span))
}

/// The schema of a message's Kafka key (`bindings.kafka.key`), from the
/// message or its traits: records are partitioned by their key, and
/// consumers often read it.
fn kafka_key(
    cx: &mut Context<'_, '_, '_>,
    message: &At,
    traits: &[At],
) -> Option<(TypeId, faultline_source::Span)> {
    let key = std::iter::once(message)
        .chain(traits)
        .find_map(|at| at.child("bindings")?.child("kafka")?.child("key"))?;
    let span = key.span();
    Some((payload_type(cx, key, None), span))
}

/// The object a message is compared as: its content type, headers, Kafka
/// key and payload.
fn envelope(
    cx: &mut Context<'_, '_, '_>,
    content_type: &str,
    headers: Option<(TypeId, faultline_source::Span)>,
    key: Option<(TypeId, faultline_source::Span)>,
    payload: Option<(TypeId, faultline_source::Span)>,
    message: &At,
) -> TypeId {
    let types = &mut cx.lowerer.api_mut().types;
    let any = types.alloc_kind(TypeKind::Any);
    let content_type = types.alloc_kind(TypeKind::Enum(EnumType {
        values: vec![EnumValue::new(Literal::string(content_type))],
        open: false,
        retired: Retired::default(),
        writer_exact: false,
    }));
    let mut object = ObjectType::new(any);
    let mut content_type_field = Field::new("contentType", content_type, true);
    content_type_field.origin = message.get("contentType").map(|n| n.span);
    object.fields.push(content_type_field);
    if let Some((ty, span)) = headers {
        let mut field = Field::new("headers", ty, true);
        field.origin = Some(span);
        object.fields.push(field);
    }
    if let Some((ty, span)) = key {
        let mut field = Field::new("key", ty, true);
        field.origin = Some(span);
        object.fields.push(field);
    }
    // A message without a payload may carry anything.
    let (payload, span) = match payload {
        Some((ty, span)) => (ty, Some(span)),
        None => (any, None),
    };
    let mut field = Field::new("payload", payload, true);
    field.origin = span;
    object.fields.push(field);
    types.alloc(TypeNode::new(TypeKind::Object(object)).with_origin(message.span()))
}

/// The type of a payload or headers schema at `at`, written in `format`
/// (or, in 3.0, in the format a Multi Format Schema Object names).
fn payload_type(cx: &mut Context<'_, '_, '_>, at: At, format: Option<String>) -> TypeId {
    let (at, format) = match (cx.v3, at.get("schemaFormat"), at.child("schema")) {
        (true, Some(_), Some(schema)) => (schema, cx.string(&at, "schemaFormat")),
        _ => (at, format),
    };
    match schema_format(format.as_deref()) {
        SchemaFormat::Json(dialect) => cx.schema(&at, dialect),
        SchemaFormat::Avro => avro(cx, at),
        SchemaFormat::Protobuf => protobuf(cx, at),
        SchemaFormat::Unsupported(format) => {
            let span = at.span();
            cx.warning(
                codes::UNSUPPORTED_SCHEMA_FORMAT,
                &format!(
                    "schema format {format} is not supported; the schema is compared as \
                     accepting any value"
                ),
                span,
            );
            cx.any()
        }
    }
}

/// Lowers an Avro schema with the Avro front end and grafts its types into
/// the API's type graph.
fn avro(cx: &mut Context<'_, '_, '_>, at: At) -> TypeId {
    // The schema may live in its own `.avsc` file.
    let Some(at) = cx.resolve(at) else {
        return cx.any();
    };
    let embedded = faultline_avro::lower_embedded(at.node());
    let failed = embedded.diagnostics.has_errors();
    cx.diagnostics.extend(embedded.diagnostics);
    let Some((lowered, id)) = embedded.lowered.filter(|_| !failed) else {
        let span = at.span();
        cx.error(
            codes::INVALID_AVRO,
            "the Avro schema is invalid, so the message cannot be compared",
            span,
        );
        return cx.any();
    };
    graft(cx, lowered, id)
}

/// Lowers a Protobuf schema, written as a string, with the Protobuf front
/// end: the payload is the first message it declares.
fn protobuf(cx: &mut Context<'_, '_, '_>, at: At) -> TypeId {
    let Some(at) = cx.resolve(at) else {
        return cx.any();
    };
    let span = at.span();
    let Value::String(text) = &at.node().value else {
        cx.error(
            codes::INVALID_PROTOBUF,
            "a Protobuf schema must be written as a string of `.proto` source",
            span,
        );
        return cx.any();
    };
    let location = at.location();
    let uri = location
        .document
        .with_fragment(Some(&location.pointer.to_string()));
    let store = cx.lowerer.store();
    let embedded =
        faultline_protobuf::lower_embedded(text, &uri, span, store.loader(), store.sources());
    let failed = embedded.diagnostics.has_errors();
    cx.diagnostics.extend(embedded.diagnostics);
    let Some((lowered, id)) = embedded.lowered.filter(|_| !failed) else {
        cx.error(
            codes::INVALID_PROTOBUF,
            "the Protobuf schema is invalid, so the message cannot be compared",
            span,
        );
        return cx.any();
    };
    graft(cx, lowered, id)
}

/// Copies the types of an API lowered by another front end into this one,
/// returning the id `id` has there.
fn graft(cx: &mut Context<'_, '_, '_>, lowered: Api, id: TypeId) -> TypeId {
    let Api { types, named, .. } = lowered;
    let api = cx.lowerer.api_mut();
    let graft = api.types.graft(types);
    for (name, ty) in named {
        api.named.entry(name).or_insert(graft.map(ty));
    }
    graft.map(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_formats() {
        let f = |s: &str| schema_format(Some(s));
        assert_eq!(schema_format(None), SchemaFormat::Json(Dialect::Draft7));
        assert_eq!(
            f("application/vnd.aai.asyncapi+json;version=2.6.0"),
            SchemaFormat::Json(Dialect::Draft7)
        );
        assert_eq!(
            f("application/schema+json;version=draft-04"),
            SchemaFormat::Json(Dialect::Draft4)
        );
        assert_eq!(
            f("application/schema+yaml; version=2020-12"),
            SchemaFormat::Json(Dialect::Draft2020_12)
        );
        assert_eq!(
            f("application/vnd.oai.openapi;version=3.0.0"),
            SchemaFormat::Json(Dialect::OpenApi30)
        );
        assert_eq!(
            f("application/vnd.oai.openapi+json;version=3.1.0"),
            SchemaFormat::Json(Dialect::Draft2020_12)
        );
        assert_eq!(
            f("application/vnd.apache.avro+json;version=1.9.0"),
            SchemaFormat::Avro
        );
        assert_eq!(
            f("application/vnd.google.protobuf;version=3"),
            SchemaFormat::Protobuf
        );
        assert_eq!(
            f("application/raml+yaml;version=1.0"),
            SchemaFormat::Unsupported("application/raml+yaml;version=1.0".into())
        );
    }
}
