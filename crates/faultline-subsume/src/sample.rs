//! Generation of concrete values belonging to a type.
//!
//! Counterexamples are found by generating values that belong to one type and
//! checking whether they belong to another. Candidates are chosen where
//! constraints are most likely to differ between versions: at numeric and
//! length boundaries, with only required members or with every member, with
//! duplicated items. Every candidate returned by [`Sampler::candidates`] has
//! been confirmed [`Valid`](crate::Membership::Valid) by the validator.

use std::collections::BTreeMap;

use faultline_doc::Decimal;
use faultline_ir::{
    Api, ArrayType, Literal, NumericType, ObjectType, StringType, TypeId, TypeKind,
};

use crate::rational::Q;
use crate::validate::{Validator, object};

const MAX_DEPTH: usize = 6;
const MAX_CANDIDATES: usize = 24;

/// Produces values of the types in one [`Api`].
#[derive(Clone, Copy, Debug)]
pub struct Sampler<'a> {
    api: &'a Api,
    validator: Validator<'a>,
}

impl<'a> Sampler<'a> {
    /// A sampler of values as read.
    pub fn new(api: &'a Api) -> Self {
        Self {
            api,
            validator: Validator::new(api),
        }
    }

    /// A sampler of values as written (see [`Validator::writer`]).
    pub fn writer(api: &'a Api) -> Self {
        Self {
            api,
            validator: Validator::writer(api),
        }
    }

    /// Distinct values confirmed to belong to `ty`, most "typical" first.
    pub fn candidates(&self, ty: TypeId) -> Vec<Literal> {
        self.valid(ty, 0)
    }

    /// Unverified values near the constraints of `ty`. They are not
    /// necessarily members of `ty`; comparing two versions, values at the
    /// boundaries of one are the natural candidates to test against the other.
    pub fn probes(&self, ty: TypeId) -> Vec<Literal> {
        let mut out = self.raw(ty, 0);
        out.truncate(MAX_CANDIDATES * 2);
        out
    }

    /// One value of `ty`, if any could be found.
    pub fn sample(&self, ty: TypeId) -> Option<Literal> {
        self.valid(ty, 0).into_iter().next()
    }

    fn valid(&self, ty: TypeId, depth: usize) -> Vec<Literal> {
        let mut out: Vec<Literal> = Vec::new();
        for candidate in self.raw(ty, depth) {
            if !out.contains(&candidate) && self.validator.validate(ty, &candidate).is_valid() {
                out.push(candidate);
                if out.len() >= MAX_CANDIDATES {
                    break;
                }
            }
        }
        out
    }

    fn first(&self, ty: TypeId, depth: usize) -> Option<Literal> {
        self.valid(ty, depth).into_iter().next()
    }

    /// Unverified candidates for `ty`.
    fn raw(&self, ty: TypeId, depth: usize) -> Vec<Literal> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        match &self.api.types.get(ty).kind {
            TypeKind::Any | TypeKind::Not(_) => palette(),
            TypeKind::Never => Vec::new(),
            TypeKind::Null => vec![Literal::Null],
            TypeKind::Boolean => vec![Literal::Bool(false), Literal::Bool(true)],
            TypeKind::Bytes => vec![Literal::string(""), Literal::string("AA==")],
            TypeKind::Integer(n) => numbers(n, true),
            TypeKind::Number(n) => numbers(n, false),
            TypeKind::String(s) => strings(s),
            TypeKind::Enum(e) => e.values.iter().map(|v| v.value.clone()).collect(),
            TypeKind::Object(o) => self.objects(o, depth),
            TypeKind::Array(a) => self.arrays(a, depth),
            TypeKind::Map(m) => {
                let mut out = vec![Literal::Object(BTreeMap::new())];
                if let (Some(key), Some(value)) =
                    (self.first(m.key, depth + 1), self.first(m.value, depth + 1))
                    && let Literal::String(key) = key
                {
                    out.push(object([(key, value)]));
                }
                out
            }
            TypeKind::Union(u) => u
                .variants
                .iter()
                .flat_map(|&v| self.raw(v, depth + 1))
                .collect(),
            TypeKind::AllOf(members) => members
                .iter()
                .flat_map(|&m| self.raw(m, depth + 1))
                .collect(),
            TypeKind::Ref(target) => self.raw(*target, depth + 1),
        }
    }

    fn objects(&self, o: &ObjectType, depth: usize) -> Vec<Literal> {
        let mut minimal = Vec::new();
        let mut full = Vec::new();
        // Optional members of exclusive groups, which `full` may only hold
        // one of per group; each is also tried on its own below.
        let mut alternatives = Vec::new();
        for field in &o.fields {
            let Some(value) = self.first(field.ty, depth + 1) else {
                if field.required {
                    // A required member with no value: no object can be built.
                    return Vec::new();
                }
                continue;
            };
            if field.required {
                minimal.push((field.name.clone(), value.clone()));
            }
            let taken = |members: &[(String, Literal)]| {
                o.group_of(&field.name)
                    .is_some_and(|g| members.iter().any(|(name, _)| g.fields.contains(name)))
            };
            if field.required || !taken(&full) {
                full.push((field.name.clone(), value.clone()));
            }
            if !field.required && o.group_of(&field.name).is_some() {
                alternatives.push((field.name.clone(), value));
            }
        }
        let mut out = vec![object(minimal.clone()), object(full.clone())];
        for alternative in alternatives {
            let mut members = minimal.clone();
            members.push(alternative);
            out.push(object(members));
        }
        if let Some(extra) = self.first(o.additional, depth + 1) {
            // Enough undeclared members to reach `minProperties`, and a couple
            // more to probe `maxProperties`.
            let needed = usize::try_from(o.min_properties)
                .unwrap_or(usize::MAX)
                .saturating_sub(minimal.len());
            for count in [1, 2, needed, needed + 1] {
                if count == 0 || count > 16 {
                    continue;
                }
                for base in [&minimal, &full] {
                    let mut members = base.clone();
                    let mut key = unused_key(o);
                    for _ in 0..count {
                        while members.iter().any(|(k, _)| *k == key) {
                            key.push('_');
                        }
                        members.push((key.clone(), extra.clone()));
                    }
                    out.push(object(members));
                }
            }
        }
        out
    }

    fn arrays(&self, a: &ArrayType, depth: usize) -> Vec<Literal> {
        let prefix: Option<Vec<Literal>> = a
            .prefix_items
            .iter()
            .map(|&ty| self.first(ty, depth + 1))
            .collect();
        let Some(prefix) = prefix else {
            return vec![Literal::Array(Vec::new())];
        };
        let items = self.valid(a.items, depth + 1);
        let build = |length: u64, distinct: bool| -> Option<Literal> {
            let length = usize::try_from(length).ok()?;
            let mut values: Vec<Literal> = prefix.iter().take(length).cloned().collect();
            let mut i = 0;
            while values.len() < length {
                let item = if distinct {
                    items.get(i)?
                } else {
                    items.first()?
                };
                values.push(item.clone());
                i += 1;
            }
            Some(Literal::Array(values))
        };
        let max = a
            .max_items
            .unwrap_or(a.min_items.max(prefix.len() as u64) + 2);
        let mut out = Vec::new();
        for length in [
            a.min_items,
            a.min_items + 1,
            max,
            prefix.len() as u64 + 1,
            0,
            1,
        ] {
            if length <= max && length <= 16 {
                out.extend(build(length, true));
            }
        }
        // Duplicates, to tell unique from non-unique arrays apart.
        out.extend(build(2.max(a.min_items).min(max), false));
        if let Some(contains) = &a.contains
            && let Some(item) = self.first(contains.ty, depth + 1)
        {
            let copies = usize::try_from(contains.min.max(1)).unwrap_or(1).min(8);
            out.push(Literal::Array(vec![item; copies]));
        }
        out
    }
}

/// A key not declared by `o`, for probing additional properties.
pub(crate) fn unused_key(o: &ObjectType) -> String {
    let mut key = String::from("extra");
    while o.field(&key).is_some() {
        key.push('_');
    }
    key
}

fn palette() -> Vec<Literal> {
    vec![
        Literal::Null,
        Literal::Bool(false),
        number(Q::ZERO),
        number(Q::new(3, 2).expect("nonzero")),
        Literal::string(""),
        Literal::string("a"),
        Literal::Array(Vec::new()),
        Literal::Object(BTreeMap::new()),
    ]
}

fn number(q: Q) -> Literal {
    let d = q.to_decimal().unwrap_or_else(Decimal::zero);
    Literal::Number(d.into())
}

/// Candidate numbers around the constraints of `n`.
fn numbers(n: &NumericType, integer: bool) -> Vec<Literal> {
    let bound = |b: &Option<faultline_ir::Bound>| {
        b.as_ref()
            .and_then(|b| Q::from_decimal(&b.value).map(|q| (q, b.exclusive)))
    };
    let min = bound(&n.minimum);
    let max = bound(&n.maximum);
    let step = n.multiple_of.as_ref().and_then(Q::from_decimal);
    let one = Q::ONE;
    let half = Q::new(1, 2).expect("nonzero");

    let mut anchors: Vec<Q> = vec![Q::ZERO, one, Q::int(-1), Q::int(2), Q::int(1000)];
    if !integer {
        anchors.push(half);
    }
    for (value, _) in [min, max].into_iter().flatten() {
        anchors.push(value);
        for delta in [one, half, Q::new(1, 1000).expect("nonzero")] {
            anchors.extend(value.checked_add(delta));
            anchors.extend(value.checked_sub(delta));
        }
    }
    if let (Some((lo, _)), Some((hi, _))) = (min, max) {
        anchors.extend(lo.midpoint(hi));
    }
    if let Some(step) = step {
        let mut multiples = vec![Q::ZERO, step];
        for (value, _) in [min, max].into_iter().flatten() {
            if let Some(k) = value.checked_div(step) {
                for m in [k.floor() - 1, k.floor(), k.ceil(), k.ceil() + 1] {
                    multiples.extend(step.checked_mul(Q::int(m)));
                }
            }
        }
        anchors.extend(multiples);
    }
    let mut out: Vec<Literal> = Vec::new();
    for q in anchors {
        let q = if integer { Q::int(q.floor()) } else { q };
        if let Some(d) = q.to_decimal() {
            let literal = Literal::Number(d.into());
            if !out.contains(&literal) {
                out.push(literal);
            }
        }
    }
    out
}

/// Candidate strings of interesting lengths.
fn strings(s: &StringType) -> Vec<Literal> {
    let mut lengths = vec![s.min_length, s.min_length + 1, 0, 1, 8];
    if let Some(max) = s.max_length {
        // `max + 1` is never a member, but as a probe it exposes a narrowed
        // maximum when the other version has none.
        lengths.extend([max, max + 1, max.saturating_sub(1)]);
    }
    lengths.push(s.min_length.saturating_sub(1));
    lengths.retain(|&l| l <= 4096);
    lengths.dedup();
    let mut out: Vec<Literal> = Vec::new();
    let (min, max) = (
        usize::try_from(s.min_length).unwrap_or(usize::MAX),
        s.max_length.and_then(|m| usize::try_from(m).ok()),
    );
    if let Some(pattern) = &s.pattern
        && let Some(regex) = crate::pattern::compiled(pattern)
    {
        // Members of the pattern first, then a string it rejects: a probe
        // for a version with a looser pattern or none.
        out.extend(regex.samples(4, min, max).into_iter().map(Literal::string));
        if let Some(any) = crate::pattern::compiled("")
            && let Some(outside) = faultline_regex::difference(&any, &regex, min, max)
        {
            out.push(Literal::string(outside));
        }
    }
    out.extend(
        lengths
            .into_iter()
            .map(|l| Literal::string("a".repeat(usize::try_from(l).unwrap_or(0)))),
    );
    out
}
