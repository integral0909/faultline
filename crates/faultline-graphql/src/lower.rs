//! Lowering a GraphQL schema into the IR.
//!
//! Each field of a root operation type becomes an operation (`query/pets`,
//! `mutation/addPet`, `subscription/petAdded`): its arguments are the
//! request, its type the response, and subscriptions stream responses.
//!
//! Values are described as they appear in a response, from the client's
//! point of view:
//!
//! * a field the client selects is always present, so output fields are
//!   required, and objects are open because unselected fields are absent;
//! * nullable types are a union with `null`, so making a response field
//!   nullable is a variant added for the client to handle;
//! * input objects and argument lists are closed: unknown fields are
//!   rejected, and a non-null input without a default must be given;
//! * field arguments hang off their field, and flow the other way;
//! * every object carries `__typename`, a one-value enum of its type name, so
//!   union members and interface implementations are told apart as clients
//!   tell them apart; an interface is the object of the fields it declares
//!   with `__typename` ranging over its implementations.

use std::collections::HashMap;

use faultline_doc::{Decimal, Number};
use faultline_ir::{
    Api, ApiFormat, ArrayType, Bound, Content, EnumType, EnumValue, Field, Literal, Meta,
    NumericType, ObjectType, Operation, OperationKey, QualName, RequestBody, Response, Retired,
    StatusKey, Streaming, StringType, TypeId, TypeKind, TypeNode, UnionMode, UnionType,
};

use crate::ast::{
    Directive, FieldDef, InputValue, OperationKind, TypeDef, TypeDefKind, TypeRef, Value,
    deprecation,
};
use crate::schema::Schema;

/// The field every object value carries, naming its type.
pub const TYPENAME: &str = "__typename";

/// The media type recorded for request and response bodies.
pub const GRAPHQL_MEDIA_TYPE: &str = "application/graphql";

pub(crate) struct Lowerer<'a> {
    schema: &'a Schema,
    api: Api,
    ids: HashMap<String, TypeId>,
    pending: Vec<String>,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(schema: &'a Schema, name: &str) -> Self {
        Self {
            schema,
            api: Api::new(name, ApiFormat::GraphQl),
            ids: HashMap::new(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn lower(mut self) -> Api {
        for (kind, root) in &self.schema.roots {
            let def = self.schema.get(root).expect("roots are validated");
            let TypeDefKind::Object { fields, .. } = &def.kind else {
                unreachable!("roots are object types")
            };
            for field in fields {
                self.operation(*kind, field);
            }
        }
        while let Some(name) = self.pending.pop() {
            self.define(&name);
        }
        self.api
    }

    fn operation(&mut self, kind: OperationKind, field: &FieldDef) {
        let mut op = Operation::new(OperationKey::Rpc {
            service: kind.as_str().to_owned(),
            method: field.name.text.clone(),
        });
        let arguments = self.arguments(&field.arguments);
        op.request_body = Some(RequestBody {
            required: true,
            content: vec![Content {
                media_type: GRAPHQL_MEDIA_TYPE.to_owned(),
                ty: arguments,
                origin: Some(field.span),
            }],
            meta: Meta::default(),
            origin: Some(field.span),
        });
        let ty = self.type_ref(&field.ty);
        op.responses.push(Response {
            status: StatusKey::Default,
            headers: Vec::new(),
            content: vec![Content {
                media_type: GRAPHQL_MEDIA_TYPE.to_owned(),
                ty,
                origin: Some(field.ty.span()),
            }],
            meta: Meta::default(),
            origin: Some(field.ty.span()),
        });
        op.streaming = Some(Streaming {
            request: false,
            response: kind == OperationKind::Subscription,
        });
        op.meta = meta(field.description.as_ref(), &field.directives);
        op.origin = Some(field.name.span);
        self.api.operations.push(op);
    }

    /// `T` is `T | null`; `T!` is `T`; `[T]` is an array of `T`.
    fn type_ref(&mut self, ty: &TypeRef) -> TypeId {
        match ty {
            TypeRef::NonNull(inner, _) => self.non_null(inner),
            other => {
                let core = self.non_null(other);
                let null = self
                    .api
                    .types
                    .alloc(TypeNode::new(TypeKind::Null).with_origin(other.span()));
                self.api.types.alloc(
                    TypeNode::new(TypeKind::Union(UnionType {
                        mode: UnionMode::Any,
                        variants: vec![core, null],
                        discriminator: None,
                    }))
                    .with_origin(other.span()),
                )
            }
        }
    }

    /// The type without its nullability.
    fn non_null(&mut self, ty: &TypeRef) -> TypeId {
        match ty {
            TypeRef::NonNull(inner, _) => self.non_null(inner),
            TypeRef::List(inner, span) => {
                let items = self.type_ref(inner);
                self.api
                    .types
                    .alloc(TypeNode::new(TypeKind::Array(ArrayType::of(items))).with_origin(*span))
            }
            TypeRef::Named(name) => {
                if self.schema.is_builtin(&name.text) {
                    return self.builtin(&name.text, name.span);
                }
                match self.schema.get(&name.text).map(|t| &t.kind) {
                    // Custom scalars are usually strings (dates, ids, URLs);
                    // the scalar's name is kept as the format so a change of
                    // scalar is reported.
                    Some(TypeDefKind::Scalar) => self.api.types.alloc(
                        TypeNode::new(TypeKind::String(StringType {
                            format: Some(name.text.clone()),
                            ..StringType::default()
                        }))
                        .with_origin(name.span),
                    ),
                    _ => self.named(&name.text),
                }
            }
        }
    }

    fn builtin(&mut self, name: &str, span: faultline_source::Span) -> TypeId {
        let kind = match name {
            "Int" => TypeKind::Integer(NumericType {
                minimum: Some(Bound::inclusive(Decimal::from_i128(i128::from(i32::MIN)))),
                maximum: Some(Bound::inclusive(Decimal::from_i128(i128::from(i32::MAX)))),
                multiple_of: None,
                format: Some("Int".into()),
                encoding: None,
            }),
            "Float" => TypeKind::Number(NumericType {
                format: Some("Float".into()),
                ..NumericType::default()
            }),
            "Boolean" => TypeKind::Boolean,
            "ID" => TypeKind::String(StringType {
                format: Some("ID".into()),
                ..StringType::default()
            }),
            _ => TypeKind::String(StringType::default()),
        };
        self.api.types.alloc(TypeNode::new(kind).with_origin(span))
    }

    fn named(&mut self, name: &str) -> TypeId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.api.types.reserve();
        self.ids.insert(name.to_owned(), id);
        self.api.named.insert(QualName::new(name), id);
        self.pending.push(name.to_owned());
        id
    }

    fn define(&mut self, name: &str) {
        let id = self.ids[name];
        let def = self.schema.get(name).expect("only defined types are named");
        let kind = match &def.kind {
            TypeDefKind::Object { fields, .. } | TypeDefKind::Interface { fields, .. } => {
                let additional = self.api.types.alloc_kind(TypeKind::Any);
                let mut object = ObjectType::new(additional);
                // Responses say which object type a value is, and clients
                // dispatch on it (fragments, `__typename`): an interface's
                // values are one of its implementations.
                let names: Vec<&str> = if matches!(def.kind, TypeDefKind::Interface { .. }) {
                    self.schema.implementations(name)
                } else {
                    vec![name]
                };
                let typename = self.api.types.alloc(
                    TypeNode::new(TypeKind::Enum(EnumType {
                        values: names
                            .iter()
                            .map(|n| EnumValue::new(Literal::string(*n)))
                            .collect(),
                        open: false,
                        retired: Retired::default(),
                        writer_exact: false,
                    }))
                    .with_origin(def.name.span),
                );
                object.fields.push(Field::new(TYPENAME, typename, true));
                for field in fields {
                    let ty = self.type_ref(&field.ty);
                    let arguments = self.arguments(&field.arguments);
                    let mut out = Field::new(field.name.text.clone(), ty, true);
                    out.arguments = Some(arguments);
                    out.meta = meta(field.description.as_ref(), &field.directives);
                    out.origin = Some(field.span);
                    object.fields.push(out);
                }
                TypeKind::Object(object)
            }
            TypeDefKind::InputObject { fields } => TypeKind::Object(self.inputs(fields)),
            TypeDefKind::Union { members } => {
                let variants = members.iter().map(|m| self.named(&m.text)).collect();
                TypeKind::Union(UnionType {
                    mode: UnionMode::Tagged,
                    variants,
                    discriminator: None,
                })
            }
            TypeDefKind::Enum { values } => TypeKind::Enum(EnumType {
                values: values
                    .iter()
                    .map(|v| {
                        let mut value = EnumValue::new(Literal::string(v.name.text.clone()));
                        value.name = Some(v.name.text.clone());
                        value.meta = meta(v.description.as_ref(), &v.directives);
                        value.origin = Some(v.span);
                        value
                    })
                    .collect(),
                open: false,
                retired: Retired::default(),
                writer_exact: false,
            }),
            TypeDefKind::Scalar => unreachable!("scalars are lowered where they are used"),
        };
        let mut node = TypeNode::new(kind)
            .with_origin(def.name.span)
            .with_name(QualName::new(name));
        node.meta = type_meta(def);
        self.api.types.define(id, node);
    }

    /// Arguments as a closed object.
    fn arguments(&mut self, arguments: &[InputValue]) -> TypeId {
        let object = self.inputs(arguments);
        self.api.types.alloc_kind(TypeKind::Object(object))
    }

    /// Input values as a closed object: a non-null value without a default
    /// must be given.
    fn inputs(&mut self, values: &[InputValue]) -> ObjectType {
        let never = self.api.types.alloc_kind(TypeKind::Never);
        let mut object = ObjectType::new(never);
        for value in values {
            let ty = self.type_ref(&value.ty);
            let required = value.ty.is_non_null() && value.default.is_none();
            let mut field = Field::new(value.name.text.clone(), ty, required);
            field.meta = meta(value.description.as_ref(), &value.directives);
            field.meta.default = value.default.as_ref().map(literal);
            field.origin = Some(value.span);
            object.fields.push(field);
        }
        object
    }
}

fn meta(description: Option<&String>, directives: &[Directive]) -> Meta {
    Meta {
        description: description.cloned(),
        deprecated: deprecation(directives).is_some(),
        ..Meta::default()
    }
}

fn type_meta(def: &TypeDef) -> Meta {
    meta(def.description.as_ref(), &def.directives)
}

/// A constant as a literal. Enum values are their names, as in JSON.
fn literal(value: &Value) -> Literal {
    match value {
        Value::Int(s) | Value::Float(s) => Decimal::parse(s).map_or_else(
            |_| Literal::string(s.clone()),
            |d| Literal::Number(Number::from(d)),
        ),
        Value::String(s) | Value::Enum(s) | Value::Variable(s) => Literal::string(s.clone()),
        Value::Boolean(b) => Literal::Bool(*b),
        Value::Null => Literal::Null,
        Value::List(items) => Literal::Array(items.iter().map(literal).collect()),
        Value::Object(fields) => Literal::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), literal(v)))
                .collect(),
        ),
    }
}
