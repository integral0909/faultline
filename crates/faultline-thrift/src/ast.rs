//! The syntax tree of a Thrift file.

use faultline_source::Span;

/// A name with where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub includes: Vec<Include>,
    /// `namespace <scope> <name>` declarations, by scope (`*`, `java`, ...).
    pub namespaces: Vec<(String, Name)>,
    pub definitions: Vec<Definition>,
}

#[derive(Clone, Debug)]
pub struct Include {
    pub path: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Definition {
    Const(Const),
    Typedef(Typedef),
    Enum(Enum),
    Struct(Struct),
    Service(Service),
}

impl Definition {
    pub fn name(&self) -> &Name {
        match self {
            Definition::Const(d) => &d.name,
            Definition::Typedef(d) => &d.name,
            Definition::Enum(d) => &d.name,
            Definition::Struct(d) => &d.name,
            Definition::Service(d) => &d.name,
        }
    }
}

/// `(key = "value", ...)` after a type, field or definition.
pub type Annotations = Vec<(Name, Option<String>)>;

#[derive(Clone, Debug)]
pub struct Const {
    pub name: Name,
    pub ty: FieldType,
    pub value: ConstValue,
    pub doc: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Typedef {
    pub name: Name,
    pub ty: FieldType,
    pub doc: Option<String>,
    pub annotations: Annotations,
}

#[derive(Clone, Debug)]
pub struct Enum {
    pub name: Name,
    pub values: Vec<EnumValue>,
    pub doc: Option<String>,
    pub annotations: Annotations,
}

#[derive(Clone, Debug)]
pub struct EnumValue {
    pub name: Name,
    /// The value written (`= 3`); others follow the previous value.
    pub value: Option<(i64, Span)>,
    pub doc: Option<String>,
    pub annotations: Annotations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructKind {
    Struct,
    Union,
    Exception,
}

impl StructKind {
    pub fn keyword(self) -> &'static str {
        match self {
            StructKind::Struct => "struct",
            StructKind::Union => "union",
            StructKind::Exception => "exception",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Struct {
    pub kind: StructKind,
    pub name: Name,
    pub fields: Vec<Field>,
    pub doc: Option<String>,
    pub annotations: Annotations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requiredness {
    Required,
    Optional,
    /// Neither: written when set, and read when present.
    Default,
}

#[derive(Clone, Debug)]
pub struct Field {
    /// The id written (`1:`); `None` for an implicit one.
    pub id: Option<(i64, Span)>,
    pub requiredness: Requiredness,
    pub ty: FieldType,
    pub name: Name,
    pub default: Option<ConstValue>,
    pub doc: Option<String>,
    pub annotations: Annotations,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BaseType {
    Bool,
    /// `byte` and `i8`.
    I8,
    I16,
    I32,
    I64,
    Double,
    String,
    Binary,
    Uuid,
}

impl BaseType {
    pub fn parse(name: &str) -> Option<BaseType> {
        Some(match name {
            "bool" => BaseType::Bool,
            "byte" | "i8" => BaseType::I8,
            "i16" => BaseType::I16,
            "i32" => BaseType::I32,
            "i64" => BaseType::I64,
            "double" => BaseType::Double,
            "string" => BaseType::String,
            "binary" => BaseType::Binary,
            "uuid" => BaseType::Uuid,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BaseType::Bool => "bool",
            BaseType::I8 => "i8",
            BaseType::I16 => "i16",
            BaseType::I32 => "i32",
            BaseType::I64 => "i64",
            BaseType::Double => "double",
            BaseType::String => "string",
            BaseType::Binary => "binary",
            BaseType::Uuid => "uuid",
        }
    }
}

#[derive(Clone, Debug)]
pub enum FieldTypeKind {
    Base(BaseType),
    /// A defined type, possibly qualified by an include (`shared.Thing`).
    Named(String),
    List(Box<FieldType>),
    Set(Box<FieldType>),
    Map(Box<FieldType>, Box<FieldType>),
}

#[derive(Clone, Debug)]
pub struct FieldType {
    pub kind: FieldTypeKind,
    pub span: Span,
    pub annotations: Annotations,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Double(f64),
    Str(String),
    /// A constant or enum value by name.
    Ident(String),
    List(Vec<ConstValue>),
    Map(Vec<(ConstValue, ConstValue)>),
}

#[derive(Clone, Debug)]
pub struct Service {
    pub name: Name,
    pub extends: Option<Name>,
    pub functions: Vec<Function>,
    pub doc: Option<String>,
    pub annotations: Annotations,
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: Name,
    pub oneway: bool,
    /// `None` for `void`.
    pub returns: Option<FieldType>,
    pub return_span: Span,
    pub arguments: Vec<Field>,
    pub throws: Vec<Field>,
    pub doc: Option<String>,
    pub annotations: Annotations,
}
