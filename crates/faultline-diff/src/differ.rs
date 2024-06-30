//! The comparison walk's state, and how evidence is attached to changes.
//!
//! Each change describes one local difference (a lowered `maxLength`, a new
//! required member), but whether values are really lost or gained is a
//! property of the whole type at that position: a narrowed bound may be
//! irrelevant if the type is otherwise restricted. Evidence is therefore
//! computed for the node with the inclusion checkers and then *attributed*:
//! a witness counts for a change only if the reason the other version rejects
//! it is the facet that change describes. When the node's witness belongs to
//! another change, a witness specific to this one is searched for.

use std::collections::HashSet;

use std::rc::Rc;

use faultline_ir::{Api, IrPath, Literal, Polarity, Segment, TypeId};
use faultline_subsume::{Checker, Membership, Reason, Sampler, Validator, Verdict};

use crate::change::{Change, ChangeKind, Effect, Evidence, Facet};

/// A position during the walk: segments shared with the parent, so going
/// one level deeper is constant time however deep the walk is. Converted
/// to an [`IrPath`] only when a change is recorded there.
#[derive(Clone, Debug, Default)]
pub(crate) struct Trail(Option<Rc<Link>>);

#[derive(Debug)]
struct Link {
    parent: Trail,
    segment: Segment,
    depth: usize,
}

impl Trail {
    pub(crate) fn new() -> Self {
        Self(None)
    }

    pub(crate) fn join(&self, segment: Segment) -> Trail {
        Trail(Some(Rc::new(Link {
            parent: self.clone(),
            segment,
            depth: self.depth() + 1,
        })))
    }

    pub(crate) fn depth(&self) -> usize {
        self.0.as_ref().map_or(0, |link| link.depth)
    }

    pub(crate) fn to_path(&self) -> IrPath {
        let mut segments = Vec::with_capacity(self.depth());
        let mut at = self;
        while let Some(link) = &at.0 {
            segments.push(link.segment.clone());
            at = &link.parent;
        }
        segments.into_iter().rev().collect()
    }
}

/// Which way a local difference can move the value set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    /// Can only remove values (a tighter bound).
    Narrowing,
    /// Can only add values (a looser bound).
    Widening,
    /// Could do either.
    Either,
}

pub(crate) struct Differ<'a> {
    pub(crate) old: &'a Api,
    pub(crate) new: &'a Api,
    lose: Checker<'a>,
    gain: Checker<'a>,
    /// Values as read by each version.
    old_values: Validator<'a>,
    new_values: Validator<'a>,
    /// Values as written by each version, where witnesses come from.
    old_written: Validator<'a>,
    new_written: Validator<'a>,
    old_samples: Sampler<'a>,
    new_samples: Sampler<'a>,
    visited: HashSet<(TypeId, TypeId, Polarity)>,
    pub(crate) visited_old: HashSet<TypeId>,
    pub(crate) changes: Vec<Change>,
}

impl<'a> Differ<'a> {
    pub(crate) fn new(old: &'a Api, new: &'a Api) -> Self {
        Self {
            old,
            new,
            lose: Checker::new(old, new),
            gain: Checker::new(new, old),
            old_values: Validator::new(old),
            new_values: Validator::new(new),
            old_written: Validator::writer(old),
            new_written: Validator::writer(new),
            old_samples: Sampler::writer(old),
            new_samples: Sampler::writer(new),
            visited: HashSet::new(),
            visited_old: HashSet::new(),
            changes: Vec::new(),
        }
    }

    /// Marks a pair as compared under `polarity`; returns `false` if it
    /// already was, which is what terminates the walk on recursive types.
    pub(crate) fn first_visit(&mut self, old: TypeId, new: TypeId, polarity: Polarity) -> bool {
        self.visited_old.insert(old);
        self.visited.insert((old, new, polarity))
    }

    pub(crate) fn push(&mut self, change: Change) {
        self.changes.push(change);
    }

    /// Records a change to the API's shape (no value-level effect).
    pub(crate) fn surface(
        &mut self,
        kind: ChangeKind,
        path: &Trail,
        polarity: Polarity,
        spans: (
            Option<faultline_source::Span>,
            Option<faultline_source::Span>,
        ),
    ) {
        self.push(Change {
            kind,
            path: path.to_path(),
            polarity,
            old_span: spans.0,
            new_span: spans.1,
            effect: None,
        });
    }

    /// Records the local changes found at one pair of types, attributing
    /// evidence from the node-level comparison.
    pub(crate) fn emit_local(
        &mut self,
        old: TypeId,
        new: TypeId,
        path: &Trail,
        polarity: Polarity,
        local: Vec<(ChangeKind, Direction)>,
    ) {
        if local.is_empty() {
            return;
        }
        let needs_lost = local.iter().any(|(_, d)| *d != Direction::Widening);
        let needs_gained = local.iter().any(|(_, d)| *d != Direction::Narrowing);
        let lost = if needs_lost {
            self.lose.subsumes(old, new)
        } else {
            Verdict::Yes
        };
        let gained = if needs_gained {
            self.gain.subsumes(new, old)
        } else {
            Verdict::Yes
        };
        let spans = (
            self.old.types.get(old).origin,
            self.new.types.get(new).origin,
        );

        let path = path.to_path();
        for (kind, direction) in local {
            let lost = if direction == Direction::Widening {
                Evidence::None
            } else {
                self.attribute(&kind, &lost, old, new, true)
            };
            let gained = if direction == Direction::Narrowing {
                Evidence::None
            } else {
                self.attribute(&kind, &gained, old, new, false)
            };
            self.push(Change {
                kind,
                path: path.clone(),
                polarity,
                old_span: spans.0,
                new_span: spans.1,
                effect: Some(Effect { lost, gained }),
            });
        }
    }

    /// Evidence for `kind` in one direction (`lost`: in old, not in new).
    fn attribute(
        &self,
        kind: &ChangeKind,
        node: &Verdict,
        old: TypeId,
        new: TypeId,
        lost: bool,
    ) -> Evidence {
        match node {
            Verdict::Yes => Evidence::None,
            Verdict::No(w) if self.explains(kind, w, old, new, lost) => {
                Evidence::Witness(w.clone())
            }
            Verdict::No(_) | Verdict::Unknown(_) => self
                .find_explained(kind, old, new, lost)
                .map_or(Evidence::Unknown, Evidence::Witness),
        }
    }

    /// Whether `witness` is rejected by the other version *because of* the
    /// facet `kind` describes.
    fn explains(
        &self,
        kind: &ChangeKind,
        witness: &Literal,
        old: TypeId,
        new: TypeId,
        lost: bool,
    ) -> bool {
        let rejection = if lost {
            self.new_values.validate(new, witness)
        } else {
            self.old_values.validate(old, witness)
        };
        let Membership::Invalid(reason) = rejection else {
            return false;
        };
        explains(kind, &reason, witness)
    }

    /// Searches values of one version, and probes derived from the other,
    /// for a witness the change explains.
    fn find_explained(
        &self,
        kind: &ChangeKind,
        old: TypeId,
        new: TypeId,
        lost: bool,
    ) -> Option<Literal> {
        let (from_values, to_values, from_samples, to_samples, from, to) = if lost {
            (
                self.old_written,
                self.new_values,
                self.old_samples,
                self.new_samples,
                old,
                new,
            )
        } else {
            (
                self.new_written,
                self.old_values,
                self.new_samples,
                self.old_samples,
                new,
                old,
            )
        };
        let mut candidates = from_samples.candidates(from);
        candidates.extend(
            to_samples
                .probes(to)
                .into_iter()
                .filter(|v| from_values.validate(from, v).is_valid()),
        );
        if let ChangeKind::EnumValueRemoved { value } | ChangeKind::EnumValueAdded { value } = kind
        {
            candidates.insert(0, value.clone());
        }
        candidates.into_iter().find(|candidate| {
            from_values.validate(from, candidate).is_valid()
                && matches!(to_values.validate(to, candidate), Membership::Invalid(reason) if explains(kind, &reason, candidate))
        })
    }
}

/// Whether a rejection `reason` is accounted for by change `kind`.
fn explains(kind: &ChangeKind, reason: &Reason, witness: &Literal) -> bool {
    match (kind, reason) {
        (ChangeKind::BoundChanged { facet, .. }, reason) => facet_reason(*facet, reason),
        (
            ChangeKind::EnumValueRemoved { value } | ChangeKind::EnumValueAdded { value },
            Reason::NotInEnum,
        ) => value == witness,
        // A member the new version requires, or one the old version required
        // and the new one no longer declares (a value without it is new).
        (
            ChangeKind::FieldAdded { name, .. }
            | ChangeKind::FieldRequiredChanged { name, .. }
            | ChangeKind::FieldRemoved {
                name,
                required: true,
            },
            Reason::MissingRequired(missing),
        ) => missing.contains(name),
        (
            ChangeKind::FieldRemoved { name, .. } | ChangeKind::FieldAdded { name, .. },
            Reason::UnexpectedProperty(unexpected),
        ) => name == unexpected,
        (
            ChangeKind::FieldGroupChanged { name, .. },
            Reason::ExclusiveMembers { first, second, .. },
        ) => name == first || name == second,
        // Composite changes are explained by any rejection at the node.
        (ChangeKind::UniqueItemsChanged { .. }, Reason::DuplicateItems)
        | (ChangeKind::NumericKindChanged { .. }, Reason::WrongType { .. })
        | (
            ChangeKind::TypeChanged { .. }
            | ChangeKind::ConstraintChanged
            | ChangeKind::VariantAdded { .. }
            | ChangeKind::VariantRemoved { .. }
            | ChangeKind::UnionModeChanged { .. }
            | ChangeKind::PatternChanged { .. },
            _,
        ) => true,
        _ => false,
    }
}

fn facet_reason(facet: Facet, reason: &Reason) -> bool {
    matches!(
        (facet, reason),
        (Facet::Minimum, Reason::BelowMinimum)
            | (Facet::Maximum, Reason::AboveMaximum)
            | (Facet::MultipleOf, Reason::NotMultiple)
            | (Facet::MinLength, Reason::TooShort)
            | (Facet::MaxLength, Reason::TooLong)
            | (Facet::MinItems, Reason::TooFewItems)
            | (Facet::MaxItems, Reason::TooManyItems)
            | (Facet::MinProperties, Reason::TooFewProperties)
            | (Facet::MaxProperties, Reason::TooManyProperties)
            | (
                Facet::MinContains | Facet::MaxContains,
                Reason::ContainsCount
            )
    )
}
