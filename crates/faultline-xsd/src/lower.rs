//! Lowering schema components into the IR.
//!
//! Simple types become the kind of their primitive base, narrowed by their
//! facets: a restriction of `xs:string` with `maxLength` is a string with a
//! maximum length, an enumeration is an enum, a list is an array and a union
//! a union. Patterns are anchored, as XML Schema patterns match the whole
//! value.

use std::collections::{HashMap, HashSet};

use faultline_doc::{Decimal, Number};
use faultline_ir::{
    Api, ApiFormat, ArrayType, Bound, EnumType, EnumValue, Literal, NumericType, QualName, Retired,
    StringType, TypeId, TypeKind, TypeNode, UnionMode, UnionType,
};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::builtins::{builtin, is_xs};
use crate::codes;
use crate::model::{Components, Facets, QName, SimpleType, SimpleVariety, TypeDef, TypeRef};

/// Lowers the components of a schema set into an [`Api`].
pub struct Lowerer<'c> {
    pub(crate) components: &'c Components,
    pub api: Api,
    /// Named types lowered so far.
    pub(crate) ids: HashMap<QName, TypeId>,
    /// Simple types being lowered, to report circular definitions.
    visiting: HashSet<QName>,
    pub diagnostics: Diagnostics,
}

impl std::fmt::Debug for Lowerer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lowerer")
            .field("api", &self.api.name)
            .finish_non_exhaustive()
    }
}

/// The name a type is known by in the IR: `{namespace}local`, or `local`
/// without a namespace.
pub fn type_name(name: &QName) -> QualName {
    QualName::new(name.to_string())
}

impl<'c> Lowerer<'c> {
    pub fn new(components: &'c Components, name: &str) -> Self {
        Self {
            components,
            api: Api::new(name, ApiFormat::XmlSchema),
            ids: HashMap::new(),
            visiting: HashSet::new(),
            diagnostics: Diagnostics::new(),
        }
    }

    pub(crate) fn error(
        &mut self,
        code: faultline_source::DiagnosticCode,
        message: String,
        span: Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    pub(crate) fn alloc(&mut self, kind: TypeKind, span: Option<Span>) -> TypeId {
        let mut node = TypeNode::new(kind);
        node.origin = span;
        self.api.types.alloc(node)
    }

    /// The global element `name`, for front ends whose messages name
    /// elements (WSDL).
    pub fn components_element(&self, name: &QName) -> Option<crate::model::ElementDecl> {
        self.components.elements.get(name).cloned()
    }

    pub(crate) fn any(&mut self) -> TypeId {
        self.alloc(TypeKind::Any, None)
    }

    /// The type of the simple type `name`, built in or declared.
    pub fn simple_named(&mut self, name: &QName, span: Span) -> TypeId {
        if is_xs(name.namespace.as_deref()) {
            let types = &mut self.api.types;
            let mut alloc = |kind: TypeKind| types.alloc_kind(kind);
            if let Some(kind) = builtin(&name.local, &mut alloc) {
                return self.alloc(kind, Some(span));
            }
        }
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        match self.components.types.get(name) {
            Some(TypeDef::Simple(simple)) => {
                if !self.visiting.insert(name.clone()) {
                    self.error(
                        codes::INVALID,
                        format!("simple type {name} is defined in terms of itself"),
                        simple.span,
                    );
                    return self.any();
                }
                let kind = self.simple_kind(simple);
                self.visiting.remove(name);
                let mut node = TypeNode::new(kind).with_origin(simple.span);
                node.meta.description.clone_from(&simple.documentation);
                let id = self.api.types.alloc(node.with_name(type_name(name)));
                self.api.named.insert(type_name(name), id);
                self.ids.insert(name.clone(), id);
                id
            }
            Some(TypeDef::Complex(_)) => {
                self.error(
                    codes::INVALID,
                    format!("{name} is a complex type where a simple type is needed"),
                    span,
                );
                self.any()
            }
            None => {
                self.error(
                    codes::UNRESOLVED,
                    format!("type {name} is not declared"),
                    span,
                );
                self.any()
            }
        }
    }

    /// The type of a reference in a simple-type context.
    pub fn simple_ref(&mut self, reference: &TypeRef) -> TypeId {
        match reference {
            TypeRef::Named(name, span) => self.simple_named(name, *span),
            TypeRef::Anonymous(def) => match def.as_ref() {
                TypeDef::Simple(simple) => {
                    let kind = self.simple_kind(simple);
                    self.alloc(kind, Some(simple.span))
                }
                TypeDef::Complex(complex) => {
                    let span = complex.span;
                    self.error(
                        codes::INVALID,
                        "a complex type where a simple type is needed".into(),
                        span,
                    );
                    self.any()
                }
            },
        }
    }

    fn simple_kind(&mut self, simple: &SimpleType) -> TypeKind {
        match &simple.variety {
            SimpleVariety::Restriction { base, facets } => {
                let base = self.simple_ref(base);
                self.restrict(base, facets, simple.span)
            }
            SimpleVariety::List { item } => {
                let item = self.simple_ref(item);
                TypeKind::Array(ArrayType::of(item))
            }
            SimpleVariety::Union { members } => {
                let variants = members.iter().map(|m| self.simple_ref(m)).collect();
                TypeKind::Union(UnionType {
                    mode: UnionMode::Any,
                    variants,
                    discriminator: None,
                })
            }
        }
    }

    /// The kind of `base` narrowed by `facets`.
    pub(crate) fn restrict(&mut self, base: TypeId, facets: &Facets, span: Span) -> TypeKind {
        let resolved = self.api.types.resolve(base);
        let mut kind = self.api.types.get(resolved).kind.clone();
        if !facets.enumeration.is_empty() {
            return self.enumeration(&kind, facets);
        }
        match &mut kind {
            TypeKind::String(string) => {
                let mut min = Some(string.min_length);
                narrow_lengths(&mut min, &mut string.max_length, facets);
                string.min_length = min.unwrap_or(0);
                if let Some(pattern) = anchored(&facets.patterns) {
                    if string.pattern.is_some() {
                        // Both patterns apply.
                        let own = self.alloc(
                            TypeKind::String(StringType {
                                pattern: Some(pattern),
                                ..StringType::default()
                            }),
                            Some(span),
                        );
                        let narrowed = self.alloc(kind, Some(span));
                        return TypeKind::AllOf(vec![narrowed, own]);
                    }
                    string.pattern = Some(pattern);
                }
            }
            TypeKind::Integer(numeric) | TypeKind::Number(numeric) => {
                self.narrow_numbers(numeric, facets, span);
                if facets.fraction_digits == Some(0) && matches!(kind, TypeKind::Number(_)) {
                    let TypeKind::Number(numeric) = kind else {
                        unreachable!()
                    };
                    kind = TypeKind::Integer(numeric);
                }
            }
            TypeKind::Array(array) => {
                let mut min = Some(array.min_items);
                narrow_lengths(&mut min, &mut array.max_items, facets);
                array.min_items = min.unwrap_or(0);
            }
            TypeKind::Any => {
                if let Some(pattern) = anchored(&facets.patterns) {
                    return TypeKind::String(StringType {
                        pattern: Some(pattern),
                        ..StringType::default()
                    });
                }
            }
            _ => {}
        }
        kind
    }

    fn narrow_numbers(&mut self, numeric: &mut NumericType, facets: &Facets, span: Span) {
        let decimal = |text: &Option<String>, facet: &str, this: &mut Self| -> Option<Decimal> {
            let text = text.as_ref()?;
            if let Ok(d) = Decimal::parse(text.trim()) {
                return Some(d);
            }
            this.error(
                codes::INVALID,
                format!("{facet} value {text:?} is not a number"),
                span,
            );
            None
        };
        let lower = [
            (decimal(&facets.min_inclusive, "minInclusive", self), false),
            (decimal(&facets.min_exclusive, "minExclusive", self), true),
        ];
        let upper = [
            (decimal(&facets.max_inclusive, "maxInclusive", self), false),
            (decimal(&facets.max_exclusive, "maxExclusive", self), true),
        ];
        for (value, exclusive) in lower {
            let Some(value) = value else { continue };
            let bound = Bound { value, exclusive };
            if numeric
                .minimum
                .as_ref()
                .is_none_or(|m| tighter_min(&bound, m))
            {
                numeric.minimum = Some(bound);
            }
        }
        for (value, exclusive) in upper {
            let Some(value) = value else { continue };
            let bound = Bound { value, exclusive };
            if numeric
                .maximum
                .as_ref()
                .is_none_or(|m| tighter_max(&bound, m))
            {
                numeric.maximum = Some(bound);
            }
        }
        if let Some(digits) = facets.total_digits.filter(|d| *d > 0 && *d <= 38) {
            // At most `digits` digits in all: |value| < 10^digits.
            let limit = Decimal::from_u128(10u128.pow(u32::try_from(digits).unwrap_or(38)));
            let upper = Bound {
                value: limit.clone(),
                exclusive: true,
            };
            let lower = Bound {
                value: limit.neg(),
                exclusive: true,
            };
            if numeric
                .maximum
                .as_ref()
                .is_none_or(|m| tighter_max(&upper, m))
            {
                numeric.maximum = Some(upper);
            }
            if numeric
                .minimum
                .as_ref()
                .is_none_or(|m| tighter_min(&lower, m))
            {
                numeric.minimum = Some(lower);
            }
        }
        if let Some(digits) = facets.fraction_digits.filter(|d| *d > 0 && *d <= 38)
            && numeric.multiple_of.is_none()
            && let Ok(step) = Decimal::parse(&format!("1e-{digits}"))
        {
            numeric.multiple_of = Some(step);
        }
    }

    /// An enumeration of values of `base`'s kind.
    fn enumeration(&mut self, base: &TypeKind, facets: &Facets) -> TypeKind {
        let mut values = Vec::new();
        for (text, value_span) in &facets.enumeration {
            let literal = match base {
                TypeKind::Integer(_) | TypeKind::Number(_) => {
                    let Ok(d) = Decimal::parse(text.trim()) else {
                        self.error(
                            codes::INVALID,
                            format!("enumeration value {text:?} is not a number"),
                            *value_span,
                        );
                        continue;
                    };
                    Literal::Number(Number::from(d))
                }
                TypeKind::Boolean => Literal::Bool(matches!(text.trim(), "true" | "1")),
                TypeKind::Array(_) => {
                    Literal::Array(text.split_whitespace().map(Literal::string).collect())
                }
                _ => Literal::string(text.clone()),
            };
            let mut value = EnumValue::new(literal);
            value.origin = Some(*value_span);
            if !values.iter().any(|v: &EnumValue| v.value == value.value) {
                values.push(value);
            }
        }
        TypeKind::Enum(EnumType {
            values,
            open: false,
            retired: Retired::default(),
            writer_exact: false,
        })
    }
}

/// XML Schema patterns match whole values; several patterns of one
/// restriction step were already joined as alternatives.
fn anchored(patterns: &[String]) -> Option<String> {
    let pattern = patterns.first()?;
    Some(format!("^(?:{pattern})$"))
}

fn narrow_lengths(min: &mut Option<u64>, max: &mut Option<u64>, facets: &Facets) {
    let lower = facets.length.or(facets.min_length);
    let upper = facets.length.or(facets.max_length);
    if let Some(lower) = lower {
        *min = Some(min.map_or(lower, |m| m.max(lower)));
    }
    if let Some(upper) = upper {
        *max = Some(max.map_or(upper, |m| m.min(upper)));
    }
}

fn tighter_min(candidate: &Bound, current: &Bound) -> bool {
    candidate.value > current.value || (candidate.value == current.value && candidate.exclusive)
}

fn tighter_max(candidate: &Bound, current: &Bound) -> bool {
    candidate.value < current.value || (candidate.value == current.value && candidate.exclusive)
}
