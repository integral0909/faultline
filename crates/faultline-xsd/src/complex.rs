//! Complex types and elements.
//!
//! An element of complex type is compared as an object:
//!
//! * child elements are members named by their local name; an element that
//!   may be absent (`minOccurs="0"`, or inside an optional group or a
//!   `choice`) is optional, and one that may repeat is an array whose item
//!   count follows `minOccurs` and `maxOccurs`;
//! * attributes are members named `@name`, required when `use="required"`;
//! * the text of simple content, or of mixed content, is the member
//!   `#text`;
//! * the options of a `choice` form an exclusive group;
//! * `xs:any` allows other members, `xs:anyAttribute` other `@` members;
//!   otherwise objects are closed, as validation rejects undeclared
//!   content.
//!
//! An element of simple type is its value. `nillable` adds `null`.

use std::collections::HashSet;

use faultline_ir::{
    ArrayType, ExclusiveGroup, Field, Literal, ObjectType, TypeId, TypeKind, TypeNode, UnionMode,
    UnionType,
};
use faultline_source::Span;

use crate::builtins::is_xs;
use crate::codes;
use crate::lower::{Lowerer, type_name};
use crate::model::{
    AttributeUse, ComplexType, ContentModel, Derivation, ElementDecl, Occurs, Particle, QName,
    Term, TypeDef, TypeRef,
};

/// Groups nest at most this deep, which also stops circular groups.
const MAX_GROUP_DEPTH: usize = 64;

/// The members of an object being built.
#[derive(Default)]
struct Members {
    fields: Vec<Field>,
    exclusive: Vec<ExclusiveGroup>,
    /// Whether a `sequence` fixes the elements' order.
    ordered: bool,
    any_element: bool,
    any_attribute: bool,
    text: Option<TypeId>,
}

/// How a particle is nested: whether it may be absent and whether it may
/// repeat, because of the particles around it.
#[derive(Clone, Copy)]
struct Context {
    optional: bool,
    repeated: bool,
    depth: usize,
}

impl Lowerer<'_> {
    /// The type of a declared element: its value type, or `null` too when
    /// it is nillable.
    pub fn element_type(&mut self, decl: &ElementDecl) -> TypeId {
        let ty = match &decl.ty {
            None => self.any(),
            Some(reference) => self.type_ref(reference),
        };
        if decl.nillable {
            let null = self.alloc(TypeKind::Null, Some(decl.span));
            return self.alloc(
                TypeKind::Union(UnionType {
                    mode: UnionMode::Any,
                    variants: vec![ty, null],
                    discriminator: None,
                }),
                Some(decl.span),
            );
        }
        if let Some(fixed) = &decl.fixed {
            return self.fixed(ty, fixed, decl.span);
        }
        ty
    }

    /// The type a reference names, simple or complex.
    pub fn type_ref(&mut self, reference: &TypeRef) -> TypeId {
        match reference {
            TypeRef::Named(name, span) => {
                if !is_xs(name.namespace.as_deref())
                    && matches!(self.components.types.get(name), Some(TypeDef::Complex(_)))
                {
                    return self.complex_named(name);
                }
                self.simple_named(name, *span)
            }
            TypeRef::Anonymous(def) => match def.as_ref() {
                TypeDef::Simple(_) => self.simple_ref(reference),
                TypeDef::Complex(complex) => {
                    let kind = self.complex_kind(complex, &mut HashSet::new());
                    let mut node = TypeNode::new(kind).with_origin(complex.span);
                    node.meta.description.clone_from(&complex.documentation);
                    self.api.types.alloc(node)
                }
            },
        }
    }

    /// The named complex type `name`, lowered on first use. Its id is
    /// reserved first, so recursive types refer to themselves.
    pub fn complex_named(&mut self, name: &QName) -> TypeId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.api.types.reserve();
        self.ids.insert(name.clone(), id);
        self.api.named.insert(type_name(name), id);
        let Some(TypeDef::Complex(complex)) = self.components.types.get(name) else {
            unreachable!("only declared complex types are lowered here")
        };
        let complex: &ComplexType = complex;
        let kind = self.complex_kind(complex, &mut HashSet::new());
        let mut node = TypeNode::new(kind).with_origin(complex.span);
        node.meta.description.clone_from(&complex.documentation);
        self.api.types.define(id, node.with_name(type_name(name)));
        id
    }

    fn complex_kind(&mut self, complex: &ComplexType, bases: &mut HashSet<QName>) -> TypeKind {
        let mut members = Members::default();
        self.complex_members(complex, &mut members, bases);
        let additional = if members.any_element {
            self.any()
        } else {
            self.alloc(TypeKind::Never, None)
        };
        let mut object = ObjectType::new(additional);
        if let Some(text) = members.text {
            let mut field = Field::new("#text", text, !complex.mixed);
            field.origin = Some(complex.span);
            members.fields.insert(0, field);
        }
        if members.ordered {
            object.order = members
                .fields
                .iter()
                .filter(|f| !f.name.starts_with('@') && !f.name.starts_with('#'))
                .map(|f| f.name.clone())
                .collect();
        }
        object.fields = members.fields;
        object.exclusive = members.exclusive;
        if members.any_attribute && !members.any_element {
            let any = self.any();
            object.pattern_properties.push(("^@".to_owned(), any));
        }
        TypeKind::Object(object)
    }

    /// Adds the members `complex` contributes, its base's first.
    fn complex_members(
        &mut self,
        complex: &ComplexType,
        members: &mut Members,
        bases: &mut HashSet<QName>,
    ) {
        if let Some((derivation, base, span)) = &complex.base {
            if !bases.insert(base.clone()) {
                self.error(
                    codes::INVALID,
                    format!("type {base} derives from itself"),
                    *span,
                );
                return;
            }
            match self.components.types.get(base) {
                Some(TypeDef::Complex(parent)) => {
                    let parent: &ComplexType = parent;
                    let mut inherited = Members::default();
                    self.complex_members(parent, &mut inherited, bases);
                    // A restriction restates the content it keeps; an
                    // extension adds to its base's. Attributes are inherited
                    // either way.
                    if *derivation == Derivation::Extension {
                        members.fields = inherited.fields;
                        members.exclusive = inherited.exclusive;
                        members.any_element = inherited.any_element;
                        members.text = inherited.text;
                    } else {
                        members.fields = inherited
                            .fields
                            .into_iter()
                            .filter(|f| f.name.starts_with('@'))
                            .collect();
                        members.text = inherited.text;
                    }
                    members.any_attribute |= inherited.any_attribute;
                }
                Some(TypeDef::Simple(_)) => {}
                None if is_xs(base.namespace.as_deref()) => {}
                None => self.error(
                    codes::UNRESOLVED,
                    format!("type {base} is not declared"),
                    *span,
                ),
            }
        }
        match &complex.content {
            ContentModel::Empty => {}
            ContentModel::Simple { base, facets } => {
                // The base is a simple type, or a complex type with simple
                // content whose text was inherited above.
                let text = match base {
                    TypeRef::Named(name, _)
                        if matches!(self.components.types.get(name), Some(TypeDef::Complex(_))) =>
                    {
                        members.text
                    }
                    _ => Some(self.simple_ref(base)),
                };
                if let Some(text) = text {
                    let kind = self.restrict(text, facets, complex.span);
                    members.text = Some(self.alloc(kind, Some(complex.span)));
                }
            }
            ContentModel::Elements(particle) => {
                let context = Context {
                    optional: false,
                    repeated: false,
                    depth: 0,
                };
                self.particle(particle, context, members);
            }
        }
        if complex.mixed && members.text.is_none() {
            let text = self.alloc(TypeKind::String(faultline_ir::StringType::default()), None);
            members.text = Some(text);
        }
        self.attribute_uses(&complex.attributes, members, 0);
        members.any_attribute |= complex.any_attribute;
    }

    fn particle(&mut self, particle: &Particle, outer: Context, members: &mut Members) {
        if outer.depth > MAX_GROUP_DEPTH {
            self.error(
                codes::INVALID,
                "model groups nest too deeply (a group may refer to itself)".into(),
                particle.span,
            );
            return;
        }
        let occurs = particle.occurs;
        let context = Context {
            optional: outer.optional || occurs.min == 0,
            repeated: outer.repeated || occurs.repeats(),
            depth: outer.depth + 1,
        };
        match &particle.term {
            Term::Element(decl) => self.element_member(decl, occurs, outer, members),
            Term::ElementRef(name) => {
                let Some(decl) = self.components.elements.get(name) else {
                    self.error(
                        codes::UNRESOLVED,
                        format!("element {name} is not declared"),
                        particle.span,
                    );
                    return;
                };
                let decl = decl.clone();
                self.element_member(&decl, occurs, outer, members);
            }
            Term::Sequence(items) | Term::All(items) => {
                members.ordered |= matches!(particle.term, Term::Sequence(_)) && items.len() > 1;
                for item in items {
                    self.particle(item, context, members);
                }
            }
            Term::Choice(options) => {
                let before = members.fields.len();
                let optional = Context {
                    optional: true,
                    ..context
                };
                for option in options {
                    self.particle(option, optional, members);
                }
                let names: Vec<String> = members.fields[before..]
                    .iter()
                    .map(|f| f.name.clone())
                    .collect();
                // A repeated choice may pick different options each time.
                if names.len() > 1 && !context.repeated {
                    members.exclusive.push(ExclusiveGroup {
                        name: format!("choice at {}", names.join("|")),
                        fields: names,
                        origin: Some(particle.span),
                    });
                }
            }
            Term::GroupRef(name) => {
                let Some(group) = self.components.groups.get(name) else {
                    self.error(
                        codes::UNRESOLVED,
                        format!("group {name} is not declared"),
                        particle.span,
                    );
                    return;
                };
                let group = group.clone();
                self.particle(&group, context, members);
            }
            Term::Any => members.any_element = true,
        }
    }

    /// Adds an element as a member.
    fn element_member(
        &mut self,
        decl: &ElementDecl,
        occurs: Occurs,
        outer: Context,
        members: &mut Members,
    ) {
        let value = self.element_type(decl);
        let repeated = outer.repeated || occurs.repeats();
        let optional = outer.optional || occurs.min == 0;
        let name = decl.name.local.clone();
        if let Some(existing) = members.fields.iter_mut().find(|f| f.name == name) {
            // The same element in several places (`<a/><b/><a/>`) occurs
            // several times.
            let resolved = self.api.types.resolve(existing.ty);
            if !matches!(self.api.types.get(resolved).kind, TypeKind::Array(_)) {
                let array = TypeKind::Array(ArrayType::of(existing.ty));
                existing.ty = self.alloc(array, Some(decl.span));
            }
            return;
        }
        let ty = if repeated {
            let array = ArrayType {
                min_items: if outer.repeated || optional {
                    0
                } else {
                    occurs.min
                },
                max_items: if outer.repeated { None } else { occurs.max },
                ..ArrayType::of(value)
            };
            self.alloc(TypeKind::Array(array), Some(decl.span))
        } else {
            value
        };
        let mut field = Field::new(name, ty, !optional);
        field.origin = Some(decl.span);
        field.meta.description.clone_from(&decl.documentation);
        if let Some(default) = &decl.default {
            field.meta.default = Some(Literal::string(default.clone()));
        }
        members.fields.push(field);
    }

    /// Adds attribute uses as `@name` members, overriding inherited ones
    /// with the same name and removing prohibited ones.
    fn attribute_uses(&mut self, uses: &[AttributeUse], members: &mut Members, depth: usize) {
        for attribute in uses {
            match attribute {
                AttributeUse::Declared {
                    name,
                    ty,
                    required,
                    prohibited,
                    default,
                    fixed,
                    span,
                } => {
                    let value = match ty {
                        Some(reference) => self.simple_ref(reference),
                        None => self.any(),
                    };
                    self.attribute_member(
                        &name.local,
                        value,
                        *required,
                        *prohibited,
                        default.as_deref(),
                        fixed.as_deref(),
                        *span,
                        members,
                    );
                }
                AttributeUse::Ref {
                    name,
                    required,
                    prohibited,
                    span,
                } => {
                    let Some(AttributeUse::Declared {
                        ty, default, fixed, ..
                    }) = self.components.attributes.get(name).cloned()
                    else {
                        self.error(
                            codes::UNRESOLVED,
                            format!("attribute {name} is not declared"),
                            *span,
                        );
                        continue;
                    };
                    let value = match &ty {
                        Some(reference) => self.simple_ref(reference),
                        None => self.any(),
                    };
                    self.attribute_member(
                        &name.local,
                        value,
                        *required,
                        *prohibited,
                        default.as_deref(),
                        fixed.as_deref(),
                        *span,
                        members,
                    );
                }
                AttributeUse::GroupRef(name, span) => {
                    if depth > MAX_GROUP_DEPTH {
                        self.error(
                            codes::INVALID,
                            "attribute groups nest too deeply (a group may refer to itself)".into(),
                            *span,
                        );
                        continue;
                    }
                    let Some((group, any)) = self.components.attribute_groups.get(name).cloned()
                    else {
                        self.error(
                            codes::UNRESOLVED,
                            format!("attribute group {name} is not declared"),
                            *span,
                        );
                        continue;
                    };
                    members.any_attribute |= any;
                    self.attribute_uses(&group, members, depth + 1);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn attribute_member(
        &mut self,
        local: &str,
        value: TypeId,
        required: bool,
        prohibited: bool,
        default: Option<&str>,
        fixed: Option<&str>,
        span: Span,
        members: &mut Members,
    ) {
        let name = format!("@{local}");
        members.fields.retain(|f| f.name != name);
        if prohibited {
            return;
        }
        let value = match fixed {
            Some(fixed) => self.fixed(value, fixed, span),
            None => value,
        };
        let mut field = Field::new(name, value, required);
        field.origin = Some(span);
        if let Some(default) = default {
            field.meta.default = Some(Literal::string(default));
        }
        members.fields.push(field);
    }

    /// A value that must be exactly `fixed`: an enum of that one value, of
    /// the type's kind.
    fn fixed(&mut self, ty: TypeId, fixed: &str, span: Span) -> TypeId {
        let facets = crate::model::Facets {
            enumeration: vec![(fixed.to_owned(), span)],
            ..crate::model::Facets::default()
        };
        let kind = self.restrict(ty, &facets, span);
        self.alloc(kind, Some(span))
    }
}
