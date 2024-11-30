//! Checking a JSON Type Definition and lowering it into the IR.
//!
//! | Form | IR |
//! |---|---|
//! | empty (`{}`) | any value |
//! | `type` | boolean, string, `timestamp` as a string with format `date-time`, floats as numbers, integers with their type's range |
//! | `enum` | enum of the strings |
//! | `elements` | array |
//! | `properties`, `optionalProperties` | object, closed unless `additionalProperties` is true |
//! | `values` | object whose members all have one type |
//! | `discriminator` | union whose variants are the mapping's objects, each with the discriminator fixed to its value |
//! | `ref` | the named definition |
//!
//! `nullable: true` makes any form also accept `null`. A `description` in
//! `metadata` is kept as documentation.

use std::collections::{HashMap, HashSet};

use faultline_doc::{Decimal, Node, Object};
use faultline_ir::{
    Api, ApiFormat, ArrayType, Bound, Discriminator, EnumType, EnumValue, Field, Literal,
    NumericType, ObjectType, QualName, Retired, Role, SchemaRoot, StringType, TypeId, TypeKind,
    TypeNode, UnionMode, UnionType,
};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;

/// Schemas nest at most this deep.
const MAX_DEPTH: usize = 64;

/// The keywords every form may use.
const SHARED: [&str; 3] = ["nullable", "metadata", "definitions"];

pub(crate) struct Lowerer<'d> {
    pub(crate) api: Api,
    pub(crate) diagnostics: Diagnostics,
    definitions: HashMap<String, &'d Node>,
    ids: HashMap<String, TypeId>,
}

impl<'d> Lowerer<'d> {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            api: Api::new(name, ApiFormat::JsonTypeDefinition),
            diagnostics: Diagnostics::new(),
            definitions: HashMap::new(),
            ids: HashMap::new(),
        }
    }

    fn error(&mut self, code: faultline_source::DiagnosticCode, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    fn alloc(&mut self, kind: TypeKind, span: Span) -> TypeId {
        self.api.types.alloc(TypeNode::new(kind).with_origin(span))
    }

    /// Lowers the root schema, and every definition, used or not.
    pub(crate) fn root(&mut self, root: &'d Node, role: Role) {
        let Some(object) = root.as_object() else {
            self.error(
                codes::INVALID_SCHEMA,
                "a schema is a JSON object".into(),
                root.span,
            );
            return;
        };
        if let Some(definitions) = object.get("definitions") {
            match definitions.as_object() {
                Some(definitions) => {
                    for (name, schema) in definitions.iter() {
                        self.definitions.insert(name.to_owned(), schema);
                    }
                }
                None => self.error(
                    codes::INVALID_SCHEMA,
                    "`definitions` is an object of schemas".into(),
                    definitions.span,
                ),
            }
        }
        let ty = self.schema(root, 0);
        self.api.schemas.push(SchemaRoot {
            name: self.api.name.clone(),
            ty,
            role,
            origin: Some(root.span),
        });
        let mut names: Vec<String> = self.definitions.keys().cloned().collect();
        names.sort();
        for name in names {
            let span = self.definitions[&name].span;
            self.definition(&name, span);
        }
    }

    /// The type of the definition `name`, lowering it on first use.
    fn definition(&mut self, name: &str, at: Span) -> TypeId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let Some(&schema) = self.definitions.get(name) else {
            self.error(
                codes::UNKNOWN_DEFINITION,
                format!("no definition is named {name:?}"),
                at,
            );
            return self.alloc(TypeKind::Any, at);
        };
        if let Some(cycle) = self.ref_cycle(name) {
            self.error(
                codes::CYCLE,
                format!("definition {name:?} only refers to itself ({cycle})"),
                schema.span,
            );
            let id = self.alloc(TypeKind::Any, schema.span);
            self.ids.insert(name.to_owned(), id);
            return id;
        }
        let id = self.api.types.reserve();
        self.ids.insert(name.to_owned(), id);
        self.api.named.insert(QualName::new(name), id);
        let lowered = self.schema(schema, 1);
        let node = self.api.types.get(lowered).clone();
        self.api
            .types
            .define(id, node.with_name(QualName::new(name)));
        id
    }

    /// The chain of definitions `name` leads back to itself through, if it
    /// is only a `ref` to a `ref` to ... itself, which has no type. A
    /// definition that refers to itself inside a property or an array is
    /// fine: those are values that can end.
    fn ref_cycle(&self, name: &str) -> Option<String> {
        let mut chain = vec![name];
        let mut current = name;
        loop {
            let schema = self.definitions.get(current)?.as_object()?;
            let next = schema.get("ref")?.as_str()?;
            if next == name {
                chain.push(next);
                return Some(chain.join(" -> "));
            }
            if chain.contains(&next) {
                // A cycle that `name` leads into; reported at its members.
                return None;
            }
            chain.push(next);
            current = next;
        }
    }

    fn schema(&mut self, node: &'d Node, depth: usize) -> TypeId {
        if depth > MAX_DEPTH {
            self.error(
                codes::INVALID_SCHEMA,
                "schemas nest too deeply".into(),
                node.span,
            );
            return self.alloc(TypeKind::Any, node.span);
        }
        let Some(object) = node.as_object() else {
            self.error(
                codes::INVALID_SCHEMA,
                "a schema is a JSON object".into(),
                node.span,
            );
            return self.alloc(TypeKind::Any, node.span);
        };
        if depth > 0
            && let Some(definitions) = object.member("definitions")
        {
            self.error(
                codes::INVALID_SCHEMA,
                "only the root schema may have `definitions`".into(),
                definitions.key_span,
            );
        }
        let nullable = match object.get("nullable") {
            None => false,
            Some(n) => n.as_bool().unwrap_or_else(|| {
                self.error(
                    codes::INVALID_SCHEMA,
                    "`nullable` is a boolean".into(),
                    n.span,
                );
                false
            }),
        };
        let ty = match self.form(object, node.span) {
            Some(form) => self.lower_form(form, object, node.span, depth),
            None => self.alloc(TypeKind::Any, node.span),
        };
        let ty = if nullable {
            let null = self.alloc(TypeKind::Null, node.span);
            self.alloc(
                TypeKind::Union(UnionType {
                    mode: UnionMode::Any,
                    variants: vec![ty, null],
                    discriminator: None,
                }),
                node.span,
            )
        } else {
            ty
        };
        if let Some(description) = metadata_description(object) {
            let node = self.api.types.get_mut(ty);
            if node.name.is_none() {
                node.meta.description = Some(description);
            }
        }
        ty
    }

    /// Which form the keywords of `object` make up, checking that they
    /// make up exactly one.
    fn form(&mut self, object: &Object, span: Span) -> Option<Form> {
        let mut forms = Vec::new();
        for (key, member) in object.members() {
            let form = match key {
                "ref" => Form::Ref,
                "type" => Form::Type,
                "enum" => Form::Enum,
                "elements" => Form::Elements,
                "properties" | "optionalProperties" | "additionalProperties" => Form::Properties,
                "values" => Form::Values,
                "discriminator" | "mapping" => Form::Discriminator,
                _ if SHARED.contains(&key) => continue,
                _ => {
                    self.error(
                        codes::INVALID_SCHEMA,
                        format!("`{key}` is not a JSON Type Definition keyword"),
                        member.key_span,
                    );
                    return None;
                }
            };
            if !forms.contains(&form) {
                forms.push(form);
            }
        }
        match forms.as_slice() {
            [] => Some(Form::Empty),
            [form] => Some(*form),
            _ => {
                self.error(
                    codes::INVALID_SCHEMA,
                    "a schema has the keywords of exactly one form".into(),
                    span,
                );
                None
            }
        }
    }

    fn lower_form(&mut self, form: Form, object: &'d Object, span: Span, depth: usize) -> TypeId {
        match form {
            Form::Empty => self.alloc(TypeKind::Any, span),
            Form::Ref => {
                if let Some(name) = object.get("ref").expect("the ref form").as_str() {
                    let id = self.definition(name, span);
                    self.alloc(TypeKind::Ref(id), span)
                } else {
                    self.error(codes::INVALID_SCHEMA, "`ref` is a string".into(), span);
                    self.alloc(TypeKind::Any, span)
                }
            }
            Form::Type => {
                let node = object.get("type").expect("the type form");
                let kind = node.as_str().and_then(scalar).unwrap_or_else(|| {
                    self.error(
                        codes::INVALID_SCHEMA,
                        "`type` is one of boolean, string, timestamp, float32, float64, int8, uint8, int16, uint16, int32 and uint32".into(),
                        node.span,
                    );
                    TypeKind::Any
                });
                self.alloc(kind, node.span)
            }
            Form::Enum => self.enumeration(object.get("enum").expect("the enum form"), span),
            Form::Elements => {
                let items = self.schema(
                    object.get("elements").expect("the elements form"),
                    depth + 1,
                );
                self.alloc(TypeKind::Array(ArrayType::of(items)), span)
            }
            Form::Properties => {
                let kind = self.properties(object, span, depth);
                self.alloc(kind, span)
            }
            Form::Values => {
                let values = self.schema(object.get("values").expect("the values form"), depth + 1);
                self.alloc(TypeKind::Object(ObjectType::new(values)), span)
            }
            Form::Discriminator => self.discriminator(object, span, depth),
        }
    }

    fn enumeration(&mut self, node: &Node, span: Span) -> TypeId {
        let Some(items) = node.as_array().filter(|items| !items.is_empty()) else {
            self.error(
                codes::INVALID_SCHEMA,
                "`enum` is a non-empty array of strings".into(),
                node.span,
            );
            return self.alloc(TypeKind::Any, span);
        };
        let mut seen = HashSet::new();
        let mut values = Vec::new();
        for item in items {
            let Some(text) = item.as_str() else {
                self.error(
                    codes::INVALID_SCHEMA,
                    "enum values are strings".into(),
                    item.span,
                );
                continue;
            };
            if !seen.insert(text) {
                self.error(
                    codes::DUPLICATE,
                    format!("{text:?} is listed twice"),
                    item.span,
                );
                continue;
            }
            let mut value = EnumValue::new(Literal::string(text));
            value.origin = Some(item.span);
            values.push(value);
        }
        self.alloc(
            TypeKind::Enum(EnumType {
                values,
                open: false,
                retired: Retired::default(),
                writer_exact: false,
            }),
            span,
        )
    }

    fn properties(&mut self, object: &'d Object, span: Span, depth: usize) -> TypeKind {
        let additional = match object.get("additionalProperties") {
            None => false,
            Some(node) => node.as_bool().unwrap_or_else(|| {
                self.error(
                    codes::INVALID_SCHEMA,
                    "`additionalProperties` is a boolean".into(),
                    node.span,
                );
                false
            }),
        };
        let others = self.alloc(
            if additional {
                TypeKind::Any
            } else {
                TypeKind::Never
            },
            span,
        );
        let mut lowered = ObjectType::new(others);
        let mut names = HashSet::new();
        for (key, required) in [("properties", true), ("optionalProperties", false)] {
            let Some(members) = object.get(key) else {
                continue;
            };
            let Some(members) = members.as_object() else {
                self.error(
                    codes::INVALID_SCHEMA,
                    format!("`{key}` is an object of schemas"),
                    members.span,
                );
                continue;
            };
            for (name, member) in members.members() {
                if !names.insert(name) {
                    self.error(
                        codes::DUPLICATE,
                        format!("{name:?} is both a required and an optional property"),
                        member.key_span,
                    );
                    continue;
                }
                let ty = self.schema(&member.value, depth + 1);
                let mut field = Field::new(name, ty, required);
                field.origin = Some(member.key_span);
                if let Some(description) = member.value.as_object().and_then(metadata_description) {
                    field.meta.description = Some(description);
                }
                lowered.fields.push(field);
            }
        }
        TypeKind::Object(lowered)
    }

    fn discriminator(&mut self, object: &'d Object, span: Span, depth: usize) -> TypeId {
        let (Some(tag), Some(mapping)) = (
            object.get("discriminator").and_then(Node::as_str),
            object.get("mapping").and_then(Node::as_object),
        ) else {
            self.error(
                codes::INVALID_SCHEMA,
                "a discriminator has a `discriminator` string and a `mapping` object".into(),
                span,
            );
            return self.alloc(TypeKind::Any, span);
        };
        let mut variants = Vec::new();
        let mut table = Vec::new();
        for (value, member) in mapping.members() {
            let Some(variant) = self.variant(tag, value, &member.value, depth) else {
                continue;
            };
            variants.push(variant);
            table.push((value.to_owned(), variant));
        }
        self.alloc(
            TypeKind::Union(UnionType {
                mode: UnionMode::Tagged,
                variants,
                discriminator: Some(Discriminator {
                    property: tag.to_owned(),
                    mapping: table,
                }),
            }),
            span,
        )
    }

    /// One mapping of a discriminator: a properties schema, not nullable
    /// and without the tag, whose object gets the tag fixed to `value`.
    fn variant(&mut self, tag: &str, value: &str, node: &'d Node, depth: usize) -> Option<TypeId> {
        let object = node.as_object().filter(|o| {
            o.get("nullable").and_then(Node::as_bool) != Some(true)
                && ["properties", "optionalProperties"]
                    .iter()
                    .any(|k| o.contains_key(k))
        });
        let Some(object) = object else {
            self.error(
                codes::INVALID_MAPPING,
                format!("the mapping for {value:?} is a properties schema that is not nullable"),
                node.span,
            );
            return None;
        };
        let declares_tag = ["properties", "optionalProperties"].iter().any(|k| {
            object
                .get(k)
                .and_then(Node::as_object)
                .is_some_and(|o| o.contains_key(tag))
        });
        if declares_tag {
            self.error(
                codes::INVALID_MAPPING,
                format!("the mapping for {value:?} declares the discriminator {tag:?} itself"),
                node.span,
            );
            return None;
        }
        let id = self.schema(node, depth + 1);
        let mut constant = EnumValue::new(Literal::string(value));
        constant.origin = Some(node.span);
        let fixed = self.alloc(
            TypeKind::Enum(EnumType {
                values: vec![constant],
                open: false,
                retired: Retired::default(),
                writer_exact: false,
            }),
            node.span,
        );
        if let TypeKind::Object(variant) = &mut self.api.types.get_mut(id).kind {
            let mut field = Field::new(tag, fixed, true);
            field.origin = Some(node.span);
            variant.fields.insert(0, field);
        }
        Some(id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Empty,
    Ref,
    Type,
    Enum,
    Elements,
    Properties,
    Values,
    Discriminator,
}

/// The kind a `type` names.
fn scalar(name: &str) -> Option<TypeKind> {
    let integer = |min: i128, max: i128| {
        TypeKind::Integer(NumericType {
            minimum: Some(Bound::inclusive(Decimal::from_i128(min))),
            maximum: Some(Bound::inclusive(Decimal::from_i128(max))),
            multiple_of: None,
            format: Some(name.to_owned()),
            encoding: None,
        })
    };
    let float = || {
        TypeKind::Number(NumericType {
            format: Some(name.to_owned()),
            ..NumericType::default()
        })
    };
    Some(match name {
        "boolean" => TypeKind::Boolean,
        "string" => TypeKind::String(StringType::default()),
        "timestamp" => TypeKind::String(StringType {
            format: Some("date-time".to_owned()),
            ..StringType::default()
        }),
        "float32" | "float64" => float(),
        "int8" => integer(-128, 127),
        "uint8" => integer(0, 255),
        "int16" => integer(-32_768, 32_767),
        "uint16" => integer(0, 65_535),
        "int32" => integer(i128::from(i32::MIN), i128::from(i32::MAX)),
        "uint32" => integer(0, i128::from(u32::MAX)),
        _ => return None,
    })
}

/// `metadata.description`, the conventional place for documentation.
fn metadata_description(object: &Object) -> Option<String> {
    object
        .get("metadata")?
        .get("description")?
        .as_str()
        .map(str::to_owned)
}
