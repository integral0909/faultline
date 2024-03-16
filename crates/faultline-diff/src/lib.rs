//! Comparison of two versions of an API.
//!
//! [`diff`] walks the old and the new [`Api`] in parallel and produces a
//! [`ChangeSet`]: every difference in the API's surface (operations,
//! parameters, responses, ...) and in the types at each position, with the
//! polarity of that position and, for value-level changes, an [`Effect`]
//! saying which values were lost and which were gained, backed by verified
//! witnesses where they exist.
//!
//! The diff describes; it does not judge. Whether a change is breaking is
//! decided by the rule catalog from the change kind, its polarity and its
//! effect.

mod align;
mod change;
mod differ;
mod surfaces;
mod types;

use faultline_ir::{Api, IrPath};

pub use change::{Change, ChangeKind, Effect, Evidence, Facet};

/// Every difference between two versions, ordered by path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub changes: Vec<Change>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Change> {
        self.changes.iter()
    }
}

impl<'a> IntoIterator for &'a ChangeSet {
    type Item = &'a Change;
    type IntoIter = std::slice::Iter<'a, Change>;

    fn into_iter(self) -> Self::IntoIter {
        self.changes.iter()
    }
}

/// Compares `old` with `new`.
pub fn diff(old: &Api, new: &Api) -> ChangeSet {
    let mut differ = differ::Differ::new(old, new);
    differ.operations(&old.operations, &new.operations, &IrPath::new(), false);
    differ.schemas();
    differ.unreachable_definitions();
    let mut changes = differ.changes;
    // Stable: changes at the same path keep the order the walk found them in.
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    ChangeSet { changes }
}
