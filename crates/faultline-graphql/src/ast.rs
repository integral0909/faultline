//! The syntax tree of a GraphQL schema document.

use faultline_source::Span;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

/// A constant value: argument defaults and directive arguments.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(String),
    Float(String),
    String(String),
    Boolean(bool),
    Null,
    Enum(String),
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
    /// Only valid in executable documents; reported where it appears.
    Variable(String),
}

/// `Name`, `[T]` or `T!`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeRef {
    Named(Name),
    List(Box<TypeRef>, Span),
    NonNull(Box<TypeRef>, Span),
}

impl TypeRef {
    /// The named type at the core of the reference.
    pub fn base(&self) -> &Name {
        match self {
            TypeRef::Named(name) => name,
            TypeRef::List(inner, _) | TypeRef::NonNull(inner, _) => inner.base(),
        }
    }

    pub fn span(&self) -> Span {
        match self {
            TypeRef::Named(name) => name.span,
            TypeRef::List(_, span) | TypeRef::NonNull(_, span) => *span,
        }
    }

    pub fn is_non_null(&self) -> bool {
        matches!(self, TypeRef::NonNull(..))
    }
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeRef::Named(name) => f.write_str(&name.text),
            TypeRef::List(inner, _) => write!(f, "[{inner}]"),
            TypeRef::NonNull(inner, _) => write!(f, "{inner}!"),
        }
    }
}

/// `@name(arg: value)`
#[derive(Clone, Debug, PartialEq)]
pub struct Directive {
    pub name: Name,
    pub arguments: Vec<(Name, Value)>,
    pub span: Span,
}

/// The `reason` of a `@deprecated` among `directives`, or `None` if the
/// element is not deprecated.
pub fn deprecation(directives: &[Directive]) -> Option<String> {
    let directive = directives.iter().find(|d| d.name.text == "deprecated")?;
    Some(
        directive
            .arguments
            .iter()
            .find(|(name, _)| name.text == "reason")
            .and_then(|(_, value)| match value {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "No longer supported".to_owned()),
    )
}

/// An argument or input field.
#[derive(Clone, Debug, PartialEq)]
pub struct InputValue {
    pub description: Option<String>,
    pub name: Name,
    pub ty: TypeRef,
    pub default: Option<Value>,
    pub directives: Vec<Directive>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDef {
    pub description: Option<String>,
    pub name: Name,
    pub arguments: Vec<InputValue>,
    pub ty: TypeRef,
    pub directives: Vec<Directive>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumValueDef {
    pub description: Option<String>,
    pub name: Name,
    pub directives: Vec<Directive>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDefKind {
    Scalar,
    Object {
        interfaces: Vec<Name>,
        fields: Vec<FieldDef>,
    },
    Interface {
        interfaces: Vec<Name>,
        fields: Vec<FieldDef>,
    },
    Union {
        members: Vec<Name>,
    },
    Enum {
        values: Vec<EnumValueDef>,
    },
    InputObject {
        fields: Vec<InputValue>,
    },
}

impl TypeDefKind {
    pub fn describe(&self) -> &'static str {
        match self {
            TypeDefKind::Scalar => "scalar",
            TypeDefKind::Object { .. } => "object type",
            TypeDefKind::Interface { .. } => "interface",
            TypeDefKind::Union { .. } => "union",
            TypeDefKind::Enum { .. } => "enum",
            TypeDefKind::InputObject { .. } => "input object",
        }
    }
}

/// A type definition, or an `extend` of one.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDef {
    pub description: Option<String>,
    pub name: Name,
    pub kind: TypeDefKind,
    pub directives: Vec<Directive>,
    pub extension: bool,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

impl OperationKind {
    pub fn parse(text: &str) -> Option<OperationKind> {
        match text {
            "query" => Some(OperationKind::Query),
            "mutation" => Some(OperationKind::Mutation),
            "subscription" => Some(OperationKind::Subscription),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::Query => "query",
            OperationKind::Mutation => "mutation",
            OperationKind::Subscription => "subscription",
        }
    }

    /// The root type name used when no `schema` definition names one.
    pub fn default_type(self) -> &'static str {
        match self {
            OperationKind::Query => "Query",
            OperationKind::Mutation => "Mutation",
            OperationKind::Subscription => "Subscription",
        }
    }
}

/// `schema { query: Query }` or `extend schema { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaDef {
    pub description: Option<String>,
    pub roots: Vec<(OperationKind, Name)>,
    pub directives: Vec<Directive>,
    pub extension: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DirectiveDef {
    pub description: Option<String>,
    pub name: Name,
    pub arguments: Vec<InputValue>,
    pub repeatable: bool,
    pub locations: Vec<Name>,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub schemas: Vec<SchemaDef>,
    pub types: Vec<TypeDef>,
    pub directives: Vec<DirectiveDef>,
}
