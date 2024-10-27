//! RAML data types, lowered into the IR.
//!
//! | RAML | IR |
//! |---|---|
//! | `any` | any value |
//! | `string`, with `pattern`, `minLength`, `maxLength` | string |
//! | `number`, `integer`, with `minimum`, `maximum`, `multipleOf`, `format` | number, integer (with the format's range) |
//! | `boolean`, `nil` | boolean, null |
//! | `date-only`, `time-only`, `datetime-only`, `datetime` | string with a format |
//! | `file` | string, format `binary` |
//! | `array`, `T[]`, with `items`, `minItems`, `maxItems`, `uniqueItems` | array |
//! | `object`, with `properties`, `additionalProperties`, `minProperties`, `maxProperties` | object |
//! | `A \| B`, `T?` | union, union with null |
//! | `enum` | enum of the values |
//!
//! Properties are required unless their name ends in `?` or they say
//! `required: false`; a property named `/regex/` is a pattern property.
//! Objects are open unless `additionalProperties: false`. A type that
//! names others in `type` inherits from them: an object gets their
//! properties (its own override them), a scalar their facets.
//!
//! A JSON Schema used as a type, included from a `.json` file or written
//! inline as JSON text, gets a placeholder that
//! [`crate::jsonschema`] later fills in with the JSON Schema front end.

use std::collections::HashMap;

use faultline_doc::{Decimal, Node, Number, UriRef, Value};
use faultline_ir::{
    Api, ArrayType, Bound, EnumType, EnumValue, Field, Literal, NumericType, ObjectType, QualName,
    Retired, StringType, TypeId, TypeKind, TypeNode, UnionMode, UnionType,
};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;
use crate::expr::{self, TypeExpr};

/// The declared types visible from a document: its own, and those of the
/// libraries it uses, by the name it uses them under.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    /// Declarations by name, from `types` (and the older `schemas`).
    pub types: HashMap<String, Node>,
    pub libraries: HashMap<String, Scope>,
    /// The prefix names from this scope get in the IR: `""` for the API,
    /// `"lib."` for a library.
    pub prefix: String,
}

impl Scope {
    /// The declarations of a document's `types` and `schemas`.
    pub fn of(root: &Node, prefix: &str) -> Scope {
        let mut types = HashMap::new();
        for key in ["schemas", "types"] {
            if let Some(object) = root.get(key).and_then(Node::as_object) {
                for (name, node) in object.iter() {
                    types.insert(name.to_owned(), node.clone());
                }
            }
        }
        Scope {
            types,
            libraries: HashMap::new(),
            prefix: prefix.to_owned(),
        }
    }

    /// The scope and declaration a possibly qualified name refers to.
    /// An unqualified name this scope does not declare is looked up in its
    /// libraries: templates borrowed from a library name the library's
    /// types without qualifying them.
    fn lookup<'s>(&'s self, name: &str) -> Option<(&'s Scope, &'s Node, String)> {
        self.qualified(name).or_else(|| {
            let mut libraries: Vec<&String> = self.libraries.keys().collect();
            libraries.sort();
            libraries
                .into_iter()
                .find_map(|library| self.libraries[library].lookup(name))
        })
    }

    fn qualified<'s>(&'s self, name: &str) -> Option<(&'s Scope, &'s Node, String)> {
        if let Some(node) = self.types.get(name) {
            return Some((self, node, format!("{}{name}", self.prefix)));
        }
        let (library, rest) = name.split_once('.')?;
        self.libraries.get(library)?.qualified(rest)
    }
}

/// A JSON Schema used as a type, to be lowered once the RAML types are.
#[derive(Clone, Debug)]
pub struct PendingSchema {
    /// The placeholder the schema's type replaces.
    pub id: TypeId,
    pub source: SchemaSource,
}

#[derive(Clone, Debug)]
pub enum SchemaSource {
    /// A JSON document included with `!include`.
    File(UriRef),
    /// JSON text written in the RAML document, at `span`.
    Text { text: String, span: Span },
}

/// Lowers RAML types into an API's type graph.
pub struct TypeLowerer<'a> {
    pub api: &'a mut Api,
    /// Declared types lowered so far, by qualified name.
    ids: HashMap<String, TypeId>,
    /// The root spans of included JSON documents, from the loader.
    pub json_includes: HashMap<Span, UriRef>,
    /// JSON Schemas met so far, in order.
    pub pending: Vec<PendingSchema>,
    pub diagnostics: Diagnostics,
}

impl std::fmt::Debug for TypeLowerer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeLowerer").finish_non_exhaustive()
    }
}

impl<'a> TypeLowerer<'a> {
    pub fn new(api: &'a mut Api) -> Self {
        Self {
            api,
            ids: HashMap::new(),
            json_includes: HashMap::new(),
            pending: Vec::new(),
            diagnostics: Diagnostics::new(),
        }
    }

    /// A placeholder for a JSON Schema, one per included document.
    fn pending_schema(&mut self, source: SchemaSource, span: Span) -> TypeId {
        if let SchemaSource::File(uri) = &source
            && let Some(existing) = self
                .pending
                .iter()
                .find(|p| matches!(&p.source, SchemaSource::File(u) if u == uri))
        {
            return existing.id;
        }
        let id = self.api.types.reserve();
        // Defined for now, so the arena is complete even if the schema is
        // never lowered.
        self.api
            .types
            .define(id, TypeNode::new(TypeKind::Any).with_origin(span));
        self.pending.push(PendingSchema { id, source });
        id
    }

    /// The placeholder for `node` if it is the root of an included JSON
    /// document.
    fn included_schema(&mut self, node: &Node) -> Option<TypeId> {
        let uri = self.json_includes.get(&node.span)?.clone();
        Some(self.pending_schema(SchemaSource::File(uri), node.span))
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

    /// The type of a declaration: a type expression, or a map of facets.
    pub fn declaration(&mut self, node: &Node, scope: &Scope) -> TypeId {
        if let Some(id) = self.included_schema(node) {
            return id;
        }
        match &node.value {
            Value::String(text) => self.expression_text(text, node.span, scope),
            Value::Object(_) => {
                let kind = self.declared_kind(node, scope);
                let mut type_node = TypeNode::new(kind).with_origin(node.span);
                type_node.meta.description = node
                    .get("description")
                    .and_then(Node::as_str)
                    .map(str::to_owned);
                self.api.types.alloc(type_node)
            }
            // `type: [A, B]` at the top of a declaration is handled by
            // declared_kind; elsewhere an array is malformed.
            Value::Null => self.alloc(TypeKind::Any, node.span),
            _ => {
                self.error(
                    codes::INVALID,
                    "a type must be a type expression or a declaration".into(),
                    node.span,
                );
                self.alloc(TypeKind::Any, node.span)
            }
        }
    }

    fn expression_text(&mut self, text: &str, span: Span, scope: &Scope) -> TypeId {
        let trimmed = text.trim_start();
        if trimmed.starts_with('{') {
            let source = SchemaSource::Text {
                text: text.to_owned(),
                span,
            };
            return self.pending_schema(source, span);
        }
        // An XML Schema written inline is not a RAML type.
        if trimmed.starts_with('<') {
            self.diagnostics.push(
                Diagnostic::warning(
                    "inline XML schemas are compared as accepting any value".to_owned(),
                )
                .with_code(codes::INVALID)
                .with_primary(span, ""),
            );
            return self.alloc(TypeKind::Any, span);
        }
        match expr::parse(text) {
            Ok(expr) => self.expression(&expr, span, scope),
            Err(message) => {
                self.error(codes::TYPE_EXPRESSION, message, span);
                self.alloc(TypeKind::Any, span)
            }
        }
    }

    fn expression(&mut self, expr: &TypeExpr, span: Span, scope: &Scope) -> TypeId {
        match expr {
            TypeExpr::Name(name) => self.named(name, span, scope),
            TypeExpr::Array(item) => {
                let item = self.expression(item, span, scope);
                self.alloc(TypeKind::Array(ArrayType::of(item)), span)
            }
            TypeExpr::Union(members) => {
                let variants = members
                    .iter()
                    .map(|m| self.expression(m, span, scope))
                    .collect();
                self.alloc(
                    TypeKind::Union(UnionType {
                        mode: UnionMode::Any,
                        variants,
                        discriminator: None,
                    }),
                    span,
                )
            }
            TypeExpr::Nullable(inner) => {
                let inner = self.expression(inner, span, scope);
                let null = self.alloc(TypeKind::Null, span);
                self.alloc(
                    TypeKind::Union(UnionType {
                        mode: UnionMode::Any,
                        variants: vec![inner, null],
                        discriminator: None,
                    }),
                    span,
                )
            }
        }
    }

    /// A built-in type, or a declared one lowered on first use. Declared
    /// types are reserved before their bodies are lowered, so recursive
    /// types refer to themselves.
    pub fn named(&mut self, name: &str, span: Span, scope: &Scope) -> TypeId {
        if let Some(kind) = self.built_in(name, span) {
            return self.alloc(kind, span);
        }
        let Some((found, declaration, qualified)) = scope.lookup(name) else {
            self.error(
                codes::UNRESOLVED,
                format!("type {name} is not declared"),
                span,
            );
            return self.alloc(TypeKind::Any, span);
        };
        if let Some(&id) = self.ids.get(&qualified) {
            return id;
        }
        let id = self.api.types.reserve();
        self.ids.insert(qualified.clone(), id);
        self.api.named.insert(QualName::new(&qualified), id);
        let (found, declaration) = (found.clone(), declaration.clone());
        let kind = match &declaration.value {
            _ if self.json_includes.contains_key(&declaration.span) => {
                TypeKind::Ref(self.declaration(&declaration, &found))
            }
            Value::String(text) if text.trim_start().starts_with('{') => {
                TypeKind::Ref(self.expression_text(text, declaration.span, &found))
            }
            Value::String(text) => match expr::parse(text) {
                Ok(expr) => TypeKind::Ref(self.expression(&expr, declaration.span, &found)),
                Err(message) => {
                    self.error(codes::TYPE_EXPRESSION, message, declaration.span);
                    TypeKind::Any
                }
            },
            _ => self.declared_kind(&declaration, &found),
        };
        let mut node = TypeNode::new(kind).with_origin(declaration.span);
        node.meta.description = declaration
            .get("description")
            .and_then(Node::as_str)
            .map(str::to_owned);
        self.api
            .types
            .define(id, node.with_name(QualName::new(&qualified)));
        id
    }

    fn built_in(&mut self, name: &str, span: Span) -> Option<TypeKind> {
        let string = |format: Option<&str>| {
            TypeKind::String(StringType {
                format: format.map(str::to_owned),
                ..StringType::default()
            })
        };
        Some(match name {
            "any" | "union" => TypeKind::Any,
            "object" => {
                let any = self.alloc(TypeKind::Any, span);
                TypeKind::Object(ObjectType::new(any))
            }
            "array" => {
                let any = self.alloc(TypeKind::Any, span);
                TypeKind::Array(ArrayType::of(any))
            }
            "string" => string(None),
            "number" => TypeKind::Number(NumericType::default()),
            "integer" => TypeKind::Integer(NumericType::default()),
            "boolean" => TypeKind::Boolean,
            "nil" => TypeKind::Null,
            "date-only" => string(Some("date")),
            "time-only" => string(Some("time")),
            "datetime-only" => string(Some("datetime-only")),
            "datetime" => string(Some("date-time")),
            "file" => string(Some("binary")),
            _ => return None,
        })
    }

    /// The kind a declaration with facets describes.
    fn declared_kind(&mut self, node: &Node, scope: &Scope) -> TypeKind {
        let bases: Vec<&Node> = match node.get("type").or_else(|| node.get("schema")) {
            Some(Node {
                value: Value::Array(items),
                ..
            }) => items.iter().collect(),
            Some(base) => vec![base],
            None => Vec::new(),
        };
        // Without `type`, the facets say what the type is.
        let implied = if !bases.is_empty() {
            None
        } else if node.get("properties").is_some() {
            Some("object")
        } else if node.get("items").is_some() {
            Some("array")
        } else if ["pattern", "minLength", "maxLength"]
            .iter()
            .any(|k| node.get(k).is_some())
        {
            Some("string")
        } else if ["minimum", "maximum", "multipleOf", "format"]
            .iter()
            .any(|k| node.get(k).is_some())
        {
            Some("number")
        } else if node.get("enum").is_some() {
            Some("string")
        } else {
            Some("any")
        };
        let base_ids: Vec<TypeId> = match implied {
            Some(name) => vec![self.named(name, node.span, scope)],
            None => bases
                .iter()
                .map(|b| match &b.value {
                    Value::String(text) => self.expression_text(text, b.span, scope),
                    _ => self.declaration(b, scope),
                })
                .collect(),
        };
        // A JSON Schema's type is only known once it is lowered: a type
        // based on one is that schema, as RAML allows no facets on it.
        let schemas = base_ids.iter().filter(|&&id| self.is_schema(id)).count();
        if schemas > 0 {
            if base_ids.len() > 1 {
                self.error(
                    codes::INVALID,
                    "a JSON Schema cannot be combined with other types".into(),
                    node.span,
                );
                return TypeKind::Any;
            }
            return TypeKind::Ref(base_ids[0]);
        }
        let mut kind = self.merged_base(&base_ids, node.span);
        self.apply_facets(&mut kind, node, scope);
        kind
    }

    /// Whether `id` is, or refers to, a JSON Schema's placeholder.
    fn is_schema(&self, mut id: TypeId) -> bool {
        // `resolve` would stop at a named type still being defined.
        for _ in 0..self.api.types.len() {
            if self.pending.iter().any(|p| p.id == id) {
                return true;
            }
            match self.api.types.try_get(id).map(|n| &n.kind) {
                Some(TypeKind::Ref(target)) => id = *target,
                _ => return false,
            }
        }
        false
    }

    /// The kind several bases describe together: their object members
    /// combined, or the first base's kind.
    fn merged_base(&mut self, bases: &[TypeId], span: Span) -> TypeKind {
        let mut kind: Option<TypeKind> = None;
        for &base in bases {
            let resolved = self.api.types.resolve(base);
            let Some(node) = self.api.types.try_get(resolved) else {
                // A base that is being defined (a type inheriting from
                // itself) contributes nothing.
                self.error(codes::INVALID, "a type inherits from itself".into(), span);
                continue;
            };
            let base_kind = node.kind.clone();
            kind = Some(match (kind.take(), base_kind) {
                (Some(TypeKind::Object(mut merged)), TypeKind::Object(more)) => {
                    for field in more.fields {
                        if merged.field(&field.name).is_none() {
                            merged.fields.push(field);
                        }
                    }
                    merged.pattern_properties.extend(more.pattern_properties);
                    TypeKind::Object(merged)
                }
                (None, base_kind) => base_kind,
                (Some(first), _) => first,
            });
        }
        kind.unwrap_or(TypeKind::Any)
    }

    fn apply_facets(&mut self, kind: &mut TypeKind, node: &Node, scope: &Scope) {
        if let Some(values) = node.get("enum").and_then(Node::as_array) {
            let values = values
                .iter()
                .map(|v| {
                    let mut value = EnumValue::new(Literal::from(v));
                    value.origin = Some(v.span);
                    value
                })
                .collect();
            *kind = TypeKind::Enum(EnumType {
                values,
                open: false,
                retired: Retired::default(),
                writer_exact: false,
            });
            return;
        }
        match kind {
            TypeKind::String(string) => {
                if let Some(pattern) = node.get("pattern").and_then(Node::as_str) {
                    string.pattern = Some(pattern.to_owned());
                }
                if let Some(n) = self.count(node, "minLength") {
                    string.min_length = n;
                }
                if let Some(n) = self.count(node, "maxLength") {
                    string.max_length = Some(n);
                }
            }
            TypeKind::Number(numeric) | TypeKind::Integer(numeric) => {
                if let Some(d) = self.decimal(node, "minimum") {
                    numeric.minimum = Some(Bound::inclusive(d));
                }
                if let Some(d) = self.decimal(node, "maximum") {
                    numeric.maximum = Some(Bound::inclusive(d));
                }
                if let Some(d) = self.decimal(node, "multipleOf") {
                    numeric.multiple_of = Some(d);
                }
                if let Some(format) = node.get("format").and_then(Node::as_str) {
                    apply_format(numeric, format);
                }
            }
            TypeKind::Array(array) => {
                if let Some(items) = node.get("items") {
                    array.items = self.declaration(items, scope);
                }
                if let Some(n) = self.count(node, "minItems") {
                    array.min_items = n;
                }
                if let Some(n) = self.count(node, "maxItems") {
                    array.max_items = Some(n);
                }
                if node.get("uniqueItems").and_then(Node::as_bool) == Some(true) {
                    array.unique_items = true;
                }
            }
            TypeKind::Object(object) => self.object_facets(object, node, scope),
            _ => {}
        }
    }

    fn object_facets(&mut self, object: &mut ObjectType, node: &Node, scope: &Scope) {
        if let Some(properties) = node.get("properties").and_then(Node::as_object) {
            for (key, value) in properties.iter() {
                // `/regex/` declares a pattern property.
                if key.len() >= 2 && key.starts_with('/') && key.ends_with('/') {
                    let ty = self.declaration(value, scope);
                    object
                        .pattern_properties
                        .push((key[1..key.len() - 1].to_owned(), ty));
                    continue;
                }
                let (name, mut required) = match key.strip_suffix('?') {
                    Some(name) => (name, false),
                    None => (key, true),
                };
                if let Some(explicit) = value.get("required").and_then(Node::as_bool) {
                    required = explicit;
                }
                let ty = self.declaration(value, scope);
                let mut field = Field::new(name, ty, required);
                field.origin = Some(value.span);
                field.meta.description = value
                    .get("description")
                    .and_then(Node::as_str)
                    .map(str::to_owned);
                object.fields.retain(|f| f.name != name);
                object.fields.push(field);
            }
        }
        match node.get("additionalProperties").and_then(Node::as_bool) {
            Some(false) => object.additional = self.alloc(TypeKind::Never, node.span),
            Some(true) => object.additional = self.alloc(TypeKind::Any, node.span),
            None => {}
        }
        if let Some(n) = self.count(node, "minProperties") {
            object.min_properties = n;
        }
        if let Some(n) = self.count(node, "maxProperties") {
            object.max_properties = Some(n);
        }
    }

    fn count(&mut self, node: &Node, key: &str) -> Option<u64> {
        let value = node.get(key)?;
        let n = match &value.value {
            Value::Number(Number::Finite(d)) => d.to_u64(),
            _ => None,
        };
        if n.is_none() {
            self.error(
                codes::INVALID,
                format!("{key} must be a non-negative integer"),
                value.span,
            );
        }
        n
    }

    fn decimal(&mut self, node: &Node, key: &str) -> Option<Decimal> {
        let value = node.get(key)?;
        if let Value::Number(Number::Finite(d)) = &value.value {
            return Some(d.clone());
        }
        self.error(
            codes::INVALID,
            format!("{key} must be a number"),
            value.span,
        );
        None
    }
}

/// The range a numeric `format` implies.
fn apply_format(numeric: &mut NumericType, format: &str) {
    let range = |bits: u32, signed: bool| -> (i128, i128) {
        if signed {
            (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
        } else {
            (0, (1i128 << bits) - 1)
        }
    };
    let (min, max) = match format {
        "int8" => range(8, true),
        "int16" => range(16, true),
        "int32" | "int" => range(32, true),
        "int64" | "long" => range(64, true),
        _ => {
            numeric.format = Some(format.to_owned());
            return;
        }
    };
    numeric.format = Some(format.to_owned());
    if numeric.minimum.is_none() {
        numeric.minimum = Some(Bound::inclusive(Decimal::from_i128(min)));
    }
    if numeric.maximum.is_none() {
        numeric.maximum = Some(Bound::inclusive(Decimal::from_i128(max)));
    }
}
