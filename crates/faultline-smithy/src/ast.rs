//! The syntax tree of a Smithy IDL file.

use faultline_source::Span;

/// A name or shape id with where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Model {
    /// `$version`, when given.
    pub version: Option<String>,
    pub namespace: Option<Name>,
    /// `use` statements: absolute shape ids.
    pub uses: Vec<Name>,
    pub shapes: Vec<Shape>,
    pub applies: Vec<Apply>,
}

/// A node value: the values of traits, metadata and service properties.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub value: NodeValue,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeValue {
    Null,
    Bool(bool),
    /// A number as written.
    Number(String),
    String(String),
    /// An unquoted shape id, such as an operation in a service's
    /// `operations`.
    ShapeId(String),
    Array(Vec<Node>),
    Object(Vec<(Name, Node)>),
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&Node> {
        match &self.value {
            NodeValue::Object(members) => {
                members.iter().find(|(k, _)| k.text == key).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// The text of a string or shape id.
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            NodeValue::String(s) | NodeValue::ShapeId(s) => Some(s),
            _ => None,
        }
    }
}

/// `@id` or `@id(value)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Trait {
    pub id: Name,
    /// `Null` when the trait has no value, an object for `@id(key: value)`.
    pub value: Node,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimpleType {
    Blob,
    Boolean,
    Document,
    String,
    Byte,
    Short,
    Integer,
    Long,
    Float,
    Double,
    BigInteger,
    BigDecimal,
    Timestamp,
}

impl SimpleType {
    pub fn parse(name: &str) -> Option<SimpleType> {
        Some(match name {
            "blob" => SimpleType::Blob,
            "boolean" => SimpleType::Boolean,
            "document" => SimpleType::Document,
            "string" => SimpleType::String,
            "byte" => SimpleType::Byte,
            "short" => SimpleType::Short,
            "integer" => SimpleType::Integer,
            "long" => SimpleType::Long,
            "float" => SimpleType::Float,
            "double" => SimpleType::Double,
            "bigInteger" => SimpleType::BigInteger,
            "bigDecimal" => SimpleType::BigDecimal,
            "timestamp" => SimpleType::Timestamp,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Simple(SimpleType),
    Enum,
    IntEnum,
    List,
    Map,
    Structure,
    Union,
    Service,
    Resource,
    Operation,
}

impl ShapeKind {
    pub fn keyword(self) -> &'static str {
        match self {
            ShapeKind::Simple(_) => "simple shape",
            ShapeKind::Enum => "enum",
            ShapeKind::IntEnum => "intEnum",
            ShapeKind::List => "list",
            ShapeKind::Map => "map",
            ShapeKind::Structure => "structure",
            ShapeKind::Union => "union",
            ShapeKind::Service => "service",
            ShapeKind::Resource => "resource",
            ShapeKind::Operation => "operation",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub kind: ShapeKind,
    pub name: Name,
    pub traits: Vec<Trait>,
    pub doc: Option<String>,
    /// `with [A, B]`.
    pub mixins: Vec<Name>,
    /// `for Resource`, whose identifiers and properties members may elide
    /// their targets from.
    pub for_resource: Option<Name>,
    pub members: Vec<Member>,
    /// The properties of a service or resource.
    pub properties: Option<Node>,
    /// The input, output and errors of an operation.
    pub operation: Option<OperationBody>,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub name: Name,
    /// `None` when elided (`$name`): the target comes from a mixin or the
    /// resource.
    pub target: Option<Name>,
    pub default: Option<Node>,
    pub traits: Vec<Trait>,
    pub doc: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct OperationBody {
    pub input: Option<IoShape>,
    pub output: Option<IoShape>,
    pub errors: Vec<Name>,
}

/// An operation's input or output: a structure named elsewhere, or one
/// defined in place with `:=`.
#[derive(Clone, Debug)]
pub enum IoShape {
    Target(Name),
    Inline(Box<Shape>),
}

/// `apply Shape @trait` or `apply Shape { @a @b }`.
#[derive(Clone, Debug)]
pub struct Apply {
    pub target: Name,
    pub traits: Vec<Trait>,
}
