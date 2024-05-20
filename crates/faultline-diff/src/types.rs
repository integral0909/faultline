//! Structural comparison of two types.
//!
//! The walk aligns the two type graphs — members by name, array positions by
//! index, union variants by identity — records what differs at each pair,
//! and recurses. Pairs already compared under the same polarity are skipped,
//! which terminates the walk on recursive types.

use faultline_ir::{
    Access, ArrayType, Bound, EnumType, Field, IrPath, Meta, NumericType, ObjectType, Polarity,
    Retired, Segment, StringType, TypeId, TypeKind, TypeNode, UnionType,
};

use crate::align::align_variants;
use crate::change::{Change, ChangeKind, Facet};
use crate::differ::{Differ, Direction};

type Local = Vec<(ChangeKind, Direction)>;

/// Changes to a node's shape that carry no value-level evidence.
type Shape = Vec<ChangeKind>;

impl Differ<'_> {
    /// Compares the type `old` with `new` at `path`, recording changes.
    pub(crate) fn compare(&mut self, old: TypeId, new: TypeId, path: &IrPath, polarity: Polarity) {
        let old = self.old.types.resolve(old);
        let new = self.new.types.resolve(new);
        if !self.first_visit(old, new, polarity) {
            return;
        }
        let (on, nn) = (self.old.types.get(old), self.new.types.get(new));
        self.meta_changes(on, nn, path, polarity);

        let mut local = Local::new();
        let mut shape = Shape::new();
        match (&on.kind, &nn.kind) {
            (TypeKind::String(a), TypeKind::String(b)) => strings(a, b, &mut local),
            (
                TypeKind::Integer(a) | TypeKind::Number(a),
                TypeKind::Integer(b) | TypeKind::Number(b),
            ) => {
                let (a_int, b_int) = (
                    matches!(on.kind, TypeKind::Integer(_)),
                    matches!(nn.kind, TypeKind::Integer(_)),
                );
                if a_int != b_int {
                    let direction = if b_int {
                        Direction::Narrowing
                    } else {
                        Direction::Widening
                    };
                    local.push((ChangeKind::NumericKindChanged { integer: b_int }, direction));
                }
                numbers(a, b, &mut local);
                if a.encoding != b.encoding {
                    shape.push(ChangeKind::EncodingChanged {
                        old: a.encoding.clone(),
                        new: b.encoding.clone(),
                    });
                }
            }
            (TypeKind::Enum(a), TypeKind::Enum(b)) => enums(a, b, &mut local, &mut shape),
            (TypeKind::Object(a), TypeKind::Object(b)) => {
                self.objects(a, b, path, polarity, &mut local);
                retired_fields(a, b, &mut shape);
            }
            (TypeKind::Array(a), TypeKind::Array(b)) => {
                self.arrays(a, b, path, polarity, &mut local);
            }
            (TypeKind::Map(a), TypeKind::Map(b)) => {
                let (ak, av, bk, bv) = (a.key, a.value, b.key, b.value);
                self.compare(ak, bk, &path.join(Segment::MapKey), polarity);
                self.compare(av, bv, &path.join(Segment::MapValue), polarity);
            }
            (TypeKind::Union(a), TypeKind::Union(b)) => {
                self.unions(a, b, path, polarity, &mut local);
            }
            (TypeKind::Union(a), _) => {
                let wrapped = single(new);
                self.variants(&a.variants, &wrapped, path, polarity, &mut local);
            }
            (_, TypeKind::Union(b)) => {
                let wrapped = single(old);
                self.variants(&wrapped, &b.variants, path, polarity, &mut local);
            }
            (TypeKind::AllOf(a), TypeKind::AllOf(b)) if a.len() == b.len() => {
                for (i, (&x, &y)) in a.iter().zip(b).enumerate() {
                    self.compare(x, y, &path.join(Segment::AllOf(i)), polarity);
                }
            }
            (TypeKind::Not(a), TypeKind::Not(b)) => {
                let (a, b) = (*a, *b);
                self.compare(a, b, &path.join(Segment::Not), polarity.flip());
            }
            (TypeKind::AllOf(_), TypeKind::AllOf(_)) => {
                local.push((ChangeKind::ConstraintChanged, Direction::Either));
            }
            (a, b) if std::mem::discriminant(a) == std::mem::discriminant(b) => {}
            (a, b) => local.push((
                ChangeKind::TypeChanged {
                    old: describe(on, a),
                    new: describe(nn, b),
                },
                Direction::Either,
            )),
        }
        self.emit_local(old, new, path, polarity, local);
        for kind in shape {
            self.surface(kind, path.clone(), polarity, (on.origin, nn.origin));
        }
    }

    fn meta_changes(&mut self, on: &TypeNode, nn: &TypeNode, path: &IrPath, polarity: Polarity) {
        let spans = (on.origin, nn.origin);
        let (a, b): (&Meta, &Meta) = (&on.meta, &nn.meta);
        if a.deprecated != b.deprecated {
            self.surface(
                ChangeKind::DeprecatedChanged {
                    deprecated: b.deprecated,
                },
                path.clone(),
                polarity,
                spans,
            );
        }
        if a.access != b.access {
            self.surface(
                ChangeKind::AccessChanged {
                    old: access_name(a.access).into(),
                    new: access_name(b.access).into(),
                },
                path.clone(),
                polarity,
                spans,
            );
        }
        if a.default != b.default {
            self.surface(
                ChangeKind::DefaultChanged {
                    old: a.default.clone(),
                    new: b.default.clone(),
                },
                path.clone(),
                polarity,
                spans,
            );
        }
    }

    fn objects(
        &mut self,
        a: &ObjectType,
        b: &ObjectType,
        path: &IrPath,
        polarity: Polarity,
        local: &mut Local,
    ) {
        let (old_members, new_members) = (openness(self.old, a), openness(self.new, b));
        for pair in field_pairs(a, b) {
            match pair {
                Pair::Removed(of) => {
                    let kind = ChangeKind::FieldRemoved {
                        name: of.name.clone(),
                        required: of.required,
                    };
                    // Undeclared members that may be anything absorb the
                    // removed field; a closed object now rejects it.
                    let direction = match new_members {
                        Openness::Open => Direction::Widening,
                        Openness::Closed if !of.required => Direction::Narrowing,
                        _ => Direction::Either,
                    };
                    local.push((kind, direction));
                }
                Pair::Added(nf) => {
                    let kind = ChangeKind::FieldAdded {
                        name: nf.name.clone(),
                        required: nf.required,
                    };
                    let direction = match old_members {
                        Openness::Open => Direction::Narrowing,
                        Openness::Closed if !nf.required => Direction::Widening,
                        _ => Direction::Either,
                    };
                    local.push((kind, direction));
                }
                Pair::Both(of, nf) => self.field_pair(a, b, of, nf, path, polarity, local),
            }
        }

        for (pattern, ot) in &a.pattern_properties {
            let segment = Segment::PatternProperty(pattern.clone());
            match b.pattern_properties.iter().find(|(p, _)| p == pattern) {
                Some((_, nt)) => self.compare(*ot, *nt, &path.join(segment), polarity),
                None => self.constraint(path.join(segment), polarity),
            }
        }
        for (pattern, _) in &b.pattern_properties {
            if !a.pattern_properties.iter().any(|(p, _)| p == pattern) {
                self.constraint(
                    path.join(Segment::PatternProperty(pattern.clone())),
                    polarity,
                );
            }
        }
        self.compare(
            a.additional,
            b.additional,
            &path.join(Segment::Additional),
            polarity,
        );
        match (a.property_names, b.property_names) {
            (Some(x), Some(y)) => self.compare(x, y, &path.join(Segment::PropertyNames), polarity),
            (None, None) => {}
            _ => self.constraint(path.join(Segment::PropertyNames), polarity),
        }
        bound_u64(
            Facet::MinProperties,
            Some(a.min_properties).filter(|&m| m > 0),
            Some(b.min_properties).filter(|&m| m > 0),
            true,
            local,
        );
        bound_u64(
            Facet::MaxProperties,
            a.max_properties,
            b.max_properties,
            false,
            local,
        );
        if a.dependent_required != b.dependent_required {
            local.push((ChangeKind::ConstraintChanged, Direction::Either));
        }
    }

    /// Compares a field present in both versions: identity (name, tag,
    /// aliases), presence, grouping, then its type.
    #[allow(clippy::too_many_arguments)]
    fn field_pair(
        &mut self,
        a: &ObjectType,
        b: &ObjectType,
        of: &Field,
        nf: &Field,
        path: &IrPath,
        polarity: Polarity,
        local: &mut Local,
    ) {
        let name = nf.name.clone();
        let at = path.join(Segment::Field(name.clone()));
        let spans = (of.origin, nf.origin);
        if of.name != nf.name {
            let tag = nf.tag.unwrap_or_default();
            let kind = ChangeKind::FieldRenamed {
                old: of.name.clone(),
                new: name.clone(),
                tag,
            };
            self.surface(kind, at.clone(), polarity, spans);
        }
        if let (Some(old), Some(new)) = (of.tag, nf.tag)
            && old != new
        {
            let kind = ChangeKind::FieldTagChanged {
                name: name.clone(),
                old,
                new,
            };
            self.surface(kind, at.clone(), polarity, spans);
        }
        let removed: Vec<String> = of
            .aliases
            .iter()
            .filter(|x| !nf.aliases.contains(x))
            .cloned()
            .collect();
        let added: Vec<String> = nf
            .aliases
            .iter()
            .filter(|x| !of.aliases.contains(x))
            .cloned()
            .collect();
        if !removed.is_empty() || !added.is_empty() {
            let kind = ChangeKind::FieldAliasesChanged {
                name: name.clone(),
                removed,
                added,
            };
            self.surface(kind, at.clone(), polarity, spans);
        }
        // Declarations on the field itself, as opposed to its (possibly
        // shared) type: a deprecated Protobuf field, a proto2 default.
        if of.meta.deprecated != nf.meta.deprecated {
            let kind = ChangeKind::DeprecatedChanged {
                deprecated: nf.meta.deprecated,
            };
            self.surface(kind, at.clone(), polarity, spans);
        }
        if of.meta.default != nf.meta.default {
            let kind = ChangeKind::DefaultChanged {
                old: of.meta.default.clone(),
                new: nf.meta.default.clone(),
            };
            self.surface(kind, at.clone(), polarity, spans);
        }
        if of.required != nf.required {
            let direction = if nf.required {
                Direction::Narrowing
            } else {
                Direction::Widening
            };
            let kind = ChangeKind::FieldRequiredChanged {
                name: name.clone(),
                required: nf.required,
            };
            local.push((kind, direction));
        }
        if let Some(change) = group_change(a, b, of, nf) {
            local.push(change);
        }
        let (ot, nt) = (of.ty, nf.ty);
        let child = self.child_polarity(polarity, ot, nt);
        // Skip a member only when access modifiers removed every direction;
        // unreachable definitions have none to begin with and are still
        // compared.
        if !child.is_none() || polarity.is_none() {
            self.compare(ot, nt, &at, child);
        }
        // Arguments flow from the reader to the producer of the value.
        let args_at = path.join(Segment::Arguments(name));
        match (of.arguments, nf.arguments) {
            (Some(oa), Some(na)) => self.compare(oa, na, &args_at, polarity.flip()),
            (None, None) => {}
            _ => self.constraint(args_at, polarity.flip()),
        }
    }

    fn arrays(
        &mut self,
        a: &ArrayType,
        b: &ArrayType,
        path: &IrPath,
        polarity: Polarity,
        local: &mut Local,
    ) {
        let positions = a.prefix_items.len().max(b.prefix_items.len());
        for i in 0..positions {
            match (a.prefix_items.get(i), b.prefix_items.get(i)) {
                (Some(&x), Some(&y)) => {
                    self.compare(x, y, &path.join(Segment::PrefixItem(i)), polarity);
                }
                (Some(&x), None) => {
                    self.compare(x, b.items, &path.join(Segment::PrefixItem(i)), polarity);
                }
                (None, Some(&y)) => {
                    self.compare(a.items, y, &path.join(Segment::PrefixItem(i)), polarity);
                }
                (None, None) => {}
            }
        }
        self.compare(a.items, b.items, &path.join(Segment::Items), polarity);
        bound_u64(
            Facet::MinItems,
            Some(a.min_items).filter(|&m| m > 0),
            Some(b.min_items).filter(|&m| m > 0),
            true,
            local,
        );
        bound_u64(Facet::MaxItems, a.max_items, b.max_items, false, local);
        if a.unique_items != b.unique_items {
            let direction = if b.unique_items {
                Direction::Narrowing
            } else {
                Direction::Widening
            };
            local.push((
                ChangeKind::UniqueItemsChanged {
                    unique: b.unique_items,
                },
                direction,
            ));
        }
        match (&a.contains, &b.contains) {
            (Some(x), Some(y)) => {
                self.compare(x.ty, y.ty, &path.join(Segment::Contains), polarity);
                bound_u64(Facet::MinContains, Some(x.min), Some(y.min), true, local);
                bound_u64(Facet::MaxContains, x.max, y.max, false, local);
            }
            (None, None) => {}
            _ => local.push((ChangeKind::ConstraintChanged, Direction::Either)),
        }
    }

    fn unions(
        &mut self,
        a: &UnionType,
        b: &UnionType,
        path: &IrPath,
        polarity: Polarity,
        local: &mut Local,
    ) {
        if a.mode != b.mode {
            local.push((
                ChangeKind::UnionModeChanged {
                    old: format!("{:?}", a.mode).to_lowercase(),
                    new: format!("{:?}", b.mode).to_lowercase(),
                },
                Direction::Either,
            ));
        }
        self.discriminators(a, b, path, polarity);
        self.variants(&a.variants, &b.variants, path, polarity, local);
    }

    /// Compares discriminator properties and value mappings. Mapping targets
    /// are compared by the name of the schema they select.
    fn discriminators(&mut self, a: &UnionType, b: &UnionType, path: &IrPath, polarity: Polarity) {
        let (old_api, new_api) = (self.old, self.new);
        let property = |u: &UnionType| u.discriminator.as_ref().map(|d| d.property.clone());
        if property(a) != property(b) {
            let kind = ChangeKind::DiscriminatorChanged {
                old: property(a),
                new: property(b),
            };
            self.surface(kind, path.clone(), polarity, (None, None));
            return;
        }
        let (Some(da), Some(db)) = (&a.discriminator, &b.discriminator) else {
            return;
        };
        let target = |api: &faultline_ir::Api, id: TypeId| {
            let node = api.types.get(api.types.resolve(id));
            node.name
                .as_ref()
                .map_or_else(|| node.kind.describe().to_owned(), ToString::to_string)
        };
        for (value, old_target) in &da.mapping {
            let kind = match db.mapping.iter().find(|(v, _)| v == value) {
                None => ChangeKind::DiscriminatorValueRemoved {
                    value: value.clone(),
                },
                Some((_, new_target)) => {
                    let old_name = target(old_api, *old_target);
                    let new_name = target(new_api, *new_target);
                    if old_name == new_name {
                        continue;
                    }
                    ChangeKind::DiscriminatorValueRetargeted {
                        value: value.clone(),
                        old: old_name,
                        new: new_name,
                    }
                }
            };
            self.surface(kind, path.clone(), polarity, (None, None));
        }
        for (value, _) in &db.mapping {
            if !da.mapping.iter().any(|(v, _)| v == value) {
                let kind = ChangeKind::DiscriminatorValueAdded {
                    value: value.clone(),
                };
                self.surface(kind, path.clone(), polarity, (None, None));
            }
        }
    }

    fn variants(
        &mut self,
        a: &[TypeId],
        b: &[TypeId],
        path: &IrPath,
        polarity: Polarity,
        local: &mut Local,
    ) {
        for (x, y) in align_variants(self.old, a, self.new, b) {
            match (x, y) {
                (Some(i), Some(j)) => {
                    let segment = if b.len() > 1 {
                        Some(Segment::Variant(j))
                    } else {
                        None
                    };
                    let child = segment.map_or_else(|| path.clone(), |s| path.join(s));
                    self.compare(a[i], b[j], &child, polarity);
                }
                (Some(i), None) => {
                    local.push((ChangeKind::VariantRemoved { index: i }, Direction::Either));
                }
                (None, Some(j)) => {
                    local.push((ChangeKind::VariantAdded { index: j }, Direction::Either));
                }
                (None, None) => {}
            }
        }
    }

    fn constraint(&mut self, path: IrPath, polarity: Polarity) {
        self.push(Change {
            kind: ChangeKind::ConstraintChanged,
            path,
            polarity,
            old_span: None,
            new_span: None,
            effect: None,
        });
    }

    /// The polarity for a member: directions in which either version reads
    /// or writes it. A member made `readOnly` in the new version is still
    /// compared as a request member, because the old version accepted it.
    fn child_polarity(&self, polarity: Polarity, old: TypeId, new: TypeId) -> Polarity {
        let access =
            |api: &faultline_ir::Api, id: TypeId| api.types.get(api.types.resolve(id)).meta.access;
        polarity
            .restrict(access(self.old, old))
            .union(polarity.restrict(access(self.new, new)))
    }
}

fn single(id: TypeId) -> Vec<TypeId> {
    vec![id]
}

fn describe(node: &TypeNode, kind: &TypeKind) -> String {
    match &node.name {
        Some(name) => format!("{} {name}", kind.describe()),
        None => kind.describe().to_owned(),
    }
}

fn access_name(access: Access) -> &'static str {
    match access {
        Access::ReadWrite => "read-write",
        Access::ReadOnly => "read-only",
        Access::WriteOnly => "write-only",
    }
}

fn strings(a: &StringType, b: &StringType, local: &mut Local) {
    bound_u64(
        Facet::MinLength,
        Some(a.min_length).filter(|&m| m > 0),
        Some(b.min_length).filter(|&m| m > 0),
        true,
        local,
    );
    bound_u64(Facet::MaxLength, a.max_length, b.max_length, false, local);
    if a.pattern != b.pattern {
        let direction = match (&a.pattern, &b.pattern) {
            (None, Some(_)) => Direction::Narrowing,
            (Some(_), None) => Direction::Widening,
            _ => Direction::Either,
        };
        local.push((
            ChangeKind::PatternChanged {
                old: a.pattern.clone(),
                new: b.pattern.clone(),
            },
            direction,
        ));
    }
    format_change(a.format.as_ref(), b.format.as_ref(), local);
}

fn numbers(a: &NumericType, b: &NumericType, local: &mut Local) {
    bound(
        Facet::Minimum,
        a.minimum.as_ref(),
        b.minimum.as_ref(),
        true,
        local,
    );
    bound(
        Facet::Maximum,
        a.maximum.as_ref(),
        b.maximum.as_ref(),
        false,
        local,
    );
    if a.multiple_of != b.multiple_of {
        let direction = match (&a.multiple_of, &b.multiple_of) {
            (None, Some(_)) => Direction::Narrowing,
            (Some(_), None) => Direction::Widening,
            _ => Direction::Either,
        };
        local.push((
            ChangeKind::BoundChanged {
                facet: Facet::MultipleOf,
                old: a.multiple_of.as_ref().map(ToString::to_string),
                new: b.multiple_of.as_ref().map(ToString::to_string),
            },
            direction,
        ));
    }
    format_change(a.format.as_ref(), b.format.as_ref(), local);
}

fn format_change(a: Option<&String>, b: Option<&String>, local: &mut Local) {
    if a != b {
        local.push((
            ChangeKind::FormatChanged {
                old: a.cloned(),
                new: b.cloned(),
            },
            Direction::Either,
        ));
    }
}

/// Numbers of removed fields that were not retired, retired numbers that a
/// new field reuses, and retirements that were dropped.
fn retired_fields(a: &ObjectType, b: &ObjectType, shape: &mut Shape) {
    for pair in field_pairs(a, b) {
        match pair {
            Pair::Removed(of) => {
                if let Some(tag) = of.tag
                    && !b.retired.contains_number(i64::from(tag))
                {
                    shape.push(ChangeKind::NumberNotRetired {
                        name: of.name.clone(),
                        number: i64::from(tag),
                    });
                }
            }
            Pair::Added(nf) => {
                if let Some(tag) = nf.tag
                    && (a.retired.contains_number(i64::from(tag))
                        || a.retired.contains_name(&nf.name))
                {
                    shape.push(ChangeKind::RetiredNumberReused {
                        name: nf.name.clone(),
                        number: i64::from(tag),
                    });
                }
            }
            Pair::Both(..) => {}
        }
    }
    retirements(&a.retired, &b.retired, shape);
}

/// Retired numbers and names that are retired no longer.
fn retirements(a: &Retired, b: &Retired, shape: &mut Shape) {
    for &(lo, hi) in &a.ranges {
        if !b.covers(lo, hi) {
            let what = match (lo, hi) {
                (lo, hi) if lo == hi => format!("number {lo}"),
                (lo, i64::MAX) => format!("numbers {lo} to max"),
                (lo, hi) => format!("numbers {lo} to {hi}"),
            };
            shape.push(ChangeKind::RetirementRemoved { what });
        }
    }
    for name in &a.names {
        if !b.contains_name(name) {
            shape.push(ChangeKind::RetirementRemoved {
                what: format!("name `{name}`"),
            });
        }
    }
}

/// What an object says about members it does not declare.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Openness {
    /// Any member with any value.
    Open,
    /// No other members.
    Closed,
    /// Something in between (typed or patterned members, size limits).
    Constrained,
}

fn openness(api: &faultline_ir::Api, o: &ObjectType) -> Openness {
    if !o.pattern_properties.is_empty() || o.property_names.is_some() || o.max_properties.is_some()
    {
        return Openness::Constrained;
    }
    match api.types.get(api.types.resolve(o.additional)).kind {
        TypeKind::Any => Openness::Open,
        TypeKind::Never => Openness::Closed,
        _ => Openness::Constrained,
    }
}

/// How a field of the old version corresponds to one of the new.
enum Pair<'a> {
    Removed(&'a Field),
    Added(&'a Field),
    Both(&'a Field, &'a Field),
}

/// Aligns the fields of two objects. When every field on both sides has a
/// wire tag, fields are the same field if their tags agree (a renamed field
/// keeps its data); a name that moved to another tag is still paired, so the
/// move is reported as such rather than as a removal and an addition.
/// Otherwise fields are aligned by name.
fn field_pairs<'a>(a: &'a ObjectType, b: &'a ObjectType) -> Vec<Pair<'a>> {
    let tagged = |o: &ObjectType| !o.fields.is_empty() && o.fields.iter().all(|f| f.tag.is_some());
    let mut pairs = Vec::new();
    if tagged(a) && tagged(b) {
        let mut used = vec![false; b.fields.len()];
        let mut unmatched = Vec::new();
        for of in &a.fields {
            match b.fields.iter().position(|nf| nf.tag == of.tag) {
                Some(i) => {
                    used[i] = true;
                    pairs.push(Pair::Both(of, &b.fields[i]));
                }
                None => unmatched.push(of),
            }
        }
        for of in unmatched {
            match b
                .fields
                .iter()
                .enumerate()
                .find(|(i, nf)| !used[*i] && nf.name == of.name)
            {
                Some((i, nf)) => {
                    used[i] = true;
                    pairs.push(Pair::Both(of, nf));
                }
                None => pairs.push(Pair::Removed(of)),
            }
        }
        for (i, nf) in b.fields.iter().enumerate() {
            if !used[i] {
                pairs.push(Pair::Added(nf));
            }
        }
    } else {
        for of in &a.fields {
            match b.field(&of.name) {
                Some(nf) => pairs.push(Pair::Both(of, nf)),
                None => pairs.push(Pair::Removed(of)),
            }
        }
        for nf in &b.fields {
            if a.field(&nf.name).is_none() {
                pairs.push(Pair::Added(nf));
            }
        }
    }
    pairs
}

/// A change in which fields `nf` excludes, compared with those `of`
/// excluded, and which way it moves the value set: excluding more fields
/// only removes values, excluding fewer only adds them. Group names are
/// compared too: a renamed group is visible in generated code.
fn group_change(
    a: &ObjectType,
    b: &ObjectType,
    of: &Field,
    nf: &Field,
) -> Option<(ChangeKind, Direction)> {
    let (og, ng) = (a.group_of(&of.name), b.group_of(&nf.name));
    let members = |o: &ObjectType, g: Option<&faultline_ir::ExclusiveGroup>, own: &str| {
        let mut names: Vec<String> = g
            .map(|g| g.fields.iter().filter(|f| *f != own).cloned().collect())
            .unwrap_or_default();
        // Only fields that exist in both versions can be compared.
        names.retain(|n| o.field(n).is_some());
        names.sort();
        names
    };
    let old_members: Vec<String> = members(a, og, &of.name)
        .into_iter()
        .filter(|n| b.field(n).is_some())
        .collect();
    let new_members: Vec<String> = members(b, ng, &nf.name)
        .into_iter()
        .filter(|n| a.field(n).is_some())
        .collect();
    let (old_name, new_name) = (og.map(|g| g.name.clone()), ng.map(|g| g.name.clone()));
    if old_name == new_name && old_members == new_members {
        return None;
    }
    let covers = |outer: &[String], inner: &[String]| inner.iter().all(|n| outer.contains(n));
    let direction = if covers(&new_members, &old_members) {
        Direction::Narrowing
    } else if covers(&old_members, &new_members) {
        Direction::Widening
    } else {
        Direction::Either
    };
    let kind = ChangeKind::FieldGroupChanged {
        name: nf.name.clone(),
        old: old_name,
        new: new_name,
    };
    Some((kind, direction))
}

fn enums(a: &EnumType, b: &EnumType, local: &mut Local, shape: &mut Shape) {
    // Where values are numbered (Protobuf), a number that is now spelled
    // differently is one renamed value, not a removal and an addition.
    let renamed = |value: &faultline_ir::EnumValue, other: &EnumType| {
        value.number.is_some()
            && other
                .values
                .iter()
                .any(|v| v.number == value.number && v.name != value.name)
    };
    for value in &a.values {
        let (Some(name), Some(number)) = (&value.name, value.number) else {
            continue;
        };
        for other in &b.values {
            if other.name.as_ref() == Some(name)
                && let Some(new) = other.number.filter(|n| *n != number)
            {
                shape.push(ChangeKind::EnumValueRenumbered {
                    name: name.clone(),
                    old: number,
                    new,
                });
            }
            if other.number == Some(number)
                && let Some(new) = other.name.as_ref().filter(|n| *n != name)
                && !a.values.iter().any(|v| v.name.as_ref() == Some(new))
            {
                shape.push(ChangeKind::EnumValueRenamed {
                    number,
                    old: name.clone(),
                    new: new.clone(),
                });
            }
        }
    }
    for value in &a.values {
        if let (Some(name), Some(number)) = (&value.name, value.number)
            && !b.values.iter().any(|v| v.number == Some(number))
            && !b.retired.contains_number(number)
        {
            shape.push(ChangeKind::NumberNotRetired {
                name: name.clone(),
                number,
            });
        }
    }
    for value in &b.values {
        if let (Some(name), Some(number)) = (&value.name, value.number)
            && !a.values.iter().any(|v| v.number == Some(number))
            && (a.retired.contains_number(number) || a.retired.contains_name(name))
        {
            shape.push(ChangeKind::RetiredNumberReused {
                name: name.clone(),
                number,
            });
        }
    }
    retirements(&a.retired, &b.retired, shape);
    for value in &a.values {
        if !b.values.iter().any(|v| v.value == value.value) && !renamed(value, b) {
            local.push((
                ChangeKind::EnumValueRemoved {
                    value: value.value.clone(),
                },
                Direction::Narrowing,
            ));
        }
    }
    for value in &b.values {
        if !a.values.iter().any(|v| v.value == value.value) && !renamed(value, a) {
            local.push((
                ChangeKind::EnumValueAdded {
                    value: value.value.clone(),
                },
                Direction::Widening,
            ));
        }
    }
    if a.open != b.open {
        let direction = if b.open {
            Direction::Widening
        } else {
            Direction::Narrowing
        };
        local.push((ChangeKind::ConstraintChanged, direction));
    }
}

/// Records a change of a numeric bound. `lower` bounds narrow when raised,
/// upper bounds when lowered; adding a bound narrows, removing one widens.
fn bound(facet: Facet, a: Option<&Bound>, b: Option<&Bound>, lower: bool, local: &mut Local) {
    if a == b {
        return;
    }
    let direction = match (a, b) {
        (None, Some(_)) => Direction::Narrowing,
        (Some(_), None) => Direction::Widening,
        (Some(x), Some(y)) => {
            let tighter = match x.value.cmp(&y.value) {
                std::cmp::Ordering::Equal => y.exclusive && !x.exclusive,
                std::cmp::Ordering::Less => lower,
                std::cmp::Ordering::Greater => !lower,
            };
            if tighter {
                Direction::Narrowing
            } else {
                Direction::Widening
            }
        }
        (None, None) => return,
    };
    let show = |b: &Bound| {
        if b.exclusive {
            format!("{} (exclusive)", b.value)
        } else {
            b.value.to_string()
        }
    };
    local.push((
        ChangeKind::BoundChanged {
            facet,
            old: a.map(show),
            new: b.map(show),
        },
        direction,
    ));
}

/// Records a change of a size bound (`None` means no bound).
fn bound_u64(facet: Facet, a: Option<u64>, b: Option<u64>, lower: bool, local: &mut Local) {
    if a == b {
        return;
    }
    let direction = match (a, b) {
        (None, Some(_)) => Direction::Narrowing,
        (Some(_), None) => Direction::Widening,
        (Some(x), Some(y)) => {
            if (y > x) == lower {
                Direction::Narrowing
            } else {
                Direction::Widening
            }
        }
        (None, None) => return,
    };
    local.push((
        ChangeKind::BoundChanged {
            facet,
            old: a.map(|v| v.to_string()),
            new: b.map(|v| v.to_string()),
        },
        direction,
    ));
}
