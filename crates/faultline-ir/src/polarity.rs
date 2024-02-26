//! Polarity: which direction values flow through each type.
//!
//! Compatibility is directional. For a position that receives values from
//! consumers (*in*: request bodies, parameters), a new version must accept
//! everything the old one accepted. For a position that sends values to
//! consumers (*out*: responses), a new version must produce nothing the old
//! one could not. A type reachable both ways must keep its exact value set.
//!
//! Polarity is computed by propagating from every surface root through the
//! type graph:
//!
//! * callbacks swap the directions of the operations they contain;
//! * `not` swaps direction for its operand ([`Edge::inverts`]);
//! * `readOnly` removes *in* and `writeOnly` removes *out* at the node that
//!   declares it — such values are ignored in that direction, so the subtree
//!   below them is not reached that way.

use std::fmt;

use crate::api::Api;
use crate::arena::TypeId;
use crate::graph::{Edge, for_each_edge};
use crate::types::Access;

/// A set of flow directions.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Polarity(u8);

impl Polarity {
    pub const NONE: Polarity = Polarity(0);
    pub const IN: Polarity = Polarity(1);
    pub const OUT: Polarity = Polarity(2);
    pub const BOTH: Polarity = Polarity(3);

    pub fn is_in(self) -> bool {
        self.0 & Self::IN.0 != 0
    }

    pub fn is_out(self) -> bool {
        self.0 & Self::OUT.0 != 0
    }

    pub fn is_none(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn union(self, other: Polarity) -> Polarity {
        Polarity(self.0 | other.0)
    }

    #[must_use]
    pub fn contains(self, other: Polarity) -> bool {
        self.0 & other.0 == other.0
    }

    /// Swaps *in* and *out*.
    #[must_use]
    pub fn flip(self) -> Polarity {
        Polarity(((self.0 & 1) << 1) | ((self.0 & 2) >> 1))
    }

    /// Removes the directions in which values at a position with `access`
    /// are ignored.
    #[must_use]
    pub fn restrict(self, access: Access) -> Polarity {
        match access {
            Access::ReadWrite => self,
            Access::ReadOnly => Polarity(self.0 & !Self::IN.0),
            Access::WriteOnly => Polarity(self.0 & !Self::OUT.0),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self.0 {
            0 => "none",
            1 => "in",
            2 => "out",
            _ => "both",
        }
    }
}

impl fmt::Display for Polarity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Polarity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Polarity({})", self.as_str())
    }
}

/// The polarity of every type node in an API.
#[derive(Clone, Debug)]
pub struct Polarities {
    by_type: Vec<Polarity>,
}

impl Polarities {
    /// Propagates polarity from every root of `api`.
    ///
    /// Each node is visited at most once per direction, so the cost is linear
    /// in the size of the graph even when it is cyclic.
    pub fn compute(api: &Api) -> Polarities {
        let mut by_type = vec![Polarity::NONE; api.types.len()];
        let mut work: Vec<(TypeId, Polarity)> = api
            .roots()
            .into_iter()
            .map(|root| (root.ty, root.polarity))
            .collect();

        while let Some((id, incoming)) = work.pop() {
            let Some(node) = api.types.try_get(id) else {
                continue;
            };
            let arriving = incoming.restrict(node.meta.access);
            let current = by_type[id.index()];
            // Only the directions not yet recorded need to be propagated.
            let new = Polarity(arriving.0 & !current.0);
            if new.is_none() {
                continue;
            }
            by_type[id.index()] = current.union(new);
            for_each_edge(node, |edge: Edge<'_>, child| {
                let polarity = if edge.inverts() { new.flip() } else { new };
                work.push((child, polarity));
            });
        }
        Polarities { by_type }
    }

    pub fn get(&self, id: TypeId) -> Polarity {
        self.by_type
            .get(id.index())
            .copied()
            .unwrap_or(Polarity::NONE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_operations() {
        assert_eq!(Polarity::IN.union(Polarity::OUT), Polarity::BOTH);
        assert_eq!(Polarity::IN.flip(), Polarity::OUT);
        assert_eq!(Polarity::BOTH.flip(), Polarity::BOTH);
        assert_eq!(Polarity::NONE.flip(), Polarity::NONE);
        assert_eq!(Polarity::BOTH.restrict(Access::ReadOnly), Polarity::OUT);
        assert_eq!(Polarity::BOTH.restrict(Access::WriteOnly), Polarity::IN);
        assert_eq!(Polarity::IN.restrict(Access::ReadOnly), Polarity::NONE);
        assert!(Polarity::BOTH.contains(Polarity::IN));
        assert!(!Polarity::IN.contains(Polarity::BOTH));
    }
}
