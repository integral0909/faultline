//! Reading and lowering an Avro schema (`.avsc`).
//!
//! Avro compatibility is schema resolution: a reader schema reads data
//! written with a writer schema. The IR describes each schema as a reader
//! sees data, so the roles of the comparison map onto Avro's compatibility
//! modes:
//!
//! * `input` — the new schema reads data written with the old one
//!   (BACKWARD);
//! * `output` — the old schema reads data written with the new one
//!   (FORWARD);
//! * `both` — FULL.
//!
//! As a reader sees them:
//!
//! * a record is an object that ignores fields it does not declare, so it is
//!   open; a field with a default is filled in when missing, so only fields
//!   without defaults are required; aliases let a renamed field match;
//! * an enum with a `default` symbol reads unknown symbols as the default,
//!   so it is open;
//! * `int` and `long` are 32- and 64-bit integers, and `double` a number
//!   (`float` one marked by its format), so the promotions Avro allows
//!   (`int` to `long`, `float` or `double`, and so on) are widenings;
//! * `bytes`, `string` and `fixed` are strings, since Avro resolves `bytes`
//!   and `string` into each other; `fixed` has an exact length;
//! * a union is tagged by its branches; `["null", T]` is `T` or null.

use std::collections::HashMap;

use faultline_doc::{Decimal, Node, Number, Value};
use faultline_ir::{
    Api, ApiFormat, ArrayType, Bound, EnumType, EnumValue, Field, Literal, MapType, NumericType,
    ObjectType, QualName, Retired, Role, SchemaRoot, StringType, TypeId, TypeKind, TypeNode,
    UnionMode, UnionType,
};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;

const PRIMITIVES: [&str; 8] = [
    "null", "boolean", "int", "long", "float", "double", "bytes", "string",
];

/// What a named type is, for checking defaults and union branches.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shape {
    Null,
    Boolean,
    Int,
    Long,
    Float,
    Double,
    Bytes,
    String,
    Record,
    Enum(Vec<String>),
    Fixed,
    Array,
    Map,
    Union(Vec<Shape>),
}

impl Shape {
    /// The key identifying union branches: at most one branch per key.
    fn branch_key(&self, name: Option<&str>) -> String {
        match (self, name) {
            (Shape::Record | Shape::Enum(_) | Shape::Fixed, Some(name)) => name.to_owned(),
            (Shape::Null, _) => "null".into(),
            (Shape::Boolean, _) => "boolean".into(),
            (Shape::Int, _) => "int".into(),
            (Shape::Long, _) => "long".into(),
            (Shape::Float, _) => "float".into(),
            (Shape::Double, _) => "double".into(),
            (Shape::Bytes, _) => "bytes".into(),
            (Shape::String, _) => "string".into(),
            (Shape::Array, _) => "array".into(),
            (Shape::Map, _) => "map".into(),
            (Shape::Union(_), _) => "union".into(),
            _ => "named".into(),
        }
    }
}

/// A lowered type with what default checking needs.
struct Lowered {
    id: TypeId,
    shape: Shape,
    name: Option<String>,
}

pub(crate) struct Lowerer {
    pub(crate) api: Api,
    named: HashMap<String, (TypeId, Shape)>,
    pub(crate) diagnostics: Diagnostics,
}

impl Lowerer {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            api: Api::new(name, ApiFormat::Avro),
            named: HashMap::new(),
            diagnostics: Diagnostics::new(),
        }
    }

    /// Lowers the top-level schema as a root named after its full name.
    pub(crate) fn root(&mut self, node: &Node, role: Role) {
        let Some(lowered) = self.schema(node, "") else {
            return;
        };
        let name = lowered.name.unwrap_or_else(|| self.api.name.clone());
        self.api.schemas.push(SchemaRoot {
            name,
            ty: lowered.id,
            role,
            origin: Some(node.span),
        });
    }

    /// Lowers one schema that another format embeds, returning its type.
    pub(crate) fn embedded(&mut self, node: &Node) -> Option<TypeId> {
        self.schema(node, "").map(|lowered| lowered.id)
    }

    fn error(&mut self, code: faultline_source::DiagnosticCode, span: Span, message: String) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, "here"),
        );
    }

    fn node(&mut self, kind: TypeKind, span: Span) -> TypeId {
        self.api.types.alloc(TypeNode::new(kind).with_origin(span))
    }

    fn schema(&mut self, node: &Node, namespace: &str) -> Option<Lowered> {
        match &node.value {
            Value::String(name) => self.reference(name, namespace, node.span),
            Value::Array(branches) => self.union(branches, namespace, node.span),
            Value::Object(_) => self.complex(node, namespace),
            _ => {
                self.error(
                    codes::INVALID_SCHEMA,
                    node.span,
                    format!(
                        "a schema is a type name, an object or a union, not {}",
                        node.kind().describe()
                    ),
                );
                None
            }
        }
    }

    fn primitive(&mut self, name: &str, span: Span, logical: Option<String>) -> Lowered {
        let integer = |bits: u32| {
            let max = (1i128 << (bits - 1)) - 1;
            NumericType {
                minimum: Some(Bound::inclusive(Decimal::from_i128(-max - 1))),
                maximum: Some(Bound::inclusive(Decimal::from_i128(max))),
                multiple_of: None,
                format: None,
                encoding: None,
            }
        };
        let (kind, shape) = match name {
            "null" => (TypeKind::Null, Shape::Null),
            "boolean" => (TypeKind::Boolean, Shape::Boolean),
            "int" => (TypeKind::Integer(integer(32)), Shape::Int),
            "long" => (TypeKind::Integer(integer(64)), Shape::Long),
            "float" => (
                TypeKind::Number(NumericType {
                    format: Some("float".into()),
                    ..NumericType::default()
                }),
                Shape::Float,
            ),
            // `double` is the plain number; `float` is marked, so narrowing
            // to it (losing precision) is reported.
            "double" => (TypeKind::Number(NumericType::default()), Shape::Double),
            "bytes" => (
                TypeKind::String(StringType {
                    format: Some("bytes".into()),
                    ..StringType::default()
                }),
                Shape::Bytes,
            ),
            _ => (TypeKind::String(StringType::default()), Shape::String),
        };
        let kind = with_logical(kind, logical);
        Lowered {
            id: self.node(kind, span),
            shape,
            name: None,
        }
    }

    fn reference(&mut self, name: &str, namespace: &str, span: Span) -> Option<Lowered> {
        if PRIMITIVES.contains(&name) {
            return Some(self.primitive(name, span, None));
        }
        let full = full_name(name, namespace);
        let found = self
            .named
            .get(&full)
            .or_else(|| self.named.get(name))
            .cloned();
        if let Some((id, shape)) = found {
            return Some(Lowered {
                id,
                shape,
                name: Some(full),
            });
        }
        self.error(
            codes::UNKNOWN_TYPE,
            span,
            format!("unknown type `{name}`; named types must be defined before they are used"),
        );
        None
    }

    fn union(&mut self, branches: &[Node], namespace: &str, span: Span) -> Option<Lowered> {
        let mut variants = Vec::new();
        let mut shapes = Vec::new();
        let mut keys: HashMap<String, Span> = HashMap::new();
        for branch in branches {
            if branch.as_array().is_some() {
                self.error(
                    codes::INVALID_UNION,
                    branch.span,
                    "a union cannot contain another union".into(),
                );
                continue;
            }
            let lowered = self.schema(branch, namespace)?;
            let key = lowered.shape.branch_key(lowered.name.as_deref());
            if let Some(first) = keys.insert(key.clone(), branch.span) {
                self.diagnostics.push(
                    Diagnostic::error(format!("a union has two `{key}` branches"))
                        .with_code(codes::INVALID_UNION)
                        .with_primary(branch.span, "second branch")
                        .with_secondary(first, "first branch"),
                );
            }
            variants.push(lowered.id);
            shapes.push(lowered.shape);
        }
        let id = self.node(
            TypeKind::Union(UnionType {
                mode: UnionMode::Tagged,
                variants,
                discriminator: None,
            }),
            span,
        );
        Some(Lowered {
            id,
            shape: Shape::Union(shapes),
            name: None,
        })
    }

    fn complex(&mut self, node: &Node, namespace: &str) -> Option<Lowered> {
        let Some(ty) = node.get("type") else {
            self.error(
                codes::INVALID_SCHEMA,
                node.span,
                "a schema object needs a `type`".into(),
            );
            return None;
        };
        let logical = node.get("logicalType").and_then(Node::as_str).map(|l| {
            match (l, node.get("precision"), node.get("scale")) {
                ("decimal", Some(p), scale) => format!(
                    "decimal({},{})",
                    p.as_number().map(ToString::to_string).unwrap_or_default(),
                    scale
                        .and_then(Node::as_number)
                        .map_or_else(|| "0".to_owned(), ToString::to_string)
                ),
                _ => l.to_owned(),
            }
        });
        let Some(kind) = ty.as_str() else {
            // `{"type": {...}}` or `{"type": [...]}` wraps another schema.
            return self.schema(ty, namespace);
        };
        match kind {
            "record" | "error" => self.record(node, namespace),
            "enum" => self.enumeration(node, namespace),
            "fixed" => self.fixed(node, namespace, logical),
            "array" => {
                let items = self.required(node, "items")?;
                let items = self.schema(items, namespace)?;
                Some(Lowered {
                    id: self.node(TypeKind::Array(ArrayType::of(items.id)), node.span),
                    shape: Shape::Array,
                    name: None,
                })
            }
            "map" => {
                let values = self.required(node, "values")?;
                let values = self.schema(values, namespace)?;
                let key = self.node(TypeKind::String(StringType::default()), node.span);
                Some(Lowered {
                    id: self.node(
                        TypeKind::Map(MapType {
                            key,
                            value: values.id,
                        }),
                        node.span,
                    ),
                    shape: Shape::Map,
                    name: None,
                })
            }
            name if PRIMITIVES.contains(&name) => {
                if let Some(logical) = &logical {
                    self.check_logical(logical, name, node.span);
                }
                Some(self.primitive(name, node.span, logical))
            }
            name => {
                // A reference to a named type, written as an object.
                self.reference(name, namespace, ty.span)
            }
        }
    }

    fn required<'n>(&mut self, node: &'n Node, key: &str) -> Option<&'n Node> {
        let found = node.get(key);
        if found.is_none() {
            let ty = node.get("type").and_then(Node::as_str).unwrap_or("schema");
            self.error(
                codes::INVALID_SCHEMA,
                node.span,
                format!("{} {ty} schema needs `{key}`", article(ty)),
            );
        }
        found
    }

    fn check_logical(&mut self, logical: &str, underlying: &str, span: Span) {
        let fits = match logical.split('(').next().unwrap_or(logical) {
            "decimal" => matches!(underlying, "bytes" | "fixed"),
            "uuid" => matches!(underlying, "string" | "fixed"),
            "date" | "time-millis" => underlying == "int",
            "time-micros"
            | "timestamp-millis"
            | "timestamp-micros"
            | "timestamp-nanos"
            | "local-timestamp-millis"
            | "local-timestamp-micros"
            | "local-timestamp-nanos" => underlying == "long",
            "duration" => underlying == "fixed",
            // Unknown logical types are ignored by Avro readers.
            _ => true,
        };
        if !fits {
            self.diagnostics.push(
                Diagnostic::warning(format!(
                    "logical type `{logical}` does not apply to `{underlying}`, so readers ignore it"
                ))
                .with_code(codes::LOGICAL_TYPE)
                .with_primary(span, "here"),
            );
        }
    }

    /// Registers a named type's full name, reporting invalid and duplicate
    /// names. Returns the full name and the namespace for nested names.
    fn declare(
        &mut self,
        node: &Node,
        namespace: &str,
        shape: Shape,
    ) -> Option<(String, String, TypeId)> {
        let name_node = self.required(node, "name")?;
        let Some(name) = name_node.as_str() else {
            self.error(
                codes::INVALID_SCHEMA,
                name_node.span,
                "`name` must be a string".into(),
            );
            return None;
        };
        let namespace = match node.get("namespace").and_then(Node::as_str) {
            Some(ns) if !name.contains('.') => ns.to_owned(),
            _ => namespace.to_owned(),
        };
        let full = full_name(name, &namespace);
        for part in full.split('.') {
            if !valid_name(part) {
                self.error(
                    codes::INVALID_NAME,
                    name_node.span,
                    format!(
                        "`{full}` is not a valid name: each part must match [A-Za-z_][A-Za-z0-9_]*"
                    ),
                );
                break;
            }
        }
        if PRIMITIVES.contains(&name) {
            self.error(
                codes::INVALID_NAME,
                name_node.span,
                format!("`{name}` is a primitive type and cannot be redefined"),
            );
        }
        if self.named.contains_key(&full) {
            self.error(
                codes::DUPLICATE,
                name_node.span,
                format!("`{full}` is defined twice"),
            );
            return None;
        }
        let id = self.api.types.reserve();
        self.named.insert(full.clone(), (id, shape));
        self.api.named.insert(QualName::new(&full), id);
        let nested = full
            .rsplit_once('.')
            .map(|(ns, _)| ns.to_owned())
            .unwrap_or_default();
        Some((full, nested, id))
    }

    fn record(&mut self, node: &Node, namespace: &str) -> Option<Lowered> {
        let (full, namespace, id) = self.declare(node, namespace, Shape::Record)?;
        let any = self.api.types.alloc_kind(TypeKind::Any);
        let mut object = ObjectType::new(any);
        // Writers write every field and nothing else; readers ignore unknown
        // fields and fill in defaults.
        object.writer_exact = true;
        let fields = self.required(node, "fields").and_then(|f| {
            f.as_array().or_else(|| {
                self.error(
                    codes::INVALID_SCHEMA,
                    f.span,
                    "`fields` must be an array".into(),
                );
                None
            })
        });
        let mut seen: HashMap<String, Span> = HashMap::new();
        for field in fields.unwrap_or_default() {
            let Some(name_node) = field.get("name") else {
                self.error(
                    codes::INVALID_SCHEMA,
                    field.span,
                    "a field needs a `name`".into(),
                );
                continue;
            };
            let Some(name) = name_node.as_str() else {
                self.error(
                    codes::INVALID_SCHEMA,
                    name_node.span,
                    "`name` must be a string".into(),
                );
                continue;
            };
            if !valid_name(name) {
                self.error(
                    codes::INVALID_NAME,
                    name_node.span,
                    format!("`{name}` is not a valid field name"),
                );
            }
            if let Some(first) = seen.insert(name.to_owned(), name_node.span) {
                self.diagnostics.push(
                    Diagnostic::error(format!("field `{name}` is defined twice in `{full}`"))
                        .with_code(codes::DUPLICATE)
                        .with_primary(name_node.span, "defined again")
                        .with_secondary(first, "first defined here"),
                );
                continue;
            }
            let Some(ty_node) = field.get("type") else {
                self.error(
                    codes::INVALID_SCHEMA,
                    field.span,
                    format!("field `{name}` needs a `type`"),
                );
                continue;
            };
            let Some(lowered) = self.schema(ty_node, &namespace) else {
                continue;
            };
            let default = field.get("default");
            if let Some(default) = default {
                self.check_default(default, &lowered.shape, name);
            }
            let mut out = Field::new(name, lowered.id, default.is_none());
            out.aliases = strings(field.get("aliases"));
            out.meta.description = field.get("doc").and_then(Node::as_str).map(str::to_owned);
            out.meta.default = default.map(Literal::from);
            out.origin = Some(field.span);
            object.fields.push(out);
        }
        let mut type_node = TypeNode::new(TypeKind::Object(object))
            .with_origin(node.span)
            .with_name(QualName::new(&full));
        type_node.meta.description = node.get("doc").and_then(Node::as_str).map(str::to_owned);
        self.api.types.define(id, type_node);
        Some(Lowered {
            id,
            shape: Shape::Record,
            name: Some(full),
        })
    }

    fn enumeration(&mut self, node: &Node, namespace: &str) -> Option<Lowered> {
        let symbols = self.required(node, "symbols")?;
        let Some(list) = symbols.as_array() else {
            self.error(
                codes::INVALID_SCHEMA,
                symbols.span,
                "`symbols` must be an array".into(),
            );
            return None;
        };
        let mut names: Vec<String> = Vec::new();
        let mut values = Vec::new();
        for symbol in list {
            let Some(text) = symbol.as_str() else {
                self.error(
                    codes::INVALID_SCHEMA,
                    symbol.span,
                    "a symbol must be a string".into(),
                );
                continue;
            };
            if !valid_name(text) {
                self.error(
                    codes::INVALID_NAME,
                    symbol.span,
                    format!("`{text}` is not a valid symbol"),
                );
            }
            if names.iter().any(|n| n == text) {
                self.error(
                    codes::DUPLICATE,
                    symbol.span,
                    format!("symbol `{text}` is listed twice"),
                );
                continue;
            }
            names.push(text.to_owned());
            let mut value = EnumValue::new(Literal::string(text));
            value.name = Some(text.to_owned());
            value.origin = Some(symbol.span);
            values.push(value);
        }
        let default = node.get("default");
        if let Some(default) = default
            && !default
                .as_str()
                .is_some_and(|d| names.iter().any(|n| n == d))
        {
            self.error(
                codes::INVALID_DEFAULT,
                default.span,
                "the enum default must be one of its symbols".into(),
            );
        }
        let (full, _, id) = self.declare(node, namespace, Shape::Enum(names.clone()))?;
        let mut type_node = TypeNode::new(TypeKind::Enum(EnumType {
            values,
            // A reader with a default reads unknown symbols as the default.
            open: default.is_some(),
            retired: Retired::default(),
            // Writers only write listed symbols; readers with a default
            // accept others.
            writer_exact: true,
        }))
        .with_origin(node.span)
        .with_name(QualName::new(&full));
        type_node.meta.description = node.get("doc").and_then(Node::as_str).map(str::to_owned);
        type_node.meta.default = default.map(Literal::from);
        self.api.types.define(id, type_node);
        Some(Lowered {
            id,
            shape: Shape::Enum(names),
            name: Some(full),
        })
    }

    fn fixed(&mut self, node: &Node, namespace: &str, logical: Option<String>) -> Option<Lowered> {
        let size_node = self.required(node, "size")?;
        let size = size_node
            .as_number()
            .and_then(|n| n.to_string().parse::<u64>().ok());
        let Some(size) = size else {
            self.error(
                codes::INVALID_SCHEMA,
                size_node.span,
                "`size` must be a non-negative integer".into(),
            );
            return None;
        };
        if let Some(logical) = &logical {
            self.check_logical(logical, "fixed", node.span);
        }
        let (full, _, id) = self.declare(node, namespace, Shape::Fixed)?;
        let kind = with_logical(
            TypeKind::String(StringType {
                min_length: size,
                max_length: Some(size),
                pattern: None,
                format: Some("fixed".into()),
            }),
            logical,
        );
        self.api.types.define(
            id,
            TypeNode::new(kind)
                .with_origin(node.span)
                .with_name(QualName::new(&full)),
        );
        Some(Lowered {
            id,
            shape: Shape::Fixed,
            name: Some(full),
        })
    }

    /// A default must be a value of the field's type; for a union, of its
    /// first branch.
    fn check_default(&mut self, default: &Node, shape: &Shape, field: &str) {
        let target = match shape {
            Shape::Union(branches) => branches.first(),
            other => Some(other),
        };
        let Some(target) = target else {
            return;
        };
        let fits = match (target, &default.value) {
            (Shape::Int | Shape::Long, Value::Number(n)) => is_integer(n),
            (Shape::Enum(symbols), Value::String(s)) => symbols.contains(s),
            (target, value) => matches!(
                (target, value),
                (Shape::Null, Value::Null)
                    | (Shape::Boolean, Value::Bool(_))
                    | (Shape::Float | Shape::Double, Value::Number(_))
                    | (
                        Shape::Bytes | Shape::String | Shape::Fixed,
                        Value::String(_)
                    )
                    | (Shape::Array, Value::Array(_))
                    | (Shape::Map | Shape::Record, Value::Object(_))
            ),
        };
        if !fits {
            let mut d =
                Diagnostic::error(format!("the default of `{field}` does not match its type"))
                    .with_code(codes::INVALID_DEFAULT)
                    .with_primary(default.span, "invalid default");
            if matches!(shape, Shape::Union(_)) {
                d = d.with_help(
                    "a union's default must match its first branch; put `\"null\"` first for a null default",
                );
            }
            self.diagnostics.push(d);
        }
    }
}

fn with_logical(kind: TypeKind, logical: Option<String>) -> TypeKind {
    let Some(logical) = logical else {
        return kind;
    };
    match kind {
        TypeKind::Integer(mut n) => {
            n.format = Some(logical);
            TypeKind::Integer(n)
        }
        TypeKind::Number(mut n) => {
            n.format = Some(logical);
            TypeKind::Number(n)
        }
        TypeKind::String(mut s) => {
            s.format = Some(logical);
            TypeKind::String(s)
        }
        other => other,
    }
}

fn full_name(name: &str, namespace: &str) -> String {
    if name.contains('.') || namespace.is_empty() {
        name.to_owned()
    } else {
        format!("{namespace}.{name}")
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_integer(n: &Number) -> bool {
    matches!(n, Number::Finite(d) if d.is_integer())
}

fn strings(node: Option<&Node>) -> Vec<String> {
    node.and_then(Node::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Node::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}
