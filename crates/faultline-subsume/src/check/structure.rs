//! Inclusion for arrays and objects.
//!
//! Structural types are compared position by position (array items) and
//! member by member (object properties), recursing through the checker so
//! that recursive types are handled coinductively. A counterexample found for
//! a nested position is embedded into a value of the enclosing type and
//! re-validated, so the witness reported for an object is a whole object.

use faultline_ir::{ArrayType, Literal, ObjectType, TypeId};

use super::{Checker, Verdict};

impl Checker<'_> {
    pub(super) fn arrays(&mut self, l: TypeId, a: &ArrayType, r: TypeId, b: &ArrayType) -> Verdict {
        let mut proven = a.min_items >= b.min_items
            && b.max_items
                .is_none_or(|b_max| a.max_items.is_some_and(|a_max| a_max <= b_max));
        if b.unique_items && !a.unique_items && a.max_items.is_none_or(|max| max > 1) {
            proven = false;
        }
        if let Some(bc) = &b.contains {
            // Items matching the left `contains` also match the right one, so
            // the right minimum is met; nothing bounds how many other items
            // match the right schema, so a right maximum cannot be proven.
            let covered = bc.max.is_none()
                && a.contains
                    .as_ref()
                    .is_some_and(|ac| ac.min >= bc.min && self.subsumes(ac.ty, bc.ty).is_yes());
            if !covered {
                proven = false;
            }
        }

        let positions = a.prefix_items.len().max(b.prefix_items.len());
        let reachable = |i: usize| a.max_items.is_none_or(|max| (i as u64) < max);
        for i in (0..=positions).take_while(|&i| reachable(i)) {
            // Index `positions` stands for every index after the prefixes.
            let left_item = a.prefix_items.get(i).copied().unwrap_or(a.items);
            let right_item = b.prefix_items.get(i).copied().unwrap_or(b.items);
            match self.subsumes(left_item, right_item) {
                Verdict::Yes => {}
                Verdict::No(w) => {
                    if let Some(witness) = self.embed_item(l, r, i, &w) {
                        return Verdict::No(witness);
                    }
                    proven = false;
                }
                Verdict::Unknown(_) => proven = false,
            }
        }
        if proven {
            return Verdict::Yes;
        }
        if let Some(witness) = self.longer_than(l, a, r, b) {
            return Verdict::No(witness);
        }
        self.search(l, r, "array constraints could not be compared exactly")
    }

    /// When the right type allows fewer items than the left, an array of
    /// the left type one item too long for the right. Sampling only builds
    /// short arrays, so a limit of a hundred would otherwise go unproven.
    fn longer_than(&self, l: TypeId, a: &ArrayType, r: TypeId, b: &ArrayType) -> Option<Literal> {
        const MAX_LENGTH: u64 = 10_000;
        let limit = b.max_items?;
        if a.max_items.is_some_and(|max| max <= limit) || limit >= MAX_LENGTH || a.unique_items {
            return None;
        }
        let length = usize::try_from((limit + 1).max(a.min_items)).ok()?;
        self.left_samples
            .candidates(l)
            .into_iter()
            .find_map(|base| {
                let Literal::Array(mut items) = base else {
                    return None;
                };
                if items.is_empty() || items.len() > length {
                    return None;
                }
                let prefix = a.prefix_items.len();
                // Items past the prefixes all have the same type, so any of
                // them can be repeated.
                let filler = items.get(prefix).or(items.last())?.clone();
                items.resize(length, filler);
                let candidate = Literal::Array(items);
                self.confirmed(l, r, &candidate).then_some(candidate)
            })
    }

    pub(super) fn objects(
        &mut self,
        l: TypeId,
        a: &ObjectType,
        r: TypeId,
        b: &ObjectType,
    ) -> Verdict {
        let mut proven = true;
        let names: Vec<&str> = a
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .chain(
                b.fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .filter(|n| a.field(n).is_none()),
            )
            .collect();

        // Left values are as written: an exact writer produces every declared
        // member and no other.
        let exact = a.writer_exact;
        for name in names {
            let (mut lf, rf) = (a.field(name), b.field(name));
            // A right field reads a left one written under one of its
            // aliases; the pair is compared under the right field's name.
            if rf.is_none() && b.fields.iter().any(|f| f.aliases.iter().any(|x| x == name)) {
                continue;
            }
            if lf.is_none()
                && let Some(rf) = rf
            {
                lf = a.fields.iter().find(|f| rf.aliases.contains(&f.name));
            }
            let left_name = lf.map_or(name, |f| f.name.as_str());
            let left_required = lf.is_some_and(|f| f.required || exact);
            let right_required = rf.is_some_and(|f| f.required);
            if right_required && !left_required {
                if let Some(w) = self.embed_without(l, r, name) {
                    return Verdict::No(w);
                }
                proven = false;
            }
            if exact && lf.is_none() {
                // Never written, so nothing to compare.
                continue;
            }
            let left_ty = lf
                .map(|f| f.ty)
                .or_else(|| a.pattern_properties.is_empty().then_some(a.additional));
            let right_ty = rf
                .map(|f| f.ty)
                .or_else(|| b.pattern_properties.is_empty().then_some(b.additional));
            let (Some(left_ty), Some(right_ty)) = (left_ty, right_ty) else {
                proven = false;
                continue;
            };
            match self.subsumes(left_ty, right_ty) {
                Verdict::Yes => {}
                Verdict::No(w) => {
                    if let Some(witness) = self.embed_member(l, r, left_name, &w) {
                        return Verdict::No(witness);
                    }
                    proven = false;
                }
                Verdict::Unknown(_) => proven = false,
            }
        }

        // Members declared by neither side: an exact writer produces none.
        if exact {
            return if proven && self.implies_member_constraints(a, b) {
                Verdict::Yes
            } else {
                self.search(l, r, "object constraints could not be compared exactly")
            };
        }
        match self.undeclared(l, a, r, b) {
            Verdict::Yes => {}
            Verdict::No(witness) => return Verdict::No(witness),
            Verdict::Unknown(_) => proven = false,
        }

        if let Some(right_names) = b.property_names {
            let covered = a
                .property_names
                .is_some_and(|left_names| self.subsumes(left_names, right_names).is_yes());
            if !covered {
                proven = false;
            }
        }
        if a.min_properties < b.min_properties
            || b.max_properties
                .is_some_and(|b_max| a.max_properties.is_none_or(|a_max| a_max > b_max))
        {
            proven = false;
        }
        if !self.implies_member_constraints(a, b) {
            proven = false;
        }

        if proven {
            Verdict::Yes
        } else {
            self.search(l, r, "object constraints could not be compared exactly")
        }
    }

    /// Members neither object declares: pattern properties and additional
    /// members.
    fn undeclared(&mut self, l: TypeId, a: &ObjectType, r: TypeId, b: &ObjectType) -> Verdict {
        let same_patterns = a.pattern_properties.len() == b.pattern_properties.len()
            && a.pattern_properties
                .iter()
                .zip(&b.pattern_properties)
                .all(|((pa, _), (pb, _))| pa == pb);
        if !same_patterns {
            return Verdict::Unknown("the pattern properties differ".into());
        }
        let mut proven = true;
        for ((_, ta), (_, tb)) in a.pattern_properties.iter().zip(&b.pattern_properties) {
            if !self.subsumes(*ta, *tb).is_yes() {
                proven = false;
            }
        }
        match self.subsumes(a.additional, b.additional) {
            Verdict::Yes => {}
            Verdict::No(w) if a.pattern_properties.is_empty() => {
                let key = unused_name(a, b);
                if let Some(witness) = self.embed_member(l, r, &key, &w) {
                    return Verdict::No(witness);
                }
                proven = false;
            }
            _ => proven = false,
        }
        if proven {
            Verdict::Yes
        } else {
            Verdict::Unknown("undeclared members could not be compared exactly".into())
        }
    }

    /// Whether every constraint `b` places on which members appear together
    /// (exclusive groups, dependent requirements) already holds for `a`.
    fn implies_member_constraints(&self, a: &ObjectType, b: &ObjectType) -> bool {
        // Every pair of members the right side excludes must be excluded on
        // the left too; the samples include objects with every optional
        // member, so a search finds the pair otherwise.
        let groups = b.exclusive.iter().all(|group| {
            let possible: Vec<&String> = group
                .fields
                .iter()
                .filter(|name| a.field(name).is_some() || !self.left_is_empty(a.additional))
                .collect();
            possible.len() < 2
                || a.group_of(possible[0])
                    .is_some_and(|left| possible.iter().all(|name| left.fields.contains(name)))
        });
        let dependencies = b.dependent_required.iter().all(|(property, required)| {
            required.iter().all(|name| {
                a.field(name).is_some_and(|f| f.required)
                    || a.dependent_required
                        .iter()
                        .any(|(p, reqs)| p == property && reqs.contains(name))
            })
        });
        groups && dependencies
    }

    /// An array of the left type with `witness` at index `index`, rejected by
    /// the right type.
    fn embed_item(&self, l: TypeId, r: TypeId, index: usize, witness: &Literal) -> Option<Literal> {
        self.left_samples
            .candidates(l)
            .into_iter()
            .find_map(|base| {
                let Literal::Array(mut items) = base else {
                    return None;
                };
                while items.len() < index {
                    let filler = items.first()?.clone();
                    items.push(filler);
                }
                if items.len() == index {
                    items.push(witness.clone());
                } else {
                    items[index] = witness.clone();
                }
                let candidate = Literal::Array(items);
                self.confirmed(l, r, &candidate).then_some(candidate)
            })
    }

    /// An object of the left type with member `name` set to `witness`,
    /// rejected by the right type.
    fn embed_member(&self, l: TypeId, r: TypeId, name: &str, witness: &Literal) -> Option<Literal> {
        self.left_samples
            .candidates(l)
            .into_iter()
            .find_map(|base| {
                let Literal::Object(mut members) = base else {
                    return None;
                };
                members.insert(name.to_owned(), witness.clone());
                let candidate = Literal::Object(members);
                self.confirmed(l, r, &candidate).then_some(candidate)
            })
    }

    /// An object of the left type without member `name`, rejected by the
    /// right type.
    fn embed_without(&self, l: TypeId, r: TypeId, name: &str) -> Option<Literal> {
        self.left_samples
            .candidates(l)
            .into_iter()
            .find_map(|base| {
                let Literal::Object(mut members) = base else {
                    return None;
                };
                members.remove(name);
                let candidate = Literal::Object(members);
                self.confirmed(l, r, &candidate).then_some(candidate)
            })
    }
}

/// A member name declared by neither object.
fn unused_name(a: &ObjectType, b: &ObjectType) -> String {
    let mut name = String::from("extra");
    while a.field(&name).is_some() || b.field(&name).is_some() {
        name.push('_');
    }
    name
}
