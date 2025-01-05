//! Lowering a resolved Smithy model's shapes into the IR.
//!
//! | Smithy | IR |
//! |---|---|
//! | `blob` | bytes |
//! | `boolean` | boolean |
//! | `document` | any value |
//! | `string` | string, with `@pattern` and `@length` |
//! | `byte`, `short`, `integer`, `long` | integer with the type's range, narrowed by `@range` |
//! | `float`, `double`, `bigDecimal` | number, bounded by `@range` |
//! | `bigInteger` | integer |
//! | `timestamp` | string with format `date-time`; a number with `@timestampFormat("epoch-seconds")` |
//! | `enum`, `intEnum` | enum of the values (`@enumValue`, or the member's name) |
//! | `list` | array; `@uniqueItems` makes items unique, `@sparse` lets them be null |
//! | `map` | object whose members all have the value's type, their names the key's |
//! | `structure` | object; `@required` members are required |
//! | `union` | object with exactly one of its members set |
//!
//! Structures are open, as Smithy clients and servers ignore members they
//! do not know; unions are closed, as a union whose member is unknown has
//! no value a reader can use. Documentation comments and `@documentation`, `@deprecated`
//! and member defaults are kept. The model is compared as its shapes, not
//! as one protocol's serialization of them.

use std::collections::HashMap;

use faultline_doc::{Decimal, Number};
use faultline_ir::{
    Api, ApiFormat, ArrayType, Bound, EnumType, EnumValue, ExclusiveGroup, Field, Literal, Meta,
    NumericType, ObjectType, QualName, Retired, StringType, TypeId, TypeKind, TypeNode, UnionMode,
    UnionType,
};
use faultline_source::Span;

use crate::ast::{Node, NodeValue, ShapeKind, SimpleType};
use crate::resolve::{PRELUDE, RMember, RShape, Resolved, prelude};

pub(crate) struct Lowerer<'m> {
    pub(crate) model: &'m Resolved,
    pub(crate) api: Api,
    /// The namespace whose shapes keep their plain names.
    namespace: String,
    ids: HashMap<String, TypeId>,
    pending: Vec<String>,
}

impl<'m> Lowerer<'m> {
    pub(crate) fn new(model: &'m Resolved, name: &str, namespace: &str) -> Self {
        Self {
            model,
            api: Api::new(name, ApiFormat::Smithy),
            namespace: namespace.to_owned(),
            ids: HashMap::new(),
            pending: Vec::new(),
        }
    }

    fn alloc(&mut self, kind: TypeKind, span: Option<Span>) -> TypeId {
        let mut node = TypeNode::new(kind);
        node.origin = span;
        self.api.types.alloc(node)
    }

    /// Defines the named types reserved so far.
    pub(crate) fn drain(&mut self) {
        while let Some(id) = self.pending.pop() {
            self.define(&id);
        }
    }

    /// A shape's name in the IR: plain in the model's own namespace,
    /// absolute elsewhere.
    fn display_name(&self, id: &str) -> String {
        match id.split_once('#') {
            Some((namespace, name)) if namespace == self.namespace => name.to_owned(),
            _ => id.to_owned(),
        }
    }

    /// The type of the shape `id` as a member targets it: a fresh node
    /// for prelude and simple shapes, the named type for aggregates.
    pub(crate) fn target(&mut self, id: &str, member: Option<&RMember>) -> TypeId {
        let span = member.map(|m| m.span);
        let Some(shape) = self.model.get(id) else {
            // A prelude shape.
            let name = id
                .strip_prefix(PRELUDE)
                .map_or(id, |n| n.trim_start_matches('#'));
            if name == "Unit" {
                let any = self.alloc(TypeKind::Any, None);
                return self.alloc(TypeKind::Object(ObjectType::new(any)), span);
            }
            let kind = prelude_kind(name, member.and_then(timestamp_format));
            let kind = constrain(
                kind,
                member.map(|m| m.traits.as_slice()).unwrap_or_default(),
            );
            return self.alloc(kind, span);
        };
        if let ShapeKind::Simple(simple) = shape.kind {
            // A member's timestamp format overrides its target's.
            let format = member.and_then(timestamp_format).or_else(|| {
                shape
                    .get_trait(&prelude("timestampFormat"))
                    .and_then(|t| t.value.as_str())
            });
            let kind = base_kind(simple, format);
            let kind = constrain(kind, &shape.traits);
            let kind = constrain(
                kind,
                member.map(|m| m.traits.as_slice()).unwrap_or_default(),
            );
            let ty = self.alloc(kind, Some(shape.span));
            self.api.types.get_mut(ty).meta = meta(shape.doc.as_ref(), shape);
            return ty;
        }
        if let Some(&ty) = self.ids.get(id) {
            return ty;
        }
        let ty = self.api.types.reserve();
        self.ids.insert(id.to_owned(), ty);
        self.api
            .named
            .insert(QualName::new(self.display_name(id)), ty);
        self.pending.push(id.to_owned());
        ty
    }

    fn define(&mut self, id: &str) {
        let shape = self
            .model
            .get(id)
            .expect("only defined shapes are reserved");
        let ty = self.ids[id];
        let kind = match shape.kind {
            ShapeKind::Enum | ShapeKind::IntEnum => enumeration(shape),
            ShapeKind::List => self.list(shape),
            ShapeKind::Map => self.map(shape),
            ShapeKind::Structure | ShapeKind::Union => self.structure(shape),
            // Services, resources and operations are not types; a member
            // targeting one is reported by resolution.
            _ => TypeKind::Any,
        };
        let mut node = TypeNode::new(kind).with_origin(shape.span);
        node.meta = meta(shape.doc.as_ref(), shape);
        self.api
            .types
            .define(ty, node.with_name(QualName::new(self.display_name(id))));
    }

    fn member_named(shape: &'m RShape, name: &str) -> Option<&'m RMember> {
        shape.members.iter().find(|m| m.name == name)
    }

    fn list(&mut self, shape: &'m RShape) -> TypeKind {
        let Some(member) = Self::member_named(shape, "member") else {
            return TypeKind::Array(ArrayType::of(self.alloc(TypeKind::Any, None)));
        };
        let mut items = self.target(&member.target, Some(member));
        if shape.has_trait("sparse") {
            let null = self.alloc(TypeKind::Null, None);
            items = self.alloc(
                TypeKind::Union(UnionType {
                    mode: UnionMode::Any,
                    variants: vec![items, null],
                    discriminator: None,
                }),
                Some(member.span),
            );
        }
        let mut array = ArrayType::of(items);
        array.unique_items = shape.has_trait("uniqueItems");
        if let Some((min, max)) = length(&shape.traits) {
            array.min_items = min.unwrap_or(0);
            array.max_items = max;
        }
        TypeKind::Array(array)
    }

    fn map(&mut self, shape: &'m RShape) -> TypeKind {
        let value = match Self::member_named(shape, "value") {
            Some(member) => self.target(&member.target, Some(member)),
            None => self.alloc(TypeKind::Any, None),
        };
        let mut object = ObjectType::new(value);
        if let Some(key) = Self::member_named(shape, "key") {
            let names = self.target(&key.target, Some(key));
            // Keys without constraints add nothing: every name is a string.
            if !matches!(&self.api.types.get(names).kind, TypeKind::String(s) if *s == StringType::default())
            {
                object.property_names = Some(names);
            }
        }
        if let Some((min, max)) = length(&shape.traits) {
            object.min_properties = min.unwrap_or(0);
            object.max_properties = max;
        }
        TypeKind::Object(object)
    }

    fn structure(&mut self, shape: &'m RShape) -> TypeKind {
        // A union holds one of its members and nothing else: a member a
        // reader does not know leaves it without a value it can use.
        let others = if shape.kind == ShapeKind::Union {
            TypeKind::Never
        } else {
            TypeKind::Any
        };
        let others = self.alloc(others, None);
        let mut object = ObjectType::new(others);
        for member in &shape.members {
            let ty = self.target(&member.target, Some(member));
            let required = shape.kind == ShapeKind::Structure && member.has_trait("required");
            let mut field = Field::new(member.name.clone(), ty, required);
            field.meta = member_meta(member);
            field.meta.default = member
                .default
                .as_ref()
                .or_else(|| member.get_trait(&prelude("default")).map(|t| &t.value))
                .and_then(literal);
            field.origin = Some(member.span);
            object.fields.push(field);
        }
        if shape.kind == ShapeKind::Union && !object.fields.is_empty() {
            object.exclusive.push(ExclusiveGroup {
                name: shape.name.clone(),
                fields: object.fields.iter().map(|f| f.name.clone()).collect(),
                origin: Some(shape.span),
            });
            object.min_properties = 1;
        }
        TypeKind::Object(object)
    }
}

/// A member's `@timestampFormat`.
fn timestamp_format(member: &RMember) -> Option<&str> {
    member
        .get_trait(&prelude("timestampFormat"))
        .and_then(|t| t.value.as_str())
}

/// The kind of a prelude shape.
fn prelude_kind(name: &str, timestamp_format: Option<&str>) -> TypeKind {
    let simple = match name.trim_start_matches("Primitive") {
        "String" => SimpleType::String,
        "Blob" => SimpleType::Blob,
        "BigInteger" => SimpleType::BigInteger,
        "BigDecimal" => SimpleType::BigDecimal,
        "Timestamp" => SimpleType::Timestamp,
        "Boolean" => SimpleType::Boolean,
        "Byte" => SimpleType::Byte,
        "Short" => SimpleType::Short,
        "Integer" => SimpleType::Integer,
        "Long" => SimpleType::Long,
        "Float" => SimpleType::Float,
        "Double" => SimpleType::Double,
        _ => SimpleType::Document,
    };
    base_kind(simple, timestamp_format)
}

fn base_kind(simple: SimpleType, timestamp_format: Option<&str>) -> TypeKind {
    let integer = |bits: Option<u32>, format: &str| {
        let (minimum, maximum) = bits.map_or((None, None), |bits| {
            let max = (1i128 << (bits - 1)) - 1;
            (
                Some(Bound::inclusive(Decimal::from_i128(-max - 1))),
                Some(Bound::inclusive(Decimal::from_i128(max))),
            )
        });
        TypeKind::Integer(NumericType {
            minimum,
            maximum,
            multiple_of: None,
            format: Some(format.to_owned()),
            encoding: None,
        })
    };
    let number = |format: &str| {
        TypeKind::Number(NumericType {
            format: Some(format.to_owned()),
            ..NumericType::default()
        })
    };
    match simple {
        SimpleType::Blob => TypeKind::Bytes,
        SimpleType::Boolean => TypeKind::Boolean,
        SimpleType::Document => TypeKind::Any,
        SimpleType::String => TypeKind::String(StringType::default()),
        SimpleType::Byte => integer(Some(8), "byte"),
        SimpleType::Short => integer(Some(16), "short"),
        SimpleType::Integer => integer(Some(32), "integer"),
        SimpleType::Long => integer(Some(64), "long"),
        SimpleType::BigInteger => integer(None, "bigInteger"),
        SimpleType::Float => number("float"),
        SimpleType::Double => number("double"),
        SimpleType::BigDecimal => number("bigDecimal"),
        SimpleType::Timestamp => match timestamp_format {
            Some("epoch-seconds") => number("epoch-seconds"),
            Some(format) => TypeKind::String(StringType {
                format: Some(format.to_owned()),
                ..StringType::default()
            }),
            None => TypeKind::String(StringType {
                format: Some("date-time".to_owned()),
                ..StringType::default()
            }),
        },
    }
}

/// Applies `@pattern`, `@length` and `@range` from `traits`.
fn constrain(mut kind: TypeKind, traits: &[crate::resolve::RTrait]) -> TypeKind {
    let get = |name: &str| {
        traits
            .iter()
            .find(|t| t.id == prelude(name))
            .map(|t| &t.value)
    };
    match &mut kind {
        TypeKind::String(string) => {
            if let Some(pattern) = get("pattern").and_then(Node::as_str) {
                string.pattern = Some(pattern.to_owned());
            }
            if let Some((min, max)) = length(traits) {
                if let Some(min) = min {
                    string.min_length = min;
                }
                if max.is_some() {
                    string.max_length = max;
                }
            }
        }
        TypeKind::Integer(numeric) | TypeKind::Number(numeric) => {
            if let Some(range) = get("range") {
                // `@range` narrows the type's own bounds.
                if let Some(min) = range.get("min").and_then(decimal) {
                    let narrower = numeric.minimum.as_ref().is_none_or(|b| b.value < min);
                    if narrower {
                        numeric.minimum = Some(Bound::inclusive(min));
                    }
                }
                if let Some(max) = range.get("max").and_then(decimal) {
                    let narrower = numeric.maximum.as_ref().is_none_or(|b| b.value > max);
                    if narrower {
                        numeric.maximum = Some(Bound::inclusive(max));
                    }
                }
            }
        }
        _ => {}
    }
    kind
}

/// `@length(min: .., max: ..)`.
fn length(traits: &[crate::resolve::RTrait]) -> Option<(Option<u64>, Option<u64>)> {
    let value = &traits.iter().find(|t| t.id == prelude("length"))?.value;
    let bound = |key: &str| match &value.get(key)?.value {
        NodeValue::Number(n) => n.parse::<u64>().ok(),
        _ => None,
    };
    Some((bound("min"), bound("max")))
}

fn decimal(node: &Node) -> Option<Decimal> {
    match &node.value {
        NodeValue::Number(n) => Decimal::parse(n).ok(),
        _ => None,
    }
}

fn enumeration(shape: &RShape) -> TypeKind {
    let values = shape
        .members
        .iter()
        .map(|member| {
            // `NAME = value` is shorthand for `@enumValue(value)`.
            let written = member
                .get_trait(&prelude("enumValue"))
                .map(|t| &t.value)
                .or(member.default.as_ref());
            let literal = match (shape.kind, written) {
                (ShapeKind::IntEnum, Some(value)) => literal(value),
                (ShapeKind::IntEnum, None) => None,
                (_, Some(value)) => value.as_str().map(Literal::string),
                (_, None) => Some(Literal::string(member.name.clone())),
            }
            .unwrap_or_else(|| Literal::string(member.name.clone()));
            let mut value = EnumValue::new(literal);
            value.name = Some(member.name.clone());
            value.meta = member_meta(member);
            value.origin = Some(member.span);
            value
        })
        .collect();
    TypeKind::Enum(EnumType {
        values,
        open: false,
        retired: Retired::default(),
        writer_exact: false,
    })
}

/// Documentation (a doc comment or `@documentation`) and deprecation.
pub(crate) fn meta(doc: Option<&String>, shape: &RShape) -> Meta {
    Meta {
        description: doc.cloned().or_else(|| {
            shape
                .get_trait(&prelude("documentation"))
                .and_then(|t| t.value.as_str())
                .map(str::to_owned)
        }),
        deprecated: shape.has_trait("deprecated"),
        ..Meta::default()
    }
}

fn member_meta(member: &RMember) -> Meta {
    Meta {
        description: member.doc.clone().or_else(|| {
            member
                .get_trait(&prelude("documentation"))
                .and_then(|t| t.value.as_str())
                .map(str::to_owned)
        }),
        deprecated: member.has_trait("deprecated"),
        ..Meta::default()
    }
}

/// A node value as a literal, where it is one.
pub(crate) fn literal(node: &Node) -> Option<Literal> {
    Some(match &node.value {
        NodeValue::Null => Literal::Null,
        NodeValue::Bool(b) => Literal::Bool(*b),
        NodeValue::Number(n) => Literal::Number(Number::Finite(Decimal::parse(n).ok()?)),
        NodeValue::String(s) | NodeValue::ShapeId(s) => Literal::string(s.clone()),
        NodeValue::Array(items) => {
            Literal::Array(items.iter().map(literal).collect::<Option<_>>()?)
        }
        NodeValue::Object(members) => Literal::Object(
            members
                .iter()
                .map(|(k, v)| Some((k.text.clone(), literal(v)?)))
                .collect::<Option<_>>()?,
        ),
    })
}
