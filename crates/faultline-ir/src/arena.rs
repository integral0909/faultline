use std::fmt;

use crate::types::{TypeKind, TypeNode};

/// Index of a node in a [`TypeArena`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(u32);

impl TypeId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

impl fmt::Debug for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.0)
    }
}

/// Storage for the type graph of one API.
///
/// Front ends build recursive types by [reserving](Self::reserve) an id for a
/// named type before lowering its body, so references inside the body can
/// point at it, and [defining](Self::define) it afterwards. A reserved slot that
/// is never defined is reported by [`validate`](crate::validate).
#[derive(Clone, Debug, Default)]
pub struct TypeArena {
    nodes: Vec<Option<TypeNode>>,
}

impl TypeArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, node: TypeNode) -> TypeId {
        let id = self.next_id();
        self.nodes.push(Some(node));
        id
    }

    /// Shorthand for allocating an anonymous node with default metadata.
    pub fn alloc_kind(&mut self, kind: TypeKind) -> TypeId {
        self.alloc(TypeNode::new(kind))
    }

    /// Allocates an id whose node will be supplied later with
    /// [`define`](Self::define).
    pub fn reserve(&mut self) -> TypeId {
        let id = self.next_id();
        self.nodes.push(None);
        id
    }

    /// Supplies the node for a reserved id.
    ///
    /// # Panics
    ///
    /// If `id` was already defined; that is a front-end bug, not an input
    /// problem.
    pub fn define(&mut self, id: TypeId, node: TypeNode) {
        let slot = &mut self.nodes[id.index()];
        assert!(slot.is_none(), "type {id:?} defined twice");
        *slot = Some(node);
    }

    /// The node for `id`.
    ///
    /// # Panics
    ///
    /// If `id` is reserved but not yet defined, or out of range. A validated
    /// API never triggers either.
    pub fn get(&self, id: TypeId) -> &TypeNode {
        self.try_get(id)
            .unwrap_or_else(|| panic!("type {id:?} is undefined"))
    }

    pub fn get_mut(&mut self, id: TypeId) -> &mut TypeNode {
        self.nodes
            .get_mut(id.index())
            .and_then(Option::as_mut)
            .unwrap_or_else(|| panic!("type {id:?} is undefined"))
    }

    /// The node for `id`, or `None` if it is out of range or undefined.
    pub fn try_get(&self, id: TypeId) -> Option<&TypeNode> {
        self.nodes.get(id.index()).and_then(Option::as_ref)
    }

    pub fn contains(&self, id: TypeId) -> bool {
        id.index() < self.nodes.len()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// All ids in allocation order.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = TypeId> + use<> {
        (0..self.nodes.len() as u32).map(TypeId)
    }

    /// Follows `Ref` indirections to the first non-reference node.
    ///
    /// Stops after as many steps as there are nodes, so a (malformed) cycle of
    /// references cannot loop forever; validation reports such cycles.
    pub fn resolve(&self, mut id: TypeId) -> TypeId {
        for _ in 0..self.nodes.len() {
            match self.try_get(id).map(|n| &n.kind) {
                Some(TypeKind::Ref(target)) => id = *target,
                _ => break,
            }
        }
        id
    }

    /// Moves every node of `other` into this arena, renumbering the ids
    /// they refer to, and returns the mapping for ids of `other` (its roots
    /// and named types). Front ends use it to embed a type graph lowered by
    /// another front end, such as an Avro payload in an AsyncAPI message.
    pub fn graft(&mut self, other: TypeArena) -> Graft {
        let offset = self.next_id().0;
        let graft = Graft { offset };
        for slot in other.nodes {
            self.nodes.push(slot.map(|mut node| {
                crate::graph::for_each_id_mut(&mut node.kind, |id| *id = graft.map(*id));
                node
            }));
        }
        graft
    }

    fn next_id(&self) -> TypeId {
        TypeId(u32::try_from(self.nodes.len()).expect("more than u32::MAX types"))
    }
}

/// How ids of a grafted arena map into the arena it was grafted onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Graft {
    offset: u32,
}

impl Graft {
    pub fn map(self, id: TypeId) -> TypeId {
        TypeId(id.0 + self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grafting_renumbers_references() {
        use crate::types::{Discriminator, ObjectType, UnionMode, UnionType};
        let mut host = TypeArena::new();
        host.alloc_kind(TypeKind::Null);
        let mut guest = TypeArena::new();
        let any = guest.alloc_kind(TypeKind::Any);
        let node = guest.reserve();
        let object = guest.alloc_kind(TypeKind::Object(ObjectType::new(any)));
        guest.define(
            node,
            TypeNode::new(TypeKind::Union(UnionType {
                mode: UnionMode::Tagged,
                variants: vec![object, node],
                discriminator: Some(Discriminator {
                    property: "kind".into(),
                    mapping: vec![("o".into(), object)],
                }),
            })),
        );
        let graft = host.graft(guest);
        assert_eq!(host.len(), 4);
        let TypeKind::Union(union) = &host.get(graft.map(node)).kind else {
            panic!()
        };
        assert_eq!(union.variants, [graft.map(object), graft.map(node)]);
        assert_eq!(
            union.discriminator.as_ref().unwrap().mapping[0].1,
            graft.map(object)
        );
        let TypeKind::Object(obj) = &host.get(graft.map(object)).kind else {
            panic!()
        };
        assert_eq!(obj.additional, graft.map(any));
        assert_eq!(graft.map(any), TypeId(1));
    }

    #[test]
    fn reserve_then_define_supports_recursion() {
        let mut arena = TypeArena::new();
        let list = arena.reserve();
        let next = arena.alloc_kind(TypeKind::Ref(list));
        arena.define(list, TypeNode::new(TypeKind::AllOf(vec![next])));
        assert_eq!(arena.get(next).kind, TypeKind::Ref(list));
        assert_eq!(arena.resolve(next), list);
    }

    #[test]
    fn undefined_slots_are_visible_through_try_get() {
        let mut arena = TypeArena::new();
        let id = arena.reserve();
        assert!(arena.try_get(id).is_none());
        assert!(arena.contains(id));
        assert!(!arena.contains(TypeId::from_raw(5)));
    }

    #[test]
    fn resolve_terminates_on_reference_cycles() {
        let mut arena = TypeArena::new();
        let a = arena.reserve();
        let b = arena.alloc_kind(TypeKind::Ref(a));
        arena.define(a, TypeNode::new(TypeKind::Ref(b)));
        let _ = arena.resolve(a);
    }

    #[test]
    #[should_panic(expected = "defined twice")]
    fn double_definition_is_a_bug() {
        let mut arena = TypeArena::new();
        let id = arena.reserve();
        arena.define(id, TypeNode::new(TypeKind::Any));
        arena.define(id, TypeNode::new(TypeKind::Any));
    }
}
