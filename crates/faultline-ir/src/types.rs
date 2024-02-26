//! Type nodes: the value-level half of the IR.
//!
//! A [`TypeKind`] describes a *set of values*. Compatibility questions are
//! ultimately questions about these sets (does the new set contain the old
//! one?), so every constraint that narrows or widens the set is represented
//! explicitly, in a form independent of the format it came from.

use faultline_doc::Decimal;
use faultline_source::Span;
use indexmap::IndexMap;

use crate::arena::TypeId;
use crate::literal::Literal;

/// A qualified name for a named type, e.g. `Pet`, `acme.v1.Order`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QualName(pub String);

impl QualName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for QualName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One node in the type arena.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeNode {
    pub kind: TypeKind,
    /// Set when this node is the definition of a named type.
    pub name: Option<QualName>,
    pub meta: Meta,
    /// Where the node was defined. Nodes synthesized by a front end (for
    /// example the `null` branch of an OpenAPI 3.0 `nullable`) have none.
    pub origin: Option<Span>,
}

impl TypeNode {
    pub fn new(kind: TypeKind) -> Self {
        Self {
            kind,
            name: None,
            meta: Meta::default(),
            origin: None,
        }
    }

    #[must_use]
    pub fn with_origin(mut self, span: Span) -> Self {
        self.origin = Some(span);
        self
    }

    #[must_use]
    pub fn with_name(mut self, name: QualName) -> Self {
        self.name = Some(name);
        self
    }
}

/// Descriptive information that does not change the value set but can matter
/// for compatibility (deprecation, access direction) or for reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub access: Access,
    pub default: Option<Literal>,
    /// Vendor extensions (`x-*` in OpenAPI), in document order.
    pub extensions: IndexMap<String, Literal>,
}

/// Which direction values at a position may flow, as declared by
/// `readOnly`/`writeOnly`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Access {
    #[default]
    ReadWrite,
    /// Only sent by the API (responses). Ignored in requests.
    ReadOnly,
    /// Only sent to the API (requests). Never returned.
    WriteOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    /// Every value.
    Any,
    /// No value (a `false` schema, an empty union).
    Never,
    Null,
    Boolean,
    Integer(NumericType),
    Number(NumericType),
    String(StringType),
    /// Opaque binary data (Protobuf/Avro `bytes`).
    Bytes,
    Enum(EnumType),
    Object(ObjectType),
    Array(ArrayType),
    Map(MapType),
    Union(UnionType),
    /// Values accepted by every member (`allOf`).
    AllOf(Vec<TypeId>),
    /// Values *not* accepted by the operand.
    Not(TypeId),
    /// Indirection to another node, usually a named type. References are how
    /// cycles are expressed; the arena itself is a graph.
    Ref(TypeId),
}

impl TypeKind {
    /// A short lowercase name for messages and dumps.
    pub fn describe(&self) -> &'static str {
        match self {
            TypeKind::Any => "any",
            TypeKind::Never => "never",
            TypeKind::Null => "null",
            TypeKind::Boolean => "boolean",
            TypeKind::Integer(_) => "integer",
            TypeKind::Number(_) => "number",
            TypeKind::String(_) => "string",
            TypeKind::Bytes => "bytes",
            TypeKind::Enum(_) => "enum",
            TypeKind::Object(_) => "object",
            TypeKind::Array(_) => "array",
            TypeKind::Map(_) => "map",
            TypeKind::Union(_) => "union",
            TypeKind::AllOf(_) => "allOf",
            TypeKind::Not(_) => "not",
            TypeKind::Ref(_) => "ref",
        }
    }
}

/// An inclusive or exclusive numeric bound.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Bound {
    pub value: Decimal,
    pub exclusive: bool,
}

impl Bound {
    pub fn inclusive(value: Decimal) -> Self {
        Self {
            value,
            exclusive: false,
        }
    }

    pub fn exclusive(value: Decimal) -> Self {
        Self {
            value,
            exclusive: true,
        }
    }
}

/// Constraints shared by integers and numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NumericType {
    pub minimum: Option<Bound>,
    pub maximum: Option<Bound>,
    pub multiple_of: Option<Decimal>,
    /// The declared representation (`int32`, `int64`, `float`, `double`, ...).
    pub format: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct StringType {
    /// Minimum length in Unicode code points (JSON Schema semantics).
    pub min_length: u64,
    pub max_length: Option<u64>,
    /// ECMA-262 regular expression, unanchored as in JSON Schema.
    pub pattern: Option<String>,
    pub format: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumType {
    pub values: Vec<EnumValue>,
    /// Whether values outside the list can occur (Protobuf 3 enums): an open
    /// enum's list documents known values rather than restricting them.
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumValue {
    pub value: Literal,
    /// Symbolic name where the format has one (Protobuf, GraphQL, Avro).
    pub name: Option<String>,
    /// Numeric tag where the format has one (Protobuf).
    pub number: Option<i64>,
    pub meta: Meta,
    pub origin: Option<Span>,
}

impl EnumValue {
    pub fn new(value: Literal) -> Self {
        Self {
            value,
            name: None,
            number: None,
            meta: Meta::default(),
            origin: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectType {
    pub fields: Vec<Field>,
    /// Type of members not matched by `fields` or `pattern_properties`.
    /// `Any` for open objects, `Never` for closed ones.
    pub additional: TypeId,
    /// `(ECMA-262 pattern, type)` pairs, in document order.
    pub pattern_properties: Vec<(String, TypeId)>,
    /// Constraint every member name must satisfy (a string type).
    pub property_names: Option<TypeId>,
    pub min_properties: u64,
    pub max_properties: Option<u64>,
    /// `property → properties that become required when it is present`.
    pub dependent_required: Vec<(String, Vec<String>)>,
}

impl ObjectType {
    /// An object with no declared fields whose other members have type
    /// `additional`.
    pub fn new(additional: TypeId) -> Self {
        Self {
            fields: Vec::new(),
            additional,
            pattern_properties: Vec::new(),
            property_names: None,
            min_properties: 0,
            max_properties: None,
            dependent_required: Vec::new(),
        }
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// A declared member of an object or message.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The member name as it appears in serialized data.
    pub name: String,
    pub ty: TypeId,
    pub required: bool,
    /// Wire tag (Protobuf field number).
    pub tag: Option<u32>,
    /// Alternative names the field is also known by (Avro aliases).
    pub aliases: Vec<String>,
    pub meta: Meta,
    pub origin: Option<Span>,
}

impl Field {
    pub fn new(name: impl Into<String>, ty: TypeId, required: bool) -> Self {
        Self {
            name: name.into(),
            ty,
            required,
            tag: None,
            aliases: Vec::new(),
            meta: Meta::default(),
            origin: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArrayType {
    /// Type of items after `prefix_items`.
    pub items: TypeId,
    /// Positional item types (tuple validation).
    pub prefix_items: Vec<TypeId>,
    pub min_items: u64,
    pub max_items: Option<u64>,
    pub unique_items: bool,
    pub contains: Option<Contains>,
}

impl ArrayType {
    pub fn of(items: TypeId) -> Self {
        Self {
            items,
            prefix_items: Vec::new(),
            min_items: 0,
            max_items: None,
            unique_items: false,
            contains: None,
        }
    }
}

/// At least `min` and at most `max` items must match `ty`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Contains {
    pub ty: TypeId,
    pub min: u64,
    pub max: Option<u64>,
}

/// A map with typed keys, as in Protobuf `map<K, V>` or Avro maps.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MapType {
    pub key: TypeId,
    pub value: TypeId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnionType {
    pub mode: UnionMode,
    pub variants: Vec<TypeId>,
    pub discriminator: Option<Discriminator>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnionMode {
    /// At least one variant matches (`anyOf`).
    Any,
    /// Exactly one variant matches (`oneOf`).
    Exclusive,
    /// The variant is identified by the data itself (a Protobuf `oneof`
    /// field, a GraphQL union's `__typename`, an Avro union branch).
    Tagged,
}

/// An OpenAPI discriminator: `property` selects the variant by value.
#[derive(Clone, Debug, PartialEq)]
pub struct Discriminator {
    pub property: String,
    /// Explicit `value → variant` mappings, in document order.
    pub mapping: Vec<(String, TypeId)>,
}
