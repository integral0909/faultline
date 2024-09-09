//! The components of a set of XML Schema documents, as read from them.
//!
//! Components keep the names they refer to other components by; the
//! lowering resolves them. Every component carries the span of its
//! declaration.

use std::collections::HashMap;

use faultline_source::Span;
pub use faultline_xml::QName;

/// How many times a particle may occur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurs {
    pub min: u64,
    /// `None` for `unbounded`.
    pub max: Option<u64>,
}

impl Occurs {
    pub const ONCE: Occurs = Occurs {
        min: 1,
        max: Some(1),
    };

    /// Whether more than one occurrence is allowed.
    pub fn repeats(self) -> bool {
        self.max != Some(1) && self.max != Some(0)
    }
}

/// A reference to a type: by name, or declared inline.
#[derive(Clone, Debug, PartialEq)]
pub enum TypeRef {
    Named(QName, Span),
    Anonymous(Box<TypeDef>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDef {
    Simple(SimpleType),
    Complex(Box<ComplexType>),
}

impl TypeDef {
    pub fn span(&self) -> Span {
        match self {
            TypeDef::Simple(t) => t.span,
            TypeDef::Complex(t) => t.span,
        }
    }
}

/// An element declaration, global or local.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementDecl {
    pub name: QName,
    /// `None` means `xs:anyType`.
    pub ty: Option<TypeRef>,
    pub nillable: bool,
    pub abstract_: bool,
    pub default: Option<String>,
    pub fixed: Option<String>,
    pub substitution_group: Option<QName>,
    pub documentation: Option<String>,
    pub span: Span,
}

/// The constraining facets of a simple type restriction.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facets {
    pub enumeration: Vec<(String, Span)>,
    /// Several patterns in one restriction are alternatives; patterns of
    /// successive restrictions all apply.
    pub patterns: Vec<String>,
    pub length: Option<u64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub min_inclusive: Option<String>,
    pub max_inclusive: Option<String>,
    pub min_exclusive: Option<String>,
    pub max_exclusive: Option<String>,
    pub total_digits: Option<u64>,
    pub fraction_digits: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SimpleVariety {
    Restriction {
        base: TypeRef,
        facets: Box<Facets>,
    },
    /// A whitespace-separated list of items.
    List {
        item: TypeRef,
    },
    Union {
        members: Vec<TypeRef>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimpleType {
    pub name: Option<QName>,
    pub variety: SimpleVariety,
    pub documentation: Option<String>,
    pub span: Span,
}

/// How a complex type is derived from its base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Derivation {
    Extension,
    Restriction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContentModel {
    /// No child elements and no text.
    Empty,
    /// Text of a simple type (`simpleContent`), possibly with attributes.
    Simple { base: TypeRef, facets: Facets },
    /// Child elements (`complexContent`, or a particle directly).
    Elements(Particle),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComplexType {
    pub name: Option<QName>,
    /// The type this one derives from, if any, and how.
    pub base: Option<(Derivation, QName, Span)>,
    pub content: ContentModel,
    pub attributes: Vec<AttributeUse>,
    /// `anyAttribute`: other attributes are allowed.
    pub any_attribute: bool,
    pub mixed: bool,
    pub abstract_: bool,
    pub documentation: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Particle {
    pub term: Term,
    pub occurs: Occurs,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Element(ElementDecl),
    /// `<xs:element ref="..."/>`.
    ElementRef(QName),
    Sequence(Vec<Particle>),
    Choice(Vec<Particle>),
    All(Vec<Particle>),
    /// `<xs:group ref="..."/>`.
    GroupRef(QName),
    /// `xs:any`: elements not declared here.
    Any,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AttributeUse {
    Declared {
        name: QName,
        ty: Option<TypeRef>,
        required: bool,
        /// `use="prohibited"`, which removes an inherited attribute.
        prohibited: bool,
        default: Option<String>,
        fixed: Option<String>,
        span: Span,
    },
    Ref {
        name: QName,
        required: bool,
        prohibited: bool,
        span: Span,
    },
    GroupRef(QName, Span),
}

/// Every global component of a set of schema documents, by name.
#[derive(Debug, Default)]
pub struct Components {
    pub elements: HashMap<QName, ElementDecl>,
    /// Global elements in document order, for schema roots.
    pub element_order: Vec<QName>,
    pub types: HashMap<QName, TypeDef>,
    pub groups: HashMap<QName, Particle>,
    pub attribute_groups: HashMap<QName, (Vec<AttributeUse>, bool)>,
    pub attributes: HashMap<QName, AttributeUse>,
}
