//! The syntax tree of one `.proto` file, before names are resolved.

use faultline_source::Span;

/// `syntax = "..."` or `edition = "..."`; proto2 when absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Syntax {
    Proto2,
    Proto3,
    /// An edition such as `2023`, whose defaults are proto3-like with
    /// explicit field presence.
    Edition(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub syntax: Syntax,
    pub package: Option<Name>,
    pub imports: Vec<Import>,
    pub options: Vec<OptionDecl>,
    pub messages: Vec<Message>,
    pub enums: Vec<Enum>,
    pub services: Vec<Service>,
    pub extends: Vec<Extend>,
}

/// An identifier (or dotted name) and where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportKind {
    Default,
    /// Re-exported to files importing this one.
    Public,
    Weak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub path: String,
    pub kind: ImportKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    /// An identifier such as `true`, `SPEED` or `inf`.
    Ident(String),
    Int(i128),
    Float(f64),
    Str(Vec<u8>),
    /// A message value in text format (`{ a: 1 }`), kept as written.
    Aggregate(String),
}

impl OptionValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            OptionValue::Ident(s) if s == "true" => Some(true),
            OptionValue::Ident(s) if s == "false" => Some(false),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            OptionValue::Str(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        }
    }

    pub fn as_ident(&self) -> Option<&str> {
        match self {
            OptionValue::Ident(s) => Some(s),
            _ => None,
        }
    }
}

/// `option name = value;` or one `name = value` inside `[...]`.
#[derive(Clone, Debug, PartialEq)]
pub struct OptionDecl {
    /// As written, without spaces: `deprecated`, `(my.ext).field`,
    /// `features.field_presence`.
    pub name: String,
    pub value: OptionValue,
    pub span: Span,
}

/// Looks up an option by name.
pub fn option<'a>(options: &'a [OptionDecl], name: &str) -> Option<&'a OptionDecl> {
    options.iter().rev().find(|o| o.name == name)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub name: Name,
    pub doc: Option<String>,
    pub span: Span,
    pub fields: Vec<Field>,
    pub oneofs: Vec<Oneof>,
    pub messages: Vec<Message>,
    pub enums: Vec<Enum>,
    pub extends: Vec<Extend>,
    pub reserved: Vec<Reserved>,
    pub extension_ranges: Vec<(i64, i64)>,
    pub options: Vec<OptionDecl>,
}

impl Message {
    pub fn new(name: Name, span: Span, doc: Option<String>) -> Self {
        Self {
            name,
            doc,
            span,
            fields: Vec::new(),
            oneofs: Vec::new(),
            messages: Vec::new(),
            enums: Vec::new(),
            extends: Vec::new(),
            reserved: Vec::new(),
            extension_ranges: Vec::new(),
            options: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Optional,
    Required,
    Repeated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scalar {
    Double,
    Float,
    Int32,
    Int64,
    Uint32,
    Uint64,
    Sint32,
    Sint64,
    Fixed32,
    Fixed64,
    Sfixed32,
    Sfixed64,
    Bool,
    String,
    Bytes,
}

impl Scalar {
    pub fn parse(name: &str) -> Option<Scalar> {
        Some(match name {
            "double" => Scalar::Double,
            "float" => Scalar::Float,
            "int32" => Scalar::Int32,
            "int64" => Scalar::Int64,
            "uint32" => Scalar::Uint32,
            "uint64" => Scalar::Uint64,
            "sint32" => Scalar::Sint32,
            "sint64" => Scalar::Sint64,
            "fixed32" => Scalar::Fixed32,
            "fixed64" => Scalar::Fixed64,
            "sfixed32" => Scalar::Sfixed32,
            "sfixed64" => Scalar::Sfixed64,
            "bool" => Scalar::Bool,
            "string" => Scalar::String,
            "bytes" => Scalar::Bytes,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scalar::Double => "double",
            Scalar::Float => "float",
            Scalar::Int32 => "int32",
            Scalar::Int64 => "int64",
            Scalar::Uint32 => "uint32",
            Scalar::Uint64 => "uint64",
            Scalar::Sint32 => "sint32",
            Scalar::Sint64 => "sint64",
            Scalar::Fixed32 => "fixed32",
            Scalar::Fixed64 => "fixed64",
            Scalar::Sfixed32 => "sfixed32",
            Scalar::Sfixed64 => "sfixed64",
            Scalar::Bool => "bool",
            Scalar::String => "string",
            Scalar::Bytes => "bytes",
        }
    }

    /// Whether the type may be a map key: integral types, `bool` and
    /// `string`.
    pub fn is_map_key(self) -> bool {
        !matches!(self, Scalar::Double | Scalar::Float | Scalar::Bytes)
    }
}

/// A reference to a message or enum, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeName {
    /// Written with a leading `.`: resolved from the root, not the scope.
    pub absolute: bool,
    pub parts: Vec<String>,
    pub span: Span,
}

impl std::fmt::Display for TypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.absolute {
            f.write_str(".")?;
        }
        f.write_str(&self.parts.join("."))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldType {
    Scalar(Scalar),
    Named(TypeName),
    Map(Box<FieldType>, Box<FieldType>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub label: Option<Label>,
    pub ty: FieldType,
    pub name: Name,
    pub number: i64,
    pub number_span: Span,
    pub options: Vec<OptionDecl>,
    pub doc: Option<String>,
    /// The whole declaration.
    pub span: Span,
    /// Index into the enclosing message's `oneofs`.
    pub oneof: Option<usize>,
    /// Declared as a proto2 `group`: the type is the nested message of the
    /// same (capitalized) name.
    pub group: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Oneof {
    pub name: Name,
    pub doc: Option<String>,
    pub span: Span,
    pub options: Vec<OptionDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Enum {
    pub name: Name,
    pub doc: Option<String>,
    pub span: Span,
    pub values: Vec<EnumValue>,
    pub reserved: Vec<Reserved>,
    pub options: Vec<OptionDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumValue {
    pub name: Name,
    pub number: i64,
    pub doc: Option<String>,
    pub span: Span,
    pub options: Vec<OptionDecl>,
}

/// `reserved 2, 9 to 11, 40 to max;` or `reserved "foo", "bar";`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reserved {
    /// Inclusive ranges; `max` is represented by `i64::MAX`.
    pub ranges: Vec<(i64, i64)>,
    pub names: Vec<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub name: Name,
    pub doc: Option<String>,
    pub span: Span,
    pub methods: Vec<Method>,
    pub options: Vec<OptionDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Method {
    pub name: Name,
    pub doc: Option<String>,
    pub span: Span,
    pub input: TypeName,
    pub input_stream: bool,
    pub output: TypeName,
    pub output_stream: bool,
    pub options: Vec<OptionDecl>,
}

/// `extend Foo { ... }`: fields added to another message.
#[derive(Clone, Debug, PartialEq)]
pub struct Extend {
    pub extendee: TypeName,
    pub fields: Vec<Field>,
    pub span: Span,
}
