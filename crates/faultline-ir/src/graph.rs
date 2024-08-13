//! The edges of the type graph.
//!
//! Every algorithm that walks types — polarity, validation, dumping, and later
//! comparison — enumerates children through [`for_each_edge`], so the graph
//! structure is defined in exactly one place.

use crate::arena::TypeId;
use crate::types::{TypeKind, TypeNode};

/// How a child type is reached from its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge<'a> {
    /// A declared object member.
    Field(&'a str),
    /// The arguments of a declared member (GraphQL field arguments).
    Arguments(&'a str),
    /// Members not matched by fields or pattern properties.
    Additional,
    /// Members whose names match the pattern at this index.
    PatternProperty(usize, &'a str),
    /// The constraint on member names.
    PropertyNames,
    /// Array items after the prefix.
    Items,
    PrefixItem(usize),
    Contains,
    MapKey,
    MapValue,
    Variant(usize),
    AllOf(usize),
    Not,
    Ref,
}

impl Edge<'_> {
    /// Whether the edge passes through a value constructor.
    ///
    /// A recursive type is well-founded only if every cycle crosses at least
    /// one guarded edge: `{ next: Self }` describes values of finite depth,
    /// while `Self = allOf[Self]` describes nothing at all. Comparison relies
    /// on this when it assumes compatibility for pairs it is already
    /// comparing.
    pub fn is_guarded(self) -> bool {
        !matches!(
            self,
            Edge::Variant(_) | Edge::AllOf(_) | Edge::Not | Edge::Ref
        )
    }

    /// Whether values on the child side flow in the opposite direction.
    ///
    /// `not` inverts set inclusion: widening `T` narrows `not T`. Polarity is
    /// flipped across such edges so that the direction of the required
    /// inclusion is right for the operand. Field arguments flow from the
    /// reader of a value to its producer, the opposite way from the value.
    pub fn inverts(self) -> bool {
        matches!(self, Edge::Not | Edge::Arguments(_))
    }
}

/// Calls `visit` for every child of `node`, in a deterministic order.
pub fn for_each_edge<'a>(node: &'a TypeNode, mut visit: impl FnMut(Edge<'a>, TypeId)) {
    match &node.kind {
        TypeKind::Any
        | TypeKind::Never
        | TypeKind::Null
        | TypeKind::Boolean
        | TypeKind::Integer(_)
        | TypeKind::Number(_)
        | TypeKind::String(_)
        | TypeKind::Bytes
        | TypeKind::Enum(_) => {}
        TypeKind::Object(obj) => {
            for field in &obj.fields {
                visit(Edge::Field(&field.name), field.ty);
                if let Some(arguments) = field.arguments {
                    visit(Edge::Arguments(&field.name), arguments);
                }
            }
            for (i, (pattern, ty)) in obj.pattern_properties.iter().enumerate() {
                visit(Edge::PatternProperty(i, pattern), *ty);
            }
            visit(Edge::Additional, obj.additional);
            if let Some(names) = obj.property_names {
                visit(Edge::PropertyNames, names);
            }
        }
        TypeKind::Array(arr) => {
            for (i, ty) in arr.prefix_items.iter().enumerate() {
                visit(Edge::PrefixItem(i), *ty);
            }
            visit(Edge::Items, arr.items);
            if let Some(contains) = &arr.contains {
                visit(Edge::Contains, contains.ty);
            }
        }
        TypeKind::Map(map) => {
            visit(Edge::MapKey, map.key);
            visit(Edge::MapValue, map.value);
        }
        TypeKind::Union(union) => {
            for (i, ty) in union.variants.iter().enumerate() {
                visit(Edge::Variant(i), *ty);
            }
        }
        TypeKind::AllOf(members) => {
            for (i, ty) in members.iter().enumerate() {
                visit(Edge::AllOf(i), *ty);
            }
        }
        TypeKind::Not(operand) => visit(Edge::Not, *operand),
        TypeKind::Ref(target) => visit(Edge::Ref, *target),
    }
}

/// Calls `visit` with every type id `kind` holds, mutably: its edges, and
/// the targets of discriminator mappings, which are not edges. Used to
/// renumber a type graph moved into another arena.
pub fn for_each_id_mut(kind: &mut TypeKind, mut visit: impl FnMut(&mut TypeId)) {
    match kind {
        TypeKind::Any
        | TypeKind::Never
        | TypeKind::Null
        | TypeKind::Boolean
        | TypeKind::Integer(_)
        | TypeKind::Number(_)
        | TypeKind::String(_)
        | TypeKind::Bytes
        | TypeKind::Enum(_) => {}
        TypeKind::Object(obj) => {
            for field in &mut obj.fields {
                visit(&mut field.ty);
                if let Some(arguments) = &mut field.arguments {
                    visit(arguments);
                }
            }
            for (_, ty) in &mut obj.pattern_properties {
                visit(ty);
            }
            visit(&mut obj.additional);
            if let Some(names) = &mut obj.property_names {
                visit(names);
            }
        }
        TypeKind::Array(arr) => {
            arr.prefix_items.iter_mut().for_each(&mut visit);
            visit(&mut arr.items);
            if let Some(contains) = &mut arr.contains {
                visit(&mut contains.ty);
            }
        }
        TypeKind::Map(map) => {
            visit(&mut map.key);
            visit(&mut map.value);
        }
        TypeKind::Union(union) => {
            union.variants.iter_mut().for_each(&mut visit);
            if let Some(discriminator) = &mut union.discriminator {
                for (_, ty) in &mut discriminator.mapping {
                    visit(ty);
                }
            }
        }
        TypeKind::AllOf(members) => members.iter_mut().for_each(&mut visit),
        TypeKind::Not(operand) | TypeKind::Ref(operand) => visit(operand),
    }
}

/// The children of `node` as a vector, for callers that need to iterate more
/// than once or hold edges across mutation.
pub fn edges(node: &TypeNode) -> Vec<(Edge<'_>, TypeId)> {
    let mut out = Vec::new();
    for_each_edge(node, |edge, id| out.push((edge, id)));
    out
}
