//! Lowering of assertion and annotation keywords: `type` and the keywords
//! that constrain one JSON type, `enum`/`const`, and metadata.
//!
//! JSON Schema keywords constrain a value *only if it has the keyword's type*:
//! `{"maxLength": 3}` accepts every number. The typed part of a schema is
//! therefore a union over the allowed JSON types, each carrying the keywords
//! that apply to it; with no `type` keyword, all types are allowed.

use faultline_doc::{Decimal, Node, Number, Value};
use faultline_ir::{
    Access, ArrayType, Bound, Contains, EnumType, EnumValue, Field, Literal, Meta, NumericType,
    ObjectType, StringType, TypeId, TypeKind, UnionMode, UnionType,
};
use faultline_source::Diagnostic;

use crate::codes;
use crate::lower::{Lowerer, SchemaCtx};

/// A set of JSON Schema type names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TypeSet(u8);

impl TypeSet {
    pub(crate) const NULL: TypeSet = TypeSet(1);
    pub(crate) const BOOLEAN: TypeSet = TypeSet(2);
    pub(crate) const OBJECT: TypeSet = TypeSet(4);
    pub(crate) const ARRAY: TypeSet = TypeSet(8);
    pub(crate) const NUMBER: TypeSet = TypeSet(16);
    pub(crate) const INTEGER: TypeSet = TypeSet(32);
    pub(crate) const STRING: TypeSet = TypeSet(64);
    pub(crate) const ALL: TypeSet = TypeSet(0b101_1111);
    const EMPTY: TypeSet = TypeSet(0);

    fn from_name(name: &str) -> Option<TypeSet> {
        Some(match name {
            "null" => Self::NULL,
            "boolean" => Self::BOOLEAN,
            "object" => Self::OBJECT,
            "array" => Self::ARRAY,
            "number" => Self::NUMBER,
            "integer" => Self::INTEGER,
            "string" => Self::STRING,
            _ => return None,
        })
    }

    pub(crate) fn contains(self, other: TypeSet) -> bool {
        self.0 & other.0 != 0
    }

    fn with(self, other: TypeSet) -> TypeSet {
        TypeSet(self.0 | other.0)
    }

    /// Whether a literal value has one of the types in the set.
    pub(crate) fn admits(self, literal: &Literal) -> bool {
        match literal {
            Literal::Null => self.contains(Self::NULL),
            Literal::Bool(_) => self.contains(Self::BOOLEAN),
            Literal::Number(Number::Finite(d)) if d.is_integer() => {
                self.contains(Self::NUMBER) || self.contains(Self::INTEGER)
            }
            Literal::Number(_) => self.contains(Self::NUMBER),
            Literal::String(_) => self.contains(Self::STRING),
            Literal::Array(_) => self.contains(Self::ARRAY),
            Literal::Object(_) => self.contains(Self::OBJECT),
        }
    }
}

/// The typed part of a schema.
pub(crate) struct Typed {
    pub(crate) kind: TypeKind,
    pub(crate) types: TypeSet,
    /// Whether any type-specific keyword narrows the allowed types further.
    pub(crate) constrained: bool,
}

const NUMERIC_KEYWORDS: [&str; 5] = [
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
];
const STRING_KEYWORDS: [&str; 3] = ["minLength", "maxLength", "pattern"];
const ARRAY_KEYWORDS: [&str; 9] = [
    "items",
    "prefixItems",
    "additionalItems",
    "minItems",
    "maxItems",
    "uniqueItems",
    "contains",
    "minContains",
    "maxContains",
];
const OBJECT_KEYWORDS: [&str; 8] = [
    "properties",
    "required",
    "additionalProperties",
    "patternProperties",
    "propertyNames",
    "minProperties",
    "maxProperties",
    "dependentRequired",
];

/// `format` values that describe numbers rather than strings (OpenAPI data
/// type formats).
const NUMERIC_FORMATS: [&str; 11] = [
    "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "float", "double",
    "decimal",
];

impl Lowerer<'_, '_> {
    pub(crate) fn meta(&mut self, ctx: &SchemaCtx<'_>) -> Meta {
        let mut meta = Meta {
            title: self.read_string(ctx, "title"),
            description: self.read_string(ctx, "description"),
            deprecated: self.read_bool(ctx, "deprecated").unwrap_or(false),
            ..Meta::default()
        };
        let read_only = self.read_bool(ctx, "readOnly").unwrap_or(false);
        let write_only = self.read_bool(ctx, "writeOnly").unwrap_or(false);
        meta.access = match (read_only, write_only) {
            (true, false) => Access::ReadOnly,
            (false, true) => Access::WriteOnly,
            (true, true) => {
                self.invalid(
                    ctx,
                    "writeOnly",
                    "a schema cannot be both readOnly and writeOnly",
                );
                Access::ReadWrite
            }
            (false, false) => Access::ReadWrite,
        };
        meta.default = ctx.get("default").map(Literal::from);
        if let Some(obj) = ctx.node.as_object() {
            for (key, value) in obj.iter() {
                if key.starts_with("x-") {
                    meta.extensions.insert(key.to_owned(), Literal::from(value));
                }
            }
        }
        meta
    }

    /// Lowers `type` and the type-specific keywords. Returns `None` when the
    /// schema places no constraint on the type of values.
    pub(crate) fn typed_part(&mut self, ctx: &SchemaCtx<'_>) -> Option<Typed> {
        let mut declared = self.type_keyword(ctx);
        // OpenAPI 3.0: `nullable` adds null to the *declared* types and has
        // no effect without `type` (OpenAPI 3.0.3, Schema Object).
        if ctx.dialect().has_nullable() && self.read_bool(ctx, "nullable") == Some(true) {
            declared = declared.map(|types| types.with(TypeSet::NULL));
        }
        let format = ctx.get("format").and_then(Node::as_str);
        let numeric_format = format.is_some_and(|f| NUMERIC_FORMATS.contains(&f));
        let has = |keywords: &[&str]| keywords.iter().any(|k| ctx.get(k).is_some());
        let dependencies_array = ctx
            .get("dependencies")
            .and_then(Node::as_object)
            .is_some_and(|d| d.values().any(|v| v.as_array().is_some()));

        let mut constrained_types = TypeSet::EMPTY;
        if has(&NUMERIC_KEYWORDS) || numeric_format {
            constrained_types = constrained_types
                .with(TypeSet::NUMBER)
                .with(TypeSet::INTEGER);
        }
        if has(&STRING_KEYWORDS) || (format.is_some() && !numeric_format) {
            constrained_types = constrained_types.with(TypeSet::STRING);
        }
        if has(&ARRAY_KEYWORDS) {
            constrained_types = constrained_types.with(TypeSet::ARRAY);
        }
        if has(&OBJECT_KEYWORDS) || dependencies_array {
            constrained_types = constrained_types.with(TypeSet::OBJECT);
        }

        let mut types = match declared {
            Some(types) => types,
            None if constrained_types == TypeSet::EMPTY => return None,
            None => TypeSet::ALL,
        };
        // `integer` is a subset of `number`.
        if types.contains(TypeSet::NUMBER) {
            types = TypeSet(types.0 & !TypeSet::INTEGER.0);
        }

        let mut variants = Vec::new();
        if types.contains(TypeSet::NULL) {
            variants.push(TypeKind::Null);
        }
        if types.contains(TypeSet::BOOLEAN) {
            variants.push(TypeKind::Boolean);
        }
        if types.contains(TypeSet::OBJECT) {
            variants.push(TypeKind::Object(self.object(ctx)));
        }
        if types.contains(TypeSet::ARRAY) {
            variants.push(TypeKind::Array(self.array(ctx)));
        }
        if types.contains(TypeSet::INTEGER) || types.contains(TypeSet::NUMBER) {
            let mut numeric = self.numeric(ctx);
            if numeric_format {
                numeric.format = format.map(str::to_owned);
            }
            variants.push(if types.contains(TypeSet::NUMBER) {
                TypeKind::Number(numeric)
            } else {
                TypeKind::Integer(numeric)
            });
        }
        if types.contains(TypeSet::STRING) {
            let mut string = self.string(ctx);
            if !numeric_format {
                string.format = format.map(str::to_owned);
            }
            variants.push(TypeKind::String(string));
        }

        self.check_inapplicable(ctx, types);

        // Numeric keywords mark both numeric types, so this also covers
        // `integer` having been folded into `number` above.
        let constrained = types.contains(constrained_types);
        let kind = if variants.len() == 1 {
            variants.pop().expect("one variant")
        } else {
            let variants = variants.into_iter().map(|k| self.alloc(k)).collect();
            TypeKind::Union(UnionType {
                mode: UnionMode::Any,
                variants,
                discriminator: None,
            })
        };
        Some(Typed {
            kind,
            types,
            constrained,
        })
    }

    /// Reads the scalar keywords of types the schema excludes, only to
    /// report invalid values: an ill-formed keyword makes the schema invalid
    /// whether or not it could ever apply.
    fn check_inapplicable(&mut self, ctx: &SchemaCtx<'_>, types: TypeSet) {
        if !types.contains(TypeSet::NUMBER) && !types.contains(TypeSet::INTEGER) {
            let _ = self.numeric(ctx);
        }
        if !types.contains(TypeSet::STRING) {
            let _ = self.string(ctx);
        }
        if !types.contains(TypeSet::ARRAY) {
            for keyword in ["minItems", "maxItems", "minContains", "maxContains"] {
                let _ = self.read_u64(ctx, keyword);
            }
            let _ = self.read_bool(ctx, "uniqueItems");
        }
        if !types.contains(TypeSet::OBJECT) {
            let _ = self.read_string_list(ctx, "required");
            for keyword in ["minProperties", "maxProperties"] {
                let _ = self.read_u64(ctx, keyword);
            }
        }
    }

    fn type_keyword(&mut self, ctx: &SchemaCtx<'_>) -> Option<TypeSet> {
        let node = ctx.get("type")?;
        let names: Vec<&Node> = match &node.value {
            Value::String(_) => vec![node],
            Value::Array(items) if !items.is_empty() => items.iter().collect(),
            _ => {
                self.invalid(
                    ctx,
                    "type",
                    "must be a type name or a non-empty array of type names",
                );
                return None;
            }
        };
        let mut set = TypeSet::EMPTY;
        for name in names {
            match name.as_str().and_then(TypeSet::from_name) {
                Some(t) => set = set.with(t),
                None => self.diagnostics.push(
                    Diagnostic::error("unknown type name")
                        .with_code(codes::UNKNOWN_TYPE)
                        .with_primary(
                            name.span,
                            "not one of null, boolean, object, array, number, integer, string",
                        ),
                ),
            }
        }
        (set != TypeSet::EMPTY).then_some(set)
    }

    fn numeric(&mut self, ctx: &SchemaCtx<'_>) -> NumericType {
        let mut minimum = self.read_decimal(ctx, "minimum").map(Bound::inclusive);
        let mut maximum = self.read_decimal(ctx, "maximum").map(Bound::inclusive);
        if ctx.dialect().boolean_exclusive_bounds() {
            if self.read_bool(ctx, "exclusiveMinimum") == Some(true)
                && let Some(b) = &mut minimum
            {
                b.exclusive = true;
            }
            if self.read_bool(ctx, "exclusiveMaximum") == Some(true)
                && let Some(b) = &mut maximum
            {
                b.exclusive = true;
            }
        } else {
            // Both keywords may be present; the tighter bound wins.
            if let Some(v) = self.read_decimal(ctx, "exclusiveMinimum")
                && minimum.as_ref().is_none_or(|m| v >= m.value)
            {
                minimum = Some(Bound::exclusive(v));
            }
            if let Some(v) = self.read_decimal(ctx, "exclusiveMaximum")
                && maximum.as_ref().is_none_or(|m| v <= m.value)
            {
                maximum = Some(Bound::exclusive(v));
            }
        }
        let multiple_of = self.read_decimal(ctx, "multipleOf").filter(|m| {
            let positive = !m.is_negative() && !m.is_zero();
            if !positive {
                self.invalid(ctx, "multipleOf", "must be greater than zero");
            }
            positive
        });
        NumericType {
            minimum,
            maximum,
            multiple_of,
            format: None,
            encoding: None,
        }
    }

    fn string(&mut self, ctx: &SchemaCtx<'_>) -> StringType {
        StringType {
            min_length: self.read_u64(ctx, "minLength").unwrap_or(0),
            max_length: self.read_u64(ctx, "maxLength"),
            pattern: self.read_string(ctx, "pattern"),
            format: None,
        }
    }

    fn array(&mut self, ctx: &SchemaCtx<'_>) -> ArrayType {
        let mut prefix_items = Vec::new();
        let items;
        if ctx.dialect().has_prefix_items() {
            if let Some(list) = ctx.get("prefixItems").and_then(Node::as_array) {
                for i in 0..list.len() {
                    prefix_items.push(self.child(ctx, &["prefixItems", &i.to_string()]));
                }
            }
            items = self.optional_child(ctx, "items");
        } else {
            match ctx.get("items").map(|n| &n.value) {
                Some(Value::Array(list)) => {
                    for i in 0..list.len() {
                        prefix_items.push(self.child(ctx, &["items", &i.to_string()]));
                    }
                    items = self.optional_child(ctx, "additionalItems");
                }
                Some(_) => items = self.child(ctx, &["items"]),
                None => items = self.any(),
            }
        }
        let contains = if ctx.dialect().has_draft6_keywords() && ctx.get("contains").is_some() {
            let ty = self.child(ctx, &["contains"]);
            let (min, max) = if ctx.dialect().has_2019_keywords() {
                (
                    self.read_u64(ctx, "minContains").unwrap_or(1),
                    self.read_u64(ctx, "maxContains"),
                )
            } else {
                (1, None)
            };
            Some(Contains { ty, min, max })
        } else {
            None
        };
        ArrayType {
            items,
            prefix_items,
            min_items: self.read_u64(ctx, "minItems").unwrap_or(0),
            max_items: self.read_u64(ctx, "maxItems"),
            unique_items: self.read_bool(ctx, "uniqueItems").unwrap_or(false),
            contains,
        }
    }

    fn object(&mut self, ctx: &SchemaCtx<'_>) -> ObjectType {
        let additional = self.optional_child(ctx, "additionalProperties");
        let required = self.read_string_list(ctx, "required");
        let mut object = ObjectType::new(additional);

        if let Some(properties) = ctx.get("properties").and_then(Node::as_object) {
            for (name, member) in properties.members() {
                let ty = self.child(ctx, &["properties", name]);
                let mut field = Field::new(name, ty, required.iter().any(|(r, _)| r == name));
                field.origin = Some(member.key_span);
                object.fields.push(field);
            }
        } else if let Some(node) = ctx.get("properties") {
            self.invalid_at(
                node,
                "properties",
                "must be an object mapping names to schemas",
            );
        }

        if let Some(patterns) = ctx.get("patternProperties").and_then(Node::as_object) {
            for (pattern, _) in patterns.iter() {
                let ty = self.child(ctx, &["patternProperties", pattern]);
                object.pattern_properties.push((pattern.to_owned(), ty));
            }
        }

        // Required members without a declared schema still have to exist;
        // their type is whatever the object allows for undeclared members.
        // With pattern properties that depends on the name, which would need
        // regular expression matching here, so the type is left open.
        let undeclared = if object.pattern_properties.is_empty() {
            additional
        } else {
            self.any()
        };
        for (name, span) in &required {
            if object.field(name).is_none() {
                let mut field = Field::new(name.clone(), undeclared, true);
                field.origin = Some(*span);
                object.fields.push(field);
            }
        }

        if ctx.dialect().has_draft6_keywords() && ctx.get("propertyNames").is_some() {
            object.property_names = Some(self.child(ctx, &["propertyNames"]));
        }
        object.min_properties = self.read_u64(ctx, "minProperties").unwrap_or(0);
        object.max_properties = self.read_u64(ctx, "maxProperties");

        let dependents = if ctx.dialect().has_2019_keywords() {
            ctx.get("dependentRequired").and_then(Node::as_object)
        } else {
            ctx.get("dependencies").and_then(Node::as_object)
        };
        if let Some(dependents) = dependents {
            for (property, value) in dependents.iter() {
                if let Some(list) = value.as_array() {
                    let names = list
                        .iter()
                        .filter_map(|n| n.as_str().map(str::to_owned))
                        .collect();
                    object.dependent_required.push((property.to_owned(), names));
                }
            }
        }
        object
    }

    /// `enum` and `const` as enum types, one per keyword present.
    pub(crate) fn enum_parts(&mut self, ctx: &SchemaCtx<'_>) -> Vec<EnumType> {
        let mut parts = Vec::new();
        if let Some(node) = ctx.get("enum") {
            if let Some(items) = node.as_array() {
                let mut values: Vec<EnumValue> = Vec::new();
                for item in items {
                    let literal = Literal::from(item);
                    if values.iter().any(|v| v.value == literal) {
                        self.diagnostics.push(
                            Diagnostic::warning("duplicate enum value")
                                .with_code(codes::INVALID_KEYWORD)
                                .with_primary(item.span, "already listed"),
                        );
                        continue;
                    }
                    let mut value = EnumValue::new(literal);
                    value.origin = Some(item.span);
                    values.push(value);
                }
                parts.push(EnumType {
                    values,
                    open: false,
                    retired: faultline_ir::Retired::default(),
                    writer_exact: false,
                });
            } else {
                self.invalid_at(node, "enum", "must be an array");
            }
        }
        if ctx.dialect().has_draft6_keywords()
            && let Some(node) = ctx.get("const")
        {
            let mut value = EnumValue::new(Literal::from(node));
            value.origin = Some(node.span);
            parts.push(EnumType {
                values: vec![value],
                open: false,
                retired: faultline_ir::Retired::default(),
                writer_exact: false,
            });
        }
        parts
    }

    /// The subschema under `keyword`, or `Any` if the keyword is absent.
    pub(crate) fn optional_child(&mut self, ctx: &SchemaCtx<'_>, keyword: &str) -> TypeId {
        if ctx.get(keyword).is_some() {
            self.child(ctx, &[keyword])
        } else {
            self.any()
        }
    }

    fn read_string(&mut self, ctx: &SchemaCtx<'_>, keyword: &str) -> Option<String> {
        let node = ctx.get(keyword)?;
        let value = node.as_str().map(str::to_owned);
        if value.is_none() {
            self.invalid_at(node, keyword, "must be a string");
        }
        value
    }

    pub(crate) fn read_bool(&mut self, ctx: &SchemaCtx<'_>, keyword: &str) -> Option<bool> {
        let node = ctx.get(keyword)?;
        let value = node.as_bool();
        if value.is_none() {
            self.invalid_at(node, keyword, "must be a boolean");
        }
        value
    }

    fn read_decimal(&mut self, ctx: &SchemaCtx<'_>, keyword: &str) -> Option<Decimal> {
        let node = ctx.get(keyword)?;
        let value = node.as_number().and_then(Number::as_decimal).cloned();
        if value.is_none() {
            self.invalid_at(node, keyword, "must be a finite number");
        }
        value
    }

    fn read_u64(&mut self, ctx: &SchemaCtx<'_>, keyword: &str) -> Option<u64> {
        let node = ctx.get(keyword)?;
        let value = node
            .as_number()
            .and_then(Number::as_decimal)
            .and_then(Decimal::to_u64);
        if value.is_none() {
            self.invalid_at(node, keyword, "must be a non-negative integer");
        }
        value
    }

    fn read_string_list(
        &mut self,
        ctx: &SchemaCtx<'_>,
        keyword: &str,
    ) -> Vec<(String, faultline_source::Span)> {
        let Some(node) = ctx.get(keyword) else {
            return Vec::new();
        };
        let Some(items) = node.as_array() else {
            self.invalid_at(node, keyword, "must be an array of strings");
            return Vec::new();
        };
        let mut out: Vec<(String, faultline_source::Span)> = Vec::new();
        for item in items {
            match item.as_str() {
                Some(s) if !out.iter().any(|(existing, _)| existing == s) => {
                    out.push((s.to_owned(), item.span));
                }
                Some(_) => {}
                None => self.invalid_at(item, keyword, "must contain only strings"),
            }
        }
        out
    }

    pub(crate) fn invalid(&mut self, ctx: &SchemaCtx<'_>, keyword: &str, message: &str) {
        if let Some(node) = ctx.get(keyword) {
            self.invalid_at(node, keyword, message);
        }
    }

    fn invalid_at(&mut self, node: &Node, keyword: &str, message: &str) {
        self.diagnostics.push(
            Diagnostic::error(format!("invalid value for \"{keyword}\""))
                .with_code(codes::INVALID_KEYWORD)
                .with_primary(node.span, message.to_owned())
                .with_note("the keyword is ignored"),
        );
    }
}
