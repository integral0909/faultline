//! Lowering linked Protobuf declarations into the IR.
//!
//! The IR describes the binary wire format:
//!
//! * a message is an object whose fields carry their numbers as tags, so the
//!   comparison matches fields by number; the JSON name is an alias;
//! * unknown fields are skipped by every parser, so objects are open;
//! * a `oneof` is an exclusive group;
//! * integers carry their value range, declared type and wire encoding;
//! * enums list their values by name and number, and are open unless the
//!   syntax makes them closed;
//! * each RPC is an operation whose request and response bodies are the
//!   input and output messages, with streaming recorded per direction.
//!
//! Messages and enums become named types, lowered on first use from a
//! worklist so that recursive messages need no recursion here.

use std::collections::HashMap;

use faultline_doc::{Decimal, Number};
use faultline_ir::{
    Api, ApiFormat, ArrayType, Bound, Content, EnumType, EnumValue, ExclusiveGroup, Field, Literal,
    MapType, Meta, NumericType, ObjectType, Operation, OperationKey, QualName, RequestBody,
    Response, Retired, Role, SchemaRoot, StatusKey, Streaming, TypeId, TypeKind, TypeNode,
};
use faultline_source::Span;

use crate::ast::{self, FieldType, Label, OptionDecl, OptionValue, Scalar, option};
use crate::linker::{EnumDecl, Linked, MessageDecl, SymbolKind, encoding, json_name};
use crate::load::ProtoFile;

/// The media type recorded for RPC request and response bodies.
pub const RPC_MEDIA_TYPE: &str = "application/grpc";

pub(crate) struct Lowerer<'a> {
    files: &'a [ProtoFile],
    linked: &'a Linked,
    api: Api,
    ids: HashMap<String, TypeId>,
    pending: Vec<String>,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(files: &'a [ProtoFile], linked: &'a Linked, name: &str) -> Self {
        Self {
            files,
            linked,
            api: Api::new(name, ApiFormat::Protobuf),
            ids: HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// Lowers the entry file: its services become operations; a file
    /// without services describes data, and its top-level messages become
    /// schema roots used in `role`.
    pub(crate) fn lower(mut self, role: Role) -> Api {
        let entry = &self.files[0];
        let package = entry
            .ast
            .package
            .as_ref()
            .map(|p| p.text.clone())
            .unwrap_or_default();
        let services: Vec<String> = self.linked.order[0]
            .iter()
            .filter(|name| self.linked.services.contains_key(*name))
            .cloned()
            .collect();
        for service in &services {
            self.service(service);
        }
        if services.is_empty() {
            for message in &entry.ast.messages {
                let full_name = qualify(&package, &message.name.text);
                let ty = self.named(&full_name);
                self.api.schemas.push(SchemaRoot {
                    name: full_name,
                    ty,
                    role,
                    origin: Some(message.name.span),
                });
            }
        }
        // Every type the entry file declares is compared, used or not.
        for name in &self.linked.order[0] {
            if self.linked.messages.contains_key(name) || self.linked.enums.contains_key(name) {
                self.named(name);
            }
        }
        while let Some(name) = self.pending.pop() {
            self.define(&name);
        }
        self.api
    }

    /// The id of a named message or enum, reserving it on first use.
    fn named(&mut self, full_name: &str) -> TypeId {
        if let Some(&id) = self.ids.get(full_name) {
            return id;
        }
        let id = self.api.types.reserve();
        self.ids.insert(full_name.to_owned(), id);
        self.api.named.insert(QualName::new(full_name), id);
        self.pending.push(full_name.to_owned());
        id
    }

    fn define(&mut self, full_name: &str) {
        let id = self.ids[full_name];
        let node = if let Some(message) = self.linked.messages.get(full_name) {
            self.message(message)
        } else if let Some(e) = self.linked.enums.get(full_name) {
            enumeration(e)
        } else {
            unreachable!("only resolved messages and enums are named")
        };
        self.api
            .types
            .define(id, node.with_name(QualName::new(full_name)));
    }

    fn message(&mut self, decl: &MessageDecl) -> TypeNode {
        let m = &decl.ast;
        let additional = self.api.types.alloc_kind(TypeKind::Any);
        let mut object = ObjectType::new(additional);
        for field in &m.fields {
            object.fields.push(self.field(field));
        }
        object.retired = retired(&m.reserved);
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
                    .map(|f| f.name.text.clone())
                    .collect(),
                origin: Some(oneof.span),
            })
            .collect();
        let mut node = TypeNode::new(TypeKind::Object(object)).with_origin(m.name.span);
        node.meta = meta(m.doc.as_ref(), &m.options);
        node
    }

    fn field(&mut self, field: &ast::Field) -> Field {
        let element = self.field_type(&field.ty, field.span);
        let ty = if field.label == Some(Label::Repeated) {
            let mut array = TypeNode::new(TypeKind::Array(ArrayType::of(element)));
            array.origin = Some(field.span);
            self.api.types.alloc(array)
        } else {
            element
        };
        let required = field.label == Some(Label::Required)
            || option(&field.options, "features.field_presence")
                .and_then(|o| o.value.as_ident())
                .is_some_and(|v| v == "LEGACY_REQUIRED");
        let mut out = Field::new(field.name.text.clone(), ty, required);
        out.tag = u32::try_from(field.number).ok();
        let json = option(&field.options, "json_name")
            .and_then(|o| o.value.as_str().map(str::to_owned))
            .unwrap_or_else(|| json_name(&field.name.text));
        if json != field.name.text {
            out.aliases.push(json);
        }
        out.meta = meta(field.doc.as_ref(), &field.options);
        if let Some(default) = option(&field.options, "default") {
            out.meta.default = default_literal(&default.value, &field.ty);
        }
        out.origin = Some(field.span);
        out
    }

    fn field_type(&mut self, ty: &FieldType, span: Span) -> TypeId {
        match ty {
            FieldType::Scalar(s) => self.scalar(*s, span),
            FieldType::Named(name) => {
                let resolved = self
                    .linked
                    .resolve(name)
                    .expect("the linker resolved every field type");
                debug_assert!(matches!(
                    resolved.kind,
                    SymbolKind::Message | SymbolKind::Enum
                ));
                let full_name = resolved.full_name.clone();
                self.named(&full_name)
            }
            FieldType::Map(key, value) => {
                let key = self.field_type(key, span);
                let value = self.field_type(value, span);
                let mut node = TypeNode::new(TypeKind::Map(MapType { key, value }));
                node.origin = Some(span);
                self.api.types.alloc(node)
            }
        }
    }

    /// A fresh node per use: two fields of the same scalar type are
    /// separate positions, each compared on its own.
    fn scalar(&mut self, scalar: Scalar, span: Span) -> TypeId {
        let kind = match scalar {
            Scalar::Bool => TypeKind::Boolean,
            Scalar::String => TypeKind::String(faultline_ir::StringType::default()),
            Scalar::Bytes => TypeKind::Bytes,
            Scalar::Float | Scalar::Double => TypeKind::Number(NumericType {
                format: Some(scalar.as_str().to_owned()),
                encoding: encoding(scalar).map(str::to_owned),
                ..NumericType::default()
            }),
            _ => {
                let (min, max) = integer_range(scalar);
                TypeKind::Integer(NumericType {
                    minimum: Some(Bound::inclusive(min)),
                    maximum: Some(Bound::inclusive(max)),
                    multiple_of: None,
                    format: Some(scalar.as_str().to_owned()),
                    encoding: encoding(scalar).map(str::to_owned),
                })
            }
        };
        self.api.types.alloc(TypeNode::new(kind).with_origin(span))
    }

    fn service(&mut self, full_name: &str) {
        let service = &self.linked.services[full_name];
        let deprecated_service = deprecated(&service.ast.options);
        for method in &service.ast.methods {
            let key = OperationKey::Rpc {
                service: full_name.to_owned(),
                method: method.name.text.clone(),
            };
            let mut op = Operation::new(key);
            let input = self.message_ref(&method.input);
            let output = self.message_ref(&method.output);
            op.request_body = Some(RequestBody {
                required: true,
                content: vec![Content {
                    media_type: RPC_MEDIA_TYPE.to_owned(),
                    ty: input,
                    origin: Some(method.input.span),
                }],
                meta: Meta::default(),
                origin: Some(method.input.span),
            });
            op.responses.push(Response {
                status: StatusKey::Default,
                headers: Vec::new(),
                content: vec![Content {
                    media_type: RPC_MEDIA_TYPE.to_owned(),
                    ty: output,
                    origin: Some(method.output.span),
                }],
                meta: Meta::default(),
                origin: Some(method.output.span),
            });
            op.streaming = Some(Streaming {
                request: method.input_stream,
                response: method.output_stream,
            });
            op.meta = meta(method.doc.as_ref(), &method.options);
            op.meta.deprecated |= deprecated_service;
            op.origin = Some(method.name.span);
            self.api.operations.push(op);
        }
    }

    fn message_ref(&mut self, name: &ast::TypeName) -> TypeId {
        let resolved = self
            .linked
            .resolve(name)
            .expect("the linker resolved every RPC type");
        let full_name = resolved.full_name.clone();
        self.named(&full_name)
    }
}

fn enumeration(decl: &EnumDecl) -> TypeNode {
    let values = decl
        .ast
        .values
        .iter()
        .map(|v| {
            let mut value = EnumValue::new(Literal::string(v.name.text.clone()));
            value.name = Some(v.name.text.clone());
            value.number = Some(v.number);
            value.meta = meta(v.doc.as_ref(), &v.options);
            value.origin = Some(v.span);
            value
        })
        .collect();
    let mut node = TypeNode::new(TypeKind::Enum(EnumType {
        values,
        open: !decl.closed,
        retired: retired(&decl.ast.reserved),
    }))
    .with_origin(decl.ast.name.span);
    node.meta = meta(decl.ast.doc.as_ref(), &decl.ast.options);
    node
}

fn retired(reserved: &[ast::Reserved]) -> Retired {
    Retired {
        ranges: reserved
            .iter()
            .flat_map(|r| r.ranges.iter().copied())
            .collect(),
        names: reserved
            .iter()
            .flat_map(|r| r.names.iter().cloned())
            .collect(),
    }
}

fn meta(doc: Option<&String>, options: &[OptionDecl]) -> Meta {
    Meta {
        description: doc.cloned(),
        deprecated: deprecated(options),
        ..Meta::default()
    }
}

fn deprecated(options: &[OptionDecl]) -> bool {
    option(options, "deprecated")
        .and_then(|o| o.value.as_bool())
        .unwrap_or(false)
}

/// A proto2 `[default = ...]` as a literal of the field's type.
fn default_literal(value: &OptionValue, ty: &FieldType) -> Option<Literal> {
    Some(match value {
        OptionValue::Int(i) => Literal::Number(Number::from(Decimal::from_i128(*i))),
        OptionValue::Float(f) if f.is_nan() => Literal::Number(Number::NaN),
        OptionValue::Float(f) if f.is_infinite() => Literal::Number(if *f > 0.0 {
            Number::PosInfinity
        } else {
            Number::NegInfinity
        }),
        OptionValue::Float(f) => {
            Literal::Number(Number::from(Decimal::parse(&f.to_string()).ok()?))
        }
        OptionValue::Ident(s) if matches!(ty, FieldType::Scalar(Scalar::Bool)) => {
            Literal::Bool(s == "true")
        }
        OptionValue::Ident(s) if s == "inf" => Literal::Number(Number::PosInfinity),
        OptionValue::Ident(s) if s == "nan" => Literal::Number(Number::NaN),
        OptionValue::Ident(s) => Literal::string(s.clone()),
        OptionValue::Str(bytes) => Literal::string(String::from_utf8_lossy(bytes).into_owned()),
        OptionValue::Aggregate(_) => return None,
    })
}

/// The values an integer scalar can hold.
fn integer_range(scalar: Scalar) -> (Decimal, Decimal) {
    let signed = |bits: u32| {
        let max = (1i128 << (bits - 1)) - 1;
        (Decimal::from_i128(-max - 1), Decimal::from_i128(max))
    };
    let unsigned = |bits: u32| (Decimal::zero(), Decimal::from_u128((1u128 << bits) - 1));
    match scalar {
        Scalar::Int32 | Scalar::Sint32 | Scalar::Sfixed32 => signed(32),
        Scalar::Int64 | Scalar::Sint64 | Scalar::Sfixed64 => signed(64),
        Scalar::Uint32 | Scalar::Fixed32 => unsigned(32),
        Scalar::Uint64 | Scalar::Fixed64 => unsigned(64),
        _ => unreachable!("not an integer type"),
    }
}

fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_owned()
    } else {
        format!("{scope}.{name}")
    }
}
