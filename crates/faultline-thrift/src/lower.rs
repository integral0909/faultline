//! Lowering linked Thrift files into the IR.
//!
//! | Thrift | IR |
//! |---|---|
//! | `bool` | boolean |
//! | `byte`, `i8`, `i16`, `i32`, `i64` | integer with the type's range |
//! | `double` | number |
//! | `string`, `uuid` | string (`uuid` with that format) |
//! | `binary` | bytes |
//! | `list<T>`, `set<T>` | array (a set's items unique) |
//! | `map<K, V>` | map |
//! | `enum` | enum of the value names, with their numbers |
//! | `struct`, `exception` | object whose fields carry their ids as tags |
//! | `union` | object whose fields exclude each other, one of them set |
//! | `typedef` | the type it names |
//!
//! Numbers and collections carry their wire type as their encoding, so
//! that changing a field from `i32` to `i64`, or from `list` to `set`,
//! which readers of the old type skip, is reported even though the values
//! would still fit.
//!
//! `required` fields are required. Optional fields and fields with the
//! default requiredness are not, as readers accept them missing; writers
//! do send default-requiredness fields, which the comparison does not
//! rely on. Fields without an explicit id are aligned by name.
//!
//! Types defined in the entry file keep their names; types from included
//! files are qualified by the file's prefix, as Thrift writes them.

use std::collections::HashMap;

use faultline_doc::{Decimal, Number};
use faultline_ir::{
    Api, ApiFormat, ArrayType, Bound, EnumType, EnumValue, ExclusiveGroup, Field, Literal, MapType,
    Meta, NumericType, ObjectType, QualName, Retired, Role, SchemaRoot, StringType, TypeId,
    TypeKind, TypeNode,
};

use crate::ast::{
    Annotations, BaseType, ConstValue, Definition, Enum, FieldType, FieldTypeKind, Struct,
    StructKind,
};
use crate::linker::{DefId, Linked};
use crate::load::ThriftFile;

pub(crate) struct Lowerer<'a> {
    pub(crate) files: &'a [ThriftFile],
    pub(crate) linked: &'a Linked,
    pub(crate) api: Api,
    ids: HashMap<DefId, TypeId>,
    pending: Vec<DefId>,
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(files: &'a [ThriftFile], linked: &'a Linked, name: &str) -> Self {
        Self {
            files,
            linked,
            api: Api::new(name, ApiFormat::Thrift),
            ids: HashMap::new(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn lower(mut self, role: Role) -> Api {
        let definitions = &self.files[0].document.definitions;
        let has_services = definitions
            .iter()
            .any(|d| matches!(d, Definition::Service(_)));
        for (index, definition) in definitions.iter().enumerate() {
            let id = DefId { file: 0, index };
            match definition {
                Definition::Struct(s) => {
                    let ty = self.named(id);
                    if !has_services {
                        self.api.schemas.push(SchemaRoot {
                            name: s.name.text.clone(),
                            ty,
                            role,
                            origin: Some(s.name.span),
                        });
                    }
                }
                // Every type the entry file defines is compared, used or
                // not.
                Definition::Enum(_) => {
                    self.named(id);
                }
                Definition::Const(_) | Definition::Typedef(_) | Definition::Service(_) => {}
            }
        }
        self.services();
        self.drain();
        self.api
    }

    /// Defines the named types reserved so far.
    pub(crate) fn drain(&mut self) {
        while let Some(id) = self.pending.pop() {
            self.define(id);
        }
    }

    pub(crate) fn definition(&self, id: DefId) -> &'a Definition {
        &self.files[id.file].document.definitions[id.index]
    }

    /// The name of a definition in the IR.
    pub(crate) fn qualified(&self, id: DefId) -> String {
        let name = &self.definition(id).name().text;
        if id.file == 0 {
            name.clone()
        } else {
            format!("{}.{name}", self.files[id.file].prefix)
        }
    }

    /// The type a struct, union, exception, enum or typedef stands for.
    pub(crate) fn named(&mut self, id: DefId) -> TypeId {
        if let Some(&ty) = self.ids.get(&id) {
            return ty;
        }
        if let Definition::Typedef(typedef) = self.definition(id) {
            // A typedef is only another name: its uses are the type's.
            let ty = self.field_type(id.file, &typedef.ty);
            self.ids.insert(id, ty);
            return ty;
        }
        let ty = self.api.types.reserve();
        self.ids.insert(id, ty);
        self.api.named.insert(QualName::new(self.qualified(id)), ty);
        self.pending.push(id);
        ty
    }

    fn define(&mut self, id: DefId) {
        let ty = self.ids[&id];
        let mut node = match self.definition(id) {
            Definition::Struct(s) => self.structure(id.file, s),
            Definition::Enum(e) => enumeration(e),
            _ => unreachable!("only structs and enums are reserved"),
        };
        node.name = Some(QualName::new(self.qualified(id)));
        self.api.types.define(ty, node);
    }

    fn structure(&mut self, file: usize, s: &Struct) -> TypeNode {
        let any = self.api.types.alloc(TypeNode::new(TypeKind::Any));
        let mut object = ObjectType::new(any);
        for field in &s.fields {
            let ty = self.field_type(file, &field.ty);
            let required = field.requiredness == crate::ast::Requiredness::Required;
            let mut lowered = Field::new(field.name.text.clone(), ty, required);
            lowered.tag = tag(field.id);
            lowered.meta = meta(field.doc.as_ref(), &field.annotations);
            lowered.meta.default = field.default.as_ref().and_then(literal);
            lowered.origin = Some(field.span);
            object.fields.push(lowered);
        }
        if s.kind == StructKind::Union && !s.fields.is_empty() {
            object.exclusive.push(ExclusiveGroup {
                name: s.name.text.clone(),
                fields: s.fields.iter().map(|f| f.name.text.clone()).collect(),
                origin: Some(s.name.span),
            });
            object.min_properties = 1;
        }
        let mut node = TypeNode::new(TypeKind::Object(object)).with_origin(s.name.span);
        node.meta = meta(s.doc.as_ref(), &s.annotations);
        node
    }

    /// The type a field type stands for. Base and container types get a
    /// node per use, each compared on its own.
    pub(crate) fn field_type(&mut self, file: usize, ty: &FieldType) -> TypeId {
        let kind = match &ty.kind {
            FieldTypeKind::Base(base) => base_type(*base),
            FieldTypeKind::Named(name) => {
                let id = self
                    .linked
                    .resolve(file, name)
                    .expect("the linker resolved every type");
                return self.named(id);
            }
            FieldTypeKind::List(item) => TypeKind::Array(ArrayType {
                encoding: Some("list".to_owned()),
                ..ArrayType::of(self.field_type(file, item))
            }),
            FieldTypeKind::Set(item) => TypeKind::Array(ArrayType {
                unique_items: true,
                encoding: Some("set".to_owned()),
                ..ArrayType::of(self.field_type(file, item))
            }),
            FieldTypeKind::Map(key, value) => TypeKind::Map(MapType {
                key: self.field_type(file, key),
                value: self.field_type(file, value),
            }),
        };
        self.api
            .types
            .alloc(TypeNode::new(kind).with_origin(ty.span))
    }
}

fn base_type(base: BaseType) -> TypeKind {
    let integer = |bits: u32| {
        let max = (1i128 << (bits - 1)) - 1;
        TypeKind::Integer(NumericType {
            minimum: Some(Bound::inclusive(Decimal::from_i128(-max - 1))),
            maximum: Some(Bound::inclusive(Decimal::from_i128(max))),
            multiple_of: None,
            format: Some(base.as_str().to_owned()),
            // Each width is its own wire type: a reader expecting another
            // skips the field.
            encoding: Some(base.as_str().to_owned()),
        })
    };
    match base {
        BaseType::Bool => TypeKind::Boolean,
        BaseType::I8 => integer(8),
        BaseType::I16 => integer(16),
        BaseType::I32 => integer(32),
        BaseType::I64 => integer(64),
        BaseType::Double => TypeKind::Number(NumericType {
            format: Some("double".to_owned()),
            encoding: Some("double".to_owned()),
            ..NumericType::default()
        }),
        BaseType::String => TypeKind::String(StringType::default()),
        BaseType::Uuid => TypeKind::String(StringType {
            format: Some("uuid".to_owned()),
            ..StringType::default()
        }),
        BaseType::Binary => TypeKind::Bytes,
    }
}

/// Enum values follow the previous value when not written, starting at 0.
fn enumeration(e: &Enum) -> TypeNode {
    let mut next = 0i64;
    let values = e
        .values
        .iter()
        .map(|v| {
            let number = v.value.map_or(next, |(n, _)| n);
            next = number.saturating_add(1);
            let mut value = EnumValue::new(Literal::string(v.name.text.clone()));
            value.name = Some(v.name.text.clone());
            value.number = Some(number);
            value.meta = meta(v.doc.as_ref(), &v.annotations);
            value.origin = Some(v.name.span);
            value
        })
        .collect();
    let mut node = TypeNode::new(TypeKind::Enum(EnumType {
        values,
        open: false,
        retired: Retired::default(),
        writer_exact: false,
    }))
    .with_origin(e.name.span);
    node.meta = meta(e.doc.as_ref(), &e.annotations);
    node
}

/// A field's tag: its explicit id. Implicit ids are left out, as they
/// change when fields are added or reordered.
pub(crate) fn tag(id: Option<(i64, faultline_source::Span)>) -> Option<u32> {
    id.and_then(|(id, _)| u32::try_from(id).ok())
        .filter(|&id| id > 0)
}

/// Documentation, and deprecation from a `deprecated` annotation.
pub(crate) fn meta(doc: Option<&String>, annotations: &Annotations) -> Meta {
    Meta {
        description: doc.cloned(),
        deprecated: annotations.iter().any(|(key, _)| key.text == "deprecated"),
        ..Meta::default()
    }
}

/// A constant as a literal, where it has one: enum values by name, and
/// maps only with string keys.
fn literal(value: &ConstValue) -> Option<Literal> {
    Some(match value {
        ConstValue::Int(n) => Literal::Number(Number::Finite(Decimal::from_i128(i128::from(*n)))),
        ConstValue::Double(d) => {
            Literal::Number(Number::Finite(Decimal::parse(&d.to_string()).ok()?))
        }
        ConstValue::Str(s) => Literal::string(s.clone()),
        ConstValue::Ident(name) => {
            Literal::string(name.rsplit('.').next().unwrap_or(name).to_owned())
        }
        ConstValue::List(items) => {
            Literal::Array(items.iter().map(literal).collect::<Option<_>>()?)
        }
        ConstValue::Map(entries) => Literal::Object(
            entries
                .iter()
                .map(|(k, v)| match k {
                    ConstValue::Str(k) => Some((k.clone(), literal(v)?)),
                    _ => None,
                })
                .collect::<Option<_>>()?,
        ),
    })
}
