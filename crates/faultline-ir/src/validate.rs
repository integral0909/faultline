//! Structural invariants of a lowered [`Api`].
//!
//! Front ends guarantee these for well-formed input and turn violations they
//! can attribute to the user into diagnostics. Validation exists to catch the
//! rest before the comparison engine, which relies on them, sees the graph.

use std::collections::HashSet;
use std::fmt;

use crate::api::Api;
use crate::arena::TypeId;
use crate::graph::edges;
use crate::literal::Literal;
use crate::path::IrPath;
use crate::types::{QualName, TypeKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrError {
    /// A reserved type was never defined.
    UndefinedType {
        id: TypeId,
    },
    /// A type or root refers to an id outside the arena.
    DanglingReference {
        from: Option<TypeId>,
        to: TypeId,
    },
    /// A cycle that never passes through a value constructor, such as
    /// `A = allOf[A]`. `cycle` lists the types in order.
    UnguardedCycle {
        cycle: Vec<TypeId>,
    },
    DuplicateField {
        ty: TypeId,
        name: String,
    },
    /// An exclusive group names a field the object does not declare, or a
    /// field belongs to two groups.
    InvalidExclusiveGroup {
        ty: TypeId,
        group: String,
        field: String,
    },
    DuplicateEnumValue {
        ty: TypeId,
        value: Literal,
    },
    DuplicateOperation {
        identity: String,
    },
    DuplicateRoot {
        path: IrPath,
    },
    /// `named` maps a name to a node whose own name differs.
    NameMismatch {
        name: QualName,
        ty: TypeId,
    },
}

impl fmt::Display for IrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IrError::UndefinedType { id } => {
                write!(f, "type {id:?} was reserved but never defined")
            }
            IrError::DanglingReference {
                from: Some(from),
                to,
            } => {
                write!(f, "type {from:?} refers to nonexistent type {to:?}")
            }
            IrError::DanglingReference { from: None, to } => {
                write!(f, "a root refers to nonexistent type {to:?}")
            }
            IrError::UnguardedCycle { cycle } => {
                write!(
                    f,
                    "types {cycle:?} form a cycle without an object field or array item"
                )
            }
            IrError::DuplicateField { ty, name } => {
                write!(f, "type {ty:?} declares field \"{name}\" twice")
            }
            IrError::InvalidExclusiveGroup { ty, group, field } => write!(
                f,
                "exclusive group {group} of type {ty:?} lists \"{field}\", which is undeclared or in another group"
            ),
            IrError::DuplicateEnumValue { ty, value } => {
                write!(f, "enum {ty:?} lists {value} twice")
            }
            IrError::DuplicateOperation { identity } => {
                write!(f, "operation {identity} is declared twice")
            }
            IrError::DuplicateRoot { path } => write!(f, "root {path} is declared twice"),
            IrError::NameMismatch { name, ty } => {
                write!(
                    f,
                    "named type {name} points at {ty:?}, which has a different name"
                )
            }
        }
    }
}

impl std::error::Error for IrError {}

/// Checks every invariant and returns all violations, in a deterministic
/// order. An empty result means the API is safe to compare.
pub fn validate(api: &Api) -> Vec<IrError> {
    let mut errors = Vec::new();
    check_nodes(api, &mut errors);
    check_roots(api, &mut errors);
    check_names(api, &mut errors);
    // Cycle detection walks edges, which is only meaningful once every edge
    // target is known to exist.
    if errors.is_empty() {
        check_unguarded_cycles(api, &mut errors);
    }
    errors
}

fn check_nodes(api: &Api, errors: &mut Vec<IrError>) {
    for id in api.types.ids() {
        let Some(node) = api.types.try_get(id) else {
            errors.push(IrError::UndefinedType { id });
            continue;
        };
        for (_, child) in edges(node) {
            if !api.types.contains(child) {
                errors.push(IrError::DanglingReference {
                    from: Some(id),
                    to: child,
                });
            }
        }
        match &node.kind {
            TypeKind::Object(obj) => {
                let mut seen = HashSet::new();
                for field in &obj.fields {
                    if !seen.insert(field.name.as_str()) {
                        errors.push(IrError::DuplicateField {
                            ty: id,
                            name: field.name.clone(),
                        });
                    }
                }
                let mut grouped = HashSet::new();
                for group in &obj.exclusive {
                    for field in &group.fields {
                        if !seen.contains(field.as_str()) || !grouped.insert(field.as_str()) {
                            errors.push(IrError::InvalidExclusiveGroup {
                                ty: id,
                                group: group.name.clone(),
                                field: field.clone(),
                            });
                        }
                    }
                }
            }
            TypeKind::Enum(e) => {
                let mut seen = HashSet::new();
                for value in &e.values {
                    if !seen.insert(&value.value) {
                        errors.push(IrError::DuplicateEnumValue {
                            ty: id,
                            value: value.value.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_roots(api: &Api, errors: &mut Vec<IrError>) {
    let mut operations = HashSet::new();
    let mut duplicate_operations = false;
    for op in &api.operations {
        let identity = op.key.identity();
        if !operations.insert(identity.clone()) {
            duplicate_operations = true;
            errors.push(IrError::DuplicateOperation { identity });
        }
    }
    let mut paths = HashSet::new();
    for root in api.roots() {
        if !api.types.contains(root.ty) {
            errors.push(IrError::DanglingReference {
                from: None,
                to: root.ty,
            });
        }
        // Duplicate operations already produce duplicate roots; report the
        // operation once rather than every position inside it.
        if !paths.insert(root.path.clone()) && !duplicate_operations {
            errors.push(IrError::DuplicateRoot { path: root.path });
        }
    }
}

fn check_names(api: &Api, errors: &mut Vec<IrError>) {
    for (name, &ty) in &api.named {
        let matches = api
            .types
            .try_get(ty)
            .is_some_and(|node| node.name.as_ref() == Some(name));
        if !matches {
            errors.push(IrError::NameMismatch {
                name: name.clone(),
                ty,
            });
        }
    }
}

/// Finds cycles in the subgraph of unguarded edges with an iterative
/// three-colour depth-first search. Each cycle is reported once, starting at
/// the node where the search re-entered it.
fn check_unguarded_cycles(api: &Api, errors: &mut Vec<IrError>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Colour {
        White,
        Grey,
        Black,
    }
    let unguarded_children = |id: TypeId| -> Vec<TypeId> {
        edges(api.types.get(id))
            .into_iter()
            .filter(|(edge, _)| !edge.is_guarded())
            .map(|(_, child)| child)
            .collect()
    };

    let mut colour = vec![Colour::White; api.types.len()];
    for start in api.types.ids() {
        if colour[start.index()] != Colour::White {
            continue;
        }
        // Stack of (node, its unguarded children, index of the next child).
        let mut stack: Vec<(TypeId, Vec<TypeId>, usize)> =
            vec![(start, unguarded_children(start), 0)];
        colour[start.index()] = Colour::Grey;
        while let Some((node, children, next)) = stack.last_mut() {
            let Some(&child) = children.get(*next) else {
                colour[node.index()] = Colour::Black;
                stack.pop();
                continue;
            };
            *next += 1;
            match colour[child.index()] {
                Colour::White => {
                    colour[child.index()] = Colour::Grey;
                    stack.push((child, unguarded_children(child), 0));
                }
                Colour::Grey => {
                    let from = stack
                        .iter()
                        .position(|(id, _, _)| *id == child)
                        .expect("grey nodes are on the stack");
                    errors.push(IrError::UnguardedCycle {
                        cycle: stack[from..].iter().map(|(id, _, _)| *id).collect(),
                    });
                }
                Colour::Black => {}
            }
        }
    }
}
