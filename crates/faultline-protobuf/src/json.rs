//! Messages as the proto3 JSON mapping presents them.
//!
//! The HTTP/JSON view of a service (see [`crate::http`]) exchanges JSON, not
//! the binary wire format, so its types follow the JSON mapping:
//!
//! * fields are named by their JSON name, and none is required unless it
//!   is annotated `(google.api.field_behavior) = REQUIRED`: printers omit
//!   fields holding their default value;
//! * 64-bit integers are strings, enums are their value names, `bytes` are
//!   base64 strings, maps are objects;
//! * the well-known types have their special representations (a
//!   `Timestamp` is an RFC 3339 string, a wrapper is its bare value, a
//!   `Struct` any object).
//!
//! Parsers also accept numbers for 64-bit integers and numbers for enums;
//! the view describes what printers write, which is what documentation of
//! the JSON API describes too.

use std::collections::HashMap;

use faultline_ir::{
    Api, ArrayType, EnumType, EnumValue, ExclusiveGroup, Field, Literal, NumericType, ObjectType,
    QualName, Retired, StringType, TypeId, TypeKind, TypeNode,
};
use faultline_source::Span;

use crate::ast::{self, FieldType, Label, OptionDecl, Scalar, option};
use crate::linker::{EnumDecl, Linked, MessageDecl, json_name};

/// The option marking fields as required, output-only and so on.
pub(crate) const FIELD_BEHAVIOR: &str = "(google.api.field_behavior)";

pub(crate) struct JsonTypes<'a> {
    linked: &'a Linked,
    ids: HashMap<String, TypeId>,
    pending: Vec<String>,
}

impl<'a> JsonTypes<'a> {
    pub(crate) fn new(linked: &'a Linked) -> Self {
        Self {
            linked,
            ids: HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// The type of the named message or enum, defined later by
    /// [`JsonTypes::finish`].
    pub(crate) fn named(&mut self, api: &mut Api, full_name: &str) -> TypeId {
        if let Some(&id) = self.ids.get(full_name) {
            return id;
        }
        let id = api.types.reserve();
        self.ids.insert(full_name.to_owned(), id);
        api.named.insert(QualName::new(full_name), id);
        self.pending.push(full_name.to_owned());
        id
    }

    /// Defines every type used so far.
    pub(crate) fn finish(&mut self, api: &mut Api) {
        while let Some(name) = self.pending.pop() {
            let id = self.ids[&name];
            let node = if let Some(node) = well_known(api, &name) {
                node
            } else if let Some(message) = self.linked.messages.get(&name) {
                self.message(api, message)
            } else if let Some(e) = self.linked.enums.get(&name) {
                enumeration(e)
            } else {
                unreachable!("only resolved messages and enums are named")
            };
            api.types.define(id, node.with_name(QualName::new(&name)));
        }
    }

    fn message(&mut self, api: &mut Api, decl: &MessageDecl) -> TypeNode {
        let m = &decl.ast;
        let additional = api.types.alloc_kind(TypeKind::Any);
        let mut object = ObjectType::new(additional);
        for field in &m.fields {
            let ty = self.field_type(api, field);
            let mut out = Field::new(field_json_name(field), ty, is_required(&field.options));
            out.origin = Some(field.span);
            out.meta.description.clone_from(&field.doc);
            object.fields.push(out);
        }
        object.exclusive = m
            .oneofs
            .iter()
            .enumerate()
            .map(|(i, oneof)| ExclusiveGroup {
                name: oneof.name.text.clone(),
                fields: m
                    .fields
                    .iter()
                    .filter(|f| f.oneof == Some(i))
                    .map(field_json_name)
                    .collect(),
                origin: Some(oneof.span),
            })
            .collect();
        let mut node = TypeNode::new(TypeKind::Object(object)).with_origin(m.name.span);
        node.meta.description.clone_from(&m.doc);
        node
    }

    /// The JSON type of a field, repeated fields as arrays.
    pub(crate) fn field_type(&mut self, api: &mut Api, field: &ast::Field) -> TypeId {
        let element = self.value_type(api, &field.ty, field.span);
        if field.label == Some(Label::Repeated) {
            api.types.alloc(
                TypeNode::new(TypeKind::Array(ArrayType::of(element))).with_origin(field.span),
            )
        } else {
            element
        }
    }

    fn value_type(&mut self, api: &mut Api, ty: &FieldType, span: Span) -> TypeId {
        match ty {
            FieldType::Scalar(s) => scalar(api, *s, span),
            FieldType::Named(name) => {
                let full_name = self
                    .linked
                    .resolve(name)
                    .expect("the linker resolved every field type")
                    .full_name
                    .clone();
                self.named(api, &full_name)
            }
            // Map keys are always strings in JSON.
            FieldType::Map(_, value) => {
                let value = self.value_type(api, value, span);
                api.types.alloc(
                    TypeNode::new(TypeKind::Object(ObjectType::new(value))).with_origin(span),
                )
            }
        }
    }

    /// The message a field holds, when it holds one (for flattening query
    /// parameters).
    pub(crate) fn message_of(&self, field: &ast::Field) -> Option<&'a MessageDecl> {
        let FieldType::Named(name) = &field.ty else {
            return None;
        };
        let resolved = self.linked.resolve(name)?;
        if is_well_known(&resolved.full_name) {
            return None;
        }
        self.linked.messages.get(&resolved.full_name)
    }
}

/// The JSON name of a field: `json_name`, or the lowerCamelCase name.
pub(crate) fn field_json_name(field: &ast::Field) -> String {
    option(&field.options, "json_name")
        .and_then(|o| o.value.as_str().map(str::to_owned))
        .unwrap_or_else(|| json_name(&field.name.text))
}

/// Whether a field is annotated `(google.api.field_behavior) = REQUIRED`.
pub(crate) fn is_required(options: &[OptionDecl]) -> bool {
    options
        .iter()
        .filter(|o| o.name == FIELD_BEHAVIOR)
        .any(|o| o.value.as_ident() == Some("REQUIRED"))
}

fn scalar(api: &mut Api, scalar: Scalar, span: Span) -> TypeId {
    let string = |format: Option<&str>| {
        TypeKind::String(StringType {
            format: format.map(str::to_owned),
            ..StringType::default()
        })
    };
    // Ranges are written the way OpenAPI descriptions of JSON APIs write
    // them, as a format, so that a description and the service agree.
    let integer = |format: &str| {
        TypeKind::Integer(NumericType {
            format: Some(format.to_owned()),
            ..NumericType::default()
        })
    };
    let kind = match scalar {
        Scalar::Bool => TypeKind::Boolean,
        Scalar::String => string(None),
        Scalar::Bytes => string(Some("byte")),
        Scalar::Float => TypeKind::Number(NumericType {
            format: Some("float".into()),
            ..NumericType::default()
        }),
        Scalar::Double => TypeKind::Number(NumericType {
            format: Some("double".into()),
            ..NumericType::default()
        }),
        Scalar::Int32 | Scalar::Sint32 | Scalar::Sfixed32 => integer("int32"),
        // A `uint32` can exceed `int32`, so descriptions call it `int64`.
        Scalar::Uint32 | Scalar::Fixed32 => integer("int64"),
        // 64-bit integers are decimal strings.
        Scalar::Int64 | Scalar::Sint64 | Scalar::Sfixed64 | Scalar::Uint64 | Scalar::Fixed64 => {
            string(None)
        }
    };
    api.types.alloc(TypeNode::new(kind).with_origin(span))
}

fn enumeration(decl: &EnumDecl) -> TypeNode {
    let values = decl
        .ast
        .values
        .iter()
        .map(|v| {
            let mut value = EnumValue::new(Literal::string(v.name.text.clone()));
            value.origin = Some(v.span);
            value
        })
        .collect();
    let mut node = TypeNode::new(TypeKind::Enum(EnumType {
        values,
        open: false,
        retired: Retired::default(),
        writer_exact: false,
    }))
    .with_origin(decl.ast.name.span);
    node.meta.description.clone_from(&decl.ast.doc);
    node
}

fn is_well_known(full_name: &str) -> bool {
    full_name.starts_with("google.protobuf.")
}

/// The JSON representation of a well-known type.
fn well_known(api: &mut Api, full_name: &str) -> Option<TypeNode> {
    let name = full_name.strip_prefix("google.protobuf.")?;
    let string = |format: Option<&str>| {
        TypeKind::String(StringType {
            format: format.map(str::to_owned),
            ..StringType::default()
        })
    };
    let kind = match name {
        "Timestamp" => string(Some("date-time")),
        // 64-bit integers are decimal strings.
        "Duration" | "FieldMask" | "StringValue" | "Int64Value" | "UInt64Value" => string(None),
        "BytesValue" => string(Some("byte")),
        "Int32Value" | "UInt32Value" => TypeKind::Integer(NumericType {
            format: Some(
                if name == "Int32Value" {
                    "int32"
                } else {
                    "int64"
                }
                .into(),
            ),
            ..NumericType::default()
        }),
        "DoubleValue" | "FloatValue" => TypeKind::Number(NumericType {
            format: Some(
                if name == "FloatValue" {
                    "float"
                } else {
                    "double"
                }
                .into(),
            ),
            ..NumericType::default()
        }),
        "BoolValue" => TypeKind::Boolean,
        "Value" => TypeKind::Any,
        "NullValue" => TypeKind::Null,
        "ListValue" => {
            let any = api.types.alloc_kind(TypeKind::Any);
            TypeKind::Array(ArrayType::of(any))
        }
        "Struct" | "Empty" => {
            let any = api.types.alloc_kind(TypeKind::Any);
            TypeKind::Object(ObjectType::new(any))
        }
        "Any" => {
            let any = api.types.alloc_kind(TypeKind::Any);
            let type_url = api.types.alloc_kind(string(None));
            let mut object = ObjectType::new(any);
            object.fields.push(Field::new("@type", type_url, true));
            TypeKind::Object(object)
        }
        _ => return None,
    };
    Some(TypeNode::new(kind))
}
